use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use qsonaut_hostbridge::{
    AudioFrame, AudioOutputProvider, AudioSink, AudioSource, ConfiguredRadioProvider, HostBridge,
    HostConfig, RadioProvider, RadioProviderEntry, StaticAuthorizer,
};
use qsonaut_hostbridge_protocol::{
    AudioCodec, AudioFormat, AudioOutputInfo, AudioSourceInfo, AudioSourceKind, Capabilities,
    MediaDirection, MediaFrameHeader, RadioDeviceInfo, RadioDriver, RadioTransportKind,
    MEDIA_HEADER_VERSION,
};
use rigwright::{drivers::open_model, Radio};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use tracing::info;

#[derive(Debug, Parser)]
#[command(
    name = "qsonaut-hostbridge",
    about = "Remote radio/audio host for QSONaut"
)]
struct Cli {
    /// Override the persistent configuration file.
    #[arg(long, global = true, env = "QSONAUT_HOSTBRIDGE_CONFIG")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<CommandKind>,
}

#[derive(Debug, Subcommand)]
enum CommandKind {
    /// Create or update the host configuration.
    Config(ConfigCommand),
    /// Install and control the background service.
    Service(ServiceCommand),
    /// Run the host in the foreground.
    Run,
    /// Print the currently discovered host radios and audio sources.
    Devices,
}

#[derive(Debug, Args)]
struct ConfigCommand {
    #[command(subcommand)]
    action: ConfigAction,
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Set credentials and optional listener settings.
    Set {
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        bind: Option<SocketAddr>,
        #[arg(long)]
        name: Option<String>,
    },
    /// Print the current configuration, including the configured password.
    Show,
    /// Remove the persistent configuration.
    Reset,
}

#[derive(Debug, Args)]
struct ServiceCommand {
    #[command(subcommand)]
    action: ServiceAction,
}

#[derive(Debug, Clone, Copy, Subcommand)]
enum ServiceAction {
    /// Install a per-user systemd service and enable it at login.
    Install,
    Start,
    Stop,
    Restart,
    Status,
    /// Disable and remove the per-user service unit.
    Uninstall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct StoredConfig {
    bind: SocketAddr,
    host_name: String,
    access_key: String,
    password: String,
}

impl Default for StoredConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8765".parse().unwrap(),
            host_name: "QSONaut HostBridge".into(),
            access_key: String::new(),
            password: String::new(),
        }
    }
}

fn default_config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".config"))
                .unwrap_or_else(|| PathBuf::from("."))
        })
        .join("qsonaut-hostbridge/config.json")
}

fn config_path(cli: &Cli) -> PathBuf {
    cli.config.clone().unwrap_or_else(default_config_path)
}

fn load_config(path: &Path) -> Result<StoredConfig> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("read HostBridge config {}", path.display()))?;
    serde_json::from_str(&contents).context("parse HostBridge config")
}

fn save_config(path: &Path, config: &StoredConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config directory {}", parent.display()))?;
    }
    let contents = serde_json::to_string_pretty(config)? + "\n";
    fs::write(path, contents)
        .with_context(|| format!("write HostBridge config {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim_end().to_owned())
}

fn systemd_unit_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is required for a user service")?;
    Ok(PathBuf::from(home).join(".config/systemd/user/qsonaut-hostbridge.service"))
}

fn unit_contents(config_path: &Path) -> Result<String> {
    let executable = std::env::current_exe().context("locate qsonaut-hostbridge executable")?;
    Ok(format!(
        "[Unit]\nDescription=QSONaut HostBridge\nAfter=network-online.target\n\n[Service]\nExecStart={} run --config {}\nRestart=on-failure\nRestartSec=3\n\n[Install]\nWantedBy=default.target\n",
        systemd_quote(&executable.to_string_lossy()),
        systemd_quote(&config_path.to_string_lossy())
    ))
}

fn systemd_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .context("run systemctl --user; is systemd available?")?;
    if !status.success() {
        anyhow::bail!("systemctl --user {} failed with {status}", args.join(" "))
    }
    Ok(())
}

fn configured_radios() -> Result<ConfiguredRadioProvider> {
    let mut entries = Vec::new();
    let Ok(devices) = fs::read_dir("/dev/serial/by-id") else {
        return Ok(ConfiguredRadioProvider::new(entries));
    };
    for device in devices.flatten() {
        let file_name = device.file_name().to_string_lossy().into_owned();
        let path = device.path().to_string_lossy().into_owned();
        // Never infer a model or driver from a USB descriptor. Advertise each
        // supported protocol as an explicit candidate and let QSONaut choose.
        for spec in driver_specs() {
            add_radio_entry(&mut entries, file_name.clone(), path.clone(), spec);
        }
    }
    Ok(ConfiguredRadioProvider::new(entries))
}

#[derive(Clone, Copy)]
struct DriverSpec {
    suffix: &'static str,
    driver: RadioDriver,
    model: &'static str,
    baud: u32,
}

fn driver_specs() -> [DriverSpec; 4] {
    [
        DriverSpec {
            suffix: "icom",
            driver: RadioDriver::IcomCiv,
            model: "CI-V (generic)",
            baud: 115_200,
        },
        DriverSpec {
            suffix: "yaesu",
            driver: RadioDriver::YaesuCat,
            model: "CAT (generic)",
            baud: 38_400,
        },
        DriverSpec {
            suffix: "legacy_yaesu",
            driver: RadioDriver::YaesuLegacyCat,
            model: "classic CAT (generic)",
            baud: 4_800,
        },
        DriverSpec {
            suffix: "kenwood",
            driver: RadioDriver::KenwoodCat,
            model: "PC control (generic)",
            baud: 115_200,
        },
    ]
}

fn add_radio_entry(
    entries: &mut Vec<RadioProviderEntry>,
    file_name: String,
    path: String,
    spec: DriverSpec,
) {
    let id = format!("{}:{}", file_name, spec.suffix);
    let label = format!("{} ({})", file_name, spec.model);
    let factory_path = path.clone();
    let factory_model = spec.model.to_owned();
    entries.push(RadioProviderEntry {
        info: RadioDeviceInfo {
            id,
            label,
            driver: spec.driver,
            model: Some(spec.model.into()),
            transport: RadioTransportKind::UsbSerial,
            in_use: false,
        },
        lease_id: path,
        open: Arc::new(move || {
            Ok(Arc::new(open_model(
                &factory_model,
                factory_path.clone(),
                spec.baud,
                0xE0,
            )?) as Arc<dyn Radio>)
        }),
    });
}

struct AlsaAudioSource {
    sources: Vec<AudioSourceInfo>,
}

impl AlsaAudioSource {
    fn new() -> Self {
        let output = Command::new("arecord").arg("-L").output();
        let sources = output
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .into_iter()
            .flat_map(|devices| {
                devices
                    .lines()
                    .filter(|line| line.starts_with("hw:CARD="))
                    .filter_map(|line| {
                        let id = line.trim().to_owned();
                        let channels = alsa_channels(&id)?;
                        Some(AudioSourceInfo {
                            id: id.clone(),
                            label: format!("ALSA {id}"),
                            kind: AudioSourceKind::RadioInput,
                            formats: vec![AudioFormat {
                                codec: AudioCodec::PcmS16Le,
                                channels,
                                sample_rate_hz: 48_000,
                            }],
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        Self { sources }
    }
}

fn alsa_channels(device: &str) -> Option<u8> {
    let output = Command::new("arecord")
        .args([
            "--dump-hw-params",
            "-D",
            device,
            "-f",
            "S16_LE",
            "-r",
            "48000",
            "-c",
            "1",
            "-d",
            "1",
            "/dev/null",
        ])
        .output()
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let channels = text
        .lines()
        .find_map(|line| line.strip_prefix("CHANNELS:"))?;
    if channels.contains('1') {
        Some(1)
    } else if channels.contains('2') {
        Some(2)
    } else {
        None
    }
}

impl AudioSource for AlsaAudioSource {
    fn sources(&self) -> Vec<AudioSourceInfo> {
        self.sources.clone()
    }

    fn subscribe(
        &self,
        source_id: &str,
        format: &AudioFormat,
    ) -> Result<tokio::sync::broadcast::Receiver<AudioFrame>> {
        let source = self
            .sources
            .iter()
            .find(|source| source.id == source_id && source.formats.contains(format))
            .ok_or_else(|| anyhow::anyhow!("ALSA audio source or format is unavailable"))?;
        let (sender, receiver) = tokio::sync::broadcast::channel(8);
        let device = source.id.clone();
        let channels = format.channels;
        let child = Command::new("arecord")
            .args([
                "-D",
                &device,
                "-t",
                "raw",
                "-f",
                "S16_LE",
                "-r",
                "48000",
                "-c",
                &channels.to_string(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("start arecord for {device}"))?;
        let child = Arc::new(Mutex::new(child));
        let mut output = child
            .lock()
            .map_err(|_| anyhow::anyhow!("capture arecord process lock poisoned"))?
            .stdout
            .take()
            .context("capture arecord stdout")?;
        let capture_stopped = Arc::new(AtomicBool::new(false));
        let watcher_child = child.clone();
        let watcher_sender = sender.clone();
        let watcher_stopped = capture_stopped.clone();
        std::thread::spawn(move || {
            while !watcher_stopped.load(Ordering::Relaxed) && watcher_sender.receiver_count() > 0 {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if !watcher_stopped.load(Ordering::Relaxed) {
                if let Ok(mut child) = watcher_child.lock() {
                    let _ = child.kill();
                }
            }
        });
        info!(device = %device, channels, "HostBridge ALSA capture started");
        let reader_stopped = capture_stopped;
        std::thread::spawn(move || {
            let mut sequence = 0_u64;
            let mut timestamp_samples = 0_u64;
            let stream_id = stable_stream_id(&device);
            let bytes_per_frame = channels as usize * 2;
            let mut pcm = vec![0_u8; 960 * bytes_per_frame];
            loop {
                if sender.receiver_count() == 0 {
                    break;
                }
                if output.read_exact(&mut pcm).is_err() {
                    break;
                }
                let frame = AudioFrame {
                    header: MediaFrameHeader {
                        version: MEDIA_HEADER_VERSION,
                        stream_id,
                        direction: MediaDirection::HostToClient,
                        codec: AudioCodec::PcmS16Le,
                        sequence,
                        timestamp_samples,
                        sample_rate_hz: 48_000,
                        channels,
                        payload_bytes: pcm.len() as u32,
                    },
                    pcm_s16le: Arc::from(pcm.clone()),
                };
                if sender.send(frame).is_err() {
                    break;
                }
                sequence = sequence.wrapping_add(1);
                timestamp_samples = timestamp_samples.wrapping_add(960);
            }
            reader_stopped.store(true, Ordering::Relaxed);
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
            info!(device = %device, "HostBridge ALSA capture stopped");
        });
        Ok(receiver)
    }
}

struct AlsaAudioOutput {
    outputs: Vec<AudioOutputInfo>,
}

impl AlsaAudioOutput {
    fn new() -> Self {
        let output = Command::new("aplay").arg("-L").output();
        let outputs = output
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .into_iter()
            .flat_map(|devices| {
                devices
                    .lines()
                    .filter(|line| line.starts_with("hw:CARD="))
                    .filter_map(|line| {
                        let id = line.trim().to_owned();
                        let channels = alsa_channels(&id)?;
                        Some(AudioOutputInfo {
                            id: id.clone(),
                            label: format!("ALSA {id}"),
                            formats: vec![AudioFormat {
                                codec: AudioCodec::PcmS16Le,
                                channels,
                                sample_rate_hz: 48_000,
                            }],
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        Self { outputs }
    }
}

struct AlsaAudioSink {
    sender: mpsc::SyncSender<AudioFrame>,
}

impl AudioSink for AlsaAudioSink {
    fn try_submit(&self, frame: AudioFrame) -> Result<()> {
        self.sender
            .try_send(frame)
            .map_err(|error| anyhow::anyhow!("audio playback queue is full or closed: {error}"))
    }
}

impl AudioOutputProvider for AlsaAudioOutput {
    fn outputs(&self) -> Vec<AudioOutputInfo> {
        self.outputs.clone()
    }

    fn open(&self, output_id: &str, format: &AudioFormat) -> Result<Arc<dyn AudioSink>> {
        let output = self
            .outputs
            .iter()
            .find(|output| output.id == output_id)
            .ok_or_else(|| anyhow::anyhow!("ALSA audio output is unavailable"))?;
        if !output.formats.contains(format) {
            anyhow::bail!("requested format is unavailable for ALSA audio output")
        }
        let (sender, receiver) = mpsc::sync_channel::<AudioFrame>(8);
        let channels = format.channels.to_string();
        let mut child = Command::new("aplay")
            .args([
                "-D", output_id, "-t", "raw", "-f", "S16_LE", "-r", "48000", "-c", &channels,
            ])
            .stdin(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("start aplay for {output_id}"))?;
        let mut input = child.stdin.take().context("open aplay stdin")?;
        std::thread::spawn(move || {
            while let Ok(frame) = receiver.recv() {
                if input.write_all(&frame.pcm_s16le).is_err() {
                    break;
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        });
        Ok(Arc::new(AlsaAudioSink { sender }))
    }
}

fn stable_stream_id(value: &str) -> u32 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish() as u32
}

fn service(action: ServiceAction, config_path: &Path) -> Result<()> {
    if cfg!(not(target_os = "linux")) {
        anyhow::bail!(
            "the automatic service installer currently supports Linux systemd user services"
        )
    }
    let unit = systemd_unit_path()?;
    match action {
        ServiceAction::Install => {
            if !config_path.exists() {
                anyhow::bail!("configuration is missing; run `config set` first")
            }
            if let Some(parent) = unit.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&unit, unit_contents(config_path)?)?;
            systemctl(&["daemon-reload"])?;
            systemctl(&["enable", "qsonaut-hostbridge.service"])?;
            systemctl(&["start", "qsonaut-hostbridge.service"])?;
            println!("installed and started {}", unit.display());
        }
        ServiceAction::Uninstall => {
            let _ = systemctl(&["stop", "qsonaut-hostbridge.service"]);
            let _ = systemctl(&["disable", "qsonaut-hostbridge.service"]);
            if unit.exists() {
                fs::remove_file(&unit)?;
            }
            systemctl(&["daemon-reload"])?;
            println!("removed {}", unit.display());
        }
        ServiceAction::Start
        | ServiceAction::Stop
        | ServiceAction::Restart
        | ServiceAction::Status => {
            let verb = match action {
                ServiceAction::Start => "start",
                ServiceAction::Stop => "stop",
                ServiceAction::Restart => "restart",
                ServiceAction::Status => "status",
                _ => unreachable!(),
            };
            systemctl(&[verb, "qsonaut-hostbridge.service"])?;
        }
    }
    Ok(())
}

async fn run(config_path: &Path) -> Result<()> {
    let config = load_config(config_path)?;
    if config.access_key.is_empty() || config.password.is_empty() {
        anyhow::bail!("access key and password must be configured")
    }
    tracing_subscriber::fmt::init();
    HostBridge::new_with_audio_output(
        HostConfig {
            bind: config.bind,
            host_name: config.host_name,
        },
        Arc::new(configured_radios()?),
        Some(Arc::new(AlsaAudioSource::new())),
        Some(Arc::new(AlsaAudioOutput::new())),
        Arc::new(StaticAuthorizer::new(config.access_key, config.password)),
    )
    .run()
    .await
}

fn show_devices() -> Result<()> {
    let radios = configured_radios()?;
    let audio = AlsaAudioSource::new();
    let outputs = AlsaAudioOutput::new();
    let capabilities = Capabilities {
        radio_control: !radios.devices().is_empty(),
        radio_devices: radios.devices(),
        audio_capture: !audio.sources().is_empty(),
        audio_playback: !outputs.outputs().is_empty(),
        media_codecs: if audio.sources().is_empty() && outputs.outputs().is_empty() {
            Vec::new()
        } else {
            vec![AudioCodec::PcmS16Le]
        },
        audio_sources: audio.sources(),
        audio_outputs: outputs.outputs(),
    };
    println!("{}", serde_json::to_string_pretty(&capabilities)?);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let path = config_path(&cli);
    match cli.command.unwrap_or(CommandKind::Run) {
        CommandKind::Run => run(&path).await,
        CommandKind::Devices => show_devices(),
        CommandKind::Config(command) => match command.action {
            ConfigAction::Set {
                key,
                password,
                bind,
                name,
            } => {
                let mut config = if path
                    .metadata()
                    .map(|metadata| metadata.len() > 0)
                    .unwrap_or(false)
                {
                    load_config(&path)?
                } else {
                    StoredConfig::default()
                };
                config.access_key = match key {
                    Some(value) => value,
                    None => prompt("Access key")?,
                };
                config.password = match password {
                    Some(value) => value,
                    None => prompt("Password")?,
                };
                if let Some(bind) = bind {
                    config.bind = bind;
                }
                if let Some(name) = name {
                    config.host_name = name;
                }
                save_config(&path, &config)?;
                println!("saved {}", path.display());
                Ok(())
            }
            ConfigAction::Show => {
                println!("{}", serde_json::to_string_pretty(&load_config(&path)?)?);
                Ok(())
            }
            ConfigAction::Reset => {
                if path.exists() {
                    fs::remove_file(&path)?;
                }
                println!("removed {}", path.display());
                Ok(())
            }
        },
        CommandKind::Service(command) => service(command.action, &path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_config_round_trips() {
        let config = StoredConfig {
            access_key: "station".into(),
            password: "secret".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(serde_json::from_str::<StoredConfig>(&json).unwrap(), config);
    }

    #[test]
    fn unit_quotes_paths() {
        let unit = unit_contents(Path::new("/tmp/a path/config.json")).unwrap();
        assert!(unit.contains("\"/tmp/a path/config.json\""));
    }
}
