//! Typed JSON control messages carried inside `StreamType::Control` packets.

use serde::{Deserialize, Serialize};

use super::packet::{Packet, PacketFlags, StreamType};

/// Negotiated camera / microphone streaming parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_bitrate_kbps: u32,
    pub video_codec: VideoCodecChoice,
    pub audio_sample_rate: u32,
    pub audio_channels: u8,
    pub audio_format: AudioFormatChoice,
    pub lens_id: String,
    pub antibanding_hz: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodecChoice {
    H264,
    H265,
    Av1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormatChoice {
    Pcm16,
    Opus,
}

/// Bidirectional control-plane messages exchanged over `StreamType::Control`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// Ephemeral P-256 ECDH handshake initiation (base64url SEC1 public key).
    HandshakeInit {
        device_name: String,
        ephemeral_pub_b64: String,
        pairing_token: Option<String>,
    },
    /// Ephemeral P-256 ECDH handshake response.
    HandshakeResp {
        host_name: String,
        ephemeral_pub_b64: String,
        accepted: bool,
    },
    /// Explicit privacy consent prompt on the phone before real media starts.
    ConsentRequest {
        host_name: String,
        host_fingerprint: String,
    },
    /// User's response to [`ControlMessage::ConsentRequest`].
    ConsentResult { approved: bool },
    /// Apply camera & microphone configuration.
    Configure(StreamConfig),
    /// Request an immediate H.264/H.265 IDR keyframe (e.g. after packet loss).
    RequestIdr,
    /// Toggle camera video pause (virtual cam displays "Camera Paused" slate).
    SetVideoPaused { paused: bool },
    /// Toggle microphone mute.
    SetAudioMuted { muted: bool },
    /// Switch active Android camera lens (e.g. "0", "1", "ultrawide").
    SwitchLens { lens_id: String },
    /// Periodic phone telemetry (battery %, temperature C, dropped frames).
    Telemetry {
        battery_pct: u8,
        temp_celsius_x10: i16,
        encoded_fps_x10: u16,
    },
    /// Clean session teardown.
    Bye { reason: String },
}

impl ControlMessage {
    pub fn into_packet(&self, seq: u32, pts_us: u64) -> Result<Packet, serde_json::Error> {
        let bytes = serde_json::to_vec(self)?;
        Ok(Packet::new(
            StreamType::Control,
            PacketFlags::NONE,
            seq,
            pts_us,
            bytes,
        ))
    }

    pub fn from_packet(pkt: &Packet) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(&pkt.payload)
    }
}
