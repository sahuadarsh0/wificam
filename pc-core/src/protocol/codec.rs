//! `tokio_util::codec` streaming framing for TCP, USB AOA/ADB, and BT RFCOMM.

use std::io;

use bytes::{Buf, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use super::packet::{HEADER_LEN, Packet, PacketHeader, ProtocolError};

/// Streaming length-prefixed [`Packet`] codec for byte-stream transports.
#[derive(Debug, Default, Clone, Copy)]
pub struct PacketCodec;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

impl Decoder for PacketCodec {
    type Item = Packet;
    type Error = CodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < HEADER_LEN {
            return Ok(None);
        }
        let header = PacketHeader::decode(&src[..HEADER_LEN])?;
        let frame_len = HEADER_LEN + header.payload_len as usize;
        if src.len() < frame_len {
            src.reserve(frame_len - src.len());
            return Ok(None);
        }
        let pkt = Packet::decode(&src[..frame_len])?;
        src.advance(frame_len);
        Ok(Some(pkt))
    }
}

impl Encoder<&Packet> for PacketCodec {
    type Error = CodecError;

    fn encode(&mut self, item: &Packet, dst: &mut BytesMut) -> Result<(), Self::Error> {
        item.encode_into(dst)?;
        Ok(())
    }
}

impl Encoder<Packet> for PacketCodec {
    type Error = CodecError;

    fn encode(&mut self, item: Packet, dst: &mut BytesMut) -> Result<(), Self::Error> {
        item.encode_into(dst)?;
        Ok(())
    }
}
