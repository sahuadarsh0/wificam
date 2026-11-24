//! Safe Windows WASAPI virtual-microphone render sink with feedback guard &
//! automatic device-invalidation recovery.
//!
//! Fixes two notorious bugs found in open-source phone-mic apps:
//! 1. **Acoustic feedback guard** (Owlmic #66): refuses to render microphone
//!    audio into physical PC speakers unless `allow_physical_monitor` is
//!    explicitly enabled.
//! 2. **Automatic reopen** (AudioRelayPlus lesson): recovers cleanly from
//!    `AUDCLNT_E_DEVICE_INVALIDATED` after sleep/resume or audio-device resets.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rtrb::Consumer;
use thiserror::Error;

use super::decoder::AUDIO_SAMPLE_RATE;

/// Descriptor for a Windows WASAPI render endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioRenderDevice {
    pub id: String,
    pub friendly_name: String,
    pub is_virtual_cable: bool,
}

#[derive(Debug, Error)]
pub enum WasapiError {
    #[error("no virtual microphone cable found (install VB-Cable or Virtual-Audio-Driver)")]
    NoVirtualMicFound,

    #[error(
        "refusing to output microphone audio to physical speaker '{0}' (enable Monitor to override)"
    )]
    PhysicalSpeakerBlocked(String),

    #[error("requested audio device '{0}' not found")]
    DeviceNotFound(String),

    #[error("Windows WASAPI error: {0}")]
    Platform(String),
}

/// Heuristic identifying known virtual audio cable render endpoints that bridge
/// audio internally into a Windows virtual microphone capture endpoint.
pub fn is_known_virtual_cable_name(friendly_name: &str) -> bool {
    let lower = friendly_name.to_ascii_lowercase();
    // Skip VB-Cable's 16-channel secondary endpoint when the standard stereo CABLE Input is preferred.
    if lower.contains("16ch") || lower.contains("16 ch") {
        return false;
    }
    let positive_markers = [
        "cable input",
        "vb-audio",
        "virtual audio",
        "voicemeeter input",
        "voicemeeter aux input",
        "virtual mic",
        "wificam bridge",
        "blackhole",
    ];
    positive_markers.iter().any(|m| lower.contains(m))
}

/// Select a safe render endpoint from `devices` while enforcing the
/// anti-feedback guard.
pub fn select_safe_render_device<'a>(
    devices: &'a [AudioRenderDevice],
    preferred_id: Option<&str>,
    allow_physical_monitor: bool,
) -> Result<&'a AudioRenderDevice, WasapiError> {
    if let Some(id) = preferred_id {
        let dev = devices
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| WasapiError::DeviceNotFound(id.to_string()))?;
        if !dev.is_virtual_cable && !allow_physical_monitor {
            return Err(WasapiError::PhysicalSpeakerBlocked(
                dev.friendly_name.clone(),
            ));
        }
        return Ok(dev);
    }

    // Auto-select the first detected virtual audio cable endpoint.
    if let Some(vdev) = devices.iter().find(|d| d.is_virtual_cable) {
        return Ok(vdev);
    }

    if allow_physical_monitor && let Some(first) = devices.first() {
        return Ok(first);
    }

    Err(WasapiError::NoVirtualMicFound)
}

/// Enumerate active Windows WASAPI render endpoints (`eRender`, `DEVICE_STATE_ACTIVE`).
#[cfg(windows)]
pub fn enumerate_render_devices() -> Result<Vec<AudioRenderDevice>, WasapiError> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::{
        DEVICE_STATE_ACTIVE, IMMDeviceEnumerator, MMDeviceEnumerator, eRender,
    };
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize, STGM_READ,
    };

    // SAFETY: Standard Win32 COM initialization and IMMDeviceEnumerator enumeration;
    // every PWSTR allocated by COM is freed with CoTaskMemFree.
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        let should_uninit = hr.is_ok();

        let result = (|| -> Result<Vec<AudioRenderDevice>, WasapiError> {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| WasapiError::Platform(e.to_string()))?;

            let collection = enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
                .map_err(|e| WasapiError::Platform(e.to_string()))?;

            let count = collection
                .GetCount()
                .map_err(|e| WasapiError::Platform(e.to_string()))?;

            let mut out = Vec::with_capacity(count as usize);
            for i in 0..count {
                let device = collection
                    .Item(i)
                    .map_err(|e| WasapiError::Platform(e.to_string()))?;
                let id_pwstr = device
                    .GetId()
                    .map_err(|e| WasapiError::Platform(e.to_string()))?;
                let id = id_pwstr.to_string().unwrap_or_default();
                CoTaskMemFree(Some(id_pwstr.0.cast()));

                let friendly_name = device
                    .OpenPropertyStore(STGM_READ)
                    .ok()
                    .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName).ok())
                    .and_then(|pv| {
                        let s = PropVariantToStringAlloc(&pv).ok()?;
                        let rust_s = s.to_string().ok();
                        CoTaskMemFree(Some(s.0.cast()));
                        rust_s
                    })
                    .unwrap_or_else(|| id.clone());

                let is_virtual_cable = is_known_virtual_cable_name(&friendly_name);
                out.push(AudioRenderDevice {
                    id,
                    friendly_name,
                    is_virtual_cable,
                });
            }
            Ok(out)
        })();

        if should_uninit {
            CoUninitialize();
        }
        result
    }
}

/// Handle to a running MMCSS-elevated WASAPI virtual-microphone render thread.
pub struct WasapiRenderHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl WasapiRenderHandle {
    /// Stop the WASAPI render thread cleanly and wait for exit.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

impl Drop for WasapiRenderHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

/// Spawn the real-time MMCSS `"Pro Audio"` WASAPI render thread that drains
/// 48 kHz mono `f32` PCM from `consumer` into the selected virtual microphone
/// render endpoint, automatically reopening on `AUDCLNT_E_DEVICE_INVALIDATED`.
#[cfg(windows)]
pub fn spawn_wasapi_render_thread(
    device_id: String,
    mut consumer: Consumer<f32>,
) -> Result<WasapiRenderHandle, WasapiError> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = stop.clone();

    let join = thread::Builder::new()
        .name("wificam-wasapi-render".into())
        .spawn(move || {
            let _mmcss = super::pipeline::MmcssGuard::enter_pro_audio();
            while !stop_flag.load(Ordering::Acquire) {
                match run_render_session(&device_id, &mut consumer, &stop_flag) {
                    Ok(()) => break,
                    Err(_e) => {
                        // Device invalidated (sleep/resume/reset) — back off briefly and reopen.
                        thread::sleep(Duration::from_millis(250));
                    }
                }
            }
        })
        .map_err(|e| WasapiError::Platform(e.to_string()))?;

    Ok(WasapiRenderHandle {
        stop,
        join: Some(join),
    })
}

#[cfg(windows)]
fn run_render_session(
    device_id: &str,
    consumer: &mut Consumer<f32>,
    stop: &AtomicBool,
) -> Result<(), WasapiError> {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient,
        IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
    };
    use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows::core::HSTRING;

    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    let _ = WAVE_FORMAT_EXTENSIBLE; // keep feature import checked

    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        let should_uninit = hr.is_ok();

        let res = (|| -> Result<(), WasapiError> {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| WasapiError::Platform(e.to_string()))?;
            let id_h = HSTRING::from(device_id);
            let device = enumerator
                .GetDevice(&id_h)
                .map_err(|e| WasapiError::Platform(e.to_string()))?;
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| WasapiError::Platform(e.to_string()))?;

            let fmt = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
                nChannels: 1,
                nSamplesPerSec: AUDIO_SAMPLE_RATE,
                nAvgBytesPerSec: AUDIO_SAMPLE_RATE * 4,
                nBlockAlign: 4,
                wBitsPerSample: 32,
                cbSize: 0,
            };

            // 10 ms buffer duration in 100-nanosecond units.
            let buffer_duration_hns: i64 = 100_000;
            let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;

            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    flags,
                    buffer_duration_hns,
                    0,
                    &fmt,
                    None,
                )
                .map_err(|e| WasapiError::Platform(e.to_string()))?;

            let event = CreateEventW(None, false, false, None)
                .map_err(|e| WasapiError::Platform(e.to_string()))?;
            if let Err(e) = client.SetEventHandle(event) {
                let _ = CloseHandle(event);
                return Err(WasapiError::Platform(e.to_string()));
            }

            let buffer_frames = client
                .GetBufferSize()
                .map_err(|e| WasapiError::Platform(e.to_string()))?;
            let render: IAudioRenderClient = client
                .GetService()
                .map_err(|e| WasapiError::Platform(e.to_string()))?;

            // Pre-fill initial silent buffer BEFORE Start() as required by WASAPI event-driven mode.
            if let Ok(ptr) = render.GetBuffer(buffer_frames) {
                let dst = std::slice::from_raw_parts_mut(ptr.cast::<f32>(), buffer_frames as usize);
                dst.fill(0.0);
                let _ = render.ReleaseBuffer(buffer_frames, 0);
            }

            client
                .Start()
                .map_err(|e| WasapiError::Platform(e.to_string()))?;

            let mut session_err = Ok(());
            while !stop.load(Ordering::Acquire) {
                let wait = WaitForSingleObject(event, 100);
                if wait != WAIT_OBJECT_0 {
                    continue;
                }
                let padding = match client.GetCurrentPadding() {
                    Ok(p) => p,
                    Err(e) => {
                        session_err = Err(WasapiError::Platform(e.to_string()));
                        break;
                    }
                };
                let frames_to_write = buffer_frames.saturating_sub(padding);
                if frames_to_write == 0 {
                    continue;
                }
                let ptr = match render.GetBuffer(frames_to_write) {
                    Ok(p) => p,
                    Err(e) => {
                        session_err = Err(WasapiError::Platform(e.to_string()));
                        break;
                    }
                };
                let dst =
                    std::slice::from_raw_parts_mut(ptr.cast::<f32>(), frames_to_write as usize);

                // Drain lock-free SPSC ring buffer; zero-fill any remaining tail on underrun.
                let avail = consumer.slots().min(dst.len());
                let mut copied = 0usize;
                if avail > 0
                    && let Ok(chunk) = consumer.read_chunk(avail)
                {
                    let (a, b) = chunk.as_slices();
                    dst[..a.len()].copy_from_slice(a);
                    dst[a.len()..a.len() + b.len()].copy_from_slice(b);
                    copied = a.len() + b.len();
                    chunk.commit_all();
                }
                if copied < dst.len() {
                    dst[copied..].fill(0.0);
                }

                if let Err(e) = render.ReleaseBuffer(frames_to_write, 0) {
                    session_err = Err(WasapiError::Platform(e.to_string()));
                    break;
                }
            }

            let _ = client.Stop();
            let _ = CloseHandle(event);
            session_err
        })();

        if should_uninit {
            CoUninitialize();
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_guard_blocks_physical_speakers_and_auto_picks_virtual_cable() {
        let devices = vec![
            AudioRenderDevice {
                id: "dev-speakers".into(),
                friendly_name: "Speakers (Realtek High Definition Audio)".into(),
                is_virtual_cable: is_known_virtual_cable_name(
                    "Speakers (Realtek High Definition Audio)",
                ),
            },
            AudioRenderDevice {
                id: "dev-vbcable".into(),
                friendly_name: "CABLE Input (VB-Audio Virtual Cable)".into(),
                is_virtual_cable: is_known_virtual_cable_name(
                    "CABLE Input (VB-Audio Virtual Cable)",
                ),
            },
        ];

        assert!(!devices[0].is_virtual_cable);
        assert!(devices[1].is_virtual_cable);

        // Auto-select picks VB-Cable automatically.
        let picked = select_safe_render_device(&devices, None, false).unwrap();
        assert_eq!(picked.id, "dev-vbcable");

        // Explicitly targeting Realtek speakers with allow_monitor=false is BLOCKED!
        assert!(matches!(
            select_safe_render_device(&devices, Some("dev-speakers"), false),
            Err(WasapiError::PhysicalSpeakerBlocked(_))
        ));

        // Explicitly allowing monitor permits physical speakers.
        let monitor = select_safe_render_device(&devices, Some("dev-speakers"), true).unwrap();
        assert_eq!(monitor.id, "dev-speakers");
    }

    #[test]
    fn errors_cleanly_when_no_virtual_cable_installed() {
        let speakers_only = vec![AudioRenderDevice {
            id: "dev-1".into(),
            friendly_name: "Headphones (USB Audio)".into(),
            is_virtual_cable: false,
        }];
        assert!(matches!(
            select_safe_render_device(&speakers_only, None, false),
            Err(WasapiError::NoVirtualMicFound)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn enumerates_real_windows_wasapi_render_endpoints() {
        let devices = enumerate_render_devices().expect("WASAPI enumeration should succeed");
        // Just confirm COM + IMMDeviceEnumerator works cleanly without crashing.
        for d in &devices {
            assert!(!d.id.is_empty());
        }
    }
}
