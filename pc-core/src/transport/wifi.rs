//! Encrypted Wi-Fi LAN dual-socket transport:
//! - **TCP Control Channel** (`TCP_NODELAY`): P-256 ECDH handshake, TOFU
//!   consent, configuration, IDR requests, and USB `adb reverse` fallback.
//! - **UDP Media Channel** (2 MiB OS buffers): AES-256-GCM encrypted Opus audio
//!   and 8 KiB fragmented H.264/HEVC/AV1 video packets with zero TCP
//!   head-of-line blocking.

use std::io;
use std::net::SocketAddr;

use bytes::BytesMut;
use socket2::{Domain, Protocol, Socket, Type};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio_util::codec::{Decoder, Encoder};

use crate::protocol::{Packet, PacketCodec, ProtocolError};
use crate::security::{SecurityError, SessionCipher};

/// 2 MiB UDP socket receive/send buffer to absorb 4K IDR bursts cleanly.
pub const UDP_SOCKET_BUFFER_BYTES: usize = 2 * 1024 * 1024;

/// Maximum single UDP datagram payload accepted (64 KiB).
pub const MAX_UDP_DATAGRAM_BYTES: usize = 65_507;

#[derive(Debug, Error)]
pub enum WifiTransportError {
    #[error("network I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("wire protocol error: {0}")]
    Protocol(#[from] ProtocolError),

    #[error("security error: {0}")]
    Security(#[from] SecurityError),

    #[error("connection closed cleanly by peer")]
    Closed,
}

/// High-throughput UDP media socket with enlarged OS buffers and optional
/// AES-256-GCM session encryption.
pub struct UdpMediaSocket {
    socket: UdpSocket,
    rx_buf: Vec<u8>,
}

impl UdpMediaSocket {
    pub fn bind(addr: SocketAddr) -> Result<Self, WifiTransportError> {
        let domain = if addr.is_ipv6() {
            Domain::IPV6
        } else {
            Domain::IPV4
        };
        let sock = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
        let _ = sock.set_recv_buffer_size(UDP_SOCKET_BUFFER_BYTES);
        let _ = sock.set_send_buffer_size(UDP_SOCKET_BUFFER_BYTES);
        sock.set_nonblocking(true)?;
        sock.bind(&addr.into())?;

        let std_udp: std::net::UdpSocket = sock.into();
        let socket = UdpSocket::from_std(std_udp)?;
        Ok(Self {
            socket,
            rx_buf: vec![0u8; MAX_UDP_DATAGRAM_BYTES],
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Encode (and optionally AES-256-GCM encrypt) `pkt` and transmit as one
    /// UDP datagram to `target`.
    pub async fn send_packet(
        &self,
        pkt: &Packet,
        target: SocketAddr,
        cipher: Option<&SessionCipher>,
    ) -> Result<usize, WifiTransportError> {
        let wire = match cipher {
            Some(c) => c.seal(pkt)?.encode()?,
            None => pkt.encode()?,
        };
        let sent = self.socket.send_to(&wire, target).await?;
        Ok(sent)
    }

    /// Receive one UDP datagram, decode the wire header, and (when `cipher` is
    /// provided) authenticate & decrypt with replay protection.
    pub async fn recv_packet(
        &mut self,
        cipher: Option<&mut SessionCipher>,
    ) -> Result<(Packet, SocketAddr), WifiTransportError> {
        let (n, peer) = self.socket.recv_from(&mut self.rx_buf).await?;
        let raw_pkt = Packet::decode(&self.rx_buf[..n])?;
        let pkt = match cipher {
            Some(c) => c.open(&raw_pkt)?,
            None => raw_pkt,
        };
        Ok((pkt, peer))
    }
}

/// Framed TCP control/media stream (`TCP_NODELAY = true`) used for Wi-Fi
/// control handshakes and USB `adb reverse` tunnels.
pub struct FramedTcpConnection {
    stream: TcpStream,
    codec: PacketCodec,
    rx_buf: BytesMut,
    tx_buf: BytesMut,
}

impl FramedTcpConnection {
    pub fn from_stream(stream: TcpStream) -> Result<Self, WifiTransportError> {
        let _ = stream.set_nodelay(true);
        Ok(Self {
            stream,
            codec: PacketCodec,
            rx_buf: BytesMut::with_capacity(64 * 1024),
            tx_buf: BytesMut::with_capacity(64 * 1024),
        })
    }

    pub async fn connect(addr: SocketAddr) -> Result<Self, WifiTransportError> {
        let stream = TcpStream::connect(addr).await?;
        Self::from_stream(stream)
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.peer_addr()
    }

    /// Send one [`Packet`] (optionally encrypted with `cipher`).
    pub async fn send_packet(
        &mut self,
        pkt: &Packet,
        cipher: Option<&SessionCipher>,
    ) -> Result<(), WifiTransportError> {
        self.tx_buf.clear();
        match cipher {
            Some(c) => {
                let sealed = c.seal(pkt)?;
                self.codec
                    .encode(&sealed, &mut self.tx_buf)
                    .map_err(|e| match e {
                        crate::protocol::CodecError::Io(io_err) => WifiTransportError::Io(io_err),
                        crate::protocol::CodecError::Protocol(p) => WifiTransportError::Protocol(p),
                    })?;
            }
            None => {
                self.codec
                    .encode(pkt, &mut self.tx_buf)
                    .map_err(|e| match e {
                        crate::protocol::CodecError::Io(io_err) => WifiTransportError::Io(io_err),
                        crate::protocol::CodecError::Protocol(p) => WifiTransportError::Protocol(p),
                    })?;
            }
        }
        self.stream.write_all(&self.tx_buf).await?;
        Ok(())
    }

    /// Read the next complete [`Packet`] from the TCP byte stream.
    pub async fn recv_packet(
        &mut self,
        cipher: Option<&mut SessionCipher>,
    ) -> Result<Packet, WifiTransportError> {
        loop {
            match self.codec.decode(&mut self.rx_buf) {
                Ok(Some(raw_pkt)) => {
                    return match cipher {
                        Some(c) => Ok(c.open(&raw_pkt)?),
                        None => Ok(raw_pkt),
                    };
                }
                Ok(None) => {}
                Err(crate::protocol::CodecError::Io(e)) => return Err(WifiTransportError::Io(e)),
                Err(crate::protocol::CodecError::Protocol(p)) => {
                    return Err(WifiTransportError::Protocol(p));
                }
            }
            let n = self.stream.read_buf(&mut self.rx_buf).await?;
            if n == 0 {
                return Err(WifiTransportError::Closed);
            }
        }
    }
}

/// Async TCP listener accepting incoming Android control / `adb reverse`
/// connections.
pub struct TcpControlListener {
    listener: TcpListener,
}

impl TcpControlListener {
    pub async fn bind(addr: SocketAddr) -> Result<Self, WifiTransportError> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self { listener })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub async fn accept(&self) -> Result<(FramedTcpConnection, SocketAddr), WifiTransportError> {
        let (stream, peer) = self.listener.accept().await?;
        Ok((FramedTcpConnection::from_stream(stream)?, peer))
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use crate::protocol::{
        ControlMessage, DEFAULT_FRAGMENT_SIZE, PacketFlags, ReassembleOutcome, StreamType,
        VideoReassembler, fragment_video_frame,
    };
    use crate::security::{Direction, KeyPair};

    use super::*;

    #[tokio::test]
    async fn encrypted_dual_tcp_and_udp_session_over_localhost() {
        let pc_kp = KeyPair::generate().unwrap();
        let phone_kp = KeyPair::generate().unwrap();
        let salt = b"wifi-lan-session";

        let pc_keys = pc_kp
            .derive_session_keys(&phone_kp.public_key_b64(), salt)
            .unwrap();
        let phone_keys = phone_kp
            .derive_session_keys(&pc_kp.public_key_b64(), salt)
            .unwrap();

        let mut pc_cipher = SessionCipher::new(&pc_keys, Direction::PcToPhone);
        let mut phone_cipher = SessionCipher::new(&phone_keys, Direction::PhoneToPc);

        // 1. Bind PC TCP control listener & UDP media receiver on ephemeral localhost ports.
        let tcp_server = TcpControlListener::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let tcp_addr = tcp_server.local_addr().unwrap();

        let mut pc_udp = UdpMediaSocket::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let pc_udp_addr = pc_udp.local_addr().unwrap();
        let phone_udp = UdpMediaSocket::bind("127.0.0.1:0".parse().unwrap()).unwrap();

        // 2. Phone connects over TCP and exchanges encrypted ControlMessage.
        let accept_task = tokio::spawn(async move { tcp_server.accept().await.unwrap().0 });
        let mut phone_tcp = FramedTcpConnection::connect(tcp_addr).await.unwrap();
        let mut pc_tcp = accept_task.await.unwrap();

        let req_idr = ControlMessage::RequestIdr.into_packet(1, 1000).unwrap();
        pc_tcp
            .send_packet(&req_idr, Some(&pc_cipher))
            .await
            .unwrap();
        let got_ctrl = phone_tcp
            .recv_packet(Some(&mut phone_cipher))
            .await
            .unwrap();
        assert_eq!(
            ControlMessage::from_packet(&got_ctrl).unwrap(),
            ControlMessage::RequestIdr
        );

        // 3. Phone streams encrypted Opus audio + 20 KiB fragmented HEVC IDR over UDP!
        let opus = Packet::new(
            StreamType::AudioOpus,
            PacketFlags::HAS_FEC,
            1,
            10_000,
            Bytes::from_static(b"opus-encrypted-voice"),
        );
        phone_udp
            .send_packet(&opus, pc_udp_addr, Some(&phone_cipher))
            .await
            .unwrap();

        let (rx_audio, _) = pc_udp.recv_packet(Some(&mut pc_cipher)).await.unwrap();
        assert_eq!(rx_audio, opus);

        let video_idr = Bytes::from(vec![0x5Au8; 20_000]);
        let mut vseq = 1u32;
        let frags = fragment_video_frame(
            StreamType::VideoH265,
            PacketFlags::KEYFRAME,
            &mut vseq,
            33_333,
            video_idr.clone(),
            DEFAULT_FRAGMENT_SIZE,
        );
        for f in &frags {
            phone_udp
                .send_packet(f, pc_udp_addr, Some(&phone_cipher))
                .await
                .unwrap();
        }

        let mut reassembler = VideoReassembler::new();
        let mut assembled = None;
        for _ in 0..frags.len() {
            let (rx_frag, _) = pc_udp.recv_packet(Some(&mut pc_cipher)).await.unwrap();
            if let ReassembleOutcome::Complete(frame) = reassembler.push(rx_frag) {
                assembled = Some(frame);
            }
        }
        let frame = assembled.expect("reassembled encrypted UDP HEVC frame");
        assert_eq!(frame.data, video_idr);
        assert!(frame.flags.contains(PacketFlags::KEYFRAME));
    }
}
