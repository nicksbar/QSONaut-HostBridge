use anyhow::Result;
use clap::Parser;
use qsonaut_hostbridge::FixedRadioProvider;
use qsonaut_hostbridge::{HostBridge, HostConfig, StaticAuthorizer};
use qsonaut_hostbridge_protocol::{RadioDeviceInfo, RadioDriver, RadioTransportKind};
use rigwright::NullRadio;
use std::{net::SocketAddr, sync::Arc};

#[derive(Debug, Parser)]
#[command(
    name = "qsonaut-hostbridge",
    about = "Remote radio/audio host for QSONaut"
)]
struct Args {
    #[arg(long, env = "QSONAUT_HOSTBRIDGE_BIND", default_value = "0.0.0.0:8765")]
    bind: SocketAddr,
    #[arg(
        long,
        env = "QSONAUT_HOSTBRIDGE_NAME",
        default_value = "QSONaut HostBridge"
    )]
    name: String,
    #[arg(long, env = "QSONAUT_HOSTBRIDGE_KEY")]
    key: String,
    #[arg(long, env = "QSONAUT_HOSTBRIDGE_PASSWORD")]
    password: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let host = HostBridge::new(
        HostConfig {
            bind: args.bind,
            host_name: args.name,
        },
        Arc::new(FixedRadioProvider::new(
            RadioDeviceInfo {
                id: "null-radio".into(),
                label: "Development Null Radio".into(),
                driver: RadioDriver::Rigctld,
                model: None,
                transport: RadioTransportKind::Network,
                in_use: false,
            },
            Arc::new(NullRadio::new()),
        )),
        None,
        Arc::new(StaticAuthorizer::new(args.key, args.password)),
    );
    host.run().await
}
