//! Lock-free studio audio DSP engine (48 kHz mono, 10 ms frames).

pub mod aec;
pub mod decoder;
pub mod denoise;
pub mod drift_servo;
pub mod jitter;
pub mod pipeline;
pub mod wasapi_loopback;
pub mod wasapi_sink;

pub use aec::{AecError, AecProcessor};
pub use decoder::{AUDIO_SAMPLE_RATE, AudioDecodeError, AudioDecoder, FRAME_SAMPLES_10MS};
pub use denoise::{DenoiseMode, DenoiserBackend, DenoiserStage, RnNoiseBackend};
pub use drift_servo::{DriftServoConfig, DriftServoResampler, MAX_DRIFT_RATIO_OFFSET};
pub use jitter::{AdaptiveJitterBuffer, JitterConfig, JitterPacket, JitterStats, PlayoutAction};
#[cfg(windows)]
pub use pipeline::MmcssGuard;
pub use pipeline::{AudioDspProcessor, AudioSharedState, create_audio_ring};
pub use wasapi_loopback::WasapiLoopbackHandle;
#[cfg(windows)]
pub use wasapi_loopback::spawn_wasapi_loopback_thread;
pub use wasapi_sink::{
    AudioRenderDevice, WasapiError, WasapiRenderHandle, is_known_virtual_cable_name,
    select_safe_render_device,
};
#[cfg(windows)]
pub use wasapi_sink::{enumerate_render_devices, spawn_wasapi_render_thread};

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use opus1::{Application, Bitrate, Channels, InbandFec, OpusEncoder, SampleRate, Signal};

    use super::*;

    fn sine_frame_f32(samples: usize, freq_hz: f32, phase_offset: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| {
                let t = (phase_offset + i) as f32 / AUDIO_SAMPLE_RATE as f32;
                (2.0 * std::f32::consts::PI * freq_hz * t).sin() * 0.5
            })
            .collect()
    }

    fn make_voip_encoder(bitrate_bps: u32, loss_pct: u8) -> OpusEncoder {
        let mut enc = OpusEncoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)
            .expect("build opus1 encoder");
        enc.set_bitrate(Bitrate::BitsPerSecond(
            std::num::NonZeroU32::new(bitrate_bps).unwrap(),
        ));
        enc.set_signal(Signal::Voice);
        enc.set_inband_fec(InbandFec::On);
        enc.set_packet_loss_perc(loss_pct).unwrap();
        enc
    }

    #[test]
    fn opus_encode_decode_and_10ms_chunking() {
        let mut enc = make_voip_encoder(64_000, 10);
        let mut dec = AudioDecoder::new().expect("build opus decoder");

        // Encode 10 consecutive 20 ms frames (960 samples each) of a 440 Hz tone.
        let mut opus_buf = [0u8; 4000];
        let mut total_10ms_frames = 0usize;
        let mut peak = 0.0f32;
        for idx in 0..10 {
            let pcm = sine_frame_f32(960, 440.0, idx * 960);
            let len = enc.encode(&pcm, 960, &mut opus_buf).expect("encode");
            let decoded = dec.decode_opus(&opus_buf[..len]).expect("decode");
            assert_eq!(decoded, 960);

            let mut frame = [0.0f32; FRAME_SAMPLES_10MS];
            while dec.pop_frame_10ms(&mut frame) {
                total_10ms_frames += 1;
                for &s in &frame {
                    assert!(s.is_finite());
                    peak = peak.max(s.abs());
                }
            }
        }
        assert_eq!(total_10ms_frames, 20);
        assert!(peak > 0.2, "decoded sine energy too low: peak={peak}");
    }

    #[test]
    fn jitter_buffer_triggers_opus_fec_and_plc_on_loss() {
        let mut enc = make_voip_encoder(32_000, 15);
        let mut dec = AudioDecoder::new().unwrap();

        let mut packets = Vec::new();
        let mut buf = [0u8; 4000];
        for seq in 0..8u32 {
            let pcm = sine_frame_f32(960, 300.0, seq as usize * 960);
            let len = enc.encode(&pcm, 960, &mut buf).unwrap();
            packets.push(Bytes::copy_from_slice(&buf[..len]));
        }

        let mut jb = AdaptiveJitterBuffer::new(JitterConfig {
            min_depth: 2,
            max_depth: 8,
            shrink_after_clean_ticks: 50,
        });

        // Push packets 0, 1, 2, [DROP 3], 4, 5
        for &seq in &[0u32, 1, 2, 4, 5] {
            jb.push(JitterPacket {
                seq,
                has_fec: true,
                payload: packets[seq as usize].clone(),
            });
        }

        // seq 0, 1, 2 decode normally
        for _ in 0..3 {
            match jb.pop_next() {
                PlayoutAction::Decode(p) => {
                    assert_eq!(dec.decode_opus(&p).unwrap(), 960);
                }
                other => panic!("expected Decode, got {other:?}"),
            }
        }
        // seq 3 missing -> MUST recover via FEC using packet 4!
        match jb.pop_next() {
            PlayoutAction::DecodeFec(p4) => {
                assert_eq!(dec.decode_opus_fec(&p4).unwrap(), 960);
            }
            other => panic!("expected DecodeFec, got {other:?}"),
        }
        // seq 4 is still queued and decodes normally right after!
        match jb.pop_next() {
            PlayoutAction::Decode(p4) => {
                assert_eq!(dec.decode_opus(&p4).unwrap(), 960);
            }
            other => panic!("expected Decode for seq 4, got {other:?}"),
        }
        // seq 5 decodes, then seq 6 missing with empty queue -> PLC concealment!
        assert!(matches!(jb.pop_next(), PlayoutAction::Decode(_)));
        assert_eq!(jb.pop_next(), PlayoutAction::ConcealPlc);
        assert_eq!(dec.conceal_opus_plc().unwrap(), 960);

        assert_eq!(jb.stats().fec_recoveries, 1);
        assert_eq!(jb.stats().plc_concealments, 1);
    }

    #[test]
    fn jitter_buffer_adapts_to_jitter_and_handles_stall_live_edge_jump() {
        let mut jb = AdaptiveJitterBuffer::new(JitterConfig {
            min_depth: 2,
            max_depth: 6,
            shrink_after_clean_ticks: 20,
        });

        // Push seq 0, 1, 2, play 0, 1, 2, then arrive late seq 1 -> depth grows!
        for seq in 0..3u32 {
            jb.push(JitterPacket {
                seq,
                has_fec: false,
                payload: Bytes::from_static(b"a"),
            });
        }
        let _ = jb.pop_next();
        let _ = jb.pop_next();
        jb.push(JitterPacket {
            seq: 0,
            has_fec: false,
            payload: Bytes::from_static(b"late"),
        });
        assert_eq!(jb.stats().late_drops, 1);
        assert_eq!(jb.target_depth(), 3);

        // Simulate 150 ms burst stall dumping 20 packets at once -> jumps to live edge!
        for seq in 10..30u32 {
            jb.push(JitterPacket {
                seq,
                has_fec: false,
                payload: Bytes::from_static(b"burst"),
            });
        }
        assert!(jb.stats().live_edge_jumps >= 1);
        assert!(jb.buffered_packets() <= jb.target_depth() + 12);
    }

    #[test]
    fn pcm16_usb_passthrough_preserves_exact_samples() {
        let mut dec = AudioDecoder::new().unwrap();
        let mut bytes = Vec::with_capacity(FRAME_SAMPLES_10MS * 2);
        for i in 0..FRAME_SAMPLES_10MS {
            let s = ((i as i32 - 240) * 100) as i16;
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        assert_eq!(dec.decode_pcm16_le(&bytes).unwrap(), FRAME_SAMPLES_10MS);

        let mut out = [0.0f32; FRAME_SAMPLES_10MS];
        assert!(dec.pop_frame_10ms(&mut out));
        for (i, &sample) in out.iter().enumerate() {
            let expected = (((i as i32 - 240) * 100) as i16) as f32 / 32768.0;
            assert!((sample - expected).abs() < 1e-6);
        }
    }
}
