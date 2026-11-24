//! Live studio session orchestrator uniting transports, security/QR pairing,
//! lock-free audio DSP, hardware video decoding, virtual camera framebus, and
//! Windows session-lock privacy protection.

use std::net::{IpAddr, SocketAddr, UdpSocket as StdUdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde::Serialize;
use tokio::runtime::Runtime;

use crate::audio::{
    AdaptiveJitterBuffer, AudioDecoder, AudioDspProcessor, AudioSharedState, DenoiseMode,
    FRAME_SAMPLES_10MS, JitterConfig, JitterPacket, PlayoutAction, create_audio_ring,
};
use crate::brand::{APP_DISPLAY_NAME, DEFAULT_CONTROL_PORT, DEFAULT_MEDIA_UDP_PORT};
use crate::protocol::{
    PacketFlags, ReassembleOutcome, StreamType, VideoCodecChoice, VideoReassembler,
};
use crate::security::{KeyPair, PairingPayload};
use crate::transport::{
    AdbSupervisor, MdnsAdvertiseConfig, MdnsAdvertiser, TcpControlListener, TransportTier,
    UdpMediaSocket,
};
use crate::video::{CameraState, FrameDescriptor, PixelFormat, nv12_byte_len, render_nv12_slate};

/// Latest decoded frame converted to BGRA32 (max 640px wide) for live UI preview.
#[derive(Debug, Default)]
pub struct PreviewFrame {
    pub width: u32,
    pub height: u32,
    pub seq: u64,
    pub bgra: Vec<u8>,
}

/// Serializable telemetry snapshot polled by `pc-ui` (WinUI 3) at 20-30 Hz.
#[derive(Debug, Clone, Serialize)]
pub struct StudioTelemetry {
    pub running: bool,
    pub active_tier: String,
    pub video_paused: bool,
    pub session_locked: bool,
    pub mic_muted: bool,
    pub aec_enabled: bool,
    pub denoise_mode: u8,
    pub gain_db_x10: i16,
    pub peak_dbfs_x10: i16,
    pub rms_dbfs_x10: i16,
    pub vu_segments_16: u8,
    pub vad_pct: u8,
    pub skew_ppm_x10: i16,
    pub video_width: u32,
    pub video_height: u32,
    pub video_frames_rx: u64,
    pub audio_packets_rx: u64,
    pub rotation_deg: u16,
    pub flip_h: bool,
    pub flip_v: bool,
    pub crop_zoom_x100: u16,
    pub mf_vcam_supported: bool,
    pub vcam_active: bool,
    pub vcam_status: String,
    pub host_ip: String,
    pub control_port: u16,
    pub media_udp_port: u16,
    pub host_fingerprint: String,
    pub pairing_uri: String,
    pub virtual_mic_connected: bool,
    pub virtual_mic_name: String,
}

/// Fast integer BT.601 NV12 -> BGRA32 converter fitting inside `max_out_w x max_out_h`
/// (e.g. `960 x 540`) so both landscape (`1920x1080 -> 960x540`) and 90°/270° rotated
/// portrait frames (`1080x1920 -> 304x540`) always fit the preview buffer cleanly.
pub fn nv12_to_preview_bgra(
    nv12: &[u8],
    src_w: u32,
    src_h: u32,
    stride: u32,
    max_out_w: u32,
    out_bgra: &mut Vec<u8>,
) -> (u32, u32) {
    nv12_to_preview_bgra_box(nv12, src_w, src_h, stride, max_out_w, 540, out_bgra)
}

pub fn nv12_to_preview_bgra_box(
    nv12: &[u8],
    src_w: u32,
    src_h: u32,
    stride: u32,
    max_out_w: u32,
    max_out_h: u32,
    out_bgra: &mut Vec<u8>,
) -> (u32, u32) {
    if src_w == 0 || src_h == 0 || stride < src_w || max_out_w < 2 || max_out_h < 2 {
        out_bgra.clear();
        return (0, 0);
    }
    let display_h = if src_h == 1088 { 1080 } else { src_h };
    let y_plane_bytes = (stride as usize).saturating_mul(src_h as usize);
    let uv_plane_bytes = (stride as usize).saturating_mul((src_h as usize) / 2);
    if nv12.len() < y_plane_bytes + uv_plane_bytes {
        out_bgra.clear();
        return (0, 0);
    }

    let scale_w = max_out_w as f64 / src_w as f64;
    let scale_h = max_out_h as f64 / display_h as f64;
    let scale = scale_w.min(scale_h).min(1.0);
    let dst_w = ((src_w as f64 * scale).round() as u32).max(2) & !1;
    let dst_h = ((display_h as f64 * scale).round() as u32).max(2) & !1;

    out_bgra.resize((dst_w as usize) * (dst_h as usize) * 4, 255);
    let y_plane = &nv12[..y_plane_bytes];
    let uv_plane = &nv12[y_plane_bytes..y_plane_bytes + uv_plane_bytes];

    for dy in 0..dst_h {
        // Bilinear luma interpolation so downscaled Windows preview stays razor-sharp (no jagged aliasing).
        let sy_f = (dy as f64) * (display_h as f64) / (dst_h as f64);
        let sy0 = (sy_f.floor() as usize).min((display_h - 1) as usize);
        let sy1 = (sy0 + 1).min((display_h - 1) as usize);
        let wy = ((sy_f - sy0 as f64) * 256.0) as i32;
        let wy_inv = 256 - wy;

        let y_row0 = sy0 * stride as usize;
        let y_row1 = sy1 * stride as usize;
        let uv_row = (sy0 >> 1) * stride as usize;
        let dst_row = (dy as usize) * (dst_w as usize) * 4;

        for dx in 0..dst_w {
            let sx_f = (dx as f64) * (src_w as f64) / (dst_w as f64);
            let sx0 = (sx_f.floor() as usize).min((src_w - 1) as usize);
            let sx1 = (sx0 + 1).min((src_w - 1) as usize);
            let wx = ((sx_f - sx0 as f64) * 256.0) as i32;
            let wx_inv = 256 - wx;

            let y00 = y_plane[y_row0 + sx0] as i32;
            let y10 = y_plane[y_row0 + sx1] as i32;
            let y01 = y_plane[y_row1 + sx0] as i32;
            let y11 = y_plane[y_row1 + sx1] as i32;
            let y_top = (y00 * wx_inv + y10 * wx) >> 8;
            let y_bot = (y01 * wx_inv + y11 * wx) >> 8;
            let y_val = (y_top * wy_inv + y_bot * wy) >> 8;

            let uv_idx = uv_row + (sx0 & !1);
            let u_val = uv_plane[uv_idx] as i32 - 128;
            let v_val = uv_plane[uv_idx + 1] as i32 - 128;

            let c = (y_val - 16).max(0);
            let r = ((298 * c + 409 * v_val + 128) >> 8).clamp(0, 255) as u8;
            let g = ((298 * c - 100 * u_val - 208 * v_val + 128) >> 8).clamp(0, 255) as u8;
            let b = ((298 * c + 516 * u_val + 128) >> 8).clamp(0, 255) as u8;

            let o = dst_row + (dx as usize) * 4;
            out_bgra[o] = b;
            out_bgra[o + 1] = g;
            out_bgra[o + 2] = r;
            out_bgra[o + 3] = 255;
        }
    }

    (dst_w, dst_h)
}

/// Apply rotation (`0/90/180/270`), horizontal/vertical mirror flip, and
/// center crop zoom (`100..=250`, where 100 = 1.0x) to an NV12 frame so BOTH
/// the WinUI 3 live preview and Windows 11 Virtual Camera output reflect it.
#[allow(clippy::too_many_arguments)]
pub fn transform_nv12_frame(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    src_stride: u32,
    rotation_deg: u16,
    flip_h: bool,
    flip_v: bool,
    zoom_x100: u16,
    dst: &mut Vec<u8>,
) -> (u32, u32) {
    let disp_h = if src_h == 1088 { 1080 } else { src_h };
    let y_bytes = (src_stride as usize).saturating_mul(src_h as usize);
    let uv_bytes = (src_stride as usize).saturating_mul((src_h as usize) / 2);
    if src_w < 2 || disp_h < 2 || src.len() < y_bytes + uv_bytes {
        dst.clear();
        return (0, 0);
    }

    let rot = (rotation_deg % 360) / 90 * 90;
    let swap_dims = rot == 90 || rot == 270;
    let (out_w, out_h) = if swap_dims {
        (disp_h & !1, src_w & !1)
    } else {
        (src_w & !1, disp_h & !1)
    };
    let zoom = zoom_x100.clamp(100, 250) as u64;

    let out_y_bytes = (out_w as usize) * (out_h as usize);
    let out_uv_bytes = (out_w as usize) * (out_h as usize / 2);
    dst.resize(out_y_bytes + out_uv_bytes, 128);

    let src_y = &src[..y_bytes];
    let src_uv = &src[y_bytes..y_bytes + uv_bytes];
    let (dst_y, dst_uv) = dst.split_at_mut(out_y_bytes);

    let map_xy = |dx: u32, dy: u32| -> (usize, usize) {
        // Apply center crop zoom in output space first.
        let zx = (((dx as i64 - (out_w as i64 / 2)) * 100) / zoom as i64 + (out_w as i64 / 2))
            .clamp(0, (out_w - 1) as i64) as u32;
        let zy = (((dy as i64 - (out_h as i64 / 2)) * 100) / zoom as i64 + (out_h as i64 / 2))
            .clamp(0, (out_h - 1) as i64) as u32;

        let fx = if flip_h { out_w - 1 - zx } else { zx };
        let fy = if flip_v { out_h - 1 - zy } else { zy };

        let (sx, sy) = match rot {
            90 => (fy, disp_h - 1 - fx.min(disp_h - 1)),
            180 => (
                src_w - 1 - fx.min(src_w - 1),
                disp_h - 1 - fy.min(disp_h - 1),
            ),
            270 => (src_w - 1 - fy.min(src_w - 1), fx.min(disp_h - 1)),
            _ => (fx.min(src_w - 1), fy.min(disp_h - 1)),
        };
        (sx as usize, sy as usize)
    };

    for dy in 0..out_h {
        let drow = (dy as usize) * (out_w as usize);
        for dx in 0..out_w {
            let (sx, sy) = map_xy(dx, dy);
            dst_y[drow + dx as usize] = src_y[sy * src_stride as usize + sx];
        }
    }

    let uv_h = out_h / 2;
    for dy in 0..uv_h {
        let drow = (dy as usize) * (out_w as usize);
        for dx in (0..out_w).step_by(2) {
            let (sx, sy) = map_xy(dx, dy * 2);
            let uv_idx = (sy >> 1) * src_stride as usize + (sx & !1);
            dst_uv[drow + dx as usize] = src_uv[uv_idx];
            dst_uv[drow + dx as usize + 1] = src_uv[uv_idx + 1];
        }
    }

    (out_w, out_h)
}

/// Best-effort local LAN IPv4 address used inside QR pairing URIs.
pub fn detect_lan_ipv4() -> String {
    if let Ok(sock) = StdUdpSocket::bind("0.0.0.0:0")
        && sock.connect("8.8.8.8:80").is_ok()
        && let Ok(addr) = sock.local_addr()
        && let IpAddr::V4(v4) = addr.ip()
        && !v4.is_loopback()
    {
        return v4.to_string();
    }
    "127.0.0.1".to_string()
}

/// Top-level studio runtime controlled from WinUI 3 via `pc-core-ffi`.
pub struct StudioEngine {
    pub audio_state: Arc<AudioSharedState>,
    video_paused: Arc<AtomicBool>,
    session_locked: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    active_tier: Arc<AtomicU8>,
    video_width: Arc<AtomicU32>,
    video_height: Arc<AtomicU32>,
    video_frames_rx: Arc<AtomicU64>,
    audio_packets_rx: Arc<AtomicU64>,
    vcam_active: Arc<AtomicBool>,
    vcam_status: Arc<Mutex<String>>,
    preview_frame: Arc<Mutex<PreviewFrame>>,
    rotation_deg: Arc<AtomicU16>,
    flip_h: Arc<AtomicBool>,
    flip_v: Arc<AtomicBool>,
    crop_zoom_x100: Arc<AtomicU16>,
    pairing: PairingPayload,
    host_fingerprint: String,
    virtual_mic_connected: Arc<AtomicBool>,
    virtual_mic_name: Arc<Mutex<String>>,
    worker: Option<JoinHandle<()>>,
    _mdns: Option<MdnsAdvertiser>,
}

impl StudioEngine {
    pub fn new() -> Result<Self, String> {
        let kp = KeyPair::generate().map_err(|e| e.to_string())?;
        let token = PairingPayload::generate_token().map_err(|e| e.to_string())?;
        let host_name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| APP_DISPLAY_NAME.into());
        let host_ip = detect_lan_ipv4();
        let host_fingerprint = kp.fingerprint();

        let pairing = PairingPayload {
            ip: host_ip,
            control_port: DEFAULT_CONTROL_PORT,
            media_udp_port: DEFAULT_MEDIA_UDP_PORT,
            host_name,
            host_pub_b64: kp.public_key_b64(),
            token_b64: token,
        };

        #[cfg(windows)]
        let (vmic_ok, vmic_name) = crate::audio::enumerate_render_devices()
            .ok()
            .and_then(|devs| {
                crate::audio::select_safe_render_device(&devs, None, false)
                    .ok()
                    .map(|d| (true, d.friendly_name.clone()))
            })
            .unwrap_or_else(|| {
                (
                    false,
                    "Not installed — click 'Setup Virtual Mic' to enable Windows mic input".into(),
                )
            });

        #[cfg(not(windows))]
        let (vmic_ok, vmic_name) = (false, "Virtual Mic".to_string());

        Ok(Self {
            audio_state: AudioSharedState::new(),
            video_paused: Arc::new(AtomicBool::new(false)),
            session_locked: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            active_tier: Arc::new(AtomicU8::new(0)),
            video_width: Arc::new(AtomicU32::new(1920)),
            video_height: Arc::new(AtomicU32::new(1080)),
            video_frames_rx: Arc::new(AtomicU64::new(0)),
            audio_packets_rx: Arc::new(AtomicU64::new(0)),
            vcam_active: Arc::new(AtomicBool::new(false)),
            vcam_status: Arc::new(Mutex::new("Virtual Camera standby".into())),
            preview_frame: Arc::new(Mutex::new(PreviewFrame::default())),
            rotation_deg: Arc::new(AtomicU16::new(0)),
            flip_h: Arc::new(AtomicBool::new(false)),
            flip_v: Arc::new(AtomicBool::new(false)),
            crop_zoom_x100: Arc::new(AtomicU16::new(100)),
            pairing,
            host_fingerprint,
            virtual_mic_connected: Arc::new(AtomicBool::new(vmic_ok)),
            virtual_mic_name: Arc::new(Mutex::new(vmic_name)),
            worker: None,
            _mdns: None,
        })
    }

    pub fn pairing_uri(&self) -> String {
        self.pairing.to_uri()
    }

    /// Copy the latest preview BGRA frame if its sequence exceeds `last_seq`.
    pub fn copy_preview_bgra(
        &self,
        dst: &mut [u8],
        last_seq: u64,
    ) -> Option<(u32, u32, u64, usize)> {
        let guard = self.preview_frame.lock().ok()?;
        if guard.seq == 0 || guard.seq <= last_seq || guard.bgra.is_empty() {
            return None;
        }
        let n = guard.bgra.len();
        if dst.len() < n {
            return None;
        }
        dst[..n].copy_from_slice(&guard.bgra);
        Some((guard.width, guard.height, guard.seq, n))
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.running.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        // Start mDNS LAN advertisement so Android auto-discovers this PC.
        self._mdns = MdnsAdvertiser::start(&MdnsAdvertiseConfig {
            instance_name: self.pairing.host_name.clone(),
            control_port: self.pairing.control_port,
            media_udp_port: self.pairing.media_udp_port,
            host_fingerprint: self.host_fingerprint.clone(),
        })
        .ok();

        let running = self.running.clone();
        let audio_state = self.audio_state.clone();
        let video_paused = self.video_paused.clone();
        let session_locked = self.session_locked.clone();
        let active_tier = self.active_tier.clone();
        let video_width = self.video_width.clone();
        let video_height = self.video_height.clone();
        let video_frames_rx = self.video_frames_rx.clone();
        let audio_packets_rx = self.audio_packets_rx.clone();
        let vcam_active = self.vcam_active.clone();
        let vcam_status = self.vcam_status.clone();
        let preview_frame = self.preview_frame.clone();
        let rotation_deg = self.rotation_deg.clone();
        let flip_h = self.flip_h.clone();
        let flip_v = self.flip_v.clone();
        let crop_zoom_x100 = self.crop_zoom_x100.clone();
        let virtual_mic_connected = self.virtual_mic_connected.clone();
        let virtual_mic_name = self.virtual_mic_name.clone();
        let ctrl_port = self.pairing.control_port;
        let udp_port = self.pairing.media_udp_port;

        let handle = thread::Builder::new()
            .name("wificam-studio-engine".into())
            .spawn(move || {
                let Ok(rt) = Runtime::new() else {
                    running.store(false, Ordering::Release);
                    return;
                };
                rt.block_on(async move {
                    run_engine_loop(
                        running,
                        audio_state,
                        video_paused,
                        session_locked,
                        active_tier,
                        video_width,
                        video_height,
                        video_frames_rx,
                        audio_packets_rx,
                        vcam_active,
                        vcam_status,
                        preview_frame,
                        rotation_deg,
                        flip_h,
                        flip_v,
                        crop_zoom_x100,
                        virtual_mic_connected,
                        virtual_mic_name,
                        ctrl_port,
                        udp_port,
                    )
                    .await;
                });
            })
            .map_err(|e| e.to_string())?;

        self.worker = Some(handle);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        self._mdns = None;
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        self.active_tier.store(0, Ordering::Release);
        self.vcam_active.store(false, Ordering::Release);
    }

    pub fn set_muted(&self, muted: bool) {
        self.audio_state.muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_video_paused(&self, paused: bool) {
        self.video_paused.store(paused, Ordering::Relaxed);
    }

    pub fn set_session_locked(&self, locked: bool) {
        self.session_locked.store(locked, Ordering::Relaxed);
        if locked {
            self.audio_state.muted.store(true, Ordering::Relaxed);
        }
    }

    pub fn set_aec_enabled(&self, enabled: bool) {
        self.audio_state
            .aec_enabled
            .store(enabled, Ordering::Relaxed);
    }

    pub fn set_denoise_mode(&self, mode: DenoiseMode) {
        self.audio_state.set_denoise_mode(mode);
    }

    pub fn set_gain_db_x10(&self, gain_db_x10: i16) {
        self.audio_state
            .gain_db_x10
            .store(gain_db_x10.clamp(-240, 240), Ordering::Relaxed);
    }

    pub fn set_video_transform(
        &self,
        rotation_deg: u16,
        flip_h: bool,
        flip_v: bool,
        crop_zoom_x100: u16,
    ) {
        let norm_rot = (rotation_deg % 360) / 90 * 90;
        self.rotation_deg.store(norm_rot, Ordering::Relaxed);
        self.flip_h.store(flip_h, Ordering::Relaxed);
        self.flip_v.store(flip_v, Ordering::Relaxed);
        self.crop_zoom_x100
            .store(crop_zoom_x100.clamp(100, 250), Ordering::Relaxed);
    }

    /// Trigger `adb reverse tcp:<port> tcp:<port>` for the first online USB
    /// Android device.
    pub fn trigger_adb_reverse(&self) -> Result<String, String> {
        let sup = AdbSupervisor::discover(self.pairing.control_port).map_err(|e| e.to_string())?;
        let devices = sup.list_devices().map_err(|e| e.to_string())?;
        let online = devices
            .into_iter()
            .find(|d| d.state == crate::transport::AdbDeviceState::Online)
            .ok_or_else(|| "No online USB Debugging device detected".to_string())?;
        sup.setup_reverse(&online.serial)
            .map_err(|e| e.to_string())?;
        Ok(online.model.unwrap_or(online.serial))
    }

    pub fn telemetry(&self) -> StudioTelemetry {
        let tier_str = match self.active_tier.load(Ordering::Relaxed) {
            1 => TransportTier::UsbAoa.label(),
            2 => TransportTier::UsbAdb.label(),
            3 => TransportTier::UsbTether.label(),
            4 => TransportTier::WifiLan.label(),
            5 => TransportTier::BluetoothRfcomm.label(),
            _ => "Standby (Waiting for Phone)",
        };

        #[cfg(windows)]
        let mf_supported = crate::video::is_mf_virtual_camera_supported();
        #[cfg(not(windows))]
        let mf_supported = false;

        let vcam_status_str = self
            .vcam_status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();

        StudioTelemetry {
            running: self.running.load(Ordering::Relaxed),
            active_tier: tier_str.to_string(),
            video_paused: self.video_paused.load(Ordering::Relaxed),
            session_locked: self.session_locked.load(Ordering::Relaxed),
            mic_muted: self.audio_state.muted.load(Ordering::Relaxed),
            aec_enabled: self.audio_state.aec_enabled.load(Ordering::Relaxed),
            denoise_mode: self.audio_state.denoise_mode() as u8,
            gain_db_x10: self.audio_state.gain_db_x10.load(Ordering::Relaxed),
            peak_dbfs_x10: self.audio_state.peak_dbfs_x10.load(Ordering::Relaxed),
            rms_dbfs_x10: self.audio_state.rms_dbfs_x10.load(Ordering::Relaxed),
            vu_segments_16: self.audio_state.vu_segments_16(),
            vad_pct: self.audio_state.vad_pct.load(Ordering::Relaxed),
            skew_ppm_x10: self.audio_state.skew_ppm_x10.load(Ordering::Relaxed),
            video_width: self.video_width.load(Ordering::Relaxed),
            video_height: self.video_height.load(Ordering::Relaxed),
            video_frames_rx: self.video_frames_rx.load(Ordering::Relaxed),
            audio_packets_rx: self.audio_packets_rx.load(Ordering::Relaxed),
            rotation_deg: self.rotation_deg.load(Ordering::Relaxed),
            flip_h: self.flip_h.load(Ordering::Relaxed),
            flip_v: self.flip_v.load(Ordering::Relaxed),
            crop_zoom_x100: self.crop_zoom_x100.load(Ordering::Relaxed),
            mf_vcam_supported: mf_supported,
            vcam_active: self.vcam_active.load(Ordering::Relaxed),
            vcam_status: vcam_status_str,
            host_ip: self.pairing.ip.clone(),
            control_port: self.pairing.control_port,
            media_udp_port: self.pairing.media_udp_port,
            host_fingerprint: self.host_fingerprint.clone(),
            pairing_uri: self.pairing.to_uri(),
            virtual_mic_connected: self.virtual_mic_connected.load(Ordering::Relaxed),
            virtual_mic_name: self
                .virtual_mic_name
                .lock()
                .map(|s| s.clone())
                .unwrap_or_default(),
        }
    }
}

impl Drop for StudioEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(windows)]
fn stream_type_to_codec(st: StreamType) -> Option<VideoCodecChoice> {
    match st {
        StreamType::VideoH264 => Some(VideoCodecChoice::H264),
        StreamType::VideoH265 => Some(VideoCodecChoice::H265),
        StreamType::VideoAv1 => Some(VideoCodecChoice::Av1),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_engine_loop(
    running: Arc<AtomicBool>,
    audio_state: Arc<AudioSharedState>,
    video_paused: Arc<AtomicBool>,
    session_locked: Arc<AtomicBool>,
    active_tier: Arc<AtomicU8>,
    video_width: Arc<AtomicU32>,
    video_height: Arc<AtomicU32>,
    video_frames_rx: Arc<AtomicU64>,
    audio_packets_rx: Arc<AtomicU64>,
    vcam_active: Arc<AtomicBool>,
    vcam_status: Arc<Mutex<String>>,
    preview_frame: Arc<Mutex<PreviewFrame>>,
    rotation_deg: Arc<AtomicU16>,
    flip_h: Arc<AtomicBool>,
    flip_v: Arc<AtomicBool>,
    crop_zoom_x100: Arc<AtomicU16>,
    virtual_mic_connected: Arc<AtomicBool>,
    virtual_mic_name: Arc<Mutex<String>>,
    ctrl_port: u16,
    udp_port: u16,
) {
    // Initialize lock-free audio DSP + SPSC ring buffer.
    let (mut audio_prod, mut audio_cons_opt) = {
        let (p, c) = create_audio_ring(FRAME_SAMPLES_10MS * 16);
        (p, Some(c))
    };
    let mut audio_decoder = AudioDecoder::new().ok();
    let mut jitter = AdaptiveJitterBuffer::new(JitterConfig::default());
    let mut dsp = AudioDspProcessor::new(audio_state.clone(), 30, FRAME_SAMPLES_10MS * 3).ok();

    // Start safe WASAPI virtual-mic render thread if a virtual cable is installed.
    #[cfg(windows)]
    let _wasapi_handle = {
        let found = crate::audio::enumerate_render_devices()
            .ok()
            .and_then(|devs| {
                crate::audio::select_safe_render_device(&devs, None, false)
                    .ok()
                    .cloned()
            });
        match found {
            Some(dev) => {
                let handle = audio_cons_opt.take().and_then(|cons| {
                    crate::audio::spawn_wasapi_render_thread(dev.id.clone(), cons).ok()
                });
                if handle.is_some() {
                    virtual_mic_connected.store(true, Ordering::Release);
                    if let Ok(mut n) = virtual_mic_name.lock() {
                        *n = format!(
                            "Active: {} (Select CABLE Output as Mic in Meet / Teams / Zoom)",
                            dev.friendly_name
                        );
                    }
                }
                handle
            }
            None => {
                virtual_mic_connected.store(false, Ordering::Release);
                if let Ok(mut n) = virtual_mic_name.lock() {
                    *n = "Not installed — click 'Setup Virtual Mic' to enable Windows mic input"
                        .into();
                }
                None
            }
        }
    };
    #[cfg(not(windows))]
    let _ = (&virtual_mic_connected, &virtual_mic_name);

    // Initialize shared-memory virtual camera framebus and publish initial slate BEFORE
    // instantiating the Windows 11 Media Foundation Virtual Camera so FrameServer sees valid frames immediately.
    #[cfg(windows)]
    let mut framebus = crate::video::FramebusWriter::create_default().ok();
    #[cfg(windows)]
    let mut active_codec = VideoCodecChoice::H264;
    #[cfg(windows)]
    let mut mf_decoder = crate::video::MfVideoDecoder::new(active_codec, 1920, 1080, 60).ok();

    let mut slate_buf = vec![0u8; nv12_byte_len(1920, 1080)];
    render_nv12_slate(1920, 1080, CameraState::Disconnected, &mut slate_buf);
    #[cfg(windows)]
    if let Some(ref mut fb) = framebus {
        let _ = fb.write_frame(
            FrameDescriptor {
                state: CameraState::Disconnected,
                width: 1920,
                height: 1080,
                stride: 1920,
                pixel_format: PixelFormat::Nv12,
                fps_num: 60,
                fps_den: 1,
                pts_us: 0,
            },
            &slate_buf,
        );
    }

    // Phone-Link-style dynamic virtual camera lifecycle:
    // Do NOT register/expose "WifiCam Camera" while idle. Create & Start it ONLY
    // when a phone actually connects and delivers live video frames, and Remove()
    // it immediately on disconnect / pause / lock so apps never see a black camera!
    #[cfg(windows)]
    let cam_name = format!("{APP_DISPLAY_NAME} Camera");
    #[cfg(windows)]
    let mut live_vcam: Option<crate::video::Win11VirtualCamera> = None;
    #[cfg(windows)]
    let mut last_video_rx: Option<std::time::Instant> = None;
    #[cfg(windows)]
    let mut last_vcam_retry: Option<std::time::Instant> = None;
    let mut last_any_rx: Option<std::time::Instant> = None;

    vcam_active.store(false, Ordering::Release);
    if let Ok(mut s) = vcam_status.lock() {
        *s = "Standby — camera appears in apps only while phone is live".into();
    }
    #[cfg(not(windows))]
    let _ = (
        &vcam_active,
        &vcam_status,
        &preview_frame,
        &video_width,
        &video_height,
        &rotation_deg,
        &flip_h,
        &flip_v,
        &crop_zoom_x100,
    );

    let mut udp_sock = UdpMediaSocket::bind(SocketAddr::from(([0, 0, 0, 0], udp_port))).ok();
    let tcp_listener = TcpControlListener::bind(SocketAddr::from(([0, 0, 0, 0], ctrl_port)))
        .await
        .ok();
    // Spawn lightweight TCP control acceptor so Android's TCP reachability check & keepalive
    // stay cleanly accepted and never hit a full backlog or reset.
    let _tcp_task = tcp_listener.map(|listener| {
        let run_flag = running.clone();
        tokio::spawn(async move {
            while run_flag.load(Ordering::Acquire) {
                if let Ok(Ok((mut stream, _peer))) = tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    listener.accept_raw(),
                )
                .await
                {
                    let rf = run_flag.clone();
                    tokio::spawn(async move {
                        use tokio::io::AsyncReadExt;
                        let mut sink = [0u8; 2048];
                        while rf.load(Ordering::Acquire) {
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(2),
                                stream.read(&mut sink),
                            )
                            .await
                            {
                                Ok(Ok(0)) | Ok(Err(_)) => break,
                                _ => {}
                            }
                        }
                    });
                }
            }
        })
    });
    let mut reassembler = VideoReassembler::new();
    let mut frame_10ms = [0.0f32; FRAME_SAMPLES_10MS];
    #[cfg(windows)]
    let mut transform_scratch: Vec<u8> = Vec::with_capacity(1920 * 1080 * 3 / 2);
    #[cfg(windows)]
    let mut preview_scratch: Vec<u8> = Vec::with_capacity(1280 * 720 * 4);
    #[cfg(windows)]
    let mut preview_seq: u64 = 0;

    while running.load(Ordering::Acquire) {
        let now = std::time::Instant::now();
        let paused = video_paused.load(Ordering::Relaxed);
        let locked = session_locked.load(Ordering::Relaxed);

        // Tear down the Windows 11 virtual camera immediately when paused, locked,
        // or after 3s without live video frames (Phone-Link behavior — no black ghost camera!).
        #[cfg(windows)]
        {
            let video_timed_out = last_video_rx
                .map(|t| now.duration_since(t) > std::time::Duration::from_secs(3))
                .unwrap_or(false);
            if (paused || locked || video_timed_out) && live_vcam.is_some() {
                drop(live_vcam.take());
                vcam_active.store(false, Ordering::Release);
                if let Ok(mut s) = vcam_status.lock() {
                    *s = if locked {
                        "Privacy Locked (Win+L) — virtual camera hidden".into()
                    } else if paused {
                        "Camera Paused — virtual camera hidden".into()
                    } else {
                        "Standby — camera appears in apps only while phone is live".into()
                    };
                }
                if video_timed_out {
                    last_video_rx = None;
                    if let Ok(mut pf) = preview_frame.try_lock() {
                        pf.bgra.clear();
                        pf.width = 0;
                        pf.height = 0;
                    }
                }
            }
        }
        if let Some(t) = last_any_rx
            && now.duration_since(t) > std::time::Duration::from_secs(4)
        {
            last_any_rx = None;
            active_tier.store(0, Ordering::Relaxed);
        }

        if paused || locked {
            let st = if locked {
                CameraState::Locked
            } else {
                CameraState::Paused
            };
            render_nv12_slate(1920, 1080, st, &mut slate_buf);
            #[cfg(windows)]
            if let Some(ref mut fb) = framebus {
                let _ = fb.write_frame(
                    FrameDescriptor {
                        state: st,
                        width: 1920,
                        height: 1080,
                        stride: 1920,
                        pixel_format: PixelFormat::Nv12,
                        fps_num: 60,
                        fps_den: 1,
                        pts_us: 0,
                    },
                    &slate_buf,
                );
            }
        }

        if let Some(ref mut sock) = udp_sock {
            let rx =
                tokio::time::timeout(std::time::Duration::from_millis(50), sock.recv_packet(None))
                    .await;

            if let Ok(Ok((pkt, _peer))) = rx {
                last_any_rx = Some(std::time::Instant::now());
                active_tier.store(TransportTier::WifiLan as u8, Ordering::Relaxed);
                match pkt.stream_type {
                    StreamType::AudioPcm16 => {
                        audio_packets_rx.fetch_add(1, Ordering::Relaxed);
                        if let Some(ref mut dec) = audio_decoder
                            && dec.decode_pcm16_le(&pkt.payload).is_ok()
                            && let Some(ref mut proc_dsp) = dsp
                        {
                            while dec.pop_frame_10ms(&mut frame_10ms) {
                                let _ = proc_dsp
                                    .process_frame_into_ring(&mut frame_10ms, &mut audio_prod);
                                // Keep ring balanced at 0 ppm when no virtual mic is attached.
                                if let Some(ref mut cons) = audio_cons_opt {
                                    while cons.slots() > FRAME_SAMPLES_10MS * 3 {
                                        let _ = cons.pop();
                                    }
                                }
                            }
                        }
                    }
                    StreamType::AudioOpus => {
                        audio_packets_rx.fetch_add(1, Ordering::Relaxed);
                        jitter.push(JitterPacket {
                            seq: pkt.seq,
                            has_fec: pkt.flags.contains(PacketFlags::HAS_FEC),
                            payload: pkt.payload,
                        });
                        if let Some(ref mut dec) = audio_decoder {
                            match jitter.pop_next() {
                                PlayoutAction::Decode(p) => {
                                    let _ = dec.decode_opus(&p);
                                }
                                PlayoutAction::DecodeFec(p) => {
                                    let _ = dec.decode_opus_fec(&p);
                                }
                                PlayoutAction::ConcealPlc => {
                                    let _ = dec.conceal_opus_plc();
                                }
                                PlayoutAction::Buffering => {}
                            }
                            if let Some(ref mut proc_dsp) = dsp {
                                while dec.pop_frame_10ms(&mut frame_10ms) {
                                    let _ = proc_dsp
                                        .process_frame_into_ring(&mut frame_10ms, &mut audio_prod);
                                    if let Some(ref mut cons) = audio_cons_opt {
                                        while cons.slots() > FRAME_SAMPLES_10MS * 3 {
                                            let _ = cons.pop();
                                        }
                                    }
                                }
                            }
                        }
                    }
                    StreamType::VideoH264 | StreamType::VideoH265 | StreamType::VideoAv1 => {
                        if !paused
                            && !locked
                            && let ReassembleOutcome::Complete(au) = reassembler.push(pkt)
                        {
                            video_frames_rx.fetch_add(1, Ordering::Relaxed);
                            #[cfg(windows)]
                            {
                                if let Some(want_codec) = stream_type_to_codec(au.stream_type)
                                    && want_codec != active_codec
                                {
                                    active_codec = want_codec;
                                    mf_decoder = crate::video::MfVideoDecoder::new(
                                        active_codec,
                                        1920,
                                        1080,
                                        60,
                                    )
                                    .ok();
                                }
                                if let Some(ref mut dec) = mf_decoder
                                    && let Ok(frames) = dec.decode_access_unit(&au.data, au.pts_us)
                                {
                                    for f in frames {
                                        let rot = rotation_deg.load(Ordering::Relaxed);
                                        let fh = flip_h.load(Ordering::Relaxed);
                                        let fv = flip_v.load(Ordering::Relaxed);
                                        let zm = crop_zoom_x100.load(Ordering::Relaxed);
                                        let needs_tx = rot != 0 || fh || fv || zm > 100;

                                        let (pub_w, pub_h, pub_stride, pub_bytes): (
                                            u32,
                                            u32,
                                            u32,
                                            &[u8],
                                        ) = if needs_tx {
                                            let (tw, th) = transform_nv12_frame(
                                                &f.nv12_data,
                                                f.width,
                                                f.height,
                                                f.stride,
                                                rot,
                                                fh,
                                                fv,
                                                zm,
                                                &mut transform_scratch,
                                            );
                                            (tw, th, tw, &transform_scratch)
                                        } else {
                                            let dh = if f.height == 1088 { 1080 } else { f.height };
                                            (f.width, dh, f.stride, &f.nv12_data)
                                        };

                                        if pub_w == 0 || pub_h == 0 {
                                            continue;
                                        }

                                        video_width.store(pub_w, Ordering::Relaxed);
                                        video_height.store(pub_h, Ordering::Relaxed);
                                        if let Some(ref mut fb) = framebus {
                                            let _ = fb.write_frame(
                                                FrameDescriptor {
                                                    state: CameraState::Streaming,
                                                    width: pub_w,
                                                    height: pub_h,
                                                    stride: pub_stride,
                                                    pixel_format: PixelFormat::Nv12,
                                                    fps_num: 60,
                                                    fps_den: 1,
                                                    pts_us: f.pts_us,
                                                },
                                                pub_bytes,
                                            );
                                        }
                                        // Activate the Windows 11 Virtual Camera ONLY once real frames are flowing!
                                        last_video_rx = Some(std::time::Instant::now());
                                        if live_vcam.is_none() {
                                            let retry_ok = last_vcam_retry
                                                .map(|t| {
                                                    t.elapsed() >= std::time::Duration::from_secs(3)
                                                })
                                                .unwrap_or(true);
                                            if retry_ok {
                                                last_vcam_retry = Some(std::time::Instant::now());
                                                match crate::video::Win11VirtualCamera::create(
                                                    &cam_name,
                                                )
                                                .and_then(|mut v| {
                                                    v.start()?;
                                                    Ok(v)
                                                }) {
                                                    Ok(v) => {
                                                        live_vcam = Some(v);
                                                        vcam_active.store(true, Ordering::Release);
                                                        if let Ok(mut s) = vcam_status.lock() {
                                                            *s = format!(
                                                                "Active in Apps: \"{cam_name}\" (Win11 Media Foundation)"
                                                            );
                                                        }
                                                    }
                                                    Err(err) => {
                                                        vcam_active.store(false, Ordering::Release);
                                                        if let Ok(mut s) = vcam_status.lock() {
                                                            *s = format!(
                                                                "Virtual Camera error ({err}) — run Setup or register-vcam.ps1 as Admin"
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        let src_h_for_bgra =
                                            if needs_tx { pub_h } else { f.height };
                                        let (pw, ph) = nv12_to_preview_bgra_box(
                                            pub_bytes,
                                            pub_w,
                                            src_h_for_bgra,
                                            pub_stride,
                                            1280,
                                            720,
                                            &mut preview_scratch,
                                        );
                                        if pw > 0
                                            && ph > 0
                                            && let Ok(mut pf) = preview_frame.try_lock()
                                        {
                                            preview_seq = preview_seq.wrapping_add(1);
                                            pf.width = pw;
                                            pf.height = ph;
                                            pf.seq = preview_seq;
                                            std::mem::swap(&mut pf.bgra, &mut preview_scratch);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    StreamType::Control => {
                        // Handle remote transforms & explicit disconnect from Android.
                        if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&pkt.payload) {
                            match val.get("cmd").and_then(|c| c.as_str()) {
                                Some("video_transform") => {
                                    if let Some(rot) = val.get("rotation").and_then(|v| v.as_u64())
                                    {
                                        let norm = ((rot % 360) as u16) / 90 * 90;
                                        rotation_deg.store(norm, Ordering::Relaxed);
                                    }
                                    if let Some(fh) = val.get("flip_h").and_then(|v| v.as_bool()) {
                                        flip_h.store(fh, Ordering::Relaxed);
                                    }
                                    if let Some(fv) = val.get("flip_v").and_then(|v| v.as_bool()) {
                                        flip_v.store(fv, Ordering::Relaxed);
                                    }
                                    if let Some(zm) = val.get("zoom_x100").and_then(|v| v.as_u64())
                                    {
                                        crop_zoom_x100
                                            .store((zm as u16).clamp(100, 250), Ordering::Relaxed);
                                    }
                                }
                                Some("disconnect") => {
                                    last_any_rx = None;
                                    active_tier.store(0, Ordering::Relaxed);
                                    #[cfg(windows)]
                                    {
                                        last_video_rx = None;
                                        drop(live_vcam.take());
                                        vcam_active.store(false, Ordering::Release);
                                        if let Ok(mut s) = vcam_status.lock() {
                                            *s = "Standby — camera appears in apps only while phone is live"
                                                .into();
                                        }
                                        if let Ok(mut pf) = preview_frame.try_lock() {
                                            pf.bgra.clear();
                                            pf.width = 0;
                                            pf.height = 0;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    StreamType::Heartbeat => {}
                }
            }
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[cfg(windows)]
    drop(live_vcam);
    vcam_active.store(false, Ordering::Release);
    if let Some(h) = _tcp_task {
        h.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_engine_builds_pairing_uri_and_starts_stops_cleanly() {
        let mut engine = StudioEngine::new().expect("create studio engine");
        assert!(engine.pairing_uri().starts_with("wificam://pair?"));

        engine.set_muted(true);
        engine.set_denoise_mode(DenoiseMode::DeepFilterNet3);
        engine.set_gain_db_x10(45);

        let snap = engine.telemetry();
        assert!(snap.mic_muted);
        assert_eq!(snap.denoise_mode, DenoiseMode::DeepFilterNet3 as u8);
        assert_eq!(snap.gain_db_x10, 45);
        assert!(!snap.host_fingerprint.is_empty());

        engine.start().expect("start engine");
        assert!(engine.telemetry().running);
        engine.stop();
        assert!(!engine.telemetry().running);
    }

    #[test]
    fn nv12_to_preview_bgra_downscales_1088_cleanly() {
        let mut nv12 = vec![128u8; nv12_byte_len(1920, 1088)];
        nv12[..1920 * 1088].fill(128);
        let mut bgra = Vec::new();
        let (pw, ph) = nv12_to_preview_bgra(&nv12, 1920, 1088, 1920, 640, &mut bgra);
        assert_eq!((pw, ph), (640, 360));
        assert_eq!(bgra.len(), 640 * 360 * 4);
        assert_eq!(bgra[3], 255);
    }

    #[test]
    fn transform_nv12_frame_rotates_90_and_fits_preview_box() {
        let mut nv12 = vec![128u8; nv12_byte_len(1920, 1080)];
        nv12[..1920 * 1080].fill(200);
        let mut rotated = Vec::new();
        let (rw, rh) =
            transform_nv12_frame(&nv12, 1920, 1080, 1920, 90, true, false, 125, &mut rotated);
        assert_eq!((rw, rh), (1080, 1920));

        let mut bgra = Vec::new();
        let (pw, ph) = nv12_to_preview_bgra(&rotated, rw, rh, rw, 960, &mut bgra);
        assert!(pw <= 960 && ph <= 540, "got {pw}x{ph}");
        assert_eq!((pw, ph), (304, 540));
        assert!(bgra.len() <= 960 * 540 * 4);
    }
}
