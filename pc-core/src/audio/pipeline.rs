//! Lock-free studio audio pipeline connecting decoded 10 ms frames through
//! AEC3 -> RNNoise/DeepFilterNet3 -> Gain/Limiter -> Mute -> VU Meter ->
//! PI Clock-Drift Servo -> Lock-Free SPSC Ring Buffer (`rtrb`).
//!
//! **Architectural Invariant:** Zero `Mutex` locks are ever acquired on the
//! audio thread. Controls and telemetry cross thread boundaries via atomics;
//! PCM samples cross via `rtrb::Producer` / `rtrb::Consumer`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI16, AtomicU8, AtomicU32, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

use super::aec::{AecError, AecProcessor};
use super::decoder::FRAME_SAMPLES_10MS;
use super::denoise::{DenoiseMode, DenoiserStage};
use super::drift_servo::{DriftServoConfig, DriftServoResampler};

/// Lock-free atomic controls & real-time telemetry shared between `pc-ui` and
/// the real-time audio worker thread.
#[derive(Debug)]
pub struct AudioSharedState {
    // --- UI -> Audio Worker controls ---
    pub muted: AtomicBool,
    pub aec_enabled: AtomicBool,
    pub denoise_mode: AtomicU8,
    /// Input gain in tenths of a dB (e.g. `0` = 0.0 dB, `+60` = +6.0 dB, clamped [-240, +240]).
    pub gain_db_x10: AtomicI16,

    // --- Audio Worker -> UI live telemetry ---
    /// Peak level in tenths of dBFS (`-960` = -96.0 dBFS, `0` = 0.0 dBFS).
    pub peak_dbfs_x10: AtomicI16,
    /// RMS level in tenths of dBFS for 16-segment broadcast LED VU meter.
    pub rms_dbfs_x10: AtomicI16,
    /// Latest denoiser Voice Activity Detection percentage (`0..=100`).
    pub vad_pct: AtomicU8,
    /// Current clock-drift servo skew in tenths of ppm.
    pub skew_ppm_x10: AtomicI16,
    /// Output SPSC ring buffer occupancy in samples.
    pub out_ring_samples: AtomicU32,
}

impl AudioSharedState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            muted: AtomicBool::new(false),
            aec_enabled: AtomicBool::new(true),
            denoise_mode: AtomicU8::new(DenoiseMode::RnNoise as u8),
            gain_db_x10: AtomicI16::new(0),
            peak_dbfs_x10: AtomicI16::new(-960),
            rms_dbfs_x10: AtomicI16::new(-960),
            vad_pct: AtomicU8::new(0),
            skew_ppm_x10: AtomicI16::new(0),
            out_ring_samples: AtomicU32::new(0),
        })
    }

    pub fn set_denoise_mode(&self, mode: DenoiseMode) {
        self.denoise_mode.store(mode as u8, Ordering::Relaxed);
    }

    pub fn denoise_mode(&self) -> DenoiseMode {
        DenoiseMode::from_u8(self.denoise_mode.load(Ordering::Relaxed))
    }

    /// Map RMS dBFS onto a 16-segment broadcast LED meter (`0..=16`).
    pub fn vu_segments_16(&self) -> u8 {
        let db = self.rms_dbfs_x10.load(Ordering::Relaxed) as f32 / 10.0;
        let norm = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        (norm * 16.0).round() as u8
    }
}

/// Complete lock-free audio DSP stage (owned exclusively by the audio worker
/// thread — never wrapped in a `Mutex`).
pub struct AudioDspProcessor {
    state: Arc<AudioSharedState>,
    aec: AecProcessor,
    denoiser: DenoiserStage,
    servo: DriftServoResampler,
    resampled_scratch: Vec<f32>,
    target_ring_samples: usize,
}

impl AudioDspProcessor {
    pub fn new(
        state: Arc<AudioSharedState>,
        initial_aec_delay_ms: i32,
        target_ring_samples: usize,
    ) -> Result<Self, AecError> {
        Ok(Self {
            state,
            aec: AecProcessor::new(initial_aec_delay_ms)?,
            denoiser: DenoiserStage::new(),
            servo: DriftServoResampler::new(DriftServoConfig::default()),
            resampled_scratch: Vec::with_capacity(FRAME_SAMPLES_10MS + 32),
            target_ring_samples: target_ring_samples.max(FRAME_SAMPLES_10MS),
        })
    }

    pub fn denoiser_mut(&mut self) -> &mut DenoiserStage {
        &mut self.denoiser
    }

    /// Feed a 10 ms far-end PC speaker reference frame for WebRTC AEC3.
    pub fn feed_speaker_reference(
        &mut self,
        speaker_frame: &[f32; FRAME_SAMPLES_10MS],
    ) -> Result<(), AecError> {
        if self.state.aec_enabled.load(Ordering::Relaxed) {
            self.aec.feed_render_reference(speaker_frame)?;
        }
        Ok(())
    }

    /// Run one 10 ms microphone frame through the full DSP chain and push
    /// drift-compensated output samples into the lock-free `out_ring` SPSC
    /// producer.
    ///
    /// Returns the number of samples written to `out_ring`.
    pub fn process_frame_into_ring(
        &mut self,
        mic_frame: &mut [f32; FRAME_SAMPLES_10MS],
        out_ring: &mut Producer<f32>,
    ) -> Result<usize, AecError> {
        // 1. Acoustic Echo Cancellation (WebRTC AEC3)
        if self.state.aec_enabled.load(Ordering::Relaxed) {
            self.aec.process_capture(mic_frame)?;
        }

        // 2. AI Noise Reduction (Off / RNNoise / DeepFilterNet3)
        let mode = self.state.denoise_mode();
        let vad = self.denoiser.process(mode, mic_frame);
        self.state.vad_pct.store(
            (vad * 100.0).round().clamp(0.0, 100.0) as u8,
            Ordering::Relaxed,
        );

        // 3. Gain + soft limiter + mute
        let muted = self.state.muted.load(Ordering::Relaxed);
        let gain_db = (self
            .state
            .gain_db_x10
            .load(Ordering::Relaxed)
            .clamp(-240, 240) as f32)
            / 10.0;
        let linear_gain = if muted {
            0.0
        } else {
            10.0_f32.powf(gain_db / 20.0)
        };

        let mut peak = 0.0f32;
        let mut sum_sq = 0.0f64;
        for sample in mic_frame.iter_mut() {
            let amplified = *sample * linear_gain;
            // Smooth tanh soft-limiter above +/-0.95 prevents harsh digital clipping.
            let limited = soft_limit(amplified);
            *sample = limited;
            peak = peak.max(limited.abs());
            sum_sq += (limited as f64) * (limited as f64);
        }

        // 4. Broadcast VU telemetry (atomic, lock-free)
        let rms = (sum_sq / FRAME_SAMPLES_10MS as f64).sqrt() as f32;
        self.state
            .peak_dbfs_x10
            .store(linear_to_dbfs_x10(peak), Ordering::Relaxed);
        self.state
            .rms_dbfs_x10
            .store(linear_to_dbfs_x10(rms), Ordering::Relaxed);

        // 5. PI clock-drift micro-resampling based on current ring occupancy
        let capacity = out_ring.buffer().capacity();
        let free = out_ring.slots();
        let occupied = capacity.saturating_sub(free);
        self.state
            .out_ring_samples
            .store(occupied as u32, Ordering::Relaxed);

        self.servo
            .update_control(occupied, self.target_ring_samples, 0.010);
        self.state.skew_ppm_x10.store(
            (self.servo.skew_ppm() * 10.0).clamp(-30_000.0, 30_000.0) as i16,
            Ordering::Relaxed,
        );

        self.resampled_scratch.clear();
        self.servo.process(mic_frame, &mut self.resampled_scratch);

        // 6. Lock-free push into SPSC ring buffer (drops excess safely on stall)
        let mut written = 0usize;
        if let Ok(chunk) = out_ring.write_chunk_uninit(self.resampled_scratch.len().min(free)) {
            let n = chunk.len();
            chunk.fill_from_iter(self.resampled_scratch[..n].iter().copied());
            written = n;
        }
        Ok(written)
    }
}

/// Create a lock-free SPSC PCM ring buffer sized for ~80 ms headroom at 48 kHz.
pub fn create_audio_ring(capacity_samples: usize) -> (Producer<f32>, Consumer<f32>) {
    RingBuffer::new(capacity_samples.max(FRAME_SAMPLES_10MS * 8))
}

#[inline]
fn soft_limit(x: f32) -> f32 {
    if x.abs() <= 0.95 { x } else { x.tanh() }
}

#[inline]
fn linear_to_dbfs_x10(v: f32) -> i16 {
    if v <= 1e-5 {
        -960
    } else {
        let db = 20.0 * v.log10();
        (db * 10.0).clamp(-960.0, 0.0).round() as i16
    }
}

/// RAII guard for Windows MMCSS `"Pro Audio"` real-time thread priority.
///
/// Prevents Windows scheduler preemption glitches on the audio DSP and WASAPI
/// render threads during heavy gaming / GPU / OBS loads.
#[cfg(windows)]
pub struct MmcssGuard {
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl MmcssGuard {
    pub fn enter_pro_audio() -> Option<Self> {
        use windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
        use windows::core::w;

        let mut task_index = 0u32;
        // SAFETY: Passing a static null-terminated UTF-16 string and valid mutable u32 pointer.
        let handle =
            unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index) }.ok()?;
        Some(Self { handle })
    }
}

#[cfg(windows)]
impl Drop for MmcssGuard {
    fn drop(&mut self) {
        use windows::Win32::System::Threading::AvRevertMmThreadCharacteristics;
        // SAFETY: `self.handle` was returned by `AvSetMmThreadCharacteristicsW` and is reverted once.
        let _ = unsafe { AvRevertMmThreadCharacteristics(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_lock_free_audio_pipeline_runs_with_vu_and_mute_controls() {
        let state = AudioSharedState::new();
        state.aec_enabled.store(false, Ordering::Relaxed);
        state.set_denoise_mode(DenoiseMode::Off);

        let (mut prod, mut cons) = create_audio_ring(FRAME_SAMPLES_10MS * 10);
        let mut dsp = AudioDspProcessor::new(state.clone(), 20, FRAME_SAMPLES_10MS * 3).unwrap();

        // Push a -6 dBFS 1 kHz sine frame -> verify VU meter reads ~ -6 to -9 dBFS.
        let mut frame = [0.0f32; FRAME_SAMPLES_10MS];
        for (i, s) in frame.iter_mut().enumerate() {
            *s = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin() * 0.5;
        }
        let written = dsp.process_frame_into_ring(&mut frame, &mut prod).unwrap();
        assert!(written >= FRAME_SAMPLES_10MS - 4);
        assert!(state.peak_dbfs_x10.load(Ordering::Relaxed) > -80);
        assert!(state.vu_segments_16() >= 10);

        // Drain consumer ring
        let available = cons.slots();
        let chunk = cons.read_chunk(available).unwrap();
        assert_eq!(chunk.len(), written);
        chunk.commit_all();

        // Toggle mute atomically -> after the 2-sample cubic resampler ramp-down,
        // output becomes exact silence and VU drops to 0.
        state.muted.store(true, Ordering::Relaxed);
        let mut frame2 = [0.5f32; FRAME_SAMPLES_10MS];
        let written2 = dsp.process_frame_into_ring(&mut frame2, &mut prod).unwrap();
        let chunk2 = cons.read_chunk(written2).unwrap();
        let (a, b) = chunk2.as_slices();
        assert!(a.iter().chain(b.iter()).skip(4).all(|&x| x.abs() < 1e-6));
        chunk2.commit_all();
        assert_eq!(state.vu_segments_16(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn mmcss_pro_audio_thread_elevation_succeeds_on_windows() {
        let guard = MmcssGuard::enter_pro_audio();
        assert!(
            guard.is_some(),
            "AvSetMmThreadCharacteristicsW(Pro Audio) should succeed on Windows"
        );
    }
}
