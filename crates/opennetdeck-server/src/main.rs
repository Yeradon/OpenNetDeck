use clap::Parser;
use std::net::{IpAddr, SocketAddr};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use opennetdeck_protocol::{DEFAULT_PRIMARY_TCP_PORT, DEFAULT_SECONDARY_TCP_PORT};
use opennetdeck_server::dock::ServerMode;
use opennetdeck_server::{DiscoveryService, DockConfig, DockState, PrimaryPortServer, UsbWatcher};

#[derive(Parser, Debug)]
#[command(author, version, about = "OpenNetDeck - Elgato Network Dock Server", long_about = None)]
struct Args {
    /// Bind IP address for the primary port server and secondary bridge
    #[arg(short, long, default_value = "0.0.0.0")]
    bind: IpAddr,

    /// TCP port for primary dock control
    #[arg(short, long, default_value_t = DEFAULT_PRIMARY_TCP_PORT)]
    port: u16,

    /// TCP port for secondary child Stream Deck bridging
    #[arg(long, default_value_t = DEFAULT_SECONDARY_TCP_PORT)]
    secondary_port: u16,

    /// Operation mode: 'dock' (Network Dock hub with child proxy) or 'direct' (Direct Stream Deck on primary port)
    #[arg(long, default_value = "dock")]
    mode: ServerMode,

    /// Override child device Product ID reported to dock clients (e.g. 0x0080 for MK.2, 0x006c for XL, 0x0084 for Plus)
    #[arg(long)]
    child_pid: Option<String>,

    /// Serial number string reported to clients
    #[arg(long, default_value = "DL01A1A00001")]
    serial: String,

    /// Firmware version string reported to clients
    #[arg(long, default_value = "1.0.0.0")]
    firmware: String,

    /// MAC address reported to clients in hex format (e.g. 00:1a:7d:da:71:01)
    #[arg(long, default_value = "00:1a:7d:da:71:01")]
    mac: String,

    /// Disable mDNS announcement
    #[arg(long, default_value_t = false)]
    no_mdns: bool,
}

fn parse_mac(mac_str: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = mac_str.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(mac)
}

fn parse_pid(pid_str: &str) -> Option<u16> {
    if let Some(stripped) = pid_str.strip_prefix("0x") {
        u16::from_str_radix(stripped, 16).ok()
    } else {
        pid_str.parse::<u16>().ok()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let args = Args::parse();

    let mac_address = parse_mac(&args.mac).unwrap_or([0x00, 0x1A, 0x7D, 0xDA, 0x71, 0x01]);
    let override_pid = args.child_pid.as_deref().and_then(parse_pid);

    let config = DockConfig {
        serial_number: args.serial.clone(),
        firmware_version: args.firmware.clone(),
        mac_address,
        primary_port: args.port,
        secondary_port: args.secondary_port,
        mode: args.mode,
    };

    let bind_addr = SocketAddr::new(args.bind, args.port);
    let state = DockState::new(config.clone());

    info!(
        bind = %bind_addr,
        mode = ?args.mode,
        secondary_port = args.secondary_port,
        override_pid = ?override_pid.map(|p| format!("0x{:04x}", p)),
        serial = %config.serial_number,
        firmware = %config.firmware_version,
        "Starting OpenNetDeck server and USB hardware watcher..."
    );

    // Start mDNS advertisement if enabled
    let _mdns_service = if !args.no_mdns {
        match DiscoveryService::start(
            &config.serial_number,
            &config.firmware_version,
            config.primary_port,
        ) {
            Ok(service) => Some(service),
            Err(e) => {
                error!("Failed to start mDNS discovery service: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Spawn USB device watcher to monitor physical Stream Deck connections
    let usb_watcher = UsbWatcher::new(state.clone(), args.bind, args.secondary_port, override_pid);
    tokio::spawn(async move {
        usb_watcher.run().await;
    });

    // Run Primary Port TCP server
    let server = PrimaryPortServer::new(bind_addr, state);
    server.run().await?;

    Ok(())
}
