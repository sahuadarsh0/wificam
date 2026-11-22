//! AES-256-GCM packet AEAD with deterministic 96-bit nonces and a 64-packet
//! per-stream sliding-window replay guard.

use std::collections::HashMap;

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use bytes::Bytes;

use crate::protocol::{Packet, PacketFlags, StreamType};

use super::handshake::{Direction, SecurityError, SessionKeys};

/// Size of the AES-GCM authentication tag appended to ciphertext payloads.
pub const AEAD_TAG_LEN: usize = 16;

/// 64-packet sliding-window replay detector for a single stream.
///
/// Tolerates out-of-order UDP datagrams up to 63 sequence numbers behind the
/// highest authenticated sequence while rejecting duplicates and stale packets.
#[derive(Debug, Default)]
struct ReplayWindow {
    initialized: bool,
    highest: u32,
    bitmap: u64,
}

impl ReplayWindow {
    fn check(&self, seq: u32) -> Result<(), SecurityError> {
        if !self.initialized {
            return Ok(());
        }
        if seq > self.highest {
            return Ok(());
        }
        let diff = self.highest - seq;
        if diff >= 64 {
            return Err(SecurityError::ReplayDetected {
                seq,
                highest: self.highest,
            });
        }
        if (self.bitmap & (1u64 << diff)) != 0 {
            return Err(SecurityError::ReplayDetected {
                seq,
                highest: self.highest,
            });
        }
        Ok(())
    }

    fn commit(&mut self, seq: u32) {
        if !self.initialized {
            self.initialized = true;
            self.highest = seq;
            self.bitmap = 1;
            return;
        }
        if seq > self.highest {
            let shift = seq - self.highest;
            self.bitmap = if shift >= 64 {
                1
            } else {
                (self.bitmap << shift) | 1
            };
            self.highest = seq;
        } else {
            let diff = self.highest - seq;
            if diff < 64 {
                self.bitmap |= 1u64 << diff;
            }
        }
    }
}

/// Bidirectional AES-256-GCM session cipher for encrypting and authenticating
/// wire [`Packet`]s on LAN / Wi-Fi connections.
pub struct SessionCipher {
    tx_cipher: Aes256Gcm,
    rx_cipher: Aes256Gcm,
    tx_dir: Direction,
    rx_dir: Direction,
    tx_salt: [u8; 6],
    rx_salt: [u8; 6],
    replay_windows: HashMap<StreamType, ReplayWindow>,
}

impl SessionCipher {
    /// Create a [`SessionCipher`] for one endpoint of a session.
    ///
    /// Pass `local_tx_dir = Direction::PcToPhone` on the PC and
    /// `Direction::PhoneToPc` on Android.
    pub fn new(keys: &SessionKeys, local_tx_dir: Direction) -> Self {
        let (tx_key, rx_key, tx_salt, rx_salt, rx_dir) = match local_tx_dir {
            Direction::PcToPhone => (
                &keys.pc_to_phone_key,
                &keys.phone_to_pc_key,
                keys.pc_to_phone_salt,
                keys.phone_to_pc_salt,
                Direction::PhoneToPc,
            ),
            Direction::PhoneToPc => (
                &keys.phone_to_pc_key,
                &keys.pc_to_phone_key,
                keys.phone_to_pc_salt,
                keys.pc_to_phone_salt,
                Direction::PcToPhone,
            ),
        };

        Self {
            tx_cipher: Aes256Gcm::new(&Key::<Aes256Gcm>::from(*tx_key)),
            rx_cipher: Aes256Gcm::new(&Key::<Aes256Gcm>::from(*rx_key)),
            tx_dir: local_tx_dir,
            rx_dir,
            tx_salt,
            rx_salt,
            replay_windows: HashMap::new(),
        }
    }

    /// Encrypt a plaintext [`Packet`] in-place into an authenticated ciphertext
    /// packet with `PacketFlags::ENCRYPTED` set.
    ///
    /// Header metadata (magic, version, stream_type, flags, seq, pts_us,
    /// ciphertext length) is authenticated as AEAD Associated Data (AAD).
    pub fn seal(&self, pkt: &Packet) -> Result<Packet, SecurityError> {
        let flags = pkt.flags.union(PacketFlags::ENCRYPTED);
        let nonce_bytes = build_nonce(self.tx_dir, pkt.stream_type, &self.tx_salt, pkt.seq);
        let nonce = Nonce::from(nonce_bytes);
        let aad = build_aad(
            pkt.stream_type,
            flags,
            pkt.seq,
            pkt.pts_us,
            (pkt.payload.len() + AEAD_TAG_LEN) as u32,
        );

        let ciphertext = self
            .tx_cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &pkt.payload,
                    aad: &aad,
                },
            )
            .map_err(|_| SecurityError::EncryptFailed)?;

        Ok(Packet::new(
            pkt.stream_type,
            flags,
            pkt.seq,
            pkt.pts_us,
            Bytes::from(ciphertext),
        ))
    }

    /// Authenticate and decrypt a received [`Packet`], enforcing the per-stream
    /// 64-packet sliding-window replay guard.
    pub fn open(&mut self, pkt: &Packet) -> Result<Packet, SecurityError> {
        if !pkt.flags.contains(PacketFlags::ENCRYPTED) {
            return Err(SecurityError::NotEncrypted);
        }

        // Pre-check replay window before doing AEAD work; only commit AFTER
        // authentication succeeds so forged packets cannot advance the window!
        let window = self.replay_windows.entry(pkt.stream_type).or_default();
        window.check(pkt.seq)?;

        let nonce_bytes = build_nonce(self.rx_dir, pkt.stream_type, &self.rx_salt, pkt.seq);
        let nonce = Nonce::from(nonce_bytes);
        let aad = build_aad(
            pkt.stream_type,
            pkt.flags,
            pkt.seq,
            pkt.pts_us,
            pkt.payload.len() as u32,
        );

        let plaintext = self
            .rx_cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &pkt.payload,
                    aad: &aad,
                },
            )
            .map_err(|_| SecurityError::DecryptFailed)?;

        window.commit(pkt.seq);

        Ok(Packet::new(
            pkt.stream_type,
            pkt.flags.without(PacketFlags::ENCRYPTED),
            pkt.seq,
            pkt.pts_us,
            Bytes::from(plaintext),
        ))
    }
}

fn build_nonce(dir: Direction, stream: StreamType, salt: &[u8; 6], seq: u32) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[0] = dir as u8;
    nonce[1] = stream as u8;
    nonce[2..8].copy_from_slice(salt);
    nonce[8..12].copy_from_slice(&seq.to_be_bytes());
    nonce
}

fn build_aad(
    stream_type: StreamType,
    flags: PacketFlags,
    seq: u32,
    pts_us: u64,
    ciphertext_len: u32,
) -> [u8; 22] {
    let mut aad = [0u8; 22];
    aad[0..2].copy_from_slice(&crate::protocol::PACKET_MAGIC.to_be_bytes());
    aad[2] = crate::protocol::PROTOCOL_VERSION;
    aad[3] = stream_type as u8;
    aad[4..6].copy_from_slice(&flags.bits().to_be_bytes());
    aad[6..10].copy_from_slice(&seq.to_be_bytes());
    aad[10..18].copy_from_slice(&pts_us.to_be_bytes());
    aad[18..22].copy_from_slice(&ciphertext_len.to_be_bytes());
    aad
}
