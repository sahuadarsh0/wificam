//! 48 kHz mono audio decoder supporting both pure-Rust Opus 1.6.1 (`opus1`,
//! with in-band FEC and PLC) and zero-latency raw 16-bit PCM (USB transport).
//!
//! All decoded audio is normalized to `[-1.0, 1.0]` `f32` and sliced into exact
//! 480-sample (10 ms @ 48 kHz) frames — the native quantum of both RNNoise and
//! WebRTC AEC3.

use std::collections::VecDeque;

use opus1::{Channels, OpusDecoder, SampleRate};
use thiserror::Error;

/// Canonical internal audio sample rate (48 kHz).
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;

/// Exact 10 ms frame length at 48 kHz mono (native size for RNNoise & AEC3).
pub const FRAME_SAMPLES_10MS: usize = 480;

/// Maximum Opus single-packet duration (120 ms = 5760 samples @ 48 kHz).
const MAX_OPUS_FRAME_SAMPLES: usize = 5760;

#[derive(Debug, Error)]
pub enum AudioDecodeError {
    #[error("failed to initialize Opus decoder: {0}")]
    OpusInit(String),

    #[error("Opus decode error: {0}")]
    OpusDecode(String),

    #[error("invalid PCM16 payload length {0} (must be even)")]
    InvalidPcmLength(usize),
}

/// Unified decoder for `StreamType::AudioOpus` and `StreamType::AudioPcm16`
/// emitting exact 480-sample (10 ms) `f32` frames.
pub struct AudioDecoder {
    opus: OpusDecoder,
    scratch: Vec<f32>,
    pending: VecDeque<f32>,
    last_opus_frame_samples: usize,
}

impl AudioDecoder {
    pub fn new() -> Result<Self, AudioDecodeError> {
        let opus = OpusDecoder::new(SampleRate::Hz48000, Channels::Mono)
            .map_err(|e| AudioDecodeError::OpusInit(format!("{e:?}")))?;
        Ok(Self {
            opus,
            scratch: vec![0.0f32; MAX_OPUS_FRAME_SAMPLES],
            pending: VecDeque::with_capacity(FRAME_SAMPLES_10MS * 8),
            last_opus_frame_samples: FRAME_SAMPLES_10MS * 2, // default 20 ms
        })
    }

    /// Decode a normal Opus packet into pending 48 kHz `f32` samples.
    pub fn decode_opus(&mut self, packet: &[u8]) -> Result<usize, AudioDecodeError> {
        let n = self
            .opus
            .decode(packet, MAX_OPUS_FRAME_SAMPLES, &mut self.scratch)
            .map_err(|e| AudioDecodeError::OpusDecode(format!("{e:?}")))?;
        self.last_opus_frame_samples = n.clamp(FRAME_SAMPLES_10MS, MAX_OPUS_FRAME_SAMPLES);
        self.pending.extend(&self.scratch[..n]);
        Ok(n)
    }

    /// Recover a lost Opus packet using in-band FEC carried inside the NEXT
    /// packet (`seq + 1`).
    pub fn decode_opus_fec(&mut self, next_packet: &[u8]) -> Result<usize, AudioDecodeError> {
        let target = self.last_opus_frame_samples;
        let n = self
            .opus
            .decode_fec(next_packet, target, &mut self.scratch[..target])
            .map_err(|e| AudioDecodeError::OpusDecode(format!("{e:?}")))?;
        self.pending.extend(&self.scratch[..n]);
        Ok(n)
    }

    /// Synthesize a concealed frame via Opus PLC when a packet is lost without
    /// FEC.
    pub fn conceal_opus_plc(&mut self) -> Result<usize, AudioDecodeError> {
        let target = self.last_opus_frame_samples;
        let n = self
            .opus
            .decode(&[], target, &mut self.scratch[..target])
            .map_err(|e| AudioDecodeError::OpusDecode(format!("{e:?}")))?;
        self.pending.extend(&self.scratch[..n]);
        Ok(n)
    }

    /// Ingest raw 16-bit signed little-endian PCM (48 kHz mono, USB path).
    pub fn decode_pcm16_le(&mut self, bytes: &[u8]) -> Result<usize, AudioDecodeError> {
        if !bytes.len().is_multiple_of(2) {
            return Err(AudioDecodeError::InvalidPcmLength(bytes.len()));
        }
        let samples = bytes.len() / 2;
        self.pending.reserve(samples);
        for chunk in bytes.chunks_exact(2) {
            let s = i16::from_le_bytes([chunk[0], chunk[1]]);
            self.pending.push_back(s as f32 / 32768.0);
        }
        Ok(samples)
    }

    /// Pop the next complete 480-sample (10 ms) frame if available.
    pub fn pop_frame_10ms(&mut self, out: &mut [f32; FRAME_SAMPLES_10MS]) -> bool {
        if self.pending.len() < FRAME_SAMPLES_10MS {
            return false;
        }
        for slot in out.iter_mut() {
            *slot = self.pending.pop_front().unwrap_or(0.0);
        }
        true
    }

    /// Number of decoded samples waiting to be drained in 10 ms chunks.
    pub fn pending_samples(&self) -> usize {
        self.pending.len()
    }

    pub fn reset(&mut self) {
        self.opus.reset_state();
        self.pending.clear();
    }
}
