//! Pure-Rust WebRTC AEC3 Acoustic Echo Cancellation stage (`aec3` crate).
//!
//! Cancels PC speaker playback captured by the Android microphone (eliminating
//! voice-call echo when using desktop speakers instead of headphones).

use aec3::nodes::audio::AudioFormat;
use aec3::pipelines::linear::{self, LinearPipeline};
use thiserror::Error;

use super::decoder::{AUDIO_SAMPLE_RATE, FRAME_SAMPLES_10MS};

#[derive(Debug, Error)]
pub enum AecError {
    #[error("failed to initialize AEC3 pipeline: {0}")]
    Init(String),

    #[error("AEC3 processing error: {0}")]
    Process(String),
}

/// 48 kHz mono WebRTC AEC3 wrapper operating on 10 ms (480-sample) blocks.
pub struct AecProcessor {
    pipeline: LinearPipeline,
    scratch_out: [f32; FRAME_SAMPLES_10MS],
}

impl AecProcessor {
    /// Build an AEC3 processor with an initial estimated render-to-capture
    /// acoustic delay (typically 30-80 ms for USB/Wi-Fi phone mic).
    pub fn new(initial_delay_ms: i32) -> Result<Self, AecError> {
        let format = AudioFormat::ten_ms(AUDIO_SAMPLE_RATE, 1);
        let pipeline = linear::builder(format, format)
            .initial_delay_ms(initial_delay_ms)
            .build()
            .map_err(|e| AecError::Init(format!("{e:?}")))?;
        Ok(Self {
            pipeline,
            scratch_out: [0.0; FRAME_SAMPLES_10MS],
        })
    }

    /// Feed a 10 ms frame of far-end speaker audio captured from Windows
    /// WASAPI loopback.
    pub fn feed_render_reference(
        &mut self,
        speaker_frame: &[f32; FRAME_SAMPLES_10MS],
    ) -> Result<(), AecError> {
        self.pipeline
            .handle_render_frame(speaker_frame)
            .map_err(|e| AecError::Process(format!("{e:?}")))
    }

    /// Cancel echo from a 10 ms near-end microphone frame in-place.
    pub fn process_capture(
        &mut self,
        mic_frame: &mut [f32; FRAME_SAMPLES_10MS],
    ) -> Result<(), AecError> {
        let produced = self
            .pipeline
            .process_capture_frame(mic_frame, &mut self.scratch_out)
            .map_err(|e| AecError::Process(format!("{e:?}")))?;
        if produced {
            mic_frame.copy_from_slice(&self.scratch_out);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aec3_cancels_delayed_far_end_speaker_echo() {
        let mut aec = AecProcessor::new(20).expect("init AEC3");
        let mut seed: u32 = 0xCAFE_BABE;
        let mut next_speech_like = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let u = ((seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0;
            u * 0.35
        };

        // 2-frame (20 ms) acoustic delay ring between speaker and phone mic.
        let mut delay_line: [[f32; FRAME_SAMPLES_10MS]; 2] = [[0.0; FRAME_SAMPLES_10MS]; 2];
        let mut echo_in_energy = 0.0f64;
        let mut echo_out_energy = 0.0f64;

        // Run 150 frames (1.5 seconds) so AEC3 adaptive filter converges.
        for tick in 0..150 {
            let mut render = [0.0f32; FRAME_SAMPLES_10MS];
            for s in &mut render {
                *s = next_speech_like();
            }
            // Mic hears attenuated echo from 20 ms ago (no near-end talker).
            let mut capture = delay_line[tick % 2];
            for s in &mut capture {
                *s *= 0.6;
            }
            delay_line[tick % 2] = render;

            let in_e: f64 = capture.iter().map(|&x| (x as f64) * (x as f64)).sum();
            aec.feed_render_reference(&render).unwrap();
            aec.process_capture(&mut capture).unwrap();
            let out_e: f64 = capture.iter().map(|&x| (x as f64) * (x as f64)).sum();

            for &s in &capture {
                assert!(s.is_finite());
            }
            if tick >= 100 {
                echo_in_energy += in_e;
                echo_out_energy += out_e;
            }
        }

        // After convergence, AEC3 must suppress echo energy substantially.
        let ratio = echo_out_energy / echo_in_energy.max(1e-12);
        assert!(
            ratio < 0.25,
            "AEC3 did not converge on 20 ms echo: residual energy ratio={ratio:.4}"
        );
    }
}
