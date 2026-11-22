//! QR pairing URI serialization & Trust-On-First-Use (TOFU) peer store.

use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use crate::brand::PAIRING_URI_SCHEME;

use super::handshake::SecurityError;

/// Payload embedded inside the PC's pairing QR code.
///
/// Scanning this QR pairs the phone with the PC's ephemeral public key and
/// single-use pairing token — explicit user consent on the phone is still
/// required before live video/audio begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingPayload {
    pub ip: String,
    pub control_port: u16,
    pub media_udp_port: u16,
    pub host_name: String,
    pub host_pub_b64: String,
    pub token_b64: String,
}

impl PairingPayload {
    /// Generate a random 128-bit single-use pairing token.
    pub fn generate_token() -> Result<String, SecurityError> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|e| SecurityError::Rng(e.to_string()))?;
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Encode into `<scheme>://pair?ip=...&port=...&udp=...&pub=...&token=...&name=...`.
    pub fn to_uri(&self) -> String {
        let name_b64 = URL_SAFE_NO_PAD.encode(self.host_name.as_bytes());
        format!(
            "{PAIRING_URI_SCHEME}://pair?ip={}&port={}&udp={}&pub={}&token={}&name={}",
            self.ip,
            self.control_port,
            self.media_udp_port,
            self.host_pub_b64,
            self.token_b64,
            name_b64
        )
    }

    /// Parse a pairing URI back into [`PairingPayload`].
    pub fn from_uri(uri: &str) -> Result<Self, SecurityError> {
        let prefix = format!("{PAIRING_URI_SCHEME}://pair?");
        let query = uri
            .strip_prefix(&prefix)
            .ok_or_else(|| SecurityError::InvalidPairingUri("wrong scheme or host".into()))?;

        let mut ip = None;
        let mut port = None;
        let mut udp = None;
        let mut pub_b64 = None;
        let mut token_b64 = None;
        let mut name_b64 = None;

        for pair in query.split('&') {
            let (k, v) = pair
                .split_once('=')
                .ok_or_else(|| SecurityError::InvalidPairingUri("malformed query param".into()))?;
            match k {
                "ip" => ip = Some(v.to_string()),
                "port" => {
                    port = Some(
                        v.parse::<u16>()
                            .map_err(|_| SecurityError::InvalidPairingUri("invalid port".into()))?,
                    )
                }
                "udp" => {
                    udp =
                        Some(v.parse::<u16>().map_err(|_| {
                            SecurityError::InvalidPairingUri("invalid udp port".into())
                        })?)
                }
                "pub" => pub_b64 = Some(v.to_string()),
                "token" => token_b64 = Some(v.to_string()),
                "name" => name_b64 = Some(v.to_string()),
                _ => {}
            }
        }

        let host_name = match name_b64 {
            Some(encoded) => {
                let raw = URL_SAFE_NO_PAD
                    .decode(encoded)
                    .map_err(|_| SecurityError::InvalidPairingUri("invalid base64 name".into()))?;
                String::from_utf8(raw)
                    .map_err(|_| SecurityError::InvalidPairingUri("non-utf8 name".into()))?
            }
            None => return Err(SecurityError::InvalidPairingUri("missing name".into())),
        };

        Ok(Self {
            ip: ip.ok_or_else(|| SecurityError::InvalidPairingUri("missing ip".into()))?,
            control_port: port
                .ok_or_else(|| SecurityError::InvalidPairingUri("missing port".into()))?,
            media_udp_port: udp
                .ok_or_else(|| SecurityError::InvalidPairingUri("missing udp".into()))?,
            host_name,
            host_pub_b64: pub_b64
                .ok_or_else(|| SecurityError::InvalidPairingUri("missing pub".into()))?,
            token_b64: token_b64
                .ok_or_else(|| SecurityError::InvalidPairingUri("missing token".into()))?,
        })
    }
}

/// Trusted device record persisted locally (zero cloud).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPeer {
    pub device_id: String,
    pub display_name: String,
    pub fingerprint: String,
}

/// Local Trust-On-First-Use (TOFU) peer verification store.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TofuTrustStore {
    peers: BTreeMap<String, TrustedPeer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustStatus {
    /// Device fingerprint matches previously approved entry.
    Trusted,
    /// Never seen before — prompt user for consent before streaming.
    Unknown,
    /// Device ID exists with a DIFFERENT fingerprint (possible MITM / reinstall).
    FingerprintMismatch,
}

impl TofuTrustStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn verify(&self, device_id: &str, fingerprint: &str) -> TrustStatus {
        match self.peers.get(device_id) {
            None => TrustStatus::Unknown,
            Some(existing) if existing.fingerprint == fingerprint => TrustStatus::Trusted,
            Some(_) => TrustStatus::FingerprintMismatch,
        }
    }

    pub fn trust(&mut self, peer: TrustedPeer) {
        self.peers.insert(peer.device_id.clone(), peer);
    }

    pub fn revoke(&mut self, device_id: &str) -> bool {
        self.peers.remove(device_id).is_some()
    }

    pub fn list(&self) -> Vec<&TrustedPeer> {
        self.peers.values().collect()
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}
