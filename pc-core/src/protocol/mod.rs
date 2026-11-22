//! Binary Wire Protocol v2 — shared across USB (AOA/ADB), Wi-Fi, and Bluetooth.

pub mod codec;
pub mod control;
pub mod crc;
pub mod fragmenter;
pub mod packet;

pub use codec::{CodecError, PacketCodec};
pub use control::{AudioFormatChoice, ControlMessage, StreamConfig, VideoCodecChoice};
pub use crc::crc16_ccitt_chunks;
pub use fragmenter::{
    DEFAULT_FRAGMENT_SIZE, PriorityTxQueue, ReassembleOutcome, ReassembledFrame, VideoReassembler,
    fragment_video_frame,
};
pub use packet::{
    HEADER_LEN, MAX_PAYLOAD_LEN, PACKET_MAGIC, PROTOCOL_VERSION, Packet, PacketFlags, PacketHeader,
    ProtocolError, StreamType,
};

#[cfg(test)]
mod tests {
    use bytes::{Bytes, BytesMut};
    use tokio_util::codec::{Decoder, Encoder};

    use super::*;

    #[test]
    fn round_trip_video_keyframe_packet() {
        let pkt = Packet::new(
            StreamType::VideoH264,
            PacketFlags::KEYFRAME.union(PacketFlags::CRC_PRESENT),
            42,
            1_234_567_890,
            Bytes::from_static(b"\x00\x00\x00\x01\x65idr-payload"),
        );
        let wire = pkt.encode().expect("encode");
        assert_eq!(wire.len(), HEADER_LEN + pkt.payload.len());

        let decoded = Packet::decode(&wire).expect("decode");
        assert_eq!(decoded, pkt);
    }

    #[test]
    fn rejects_invalid_magic_and_version_and_crc() {
        let pkt = Packet::new(
            StreamType::AudioOpus,
            PacketFlags::HAS_FEC.union(PacketFlags::CRC_PRESENT),
            7,
            20_000,
            Bytes::from_static(b"opus-frame"),
        );
        let mut wire = pkt.encode().unwrap();

        // Corrupt payload -> CRC mismatch
        let last = wire.len() - 1;
        wire[last] ^= 0x01;
        assert!(matches!(
            Packet::decode(&wire),
            Err(ProtocolError::CrcMismatch { .. })
        ));

        // Corrupt version -> UnsupportedVersion
        let mut bad_ver = pkt.encode().unwrap();
        bad_ver[2] = 99;
        assert_eq!(
            Packet::decode(&bad_ver),
            Err(ProtocolError::UnsupportedVersion(99))
        );

        // Corrupt magic -> InvalidMagic
        let mut bad_magic = pkt.encode().unwrap();
        bad_magic[0] = 0xFF;
        assert!(matches!(
            Packet::decode(&bad_magic),
            Err(ProtocolError::InvalidMagic(_))
        ));
    }

    #[test]
    fn video_fragmentation_and_audio_preemption() {
        // Simulate a 25 KiB 4K keyframe split into 8 KiB chunks (4 packets).
        let big_frame = Bytes::from((0..25_000u32).map(|i| (i % 251) as u8).collect::<Vec<_>>());
        let mut v_seq = 100u32;
        let frags = fragment_video_frame(
            StreamType::VideoH265,
            PacketFlags::KEYFRAME,
            &mut v_seq,
            5_000_000,
            big_frame.clone(),
            DEFAULT_FRAGMENT_SIZE,
        );
        assert_eq!(frags.len(), 4);
        assert_eq!(v_seq, 104);
        assert!(frags[0].flags.contains(PacketFlags::FRAG_START));
        assert!(!frags[1].flags.contains(PacketFlags::FRAG_START));
        assert!(!frags[1].flags.contains(PacketFlags::FRAG_END));
        assert!(frags[3].flags.contains(PacketFlags::FRAG_END));

        // Enqueue video fragments, then inject an urgent audio packet mid-burst.
        let mut tx = PriorityTxQueue::new();
        for f in frags {
            tx.push(f);
        }
        let audio_pkt = Packet::new(
            StreamType::AudioOpus,
            PacketFlags::HAS_FEC,
            1,
            5_001_000,
            Bytes::from_static(b"10ms-voice"),
        );
        tx.push(audio_pkt.clone());

        // First popped packet MUST be the voice packet, not a video fragment!
        assert_eq!(tx.pop_next(), Some(audio_pkt));

        // Remaining 4 packets reassemble back into the exact 25 KiB video frame.
        let mut reassembler = VideoReassembler::new();
        let mut result = None;
        while let Some(pkt) = tx.pop_next() {
            match reassembler.push(pkt) {
                ReassembleOutcome::Pending => {}
                ReassembleOutcome::Complete(frame) => result = Some(frame),
                ReassembleOutcome::DroppedIncomplete { .. } => panic!("unexpected drop"),
            }
        }
        let frame = result.expect("reassembled frame");
        assert_eq!(frame.stream_type, StreamType::VideoH265);
        assert!(frame.flags.contains(PacketFlags::KEYFRAME));
        assert_eq!(frame.pts_us, 5_000_000);
        assert_eq!(frame.data, big_frame);
    }

    #[test]
    fn reassembler_detects_missing_fragment_and_recovers_next_frame() {
        let frame1 = Bytes::from(vec![0xAAu8; 20_000]);
        let mut seq = 1u32;
        let mut frags = fragment_video_frame(
            StreamType::VideoH264,
            PacketFlags::NONE,
            &mut seq,
            1000,
            frame1,
            DEFAULT_FRAGMENT_SIZE,
        );
        assert_eq!(frags.len(), 3);
        frags.remove(1); // Drop middle fragment!

        let mut reassembler = VideoReassembler::new();
        assert_eq!(
            reassembler.push(frags.remove(0)),
            ReassembleOutcome::Pending
        );
        assert_eq!(
            reassembler.push(frags.remove(0)),
            ReassembleOutcome::DroppedIncomplete { recovered: None }
        );

        // Next clean unfragmented IDR recovers immediately.
        let idr = Packet::new(
            StreamType::VideoH264,
            PacketFlags::KEYFRAME,
            seq,
            2000,
            Bytes::from_static(b"clean-idr"),
        );
        match reassembler.push(idr) {
            ReassembleOutcome::Complete(f) => assert_eq!(f.data.as_ref(), b"clean-idr"),
            other => panic!("expected complete IDR, got {other:?}"),
        }
    }

    #[test]
    fn streaming_codec_handles_partial_reads() {
        let mut codec = PacketCodec;
        let p1 = Packet::new(
            StreamType::Heartbeat,
            PacketFlags::NONE,
            1,
            100,
            Bytes::new(),
        );
        let p2 = Packet::new(
            StreamType::AudioPcm16,
            PacketFlags::CRC_PRESENT,
            2,
            200,
            Bytes::from_static(b"pcm-samples"),
        );

        let mut wire = BytesMut::new();
        codec.encode(&p1, &mut wire).unwrap();
        codec.encode(&p2, &mut wire).unwrap();

        // Drip-feed 5 bytes at a time into decoder.
        let mut rx_buf = BytesMut::new();
        let mut decoded = Vec::new();
        for chunk in wire.chunks(5) {
            rx_buf.extend_from_slice(chunk);
            while let Some(pkt) = codec.decode(&mut rx_buf).unwrap() {
                decoded.push(pkt);
            }
        }
        assert_eq!(decoded, vec![p1, p2]);
    }

    #[test]
    fn control_message_round_trip() {
        let msg = ControlMessage::Configure(StreamConfig {
            width: 3840,
            height: 2160,
            fps: 30,
            video_bitrate_kbps: 24_000,
            video_codec: VideoCodecChoice::H265,
            audio_sample_rate: 48_000,
            audio_channels: 1,
            audio_format: AudioFormatChoice::Pcm16,
            lens_id: "0".to_string(),
            antibanding_hz: 50,
        });
        let pkt = msg.into_packet(9, 12345).unwrap();
        let back = ControlMessage::from_packet(&pkt).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn golden_vectors_match_wire_format_byte_for_byte() {
        #[derive(serde::Deserialize)]
        struct VectorFile {
            vectors: Vec<GoldenVector>,
        }
        #[derive(serde::Deserialize)]
        struct GoldenVector {
            name: String,
            stream_type: u8,
            flags: u16,
            seq: u32,
            pts_us: u64,
            payload_hex: String,
            wire_hex: String,
        }

        let json_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../proto/golden_vectors.json");
        let text = std::fs::read_to_string(json_path).expect("read golden_vectors.json");
        let vf: VectorFile = serde_json::from_str(&text).expect("parse golden_vectors.json");
        assert!(!vf.vectors.is_empty());

        for v in vf.vectors {
            let payload = decode_hex(&v.payload_hex);
            let expected_wire = decode_hex(&v.wire_hex);
            let pkt = Packet::new(
                StreamType::from_u8(v.stream_type).unwrap(),
                PacketFlags::from_bits(v.flags),
                v.seq,
                v.pts_us,
                payload,
            );
            let encoded = pkt.encode().unwrap();
            assert_eq!(
                encode_hex(&encoded),
                v.wire_hex,
                "golden vector '{}' encode mismatch",
                v.name
            );
            let decoded = Packet::decode(&expected_wire).expect("decode golden vector");
            assert_eq!(decoded, pkt, "golden vector '{}' decode mismatch", v.name);
        }
    }

    fn decode_hex(s: &str) -> Vec<u8> {
        assert!(s.len() % 2 == 0);
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn encode_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
