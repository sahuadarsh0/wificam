//! Wire Protocol v2 packet header & serialization (24-byte big-endian header).

use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;

use super::crc::crc16_ccitt_chunks;

/// ASCII `"CM"` (`0x434D`) magic bytes at the start of every packet.
pub const PACKET_MAGIC: u16 = 0x434D;

/// Current binary wire protocol version.
pub const PROTOCOL_VERSION: u8 = 2;

/// Fixed packet header size in bytes.
pub const HEADER_LEN: usize = 24;

/// Maximum permitted payload length (4 MiB) to guard against OOM from corrupt frames.
pub const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// Logical media or signaling stream carried inside a [`Packet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum StreamType {
    /// Annex-B H.264 NAL units.
    VideoH264 = 1,
    /// Annex-B HEVC (H.265) NAL units.
    VideoH265 = 2,
    /// Raw uncompressed 48 kHz 16-bit signed little-endian PCM (USB low-latency path).
    AudioPcm16 = 3,
    /// 48 kHz Opus encoded frame (10 ms or 20 ms, Wi-Fi / Bluetooth path).
    AudioOpus = 4,
    /// UTF-8 JSON control/handshake message.
    Control = 5,
    /// Keepalive & RTT probe heartbeat.
    Heartbeat = 6,
}

impl StreamType {
    pub fn from_u8(v: u8) -> Result<Self, ProtocolError> {
        match v {
            1 => Ok(Self::VideoH264),
            2 => Ok(Self::VideoH265),
            3 => Ok(Self::AudioPcm16),
            4 => Ok(Self::AudioOpus),
            5 => Ok(Self::Control),
            6 => Ok(Self::Heartbeat),
            other => Err(ProtocolError::UnknownStreamType(other)),
        }
    }

    /// True if this stream carries latency-critical voice audio that must
    /// preempt queued video fragments.
    pub fn is_audio(self) -> bool {
        matches!(self, Self::AudioPcm16 | Self::AudioOpus)
    }

    /// True if this stream carries compressed video frames.
    pub fn is_video(self) -> bool {
        matches!(self, Self::VideoH264 | Self::VideoH265)
    }
}

/// Bitflag wrapper for the 16-bit `flags` field in [`PacketHeader`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PacketFlags(u16);

impl PacketFlags {
    pub const NONE: Self = Self(0);
    /// Video IDR keyframe.
    pub const KEYFRAME: Self = Self(1 << 0);
    /// Codec configuration data (SPS/PPS/VPS).
    pub const CONFIG: Self = Self(1 << 1);
    /// Opus packet carries in-band Forward Error Correction (FEC).
    pub const HAS_FEC: Self = Self(1 << 2);
    /// First chunk of a multi-packet fragmented video frame.
    pub const FRAG_START: Self = Self(1 << 3);
    /// Final chunk of a multi-packet fragmented video frame.
    pub const FRAG_END: Self = Self(1 << 4);
    /// Payload is AES-256-GCM ciphertext + 16-byte tag (header `[0..22]` is AAD).
    pub const ENCRYPTED: Self = Self(1 << 5);
    /// `crc16` field carries CRC-16/CCITT-FALSE over header `[0..22]` + payload.
    pub const CRC_PRESENT: Self = Self(1 << 6);

    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

/// Decoded 24-byte packet header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    pub stream_type: StreamType,
    pub flags: PacketFlags,
    pub seq: u32,
    pub pts_us: u64,
    pub payload_len: u32,
    pub crc16: u16,
}

impl PacketHeader {
    /// Encode header into a 24-byte array (big-endian).
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut buf = [0u8; HEADER_LEN];
        buf[0..2].copy_from_slice(&PACKET_MAGIC.to_be_bytes());
        buf[2] = PROTOCOL_VERSION;
        buf[3] = self.stream_type as u8;
        buf[4..6].copy_from_slice(&self.flags.bits().to_be_bytes());
        buf[6..10].copy_from_slice(&self.seq.to_be_bytes());
        buf[10..18].copy_from_slice(&self.pts_us.to_be_bytes());
        buf[18..22].copy_from_slice(&self.payload_len.to_be_bytes());
        buf[22..24].copy_from_slice(&self.crc16.to_be_bytes());
        buf
    }

    /// The first 22 bytes of the header, used as AAD for AES-256-GCM and as
    /// input to CRC-16.
    pub fn aad_bytes(&self) -> [u8; 22] {
        let full = self.to_bytes();
        let mut aad = [0u8; 22];
        aad.copy_from_slice(&full[..22]);
        aad
    }

    /// Parse and validate a 24-byte big-endian header.
    pub fn decode(mut raw: &[u8]) -> Result<Self, ProtocolError> {
        if raw.len() < HEADER_LEN {
            return Err(ProtocolError::TruncatedHeader {
                got: raw.len(),
                expected: HEADER_LEN,
            });
        }
        let magic = raw.get_u16();
        if magic != PACKET_MAGIC {
            return Err(ProtocolError::InvalidMagic(magic));
        }
        let version = raw.get_u8();
        if version != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion(version));
        }
        let stream_type = StreamType::from_u8(raw.get_u8())?;
        let flags = PacketFlags::from_bits(raw.get_u16());
        let seq = raw.get_u32();
        let pts_us = raw.get_u64();
        let payload_len = raw.get_u32();
        let crc16 = raw.get_u16();

        if (payload_len as usize) > MAX_PAYLOAD_LEN {
            return Err(ProtocolError::PayloadTooLarge {
                len: payload_len as usize,
                max: MAX_PAYLOAD_LEN,
            });
        }

        Ok(Self {
            stream_type,
            flags,
            seq,
            pts_us,
            payload_len,
            crc16,
        })
    }
}

/// Complete wire packet (header + payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub stream_type: StreamType,
    pub flags: PacketFlags,
    pub seq: u32,
    pub pts_us: u64,
    pub payload: Bytes,
}

impl Packet {
    /// Construct a new packet.
    pub fn new(
        stream_type: StreamType,
        flags: PacketFlags,
        seq: u32,
        pts_us: u64,
        payload: impl Into<Bytes>,
    ) -> Self {
        Self {
            stream_type,
            flags,
            seq,
            pts_us,
            payload: payload.into(),
        }
    }

    /// Build the [`PacketHeader`] for this packet, computing CRC-16 if
    /// `PacketFlags::CRC_PRESENT` is set.
    pub fn header(&self) -> Result<PacketHeader, ProtocolError> {
        if self.payload.len() > MAX_PAYLOAD_LEN {
            return Err(ProtocolError::PayloadTooLarge {
                len: self.payload.len(),
                max: MAX_PAYLOAD_LEN,
            });
        }
        let mut hdr = PacketHeader {
            stream_type: self.stream_type,
            flags: self.flags,
            seq: self.seq,
            pts_us: self.pts_us,
            payload_len: self.payload.len() as u32,
            crc16: 0,
        };
        if self.flags.contains(PacketFlags::CRC_PRESENT) {
            let aad = hdr.aad_bytes();
            hdr.crc16 = crc16_ccitt_chunks(&[&aad, &self.payload]);
        }
        Ok(hdr)
    }

    /// Encode this packet into a fresh [`BytesMut`] buffer.
    pub fn encode(&self) -> Result<BytesMut, ProtocolError> {
        let mut out = BytesMut::with_capacity(HEADER_LEN + self.payload.len());
        self.encode_into(&mut out)?;
        Ok(out)
    }

    /// Append this packet's wire representation into `dst`.
    pub fn encode_into(&self, dst: &mut BytesMut) -> Result<(), ProtocolError> {
        let hdr = self.header()?;
        dst.reserve(HEADER_LEN + self.payload.len());
        dst.put_slice(&hdr.to_bytes());
        dst.put_slice(&self.payload);
        Ok(())
    }

    /// Decode a single complete packet from `raw` (exact datagram / frame).
    pub fn decode(raw: &[u8]) -> Result<Self, ProtocolError> {
        let hdr = PacketHeader::decode(raw)?;
        let expected_total = HEADER_LEN + hdr.payload_len as usize;
        if raw.len() < expected_total {
            return Err(ProtocolError::TruncatedPayload {
                got: raw.len() - HEADER_LEN,
                expected: hdr.payload_len as usize,
            });
        }
        let payload = &raw[HEADER_LEN..expected_total];
        if hdr.flags.contains(PacketFlags::CRC_PRESENT) {
            let computed = crc16_ccitt_chunks(&[&raw[..22], payload]);
            if computed != hdr.crc16 {
                return Err(ProtocolError::CrcMismatch {
                    expected: hdr.crc16,
                    actual: computed,
                });
            }
        }
        Ok(Self {
            stream_type: hdr.stream_type,
            flags: hdr.flags,
            seq: hdr.seq,
            pts_us: hdr.pts_us,
            payload: Bytes::copy_from_slice(payload),
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("truncated packet header: got {got} bytes, expected {expected}")]
    TruncatedHeader { got: usize, expected: usize },

    #[error("invalid packet magic: 0x{0:04X} (expected 0x434D)")]
    InvalidMagic(u16),

    #[error("unsupported protocol version: {0} (expected 2)")]
    UnsupportedVersion(u8),

    #[error("unknown stream type: {0}")]
    UnknownStreamType(u8),

    #[error("payload length {len} exceeds maximum {max}")]
    PayloadTooLarge { len: usize, max: usize },

    #[error("truncated payload: got {got} bytes, expected {expected}")]
    TruncatedPayload { got: usize, expected: usize },

    #[error("CRC-16 mismatch: expected 0x{expected:04X}, got 0x{actual:04X}")]
    CrcMismatch { expected: u16, actual: u16 },
}
