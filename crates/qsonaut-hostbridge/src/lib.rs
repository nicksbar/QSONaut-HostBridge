//! HostBridge runtime. Device-specific setup belongs in adapters; this crate
//! owns sessions, authorization, protocol dispatch, and audio fan-out.

use anyhow::Result;
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use qsonaut_hostbridge_protocol::*;
use rigwright::{Mode, Radio};
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::broadcast,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub bind: SocketAddr,
    pub host_name: String,
}

#[async_trait]
pub trait Authorizer: Send + Sync {
    async fn authorize(&self, key: &str, password: &str) -> bool;
}

#[derive(Debug, Clone)]
pub struct StaticAuthorizer {
    key: Arc<str>,
    password: Arc<str>,
}
impl StaticAuthorizer {
    pub fn new(key: impl Into<Arc<str>>, password: impl Into<Arc<str>>) -> Self {
        Self {
            key: key.into(),
            password: password.into(),
        }
    }
}
#[async_trait]
impl Authorizer for StaticAuthorizer {
    async fn authorize(&self, key: &str, password: &str) -> bool {
        key == self.key.as_ref() && password == self.password.as_ref()
    }
}

#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub header: AudioFrameHeader,
    pub pcm_s16le: Arc<[u8]>,
}

#[async_trait]
pub trait AudioSource: Send + Sync {
    /// Enumerate inputs using stable host-local IDs and human-readable labels.
    fn sources(&self) -> Vec<AudioSourceInfo>;
    /// Start a fan-out subscription for one selected input and format.
    fn subscribe(
        &self,
        source_id: &str,
        format: &AudioFormat,
    ) -> Result<broadcast::Receiver<AudioFrame>>;
}

/// Host-owned radio catalog. Implementations enumerate USB/serial devices and
/// open the selected Rigwright driver without exposing device paths to clients.
pub trait RadioProvider: Send + Sync {
    fn devices(&self) -> Vec<RadioDeviceInfo>;
    fn acquire(&self, device_id: &str) -> Result<Arc<dyn Radio>>;
    fn release(&self, device_id: &str);
}

pub struct FixedRadioProvider {
    device: RadioDeviceInfo,
    radio: Arc<dyn Radio>,
    in_use: std::sync::Mutex<bool>,
}

impl FixedRadioProvider {
    pub fn new(device: RadioDeviceInfo, radio: Arc<dyn Radio>) -> Self {
        Self {
            device,
            radio,
            in_use: std::sync::Mutex::new(false),
        }
    }
}

impl RadioProvider for FixedRadioProvider {
    fn devices(&self) -> Vec<RadioDeviceInfo> {
        let mut device = self.device.clone();
        device.in_use = self.in_use.lock().map(|guard| *guard).unwrap_or(true);
        vec![device]
    }

    fn acquire(&self, device_id: &str) -> Result<Arc<dyn Radio>> {
        if device_id == self.device.id {
            let mut in_use = self
                .in_use
                .lock()
                .map_err(|_| anyhow::anyhow!("radio reservation lock poisoned"))?;
            if *in_use {
                anyhow::bail!("radio device is already in use")
            }
            *in_use = true;
            Ok(self.radio.clone())
        } else {
            anyhow::bail!("radio device is unavailable")
        }
    }

    fn release(&self, device_id: &str) {
        if device_id == self.device.id {
            if let Ok(mut in_use) = self.in_use.lock() {
                *in_use = false;
            }
        }
    }
}

struct RadioSelection {
    id: String,
    radio: Arc<dyn Radio>,
    provider: Arc<dyn RadioProvider>,
}

impl Drop for RadioSelection {
    fn drop(&mut self) {
        self.provider.release(&self.id);
    }
}

#[derive(Clone)]
pub struct HostBridge {
    config: HostConfig,
    radios: Arc<dyn RadioProvider>,
    audio: Option<Arc<dyn AudioSource>>,
    authorizer: Arc<dyn Authorizer>,
}

impl HostBridge {
    pub fn new(
        config: HostConfig,
        radios: Arc<dyn RadioProvider>,
        audio: Option<Arc<dyn AudioSource>>,
        authorizer: Arc<dyn Authorizer>,
    ) -> Self {
        Self {
            config,
            radios,
            audio,
            authorizer,
        }
    }

    pub async fn run(self) -> Result<()> {
        let listener = TcpListener::bind(self.config.bind).await?;
        info!(address = ?listener.local_addr()?, "QSONaut HostBridge listening");
        loop {
            let (stream, peer) = listener.accept().await?;
            let host = self.clone();
            tokio::spawn(async move {
                if let Err(error) = host.handle(stream).await {
                    warn!(?peer, %error, "HostBridge session ended with error");
                }
            });
        }
    }

    async fn handle(&self, stream: TcpStream) -> Result<()> {
        let websocket = accept_async(stream).await?;
        let (mut sink, mut source) = websocket.split();
        let Some(Ok(Message::Text(hello))) = source.next().await else {
            anyhow::bail!("client closed before hello")
        };
        let ClientMessage::Hello(credentials) = serde_json::from_str(&hello)? else {
            anyhow::bail!("first message must be hello")
        };
        if credentials.protocol_version != PROTOCOL_VERSION
            || !self
                .authorizer
                .authorize(&credentials.access_key, &credentials.password)
                .await
        {
            anyhow::bail!("unauthorized HostBridge client")
        }
        let session_id = Uuid::new_v4();
        let capabilities = Capabilities {
            radio_control: !self.radios.devices().is_empty(),
            radio_devices: self.radios.devices(),
            audio_capture: self.audio.is_some(),
            audio_sources: self.audio.as_ref().map(|a| a.sources()).unwrap_or_default(),
        };
        send_json(
            &mut sink,
            &ServerMessage::Hello(HostHello {
                protocol_version: PROTOCOL_VERSION,
                session_id,
                host_name: self.config.host_name.clone(),
                capabilities,
            }),
        )
        .await?;
        info!(%session_id, client = %credentials.client_name, "HostBridge client authorized");
        // Audio is opt-in per session. Capability advertisement tells the
        // client what can be selected without starting a stream implicitly.
        let mut audio_rx: Option<broadcast::Receiver<AudioFrame>> = None;
        let mut selected_radio: Option<RadioSelection> = None;
        loop {
            tokio::select! {
                message = source.next() => match message {
                    Some(Ok(Message::Text(text))) => self.dispatch(&mut sink, &mut selected_radio, &mut audio_rx, &text).await?,
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {},
                    Some(Err(error)) => return Err(error.into()),
                },
                frame = async { audio_rx.as_mut().unwrap().recv().await }, if audio_rx.is_some() => {
                    if let Ok(frame) = frame { let mut payload = Vec::with_capacity(AudioFrameHeader::BYTES + frame.pcm_s16le.len()); frame.header.encode(&mut payload); payload.extend_from_slice(&frame.pcm_s16le); sink.send(Message::Binary(payload.into())).await?; }
                }
            }
        }
        Ok(())
    }

    async fn dispatch<S>(
        &self,
        sink: &mut S,
        selected_radio: &mut Option<RadioSelection>,
        audio_rx: &mut Option<broadcast::Receiver<AudioFrame>>,
        text: &str,
    ) -> Result<()>
    where
        S: SinkExt<Message> + Unpin,
        S::Error: Into<anyhow::Error>,
    {
        match serde_json::from_str::<ClientMessage>(text)? {
            ClientMessage::Hello(_) => {
                send_json(
                    sink,
                    &ServerMessage::Error {
                        code: "already_authenticated".into(),
                        message: "hello is only valid as the first message".into(),
                    },
                )
                .await?
            }
            ClientMessage::SelectRadio { device_id } => {
                let radio = self.radios.acquire(&device_id)?;
                *selected_radio = Some(RadioSelection {
                    id: device_id,
                    radio,
                    provider: self.radios.clone(),
                });
                send_json(sink, &ServerMessage::Ack { request_id: None }).await?;
            }
            ClientMessage::GetState => {
                send_json(sink, &state_message(selected_radio.as_ref()).await?).await?
            }
            ClientMessage::SetFrequency { frequency_hz } => {
                selected_radio
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("select a radio first"))?
                    .radio
                    .set_frequency_hz(frequency_hz)
                    .await?;
                send_json(sink, &ServerMessage::Ack { request_id: None }).await?;
            }
            ClientMessage::SetMode { mode } => {
                selected_radio
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("select a radio first"))?
                    .radio
                    .set_mode(Mode::from(mode))
                    .await?;
                send_json(sink, &ServerMessage::Ack { request_id: None }).await?;
            }
            ClientMessage::SetPtt { enabled } => {
                selected_radio
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("select a radio first"))?
                    .radio
                    .set_ptt(enabled)
                    .await?;
                send_json(sink, &ServerMessage::Ack { request_id: None }).await?;
            }
            ClientMessage::SelectAudio {
                enabled,
                source_id,
                format,
            } => {
                if !enabled {
                    *audio_rx = None;
                } else {
                    let audio = self
                        .audio
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("audio capture is unavailable"))?;
                    let source = audio
                        .sources()
                        .into_iter()
                        .find(|source| source.id == source_id)
                        .ok_or_else(|| anyhow::anyhow!("audio source is unavailable"))?;
                    if !source.formats.contains(&format) {
                        anyhow::bail!("requested format is unavailable for audio source")
                    }
                    *audio_rx = Some(audio.subscribe(&source_id, &format)?);
                }
                send_json(sink, &ServerMessage::Ack { request_id: None }).await?;
            }
            ClientMessage::Ping { nonce } => {
                send_json(sink, &ServerMessage::Pong { nonce }).await?
            }
        }
        Ok(())
    }
}

async fn state_message(radio: Option<&RadioSelection>) -> Result<ServerMessage> {
    let Some(radio) = radio else {
        return Ok(ServerMessage::State(RadioState::default()));
    };
    Ok(ServerMessage::State(RadioState {
        frequency_hz: radio.radio.get_frequency_hz().await.ok(),
        mode: radio.radio.get_mode().await.ok().map(Into::into),
        ptt: radio.radio.get_ptt().await.ok(),
    }))
}

async fn send_json<S>(sink: &mut S, message: &ServerMessage) -> Result<()>
where
    S: SinkExt<Message> + Unpin,
    S::Error: Into<anyhow::Error>,
{
    sink.send(Message::Text(serde_json::to_string(message)?.into()))
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radio_reservation_blocks_second_session_until_release() {
        let provider = Arc::new(FixedRadioProvider::new(
            RadioDeviceInfo {
                id: "usb-1".into(),
                label: "USB radio".into(),
                driver: RadioDriver::IcomCiv,
                model: Some("IC-7300".into()),
                transport: RadioTransportKind::UsbSerial,
                in_use: false,
            },
            Arc::new(rigwright::NullRadio::new()),
        ));
        let first = RadioSelection {
            id: "usb-1".into(),
            radio: provider.acquire("usb-1").unwrap(),
            provider: provider.clone(),
        };
        assert!(provider.devices()[0].in_use);
        assert!(provider.acquire("usb-1").is_err());
        drop(first);
        assert!(!provider.devices()[0].in_use);
        assert!(provider.acquire("usb-1").is_ok());
    }
}
