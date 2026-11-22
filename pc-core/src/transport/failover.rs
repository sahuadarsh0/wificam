//! 4-tier automatic transport failover & heartbeat liveness state machine.
//!
//! Priority order (lowest latency / highest bandwidth first):
//! 1. **Tier 1A (`UsbAoa`)**: Plug-and-play USB Accessory (~217 Mbps, <10 ms audio)
//! 2. **Tier 1B (`UsbAdb`)**: USB Debugging `adb reverse` (<12 ms audio)
//! 3. **Tier 2 (`UsbTether`)**: USB NCM/RNDIS tethering (<15 ms audio)
//! 4. **Tier 3 (`WifiLan`)**: Encrypted Wi-Fi UDP media + TCP control (<25 ms audio)
//! 5. **Tier 4 (`BluetoothRfcomm`)**: Classic RFCOMM audio-only failover (~45 ms)

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::protocol::AudioFormatChoice;

/// Heartbeat send cadence (learned from AudioRelayPlus).
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);

/// Silence window before declaring a transport dead and failing over.
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_millis(3_000);

/// Ordered transport tiers (smaller rank = higher priority).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum TransportTier {
    UsbAoa = 1,
    UsbAdb = 2,
    UsbTether = 3,
    WifiLan = 4,
    BluetoothRfcomm = 5,
}

impl TransportTier {
    /// True if this transport supports full 1080p60 / 4K30 video streaming.
    pub const fn supports_hd_video(self) -> bool {
        !matches!(self, Self::BluetoothRfcomm)
    }

    /// Optimal default audio wire format for this transport:
    /// raw uncompressed 48 kHz PCM over wired USB (<10 ms, zero codec delay),
    /// and Opus + In-Band FEC over wireless links.
    pub const fn recommended_audio_format(self) -> AudioFormatChoice {
        match self {
            Self::UsbAoa | Self::UsbAdb | Self::UsbTether => AudioFormatChoice::Pcm16,
            Self::WifiLan | Self::BluetoothRfcomm => AudioFormatChoice::Opus,
        }
    }

    /// Recommended Opus bitrate (bps) when Opus is active on this tier.
    pub const fn recommended_opus_bitrate_bps(self) -> u32 {
        match self {
            Self::BluetoothRfcomm => 32_000,
            _ => 64_000,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::UsbAoa => "USB (Plug & Play AOA)",
            Self::UsbAdb => "USB (ADB Reverse)",
            Self::UsbTether => "USB Tethering",
            Self::WifiLan => "Wi-Fi LAN (Encrypted)",
            Self::BluetoothRfcomm => "Bluetooth RFCOMM (Audio)",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TierState {
    last_heartbeat: Instant,
    rtt_us: u32,
}

/// Decision emitted by [`FailoverManager::evaluate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverDecision {
    /// Stay on the currently active tier (or remain disconnected).
    Keep(Option<TransportTier>),
    /// Seamlessly promote/demote active streaming to `new_tier`.
    SwitchTo {
        from: Option<TransportTier>,
        to: TransportTier,
    },
    /// All transports timed out; display "Waiting for Phone" slate and mute mic.
    AllLost { previous: TransportTier },
}

/// Tracks liveness of all available transports and selects the best live tier.
#[derive(Debug)]
pub struct FailoverManager {
    active: Option<TransportTier>,
    live_tiers: BTreeMap<TransportTier, TierState>,
    timeout: Duration,
}

impl FailoverManager {
    pub fn new(timeout: Duration) -> Self {
        Self {
            active: None,
            live_tiers: BTreeMap::new(),
            timeout,
        }
    }

    /// Record a valid heartbeat or media packet received on `tier`.
    pub fn mark_alive(&mut self, tier: TransportTier, now: Instant, rtt_us: u32) {
        self.live_tiers.insert(
            tier,
            TierState {
                last_heartbeat: now,
                rtt_us,
            },
        );
    }

    /// Explicitly mark a transport disconnected (e.g. USB cable unplugged).
    pub fn mark_disconnected(&mut self, tier: TransportTier) {
        self.live_tiers.remove(&tier);
    }

    /// Prune expired transports and return whether the session should switch tiers.
    pub fn evaluate(&mut self, now: Instant) -> FailoverDecision {
        self.live_tiers
            .retain(|_, st| now.saturating_duration_since(st.last_heartbeat) <= self.timeout);

        let best = self.live_tiers.keys().next().copied();
        match (self.active, best) {
            (prev, Some(next)) if prev != Some(next) => {
                self.active = Some(next);
                FailoverDecision::SwitchTo {
                    from: prev,
                    to: next,
                }
            }
            (Some(prev), None) => {
                self.active = None;
                FailoverDecision::AllLost { previous: prev }
            }
            (same, _) => FailoverDecision::Keep(same),
        }
    }

    pub fn active_tier(&self) -> Option<TransportTier> {
        self.active
    }

    pub fn active_rtt_us(&self) -> Option<u32> {
        self.active
            .and_then(|t| self.live_tiers.get(&t).map(|s| s.rtt_us))
    }
}

impl Default for FailoverManager {
    fn default() -> Self {
        Self::new(HEARTBEAT_TIMEOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotes_to_usb_on_plug_in_and_demotes_to_wifi_on_unplug() {
        let mut fm = FailoverManager::new(Duration::from_secs(3));
        let t0 = Instant::now();

        // Start on Wi-Fi
        fm.mark_alive(TransportTier::WifiLan, t0, 8_000);
        assert_eq!(
            fm.evaluate(t0),
            FailoverDecision::SwitchTo {
                from: None,
                to: TransportTier::WifiLan
            }
        );
        assert_eq!(
            TransportTier::WifiLan.recommended_audio_format(),
            AudioFormatChoice::Opus
        );

        // User plugs in USB cable -> automatically promotes to USB AOA (raw PCM)!
        let t1 = t0 + Duration::from_millis(500);
        fm.mark_alive(TransportTier::WifiLan, t1, 8_000);
        fm.mark_alive(TransportTier::UsbAoa, t1, 1_200);
        assert_eq!(
            fm.evaluate(t1),
            FailoverDecision::SwitchTo {
                from: Some(TransportTier::WifiLan),
                to: TransportTier::UsbAoa
            }
        );
        assert_eq!(
            TransportTier::UsbAoa.recommended_audio_format(),
            AudioFormatChoice::Pcm16
        );
        assert_eq!(fm.active_rtt_us(), Some(1_200));

        // User unplugs USB cable -> seamlessly falls back to Wi-Fi!
        let t2 = t1 + Duration::from_millis(200);
        fm.mark_disconnected(TransportTier::UsbAoa);
        assert_eq!(
            fm.evaluate(t2),
            FailoverDecision::SwitchTo {
                from: Some(TransportTier::UsbAoa),
                to: TransportTier::WifiLan
            }
        );

        // Wi-Fi drops for > 3 seconds -> AllLost triggers "Waiting for Phone" slate!
        let t3 = t2 + Duration::from_secs(4);
        assert_eq!(
            fm.evaluate(t3),
            FailoverDecision::AllLost {
                previous: TransportTier::WifiLan
            }
        );
        assert_eq!(fm.active_tier(), None);
    }
}
