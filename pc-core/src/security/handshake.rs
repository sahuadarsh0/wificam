//! Ephemeral P-256 ECDH key agreement and HKDF-SHA256 session key derivation.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use p256::{PublicKey, SecretKey, ecdh::diffie_hellman};
use sha2::{Digest, Sha256};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

const HKDF_INFO: &[u8] = b"wificam-v2-session-keys";

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("OS CSPRNG failure: {0}")]
    Rng(String),

    #[error("invalid peer public key: {0}")]
    InvalidPublicKey(String),

    #[error("HKDF key derivation failed")]
    KeyDerivation,

    #[error("AEAD encryption failed")]
    EncryptFailed,

    #[error("AEAD decryption/authentication failed")]
    DecryptFailed,

    #[error("packet seq {seq} rejected by replay window (highest={highest})")]
    ReplayDetected { seq: u32, highest: u32 },

    #[error("packet is not marked ENCRYPTED")]
    NotEncrypted,

    #[error("invalid pairing URI: {0}")]
    InvalidPairingUri(String),
}

/// Direction of a media/control packet, ensuring independent keys & nonces for
/// phone -> PC vs PC -> phone traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Direction {
    PhoneToPc = 0x01,
    PcToPhone = 0x02,
}

/// Derived symmetric session keys & per-direction nonce prefixes (zeroized on drop).
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SessionKeys {
    pub phone_to_pc_key: [u8; 32],
    pub pc_to_phone_key: [u8; 32],
    pub phone_to_pc_salt: [u8; 6],
    pub pc_to_phone_salt: [u8; 6],
}

/// Ephemeral or identity P-256 keypair.
pub struct KeyPair {
    secret: SecretKey,
}

impl KeyPair {
    /// Generate a fresh P-256 keypair from the OS CSPRNG.
    pub fn generate() -> Result<Self, SecurityError> {
        let mut raw = [0u8; 32];
        loop {
            getrandom::fill(&mut raw).map_err(|e| SecurityError::Rng(e.to_string()))?;
            if let Ok(secret) = SecretKey::from_slice(&raw) {
                raw.zeroize();
                return Ok(Self { secret });
            }
        }
    }

    /// Uncompressed SEC1 public key bytes (65 bytes: `0x04 || X || Y`).
    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.secret.public_key().to_sec1_bytes().into_vec()
    }

    /// Base64url (no padding) encoded SEC1 public key for QR / JSON handshake.
    pub fn public_key_b64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.public_key_bytes())
    }

    /// Short human-readable SHA-256 fingerprint (e.g. `A1B2-C3D4-E5F6-7890`)
    /// displayed on both PC and phone during TOFU consent.
    pub fn fingerprint(&self) -> String {
        fingerprint_of_pubkey(&self.public_key_bytes())
    }

    /// Perform P-256 ECDH with the peer's base64url SEC1 public key and derive
    /// directional AES-256-GCM keys via HKDF-SHA256.
    pub fn derive_session_keys(
        &self,
        peer_pub_b64: &str,
        pairing_salt: &[u8],
    ) -> Result<SessionKeys, SecurityError> {
        let peer_bytes = URL_SAFE_NO_PAD
            .decode(peer_pub_b64)
            .map_err(|e| SecurityError::InvalidPublicKey(e.to_string()))?;
        let peer_pub = PublicKey::from_sec1_bytes(&peer_bytes)
            .map_err(|e| SecurityError::InvalidPublicKey(e.to_string()))?;

        let shared = diffie_hellman(self.secret.to_nonzero_scalar(), peer_pub.as_affine());
        let hkdf = Hkdf::<Sha256>::new(Some(pairing_salt), shared.raw_secret_bytes().as_slice());

        let mut okm = [0u8; 76];
        hkdf.expand(HKDF_INFO, &mut okm)
            .map_err(|_| SecurityError::KeyDerivation)?;

        let mut keys = SessionKeys {
            phone_to_pc_key: [0u8; 32],
            pc_to_phone_key: [0u8; 32],
            phone_to_pc_salt: [0u8; 6],
            pc_to_phone_salt: [0u8; 6],
        };
        keys.phone_to_pc_key.copy_from_slice(&okm[0..32]);
        keys.pc_to_phone_key.copy_from_slice(&okm[32..64]);
        keys.phone_to_pc_salt.copy_from_slice(&okm[64..70]);
        keys.pc_to_phone_salt.copy_from_slice(&okm[70..76]);
        okm.zeroize();
        Ok(keys)
    }
}

/// Compute a 64-bit uppercase hex fingerprint (`XXXX-XXXX-XXXX-XXXX`) from
/// raw SEC1 public key bytes.
pub fn fingerprint_of_pubkey(pub_bytes: &[u8]) -> String {
    let digest = Sha256::digest(pub_bytes);
    format!(
        "{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7]
    )
}
