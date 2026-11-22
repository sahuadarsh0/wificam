//! Live studio session orchestrator uniting transports, security/QR pairing,
//! lock-free audio DSP, hardware video decoding, virtual camera framebus, and
//! Windows session-lock privacy protection.

use std::net::{IpAddr, SocketAddr, UdpSocket as StdUdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
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
    pub mf_vcam_supported: bool,
    pub vcam_active: bool,
    pub vcam_status: String,
    pub host_ip: String,
    pub control_port: u16,
    pub media_udp_port: u16,
    pub host_fingerprint: String,
    pub pairing_uri: String,
    pub virtual_mic_name: String,
}

/// Fast integer BT.601 NV12 -> BGRA32 converter with automatic downscaling
/// to `max_out_w` (e.g. 640px) for zero-jank WinUI 3 live preview rendering.
pub fn nv12_to_preview_bgra(
    nv12: &[u8],
    src_w: u32,
    src_h: u32,
    stride: u32,
    max_out_w: u32,
    out_bgra: &mut Vec<u8>,
) -> (u32, u32) {
    if src_w == 0 || src_h == 0 || stride < src_w {
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

    let (dst_w, dst_h) = if src_w > max_out_w {
        let h = ((display_h as u64 * max_out_w as u64) / src_w as u64).max(2) as u32;
        (max_out_w, h & !1)
    } else {
        (src_w, display_h)
    };

    out_bgra.resize((dst_w as usize) * (dst_h as usize) * 4, 255);
    let y_plane = &nv12[..y_plane_bytes];
    let uv_plane = &nv12[y_plane_bytes..y_plane_bytes + uv_plane_bytes];

    for dy in 0..dst_h {
        let sy = (dy as u64 * display_h as u64 / dst_h as u64) as usize;
        let y_row = sy * stride as usize;
        let uv_row = (sy >> 1) * stride as usize;
        let dst_row = (dy as usize) * (dst_w as usize) * 4;

        for dx in 0..dst_w {
            let sx = (dx as u64 * src_w as u64 / dst_w as u64) as usize;
            let y_val = y_plane[y_row + sx] as i32;
            let uv_idx = uv_row + (sx & !1);
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
    pairing: PairingPayload,
    host_fingerprint: String,
    virtual_mic_name: String,
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
        let virtual_mic_name = crate::audio::enumerate_render_devices()
            .ok()
            .and_then(|devs| {
                crate::audio::select_safe_render_device(&devs, None, false)
                    .ok()
                    .map(|d| d.friendly_name.clone())
            })
            .unwrap_or_else(|| "Not detected (Install VB-Cable / Virtual Audio Driver)".into());

        #[cfg(not(windows))]
        let virtual_mic_name = "Virtual Mic".into();

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
            pairing,
            host_fingerprint,
            virtual_mic_name,
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
            mf_vcam_supported: mf_supported,
            vcam_active: self.vcam_active.load(Ordering::Relaxed),
            vcam_status: vcam_status_str,
            host_ip: self.pairing.ip.clone(),
            control_port: self.pairing.control_port,
            media_udp_port: self.pairing.media_udp_port,
            host_fingerprint: self.host_fingerprint.clone(),
            pairing_uri: self.pairing.to_uri(),
            virtual_mic_name: self.virtual_mic_name.clone(),
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
    let _wasapi_handle = crate::audio::enumerate_render_devices()
        .ok()
        .and_then(|devs| {
            crate::audio::select_safe_render_device(&devs, None, false)
                .ok()
                .map(|d| d.id.clone())
        })
        .and_then(|dev_id| {
            let cons = audio_cons_opt.take()?;
            crate::audio::spawn_wasapi_render_thread(dev_id, cons).ok()
        });

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

    // Activate Windows 11 Media Foundation Virtual Camera ("WifiCam Camera") so Chrome,
    // Google Meet, Teams, Zoom, Discord, OBS, and Windows Camera immediately discover it.
    #[cfg(windows)]
    let _live_vcam = {
        let cam_name = format!("{APP_DISPLAY_NAME} Camera");
        match crate::video::Win11VirtualCamera::create(&cam_name).and_then(|mut vcam| {
            vcam.start()?;
            Ok(vcam)
        }) {
            Ok(vcam) => {
                vcam_active.store(true, Ordering::Release);
                if let Ok(mut s) = vcam_status.lock() {
                    *s = format!("Active in Apps: \"{cam_name}\" (Win11 Media Foundation)");
                }
                Some(vcam)
            }
            Err(err) => {
                vcam_active.store(false, Ordering::Release);
                if let Ok(mut s) = vcam_status.lock() {
                    *s = format!(
                        "Virtual Camera inactive ({err}) — run Setup or register-vcam.ps1 as Admin"
                    );
                }
                None
            }
        }
    };
    #[cfg(not(windows))]
    let _ = (
        &vcam_active,
        &vcam_status,
        &preview_frame,
        &video_width,
        &video_height,
    );

    let mut udp_sock = UdpMediaSocket::bind(SocketAddr::from(([0, 0, 0, 0], udp_port))).ok();
    let tcp_listener = TcpControlListener::bind(SocketAddr::from(([0, 0, 0, 0], ctrl_port)))
        .await
        .ok();
    let mut reassembler = VideoReassembler::new();
    let mut frame_10ms = [0.0f32; FRAME_SAMPLES_10MS];
    #[cfg(windows)]
    let mut preview_scratch: Vec<u8> = Vec::with_capacity(640 * 360 * 4);
    #[cfg(windows)]
    let mut preview_seq: u64 = 0;

    while running.load(Ordering::Acquire) {
        let paused = video_paused.load(Ordering::Relaxed);
        let locked = session_locked.load(Ordering::Relaxed);
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
                                        let disp_h = if f.height == 1088 { 1080 } else { f.height };
                                        video_width.store(f.width, Ordering::Relaxed);
                                        video_height.store(disp_h, Ordering::Relaxed);
                                        if let Some(ref mut fb) = framebus {
                                            let _ = fb.write_frame(
                                                FrameDescriptor {
                                                    state: CameraState::Streaming,
                                                    width: f.width,
                                                    height: disp_h,
                                                    stride: f.stride,
                                                    pixel_format: PixelFormat::Nv12,
                                                    fps_num: 60,
                                                    fps_den: 1,
                                                    pts_us: f.pts_us,
                                                },
                                                &f.nv12_data,
                                            );
                                        }
                                        let (pw, ph) = nv12_to_preview_bgra(
                                            &f.nv12_data,
                                            f.width,
                                            f.height,
                                            f.stride,
                                            640,
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
                    StreamType::Control | StreamType::Heartbeat => {}
                }
            }
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[cfg(windows)]
    drop(_live_vcam);
    drop(tcp_listener);
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
}
