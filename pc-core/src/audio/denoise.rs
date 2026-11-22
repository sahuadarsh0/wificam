//! Dual-mode AI noise reduction: built-in pure-Rust RNNoise (`nnnoiseless`)
//! with a pluggable [`DenoiserBackend`] interface for optional DeepFilterNet3
//! (ONNX / full-band studio clarity mode).
//!
//! All processing operates on exact 480-sample (10 ms @ 48 kHz) frames in
//! normalized `[-1.0, 1.0]` `f32` format with zero locks on the audio thread.

use nnnoiseless::DenoiseState;

use super::decoder::FRAME_SAMPLES_10MS;

/// Runtime-selectable noise suppression mode (encoded as `u8` in atomics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DenoiseMode {
    /// Bypass noise suppression (raw studio microphone).
    Off = 0,
    /// Built-in pure-Rust RNNoise (~10 ms latency, <1% CPU, 0 MB extra deps).
    RnNoise = 1,
    /// Optional external DeepFilterNet3 backend (~20-40 ms, studio clarity on
    /// non-stationary noise like typing/traffic). Falls back to RNNoise if no
    /// external backend is registered.
    DeepFilterNet3 = 2,
}

impl DenoiseMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Off,
            2 => Self::DeepFilterNet3,
            _ => Self::RnNoise,
        }
    }
}

/// Pluggable 10 ms @ 48 kHz mono denoiser backend (e.g. RNNoise, DeepFilterNet3).
pub trait DenoiserBackend: Send {
    /// Denoise `frame` in-place (`[-1.0, 1.0]`). Returns Voice Activity
    /// Detection (VAD) probability in `[0.0, 1.0]`.
    fn process_frame(&mut self, frame: &mut [f32; FRAME_SAMPLES_10MS]) -> f32;

    /// Clear internal RNN / STFT state (called on stream reset).
    fn reset(&mut self);
}

/// Built-in RNNoise denoiser using `nnnoiseless` (pure Rust).
pub struct RnNoiseBackend {
    state: Box<DenoiseState<'static>>,
    in_scaled: [f32; FRAME_SAMPLES_10MS],
    out_scaled: [f32; FRAME_SAMPLES_10MS],
}

impl RnNoiseBackend {
    pub fn new() -> Self {
        Self {
            state: DenoiseState::new(),
            in_scaled: [0.0; FRAME_SAMPLES_10MS],
            out_scaled: [0.0; FRAME_SAMPLES_10MS],
        }
    }
}

impl Default for RnNoiseBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DenoiserBackend for RnNoiseBackend {
    fn process_frame(&mut self, frame: &mut [f32; FRAME_SAMPLES_10MS]) -> f32 {
        // nnnoiseless expects 16-bit PCM magnitude scale [-32768.0, 32767.0].
        for (dst, &src) in self.in_scaled.iter_mut().zip(frame.iter()) {
            *dst = (src * 32768.0).clamp(-32768.0, 32767.0);
        }
        let vad = self
            .state
            .process_frame(&mut self.out_scaled, &self.in_scaled);
        for (dst, &src) in frame.iter_mut().zip(self.out_scaled.iter()) {
            *dst = (src / 32768.0).clamp(-1.0, 1.0);
        }
        vad.clamp(0.0, 1.0)
    }

    fn reset(&mut self) {
        self.state = DenoiseState::new();
    }
}

/// Hot-swappable noise-suppression stage supporting `Off`, `RnNoise`, and an
/// optional registered `DeepFilterNet3` backend.
pub struct DenoiserStage {
    rnnoise: RnNoiseBackend,
    studio_backend: Option<Box<dyn DenoiserBackend>>,
}

impl DenoiserStage {
    pub fn new() -> Self {
        Self {
            rnnoise: RnNoiseBackend::new(),
            studio_backend: None,
        }
    }

    /// Attach an optional studio-clarity backend (e.g. DeepFilterNet3 ONNX).
    pub fn set_studio_backend(&mut self, backend: Option<Box<dyn DenoiserBackend>>) {
        self.studio_backend = backend;
    }

    pub fn has_studio_backend(&self) -> bool {
        self.studio_backend.is_some()
    }

    /// Process one 10 ms frame according to `mode`, returning VAD probability.
    pub fn process(&mut self, mode: DenoiseMode, frame: &mut [f32; FRAME_SAMPLES_10MS]) -> f32 {
        match mode {
            DenoiseMode::Off => 1.0,
            DenoiseMode::RnNoise => self.rnnoise.process_frame(frame),
            DenoiseMode::DeepFilterNet3 => {
                if let Some(ref mut backend) = self.studio_backend {
                    backend.process_frame(frame)
                } else {
                    self.rnnoise.process_frame(frame)
                }
            }
        }
    }

    pub fn reset(&mut self) {
        self.rnnoise.reset();
        if let Some(ref mut b) = self.studio_backend {
            b.reset();
        }
    }
}

impl Default for DenoiserStage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rnnoise_suppresses_stationary_fan_noise() {
        let mut stage = DenoiserStage::new();
        // Deterministic LCG low-passed stationary noise (realistic PC fan / AC hum).
        let mut seed: u32 = 0x1234_5678;
        let mut lp = 0.0f32;
        let mut next_fan = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = ((seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0;
            lp = 0.85 * lp + 0.15 * white;
            lp * 0.04
        };

        let mut input_energy = 0.0f64;
        let mut output_energy = 0.0f64;
        let mut last_vad = 1.0f32;

        // Warm up RNNoise for 40 frames (400 ms), then measure over next 40 frames.
        for idx in 0..80 {
            let mut frame = [0.0f32; FRAME_SAMPLES_10MS];
            for s in &mut frame {
                *s = next_fan();
            }
            let in_e: f64 = frame.iter().map(|&s| (s as f64) * (s as f64)).sum();
            let vad = stage.process(DenoiseMode::RnNoise, &mut frame);
            assert!((0.0..=1.0).contains(&vad));
            for &s in &frame {
                assert!(s.is_finite());
            }
            if idx >= 40 {
                let out_e: f64 = frame.iter().map(|&s| (s as f64) * (s as f64)).sum();
                input_energy += in_e;
                output_energy += out_e;
                last_vad = vad;
            }
        }

        // RNNoise must strongly suppress stationary fan noise (>10 dB reduction).
        let ratio = output_energy / input_energy.max(1e-12);
        assert!(
            ratio < 0.10,
            "RNNoise did not suppress fan noise enough: ratio={ratio:.4}, last_vad={last_vad:.3}"
        );
    }

    #[test]
    fn bypass_leaves_samples_untouched_and_studio_backend_swaps_cleanly() {
        struct HalfGainBackend;
        impl DenoiserBackend for HalfGainBackend {
            fn process_frame(&mut self, frame: &mut [f32; FRAME_SAMPLES_10MS]) -> f32 {
                for s in frame.iter_mut() {
                    *s *= 0.5;
                }
                0.75
            }
            fn reset(&mut self) {}
        }

        let mut stage = DenoiserStage::new();
        let original = [0.4f32; FRAME_SAMPLES_10MS];

        let mut frame = original;
        stage.process(DenoiseMode::Off, &mut frame);
        assert_eq!(frame, original);

        stage.set_studio_backend(Some(Box::new(HalfGainBackend)));
        assert!(stage.has_studio_backend());
        let vad = stage.process(DenoiseMode::DeepFilterNet3, &mut frame);
        assert!((vad - 0.75).abs() < 1e-6);
        assert!((frame[0] - 0.2).abs() < 1e-6);
    }
}
