use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use tracing::{info, warn};

pub struct DiscoveryService {
    daemon: ServiceDaemon,
}

impl DiscoveryService {
    pub fn start(serial: &str, firmware: &str, port: u16) -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new()?;

        let service_type = "_elg._tcp.local.";
        let instance_name = format!("Elgato Network Dock {}", serial);
        let host_name = format!("{}.local.", serial.to_lowercase());

        let mut properties = HashMap::new();
        properties.insert("dt".to_string(), "215".to_string());
        properties.insert("sn".to_string(), serial.to_string());
        properties.insert("fw".to_string(), firmware.to_string());
        properties.insert("vid".to_string(), "4057".to_string());
        properties.insert("pid".to_string(), "65535".to_string());

        let service_info = ServiceInfo::new(
            service_type,
            &instance_name,
            &host_name,
            "",
            port,
            properties,
        )?;

        daemon.register(service_info)?;
        info!(
            service = %service_type,
            instance = %instance_name,
            port = port,
            "mDNS / Bonjour service registered and announcing for Elgato software"
        );

        Ok(Self { daemon })
    }

    pub fn unregister(&self) {
        if let Err(e) = self.daemon.unregister("_elg._tcp.local.") {
            warn!("Error unregistering mDNS service: {}", e);
        }
    }
}
