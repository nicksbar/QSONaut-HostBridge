//! HostBridge runtime. Device-specific setup belongs in adapters; this crate
//! owns sessions, authorization, protocol dispatch, and audio fan-out.

use anyhow::Result;
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use qsonaut_hostbridge_protocol::*;
use rigwright::{Mode, Radio};
use std::{collections::HashSet, net::SocketAddr, sync::Arc};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::broadcast,
    time::{self, Duration, Instant},
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{info, warn};
use uuid::Uuid;

const MAX_MEDIA_PAYLOAD_BYTES: usize = 1_048_576;
const MAX_MEDIA_FRAME_BYTES: usize = MediaFrameHeader::BYTES + MAX_MEDIA_PAYLOAD_BYTES;

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
    pub header: MediaFrameHeader,
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

/// Host-side consumer for client-to-host media. Implementations must enqueue
/// into a bounded device/modem queue and return immediately; a slow consumer
/// must not block radio control.
pub trait AudioSink: Send + Sync {
    fn try_submit(&self, frame: AudioFrame) -> Result<()>;
}

/// Host-owned playback catalog. Implementations open only the output selected
/// by the authenticated client and keep device handles host-local.
pub trait AudioOutputProvider: Send + Sync {
    fn outputs(&self) -> Vec<AudioOutputInfo>;
    fn open(&self, output_id: &str, format: &AudioFormat) -> Result<Arc<dyn AudioSink>>;
}

/// Host-owned radio catalog. Implementations enumerate USB/serial devices and
/// open the selected Rigwright driver without exposing device paths to clients.
pub trait RadioProvider: Send + Sync {
    fn devices(&self) -> Vec<RadioDeviceInfo>;
    fn acquire(&self, device_id: &str) -> Result<Arc<dyn Radio>>;
    fn release(&self, device_id: &str);
}

/// A host-owned catalog entry backed by a lazy Rigwright factory. The factory
/// is called only after the client has acquired the exclusive lease.
pub struct RadioProviderEntry {
    pub info: RadioDeviceInfo,
    pub open: Arc<dyn Fn() -> Result<Arc<dyn Radio>> + Send + Sync>,
}

pub struct ConfiguredRadioProvider {
    entries: Vec<RadioProviderEntry>,
    in_use: std::sync::Mutex<HashSet<String>>,
}

impl ConfiguredRadioProvider {
    pub fn new(entries: Vec<RadioProviderEntry>) -> Self {
        Self {
            entries,
            in_use: std::sync::Mutex::new(HashSet::new()),
        }
    }
}

impl RadioProvider for ConfiguredRadioProvider {
    fn devices(&self) -> Vec<RadioDeviceInfo> {
        let in_use = self.in_use.lock().ok();
        self.entries
            .iter()
            .map(|entry| {
                let mut info = entry.info.clone();
                info.in_use = in_use
                    .as_ref()
                    .map(|leases| leases.contains(&info.id))
                    .unwrap_or(true);
                info
            })
            .collect()
    }

    fn acquire(&self, device_id: &str) -> Result<Arc<dyn Radio>> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.info.id == device_id)
            .ok_or_else(|| anyhow::anyhow!("radio device is unavailable"))?;
        let mut leases = self
            .in_use
            .lock()
            .map_err(|_| anyhow::anyhow!("radio reservation lock poisoned"))?;
        if !leases.insert(device_id.to_owned()) {
            anyhow::bail!("radio device is already in use")
        }
        match (entry.open)() {
            Ok(radio) => Ok(radio),
            Err(error) => {
                leases.remove(device_id);
                Err(error)
            }
        }
    }

    fn release(&self, device_id: &str) {
        if let Ok(mut leases) = self.in_use.lock() {
            leases.remove(device_id);
        }
    }
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

async fn fail_safe_ptt(selected_radio: &mut Option<RadioSelection>) {
    if let Some(selection) = selected_radio.as_ref() {
        if let Err(error) = selection.radio.set_ptt(false).await {
            warn!(%error, "failed to force radio PTT off during session cleanup");
        }
    }
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
    audio_sink: Option<Arc<dyn AudioSink>>,
    audio_outputs: Option<Arc<dyn AudioOutputProvider>>,
    authorizer: Arc<dyn Authorizer>,
}

impl HostBridge {
    pub fn new(
        config: HostConfig,
        radios: Arc<dyn RadioProvider>,
        audio: Option<Arc<dyn AudioSource>>,
        authorizer: Arc<dyn Authorizer>,
    ) -> Self {
        Self::new_with_audio_sink(config, radios, audio, None, authorizer)
    }

    pub fn new_with_audio_sink(
        config: HostConfig,
        radios: Arc<dyn RadioProvider>,
        audio: Option<Arc<dyn AudioSource>>,
        audio_sink: Option<Arc<dyn AudioSink>>,
        authorizer: Arc<dyn Authorizer>,
    ) -> Self {
        Self {
            config,
            radios,
            audio,
            audio_sink,
            audio_outputs: None,
            authorizer,
        }
    }

    pub fn new_with_audio_output(
        config: HostConfig,
        radios: Arc<dyn RadioProvider>,
        audio: Option<Arc<dyn AudioSource>>,
        audio_outputs: Option<Arc<dyn AudioOutputProvider>>,
        authorizer: Arc<dyn Authorizer>,
    ) -> Self {
        Self {
            config,
            radios,
            audio,
            audio_sink: None,
            audio_outputs,
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
        let audio_outputs = self
            .audio_outputs
            .as_ref()
            .map(|outputs| outputs.outputs())
            .unwrap_or_default();
        let capabilities = Capabilities {
            radio_control: !self.radios.devices().is_empty(),
            radio_devices: self.radios.devices(),
            audio_capture: self.audio.is_some(),
            audio_playback: self.audio_sink.is_some() || !audio_outputs.is_empty(),
            media_codecs: (self.audio.is_some()
                || self.audio_sink.is_some()
                || !audio_outputs.is_empty())
            .then_some(vec![AudioCodec::PcmS16Le])
            .unwrap_or_default(),
            audio_sources: self.audio.as_ref().map(|a| a.sources()).unwrap_or_default(),
            audio_outputs,
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
        let mut selected_audio_sink = self.audio_sink.clone();
        let mut selected_radio: Option<RadioSelection> = None;
        let mut heartbeat = time::interval(Duration::from_secs(15));
        let mut last_activity = Instant::now();
        let mut heartbeat_nonce = 0_u64;
        loop {
            tokio::select! {
                message = source.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        last_activity = Instant::now();
                        if let Err(error) = self.dispatch(&mut sink, &mut selected_radio, &mut audio_rx, &mut selected_audio_sink, &text).await {
                            send_json(&mut sink, &ServerMessage::Error { code: "request_failed".into(), message: error.to_string(), request_id: None }).await?;
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        last_activity = Instant::now();
                        if let Err(error) = self.dispatch_binary(&selected_audio_sink, &bytes).await {
                            send_json(&mut sink, &ServerMessage::Error { code: "media_failed".into(), message: error.to_string(), request_id: None }).await?;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {},
                    Some(Err(error)) => {
                        fail_safe_ptt(&mut selected_radio).await;
                        return Err(error.into());
                    }
                },
                frame = async { audio_rx.as_mut().unwrap().recv().await }, if audio_rx.is_some() => {
                    match frame {
                        Ok(frame) => {
                            if frame.pcm_s16le.len() > MAX_MEDIA_PAYLOAD_BYTES {
                                send_json(&mut sink, &ServerMessage::Error { code: "media_frame_too_large".into(), message: "audio frame exceeds the HostBridge limit".into(), request_id: None }).await?;
                                continue;
                            }
                            let mut header = frame.header;
                            header.version = MEDIA_HEADER_VERSION;
                            header.direction = MediaDirection::HostToClient;
                            header.payload_bytes = frame.pcm_s16le.len() as u32;
                            let mut payload = Vec::with_capacity(MediaFrameHeader::BYTES + frame.pcm_s16le.len());
                            header.encode(&mut payload);
                            payload.extend_from_slice(&frame.pcm_s16le);
                            if let Err(error) = sink.send(Message::Binary(payload.into())).await {
                                fail_safe_ptt(&mut selected_radio).await;
                                return Err(error.into());
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            send_json(&mut sink, &ServerMessage::Error { code: "media_frames_dropped".into(), message: format!("audio consumer fell behind; dropped {count} frames"), request_id: None }).await?;
                        }
                        Err(broadcast::error::RecvError::Closed) => audio_rx = None,
                    }
                },
                _ = heartbeat.tick() => {
                    if last_activity.elapsed() > Duration::from_secs(45) {
                        fail_safe_ptt(&mut selected_radio).await;
                        anyhow::bail!("client heartbeat timed out");
                    }
                    heartbeat_nonce = heartbeat_nonce.wrapping_add(1);
                    send_json(&mut sink, &ServerMessage::Ping { nonce: heartbeat_nonce }).await?;
                }
            }
        }
        fail_safe_ptt(&mut selected_radio).await;
        Ok(())
    }

    async fn dispatch_binary(
        &self,
        selected_audio_sink: &Option<Arc<dyn AudioSink>>,
        bytes: &[u8],
    ) -> Result<()> {
        if bytes.len() < MediaFrameHeader::BYTES {
            anyhow::bail!("media frame is shorter than its header")
        }
        let header = MediaFrameHeader::decode(bytes)
            .ok_or_else(|| anyhow::anyhow!("invalid media frame header"))?;
        if header.version != MEDIA_HEADER_VERSION {
            anyhow::bail!("unsupported media header version {}", header.version)
        }
        if header.direction != MediaDirection::ClientToHost {
            anyhow::bail!("client media frame has the wrong direction")
        }
        if header.payload_bytes as usize != bytes.len() - MediaFrameHeader::BYTES {
            anyhow::bail!("media payload length does not match its frame")
        }
        if bytes.len() > MAX_MEDIA_FRAME_BYTES {
            anyhow::bail!("media frame exceeds the HostBridge limit")
        }
        let sink = selected_audio_sink
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("select an audio output first"))?;
        sink.try_submit(AudioFrame {
            header,
            pcm_s16le: Arc::from(bytes[MediaFrameHeader::BYTES..].to_vec()),
        })
    }

    async fn dispatch<S>(
        &self,
        sink: &mut S,
        selected_radio: &mut Option<RadioSelection>,
        audio_rx: &mut Option<broadcast::Receiver<AudioFrame>>,
        selected_audio_sink: &mut Option<Arc<dyn AudioSink>>,
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
                        request_id: None,
                    },
                )
                .await?
            }
            ClientMessage::SelectRadio {
                request_id,
                device_id,
            } => {
                if let Some(previous) = selected_radio.take() {
                    if let Err(error) = previous.radio.set_ptt(false).await {
                        warn!(%error, "failed to force PTT off while changing radio selection");
                    }
                }
                let radio = self.radios.acquire(&device_id)?;
                *selected_radio = Some(RadioSelection {
                    id: device_id,
                    radio,
                    provider: self.radios.clone(),
                });
                send_json(sink, &ServerMessage::Ack { request_id }).await?;
            }
            ClientMessage::GetState { request_id: _ } => {
                send_json(sink, &state_message(selected_radio.as_ref()).await?).await?
            }
            ClientMessage::SetFrequency {
                request_id,
                frequency_hz,
            } => {
                selected_radio
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("select a radio first"))?
                    .radio
                    .set_frequency_hz(frequency_hz)
                    .await?;
                send_json(sink, &ServerMessage::Ack { request_id }).await?;
            }
            ClientMessage::SetMode { request_id, mode } => {
                selected_radio
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("select a radio first"))?
                    .radio
                    .set_mode(Mode::from(mode))
                    .await?;
                send_json(sink, &ServerMessage::Ack { request_id }).await?;
            }
            ClientMessage::SetPtt {
                request_id,
                enabled,
            } => {
                selected_radio
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("select a radio first"))?
                    .radio
                    .set_ptt(enabled)
                    .await?;
                send_json(sink, &ServerMessage::Ack { request_id }).await?;
            }
            ClientMessage::SelectAudio {
                request_id,
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
                send_json(sink, &ServerMessage::Ack { request_id }).await?;
            }
            ClientMessage::SelectAudioOutput {
                request_id,
                enabled,
                output_id,
                format,
            } => {
                if !enabled {
                    *selected_audio_sink = None;
                } else {
                    let outputs = self
                        .audio_outputs
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("audio playback is unavailable"))?;
                    let output = outputs
                        .outputs()
                        .into_iter()
                        .find(|output| output.id == output_id)
                        .ok_or_else(|| anyhow::anyhow!("audio output is unavailable"))?;
                    if !output.formats.contains(&format) {
                        anyhow::bail!("requested format is unavailable for audio output")
                    }
                    *selected_audio_sink = Some(outputs.open(&output_id, &format)?);
                }
                send_json(sink, &ServerMessage::Ack { request_id }).await?;
            }
            ClientMessage::Ping { nonce } => {
                send_json(sink, &ServerMessage::Pong { nonce }).await?
            }
            ClientMessage::Pong { .. } => {}
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
