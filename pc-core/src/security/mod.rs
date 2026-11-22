//! LAN security layer: P-256 ECDH key agreement, AES-256-GCM AEAD with
//! sliding-window replay protection, and TOFU QR pairing.

pub mod cipher;
pub mod handshake;
pub mod pairing;

pub use cipher::{AEAD_TAG_LEN, SessionCipher};
pub use handshake::{Direction, KeyPair, SecurityError, SessionKeys, fingerprint_of_pubkey};
pub use pairing::{PairingPayload, TofuTrustStore, TrustStatus, TrustedPeer};

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::brand::{DEFAULT_CONTROL_PORT, DEFAULT_MEDIA_UDP_PORT};
    use crate::protocol::{Packet, PacketFlags, StreamType};

    use super::*;

    #[test]
    fn ecdh_and_bidirectional_aead_round_trip() {
        let pc_kp = KeyPair::generate().unwrap();
        let phone_kp = KeyPair::generate().unwrap();
        let salt = b"ephemeral-pairing-salt";

        let pc_keys = pc_kp
            .derive_session_keys(&phone_kp.public_key_b64(), salt)
            .unwrap();
        let phone_keys = phone_kp
            .derive_session_keys(&pc_kp.public_key_b64(), salt)
            .unwrap();

        assert_eq!(pc_keys.phone_to_pc_key, phone_keys.phone_to_pc_key);
        assert_eq!(pc_keys.pc_to_phone_key, phone_keys.pc_to_phone_key);
        assert_ne!(pc_keys.phone_to_pc_key, pc_keys.pc_to_phone_key);

        let mut pc_cipher = SessionCipher::new(&pc_keys, Direction::PcToPhone);
        let mut phone_cipher = SessionCipher::new(&phone_keys, Direction::PhoneToPc);

        // Phone -> PC audio packet
        let voice = Packet::new(
            StreamType::AudioOpus,
            PacketFlags::HAS_FEC,
            1,
            10_000,
            Bytes::from_static(b"secret-opus-frame"),
        );
        let sealed = phone_cipher.seal(&voice).unwrap();
        assert!(sealed.flags.contains(PacketFlags::ENCRYPTED));
        assert_ne!(sealed.payload, voice.payload);
        assert_eq!(sealed.payload.len(), voice.payload.len() + AEAD_TAG_LEN);

        let opened = pc_cipher.open(&sealed).unwrap();
        assert_eq!(opened, voice);

        // PC -> Phone control packet
        let ctrl = Packet::new(
            StreamType::Control,
            PacketFlags::NONE,
            1,
            12_000,
            Bytes::from_static(br#"{"type":"request_idr"}"#),
        );
        let sealed_ctrl = pc_cipher.seal(&ctrl).unwrap();
        assert_eq!(phone_cipher.open(&sealed_ctrl).unwrap(), ctrl);
    }

    #[test]
    fn rejects_tampered_aad_and_ciphertext_and_replays() {
        let pc_kp = KeyPair::generate().unwrap();
        let phone_kp = KeyPair::generate().unwrap();
        let salt = b"salt";
        let keys = pc_kp
            .derive_session_keys(&phone_kp.public_key_b64(), salt)
            .unwrap();

        let mut pc = SessionCipher::new(&keys, Direction::PcToPhone);
        let phone = SessionCipher::new(&keys, Direction::PhoneToPc);

        let pkt = Packet::new(
            StreamType::VideoH264,
            PacketFlags::KEYFRAME,
            10,
            100_000,
            Bytes::from_static(b"video-idr"),
        );
        let sealed = phone.seal(&pkt).unwrap();

        // 1. Tamper with header metadata (pts_us -> AAD mismatch)
        let mut tampered_aad = sealed.clone();
        tampered_aad.pts_us += 1;
        assert!(matches!(
            pc.open(&tampered_aad),
            Err(SecurityError::DecryptFailed)
        ));

        // 2. Tamper with ciphertext byte
        let mut tampered_ct = sealed.clone();
        let mut bytes = tampered_ct.payload.to_vec();
        bytes[0] ^= 0x80;
        tampered_ct.payload = Bytes::from(bytes);
        assert!(matches!(
            pc.open(&tampered_ct),
            Err(SecurityError::DecryptFailed)
        ));

        // 3. Valid packet still succeeds (forged attempts did NOT poison replay window!)
        assert_eq!(pc.open(&sealed).unwrap(), pkt);

        // 4. Exact replay is rejected
        assert!(matches!(
            pc.open(&sealed),
            Err(SecurityError::ReplayDetected {
                seq: 10,
                highest: 10
            })
        ));
    }

    #[test]
    fn sliding_replay_window_allows_out_of_order_udp_and_rejects_stale() {
        let pc_kp = KeyPair::generate().unwrap();
        let phone_kp = KeyPair::generate().unwrap();
        let keys = pc_kp
            .derive_session_keys(&phone_kp.public_key_b64(), b"udp")
            .unwrap();
        let mut pc = SessionCipher::new(&keys, Direction::PcToPhone);
        let phone = SessionCipher::new(&keys, Direction::PhoneToPc);

        let make = |seq: u32| {
            phone
                .seal(&Packet::new(
                    StreamType::AudioOpus,
                    PacketFlags::NONE,
                    seq,
                    seq as u64 * 10_000,
                    Bytes::from_static(b"pkt"),
                ))
                .unwrap()
        };

        // Receive seq 100 first, then out-of-order seq 95 -> allowed!
        assert!(pc.open(&make(100)).is_ok());
        assert!(pc.open(&make(95)).is_ok());
        // Duplicate seq 95 -> rejected!
        assert!(matches!(
            pc.open(&make(95)),
            Err(SecurityError::ReplayDetected {
                seq: 95,
                highest: 100
            })
        ));
        // Too old (seq 36 is 64 behind 100) -> rejected!
        assert!(matches!(
            pc.open(&make(36)),
            Err(SecurityError::ReplayDetected {
                seq: 36,
                highest: 100
            })
        ));
        // Edge of window (seq 37 is 63 behind 100) -> accepted!
        assert!(pc.open(&make(37)).is_ok());
    }

    #[test]
    fn qr_pairing_uri_and_tofu_store_round_trip() {
        let kp = KeyPair::generate().unwrap();
        let payload = PairingPayload {
            ip: "192.168.1.42".into(),
            control_port: DEFAULT_CONTROL_PORT,
            media_udp_port: DEFAULT_MEDIA_UDP_PORT,
            host_name: "Adarsh's PC".into(),
            host_pub_b64: kp.public_key_b64(),
            token_b64: PairingPayload::generate_token().unwrap(),
        };

        let uri = payload.to_uri();
        assert!(uri.starts_with("wificam://pair?"));
        assert_eq!(PairingPayload::from_uri(&uri).unwrap(), payload);

        let mut store = TofuTrustStore::new();
        let fp = kp.fingerprint();
        assert_eq!(store.verify("pixel-9", &fp), TrustStatus::Unknown);

        store.trust(TrustedPeer {
            device_id: "pixel-9".into(),
            display_name: "Pixel 9".into(),
            fingerprint: fp.clone(),
        });
        assert_eq!(store.verify("pixel-9", &fp), TrustStatus::Trusted);
        assert_eq!(
            store.verify("pixel-9", "FFFF-FFFF-FFFF-FFFF"),
            TrustStatus::FingerprintMismatch
        );

        let json = store.to_json().unwrap();
        assert_eq!(TofuTrustStore::from_json(&json).unwrap(), store);
        assert!(store.revoke("pixel-9"));
        assert_eq!(store.verify("pixel-9", &fp), TrustStatus::Unknown);
    }
}
