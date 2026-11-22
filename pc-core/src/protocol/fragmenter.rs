//! Video frame fragmentation & reassembly + audio-priority transmit scheduler.
//!
//! Large 4K IDR keyframes (100-250 KB) would head-of-line block 10 ms Opus/PCM
//! audio packets if sent as monolithic writes over USB/TCP/BT. Splitting video
//! payloads into 8 KiB fragments allows [`PriorityTxQueue`] to inject pending
//! audio packets between fragments with sub-millisecond voice jitter.

use std::collections::VecDeque;

use bytes::{Bytes, BytesMut};

use super::packet::{MAX_PAYLOAD_LEN, Packet, PacketFlags, StreamType};

/// Default maximum video payload chunk size (8 KiB).
pub const DEFAULT_FRAGMENT_SIZE: usize = 8 * 1024;

/// Split a video access unit into one or more [`Packet`]s, advancing `seq` for
/// every emitted packet.
///
/// - Frames `<= max_chunk_size` emit a single unfragmented packet (neither
///   `FRAG_START` nor `FRAG_END` set).
/// - Larger frames emit `N >= 2` packets: first carries `FRAG_START`, middle
///   chunks carry neither flag, last carries `FRAG_END`. All chunks share
///   `pts_us` and carry monotonically increasing `seq` values.
pub fn fragment_video_frame(
    stream_type: StreamType,
    base_flags: PacketFlags,
    seq: &mut u32,
    pts_us: u64,
    frame: Bytes,
    max_chunk_size: usize,
) -> Vec<Packet> {
    assert!(
        stream_type.is_video(),
        "fragment_video_frame requires a video stream"
    );
    let chunk_size = max_chunk_size.max(256);
    let clean_flags = base_flags
        .without(PacketFlags::FRAG_START)
        .without(PacketFlags::FRAG_END);

    if frame.len() <= chunk_size {
        let s = *seq;
        *seq = seq.wrapping_add(1);
        return vec![Packet::new(stream_type, clean_flags, s, pts_us, frame)];
    }

    let total_chunks = frame.len().div_ceil(chunk_size);
    let mut out = Vec::with_capacity(total_chunks);
    let mut offset = 0usize;

    for idx in 0..total_chunks {
        let end = (offset + chunk_size).min(frame.len());
        let slice = frame.slice(offset..end);
        offset = end;

        let mut flags = clean_flags;
        if idx == 0 {
            flags = flags.union(PacketFlags::FRAG_START);
        } else if idx + 1 == total_chunks {
            flags = flags.union(PacketFlags::FRAG_END);
        }

        let s = *seq;
        *seq = seq.wrapping_add(1);
        out.push(Packet::new(stream_type, flags, s, pts_us, slice));
    }
    out
}

/// Complete reassembled video frame ready for the hardware decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReassembledFrame {
    pub stream_type: StreamType,
    pub flags: PacketFlags,
    pub pts_us: u64,
    pub data: Bytes,
}

/// Result of pushing a video packet into [`VideoReassembler`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReassembleOutcome {
    /// Fragment accepted; waiting for remaining fragments.
    Pending,
    /// A complete frame is ready.
    Complete(ReassembledFrame),
    /// A partial frame was dropped (gap / out-of-order / overflow); caller
    /// should request an IDR keyframe if needed.
    DroppedIncomplete {
        /// If the triggering packet itself was an unfragmented complete frame,
        /// it is returned here so it is not lost.
        recovered: Option<ReassembledFrame>,
    },
}

/// Stateful reassembler for fragmented video frames on a single stream.
#[derive(Debug, Default)]
pub struct VideoReassembler {
    in_progress: Option<PartialFrame>,
}

#[derive(Debug)]
struct PartialFrame {
    stream_type: StreamType,
    flags: PacketFlags,
    pts_us: u64,
    expected_seq: u32,
    buf: BytesMut,
}

impl VideoReassembler {
    pub fn new() -> Self {
        Self { in_progress: None }
    }

    /// Feed the next video [`Packet`] into the reassembler.
    pub fn push(&mut self, pkt: Packet) -> ReassembleOutcome {
        let is_start = pkt.flags.contains(PacketFlags::FRAG_START);
        let is_end = pkt.flags.contains(PacketFlags::FRAG_END);

        // Case 1: Unfragmented packet (or degenerate single-chunk start+end).
        if (!is_start && !is_end && self.in_progress.is_none()) || (is_start && is_end) {
            let had_partial = self.in_progress.take().is_some();
            let frame = ReassembledFrame {
                stream_type: pkt.stream_type,
                flags: pkt
                    .flags
                    .without(PacketFlags::FRAG_START)
                    .without(PacketFlags::FRAG_END),
                pts_us: pkt.pts_us,
                data: pkt.payload,
            };
            return if had_partial {
                ReassembleOutcome::DroppedIncomplete {
                    recovered: Some(frame),
                }
            } else {
                ReassembleOutcome::Complete(frame)
            };
        }

        // Case 2: Start of a new fragmented frame.
        if is_start {
            let dropped = self.in_progress.take().is_some();
            let mut buf = BytesMut::with_capacity(pkt.payload.len() * 4);
            buf.extend_from_slice(&pkt.payload);
            self.in_progress = Some(PartialFrame {
                stream_type: pkt.stream_type,
                flags: pkt
                    .flags
                    .without(PacketFlags::FRAG_START)
                    .without(PacketFlags::FRAG_END),
                pts_us: pkt.pts_us,
                expected_seq: pkt.seq.wrapping_add(1),
                buf,
            });
            return if dropped {
                ReassembleOutcome::DroppedIncomplete { recovered: None }
            } else {
                ReassembleOutcome::Pending
            };
        }

        // Case 3: Middle or end chunk for an active partial frame.
        let Some(mut partial) = self.in_progress.take() else {
            // Stray middle/end chunk with no start.
            return ReassembleOutcome::DroppedIncomplete { recovered: None };
        };

        let contiguous = partial.stream_type == pkt.stream_type
            && partial.pts_us == pkt.pts_us
            && partial.expected_seq == pkt.seq;

        if !contiguous || partial.buf.len() + pkt.payload.len() > MAX_PAYLOAD_LEN {
            // If this packet itself is an unfragmented frame (neither start nor end
            // and different pts), surface it as recovered while signaling the drop.
            if !is_end && partial.pts_us != pkt.pts_us {
                let recovered = ReassembledFrame {
                    stream_type: pkt.stream_type,
                    flags: pkt.flags,
                    pts_us: pkt.pts_us,
                    data: pkt.payload,
                };
                return ReassembleOutcome::DroppedIncomplete {
                    recovered: Some(recovered),
                };
            }
            return ReassembleOutcome::DroppedIncomplete { recovered: None };
        }

        partial.buf.extend_from_slice(&pkt.payload);
        partial.expected_seq = pkt.seq.wrapping_add(1);

        if is_end {
            ReassembleOutcome::Complete(ReassembledFrame {
                stream_type: partial.stream_type,
                flags: partial.flags,
                pts_us: partial.pts_us,
                data: partial.buf.freeze(),
            })
        } else {
            self.in_progress = Some(partial);
            ReassembleOutcome::Pending
        }
    }

    pub fn reset(&mut self) {
        self.in_progress = None;
    }
}

/// Strict audio-first transmit queue ensuring voice packets and heartbeats
/// always preempt queued video fragments on shared transports (USB/TCP/BT).
#[derive(Debug, Default)]
pub struct PriorityTxQueue {
    high_priority: VecDeque<Packet>,
    video_queue: VecDeque<Packet>,
}

impl PriorityTxQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, pkt: Packet) {
        if pkt.stream_type.is_video() {
            self.video_queue.push_back(pkt);
        } else {
            self.high_priority.push_back(pkt);
        }
    }

    /// Pop the next packet to transmit: Audio / Control / Heartbeat always win
    /// over pending video fragments.
    pub fn pop_next(&mut self) -> Option<Packet> {
        self.high_priority
            .pop_front()
            .or_else(|| self.video_queue.pop_front())
    }

    pub fn len(&self) -> usize {
        self.high_priority.len() + self.video_queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.high_priority.is_empty() && self.video_queue.is_empty()
    }
}
