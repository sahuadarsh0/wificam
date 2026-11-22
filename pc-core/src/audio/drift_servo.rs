//! PI-controlled cubic-Hermite clock-drift micro-resampler (±0.5% max skew).
//!
//! Android's 48 kHz ADC clock and Windows' 48 kHz DAC clock are driven by
//! independent crystals that typically drift apart by 50-300 ppm. Without
//! continuous micro-resampling, a 48 kHz stream suffers buffer underruns or
//! overruns (audible pops/crackles and lip-sync creep) every 5-15 minutes.
//!
//! [`DriftServoResampler`] smoothly adjusts the resampling ratio within
//! `[0.995, 1.005]` (imperceptible < 8.6 cents pitch shift) using a PI loop on
//! ring-buffer occupancy, keeping latency locked within ±2 ms over multi-hour
//! sessions.

use super::decoder::AUDIO_SAMPLE_RATE;

/// Maximum fractional rate adjustment (±0.5% = ±5000 ppm).
pub const MAX_DRIFT_RATIO_OFFSET: f64 = 0.005;

/// PI controller gains tuned for 10 ms update intervals at 48 kHz.
#[derive(Debug, Clone, Copy)]
pub struct DriftServoConfig {
    /// Proportional gain (ratio offset per second of buffer error).
    pub kp: f64,
    /// Integral gain (ratio offset per second^2 of accumulated error).
    pub ki: f64,
    /// Maximum absolute ratio offset (default `0.005` = ±0.5%).
    pub max_offset: f64,
}

impl Default for DriftServoConfig {
    fn default() -> Self {
        Self {
            kp: 0.018,
            ki: 0.003,
            max_offset: MAX_DRIFT_RATIO_OFFSET,
        }
    }
}

/// Smooth continuous-phase 4-point Catmull-Rom micro-resampler with PI servo.
#[derive(Debug, Clone)]
pub struct DriftServoResampler {
    cfg: DriftServoConfig,
    /// Current step size (`> 1.0` consumes input faster; `< 1.0` slower).
    ratio: f64,
    /// Accumulated integral term (seconds * seconds).
    integral: f64,
    /// Fractional sample cursor relative to the current input slice (`>= 0.0`).
    phase: f64,
    /// Last 4 input samples from the previous call to guarantee click-free
    /// cubic interpolation across frame boundaries: `[s[-4], s[-3], s[-2], s[-1]]`.
    hist: [f32; 4],
}

impl DriftServoResampler {
    pub fn new(cfg: DriftServoConfig) -> Self {
        Self {
            cfg,
            ratio: 1.0,
            integral: 0.0,
            phase: 0.0,
            hist: [0.0; 4],
        }
    }

    /// Update the PI controller from current vs. target downstream buffer fill
    /// (in samples) over `dt_seconds` (typically `0.010` for 10 ms ticks).
    ///
    /// - Positive error (`current > target`): producer is running fast -> increase
    ///   `ratio > 1.0` so we emit slightly fewer samples and drain the excess.
    /// - Negative error (`current < target`): producer is running slow -> decrease
    ///   `ratio < 1.0` so we emit slightly more samples and prevent underrun.
    pub fn update_control(
        &mut self,
        current_buffer_samples: usize,
        target_buffer_samples: usize,
        dt_seconds: f64,
    ) -> f64 {
        let error_sec = (current_buffer_samples as f64 - target_buffer_samples as f64)
            / AUDIO_SAMPLE_RATE as f64;

        let max_i = (self.cfg.max_offset * 0.8) / self.cfg.ki.max(1e-9);
        self.integral = (self.integral + error_sec * dt_seconds).clamp(-max_i, max_i);

        let offset = (self.cfg.kp * error_sec + self.cfg.ki * self.integral)
            .clamp(-self.cfg.max_offset, self.cfg.max_offset);
        self.ratio = 1.0 + offset;
        self.ratio
    }

    /// Current resampling ratio (`1.0` = nominal, `>1.0` = faster input advance).
    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    /// Estimated clock skew in parts-per-million (`(ratio - 1.0) * 1e6`).
    pub fn skew_ppm(&self) -> f64 {
        (self.ratio - 1.0) * 1_000_000.0
    }

    /// Resample `input` into `out` (appending) using 4-point Catmull-Rom cubic
    /// interpolation with continuous cross-frame phase.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        let n = input.len();
        // Pre-reserve approximate output count to avoid repeated reallocations.
        let est = ((n as f64 / self.ratio).ceil() as usize) + 2;
        out.reserve(est);

        // We interpolate at position `pos` where index `0` corresponds to
        // `hist[2]` (two samples before `input[0]`), so `y0..y3` at `idx` are
        // always available as long as `idx + 2 < n + 2`, i.e. `pos < n as f64`.
        while self.phase < n as f64 {
            let idx = self.phase.floor() as isize;
            let frac = (self.phase - idx as f64) as f32;

            let y0 = self.sample_at(input, idx - 3);
            let y1 = self.sample_at(input, idx - 2);
            let y2 = self.sample_at(input, idx - 1);
            let y3 = self.sample_at(input, idx);

            out.push(catmull_rom(y0, y1, y2, y3, frac));
            self.phase += self.ratio;
        }
        self.phase -= n as f64;

        // Shift tail samples into `hist` for the next frame.
        if n >= 4 {
            self.hist.copy_from_slice(&input[n - 4..]);
        } else {
            self.hist.copy_within(n..4, 0);
            self.hist[4 - n..].copy_from_slice(input);
        }
    }

    pub fn reset(&mut self) {
        self.ratio = 1.0;
        self.integral = 0.0;
        self.phase = 0.0;
        self.hist = [0.0; 4];
    }

    #[inline]
    fn sample_at(&self, input: &[f32], idx: isize) -> f32 {
        if idx < 0 {
            let h_idx = (4 + idx) as usize;
            self.hist[h_idx]
        } else {
            input[idx as usize]
        }
    }
}

/// 4-point Catmull-Rom cubic Hermite interpolation for `t in [0, 1]`.
#[inline]
fn catmull_rom(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * y1)
        + (-y0 + y2) * t
        + (2.0 * y0 - 5.0 * y1 + 4.0 * y2 - y3) * t2
        + (-y0 + 3.0 * y1 - 3.0 * y2 + y3) * t3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::decoder::FRAME_SAMPLES_10MS;

    #[test]
    fn absorbs_plus_and_minus_250ppm_clock_drift_over_300_seconds() {
        for &drift_ppm in &[250.0_f64, -250.0_f64] {
            let mut servo = DriftServoResampler::new(DriftServoConfig::default());
            let target_samples: usize = FRAME_SAMPLES_10MS * 3; // 30 ms nominal buffer
            let mut ring_len: f64 = target_samples as f64;
            let mut prod_frac = 0.0_f64;
            let mut out = Vec::with_capacity(512);
            let mut phase_sample = 0usize;

            let mut underruns = 0usize;
            let mut max_err_samples = 0.0_f64;

            // 300 seconds = 30,000 ticks of 10 ms.
            let prod_rate_per_tick = FRAME_SAMPLES_10MS as f64 * (1.0 + drift_ppm * 1e-6);
            for tick in 0..30_000 {
                prod_frac += prod_rate_per_tick;
                let prod_n = prod_frac.floor() as usize;
                prod_frac -= prod_n as f64;

                let input: Vec<f32> = (0..prod_n)
                    .map(|i| {
                        let t = (phase_sample + i) as f32 / AUDIO_SAMPLE_RATE as f32;
                        (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5
                    })
                    .collect();
                phase_sample += prod_n;

                servo.update_control(ring_len.round() as usize, target_samples, 0.010);
                out.clear();
                servo.process(&input, &mut out);
                ring_len += out.len() as f64;

                // Consumer drains exactly 480 samples (10 ms) per tick.
                if ring_len < FRAME_SAMPLES_10MS as f64 {
                    underruns += 1;
                    ring_len = 0.0;
                } else {
                    ring_len -= FRAME_SAMPLES_10MS as f64;
                }

                // After initial 10 s settling window, check steady-state error.
                if tick >= 1_000 {
                    let err = (ring_len - target_samples as f64).abs();
                    max_err_samples = max_err_samples.max(err);
                }
            }

            assert_eq!(underruns, 0, "drift={drift_ppm} ppm caused underruns");
            // Steady-state buffer stays within ±5 ms (240 samples) of target!
            assert!(
                max_err_samples < 240.0,
                "drift={drift_ppm} ppm steady-state error too large: {max_err_samples} samples"
            );
            // Servo converged closely to the true crystal skew!
            assert!(
                (servo.skew_ppm() - drift_ppm).abs() < 40.0,
                "servo skew {:.1} ppm did not converge to {drift_ppm} ppm",
                servo.skew_ppm()
            );
        }
    }

    #[test]
    fn unity_ratio_preserves_sine_wave_fidelity() {
        let mut servo = DriftServoResampler::new(DriftServoConfig::default());
        let input: Vec<f32> = (0..4800)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin() * 0.8)
            .collect();
        let mut out = Vec::new();
        for chunk in input.chunks(FRAME_SAMPLES_10MS) {
            servo.process(chunk, &mut out);
        }
        assert_eq!(out.len(), input.len());
        // After the 2-sample cubic pipeline latency, signal matches within 1e-5.
        for i in 4..input.len() {
            assert!(
                (out[i] - input[i - 2]).abs() < 1e-5,
                "sample mismatch at {i}: got {}, expected {}",
                out[i],
                input[i - 2]
            );
        }
    }
}
