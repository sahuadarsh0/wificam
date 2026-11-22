//! Zero-configuration LAN discovery via pure-Rust mDNS / DNS-SD (`mdns-sd`).
//!
//! Advertises `_wificam._tcp.local.` with TXT records (`ver=2`, `udp=<port>`,
//! `fp=<host-fingerprint>`) so the Android app discovers the PC automatically
//! without typing an IP address.

use std::collections::HashMap;

use mdns_sd::{ServiceDaemon, ServiceInfo};
use thiserror::Error;

use crate::brand::{APP_SLUG, MDNS_SERVICE_TYPE};

#[derive(Debug, Error)]
pub enum MdnsError {
    #[error("mDNS daemon error: {0}")]
    Daemon(String),
}

/// Configuration for advertising this PC over local LAN mDNS.
#[derive(Debug, Clone)]
pub struct MdnsAdvertiseConfig {
    pub instance_name: String,
    pub control_port: u16,
    pub media_udp_port: u16,
    pub host_fingerprint: String,
}

/// Active mDNS advertisement handle (automatically unregisters on drop).
pub struct MdnsAdvertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl MdnsAdvertiser {
    /// Start advertising `_wificam._tcp.local.` on all active LAN interfaces.
    pub fn start(cfg: &MdnsAdvertiseConfig) -> Result<Self, MdnsError> {
        let daemon = ServiceDaemon::new().map_err(|e| MdnsError::Daemon(e.to_string()))?;

        let host_label: String = cfg
            .instance_name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(32)
            .collect();
        let host_label = if host_label.is_empty() {
            APP_SLUG.to_string()
        } else {
            host_label
        };
        let host_name = format!("{host_label}.local.");

        let mut props = HashMap::new();
        props.insert("ver".to_string(), "2".to_string());
        props.insert("udp".to_string(), cfg.media_udp_port.to_string());
        props.insert("fp".to_string(), cfg.host_fingerprint.clone());

        let service = ServiceInfo::new(
            MDNS_SERVICE_TYPE,
            &cfg.instance_name,
            &host_name,
            "",
            cfg.control_port,
            props,
        )
        .map_err(|e| MdnsError::Daemon(e.to_string()))?
        .enable_addr_auto();

        let fullname = service.get_fullname().to_string();
        daemon
            .register(service)
            .map_err(|e| MdnsError::Daemon(e.to_string()))?;

        Ok(Self { daemon, fullname })
    }

    pub fn fullname(&self) -> &str {
        &self.fullname
    }

    pub fn stop(self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

impl Drop for MdnsAdvertiser {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::{DEFAULT_CONTROL_PORT, DEFAULT_MEDIA_UDP_PORT};

    #[test]
    fn advertises_and_shuts_down_mdns_service_cleanly() {
        let adv = MdnsAdvertiser::start(&MdnsAdvertiseConfig {
            instance_name: "WifiCam-TestHost".into(),
            control_port: DEFAULT_CONTROL_PORT,
            media_udp_port: DEFAULT_MEDIA_UDP_PORT,
            host_fingerprint: "1122-3344-5566-7788".into(),
        })
        .expect("mDNS daemon should start and register service");

        assert!(adv.fullname().contains(MDNS_SERVICE_TYPE));
        adv.stop();
    }
}
