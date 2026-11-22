//! Adaptive elastic jitter buffer with Opus in-band FEC peek & PLC fallback.
//!
//! Designed around real-world Wi-Fi jitter lessons:
//! - Grows target depth on bursty jitter instead of dropping late packets.
//! - Peeks at `seq + 1` when `seq` is missing so Opus can recover the lost
//!   frame from in-band FEC before falling back to PLC.
//! - Jumps to the live edge after a long Wi-Fi stall instead of accumulating
//!   permanent lag.

use std::collections::BTreeMap;

use bytes::Bytes;

/// Sequence-numbered audio packet stored inside [`AdaptiveJitterBuffer`].
#[derive(Debug, Clone)]
pub struct JitterPacket {
    pub seq: u32,
    pub has_fec: bool,
    pub payload: Bytes,
}

/// Configuration for [`AdaptiveJitterBuffer`].
#[derive(Debug, Clone, Copy)]
pub struct JitterConfig {
    /// Minimum playout depth in packets (e.g. 2 for USB = 20 ms, 3 for Wi-Fi = 30 ms).
    pub min_depth: usize,
    /// Maximum playout depth in packets before jumping to the live edge (e.g. 12 = 120 ms).
    pub max_depth: usize,
    /// Consecutive clean ticks before shrinking target depth by 1 packet.
    pub shrink_after_clean_ticks: u32,
}

impl Default for JitterConfig {
    fn default() -> Self {
        Self {
            min_depth: 2,
            max_depth: 12,
            shrink_after_clean_ticks: 150, // ~1.5-3.0 seconds of calm network
        }
    }
}

/// Decision emitted on each playout tick of [`AdaptiveJitterBuffer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayoutAction {
    /// Waiting for initial prefill (`target_depth`) to build up.
    Buffering,
    /// Decode the exact next packet in sequence.
    Decode(Bytes),
    /// Packet `seq` is missing, but `seq + 1` is already queued with `HAS_FEC`:
    /// run Opus FEC decode on this payload (keeping `seq + 1` for the next tick).
    DecodeFec(Bytes),
    /// Packet `seq` is missing and no FEC is available — run Opus PLC.
    ConcealPlc,
}

/// Telemetry snapshot for UI / diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JitterStats {
    pub buffered_packets: usize,
    pub target_depth: usize,
    pub decoded_packets: u64,
    pub fec_recoveries: u64,
    pub plc_concealments: u64,
    pub late_drops: u64,
    pub live_edge_jumps: u64,
}

/// Elastic sequence-ordered jitter buffer for incoming audio packets.
#[derive(Debug)]
pub struct AdaptiveJitterBuffer {
    cfg: JitterConfig,
    queue: BTreeMap<u32, JitterPacket>,
    started: bool,
    next_seq: u32,
    target_depth: usize,
    clean_streak: u32,
    stats: JitterStats,
}

impl AdaptiveJitterBuffer {
    pub fn new(cfg: JitterConfig) -> Self {
        let min_depth = cfg.min_depth.max(1);
        let max_depth = cfg.max_depth.max(min_depth + 1);
        let cfg = JitterConfig {
            min_depth,
            max_depth,
            shrink_after_clean_ticks: cfg.shrink_after_clean_ticks.max(10),
        };
        Self {
            cfg,
            queue: BTreeMap::new(),
            started: false,
            next_seq: 0,
            target_depth: cfg.min_depth,
            clean_streak: 0,
            stats: JitterStats {
                target_depth: cfg.min_depth,
                ..Default::default()
            },
        }
    }

    /// Push a newly arrived audio packet from the network/USB receiver.
    pub fn push(&mut self, pkt: JitterPacket) {
        if self.started && seq_lt(pkt.seq, self.next_seq) {
            // Packet arrived after we already played past its slot -> late drop.
            // Grow target depth by 1 (up to max_depth) so subsequent jittery
            // packets make it in time!
            self.stats.late_drops += 1;
            self.grow_depth();
            return;
        }

        self.queue.insert(pkt.seq, pkt);

        // Guard against unbounded backlog after a temporary Wi-Fi stall:
        // jump to the live edge keeping `target_depth` newest packets.
        if self.queue.len() > self.cfg.max_depth * 2 {
            while self.queue.len() > self.target_depth {
                self.queue.pop_first();
            }
            if let Some((&oldest_seq, _)) = self.queue.first_key_value() {
                self.next_seq = oldest_seq;
                self.started = true;
            }
            self.stats.live_edge_jumps += 1;
        }

        self.stats.buffered_packets = self.queue.len();
    }

    /// Pop the next packet action at the audio playout cadence.
    pub fn pop_next(&mut self) -> PlayoutAction {
        if !self.started {
            if self.queue.len() < self.target_depth {
                self.stats.buffered_packets = self.queue.len();
                return PlayoutAction::Buffering;
            }
            if let Some((&first_seq, _)) = self.queue.first_key_value() {
                self.next_seq = first_seq;
                self.started = true;
            }
        }

        let action = if let Some(pkt) = self.queue.remove(&self.next_seq) {
            self.next_seq = self.next_seq.wrapping_add(1);
            self.stats.decoded_packets += 1;
            self.clean_streak += 1;
            if self.clean_streak >= self.cfg.shrink_after_clean_ticks
                && self.target_depth > self.cfg.min_depth
            {
                self.target_depth -= 1;
                self.clean_streak = 0;
            }
            PlayoutAction::Decode(pkt.payload)
        } else {
            // Packet `next_seq` is missing! Check if `next_seq + 1` is already
            // here with Opus in-band FEC.
            let peek_seq = self.next_seq.wrapping_add(1);
            self.next_seq = peek_seq;
            self.clean_streak = 0;

            if let Some(next_pkt) = self.queue.get(&peek_seq)
                && next_pkt.has_fec
            {
                self.stats.fec_recoveries += 1;
                PlayoutAction::DecodeFec(next_pkt.payload.clone())
            } else {
                self.stats.plc_concealments += 1;
                if self.queue.is_empty() {
                    self.grow_depth();
                }
                PlayoutAction::ConcealPlc
            }
        };

        self.stats.buffered_packets = self.queue.len();
        self.stats.target_depth = self.target_depth;
        action
    }

    /// Current buffer occupancy in packets (used by the PI clock-drift servo).
    pub fn buffered_packets(&self) -> usize {
        self.queue.len()
    }

    /// Current adaptive target depth in packets.
    pub fn target_depth(&self) -> usize {
        self.target_depth
    }

    pub fn stats(&self) -> JitterStats {
        self.stats
    }

    pub fn reset(&mut self) {
        self.queue.clear();
        self.started = false;
        self.next_seq = 0;
        self.target_depth = self.cfg.min_depth;
        self.clean_streak = 0;
        self.stats.buffered_packets = 0;
        self.stats.target_depth = self.target_depth;
    }

    fn grow_depth(&mut self) {
        if self.target_depth < self.cfg.max_depth {
            self.target_depth += 1;
            self.clean_streak = 0;
            self.stats.target_depth = self.target_depth;
        }
    }
}

/// Serial-number arithmetic `<` for wrapping `u32` sequence numbers.
#[inline]
fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}
