//! Windows WASAPI speaker loopback capture feeding the far-end reference
//! signal into WebRTC AEC3 (`AecProcessor::feed_render_reference`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rtrb::Producer;

use super::decoder::AUDIO_SAMPLE_RATE;
use super::wasapi_sink::WasapiError;

/// Handle to a running WASAPI loopback capture thread for AEC3 far-end audio.
pub struct WasapiLoopbackHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl WasapiLoopbackHandle {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

impl Drop for WasapiLoopbackHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

/// Spawn a WASAPI loopback capture thread reading the system's default
/// `eRender` speaker output at 48 kHz mono `f32` and pushing samples into a
/// lock-free `rtrb::Producer<f32>` for WebRTC AEC3.
#[cfg(windows)]
pub fn spawn_wasapi_loopback_thread(
    mut producer: Producer<f32>,
) -> Result<WasapiLoopbackHandle, WasapiError> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = stop.clone();

    let join = thread::Builder::new()
        .name("wificam-wasapi-loopback".into())
        .spawn(move || {
            let _mmcss = super::pipeline::MmcssGuard::enter_pro_audio();
            while !stop_flag.load(Ordering::Acquire) {
                match run_loopback_session(&mut producer, &stop_flag) {
                    Ok(()) => break,
                    Err(_) => thread::sleep(Duration::from_millis(250)),
                }
            }
        })
        .map_err(|e| WasapiError::Platform(e.to_string()))?;

    Ok(WasapiLoopbackHandle {
        stop,
        join: Some(join),
    })
}

#[cfg(windows)]
fn run_loopback_session(
    producer: &mut Producer<f32>,
    stop: &AtomicBool,
) -> Result<(), WasapiError> {
    use windows::Win32::Media::Audio::{
        AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioCaptureClient,
        IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, eConsole, eRender,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };

    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        let should_uninit = hr.is_ok();

        let res = (|| -> Result<(), WasapiError> {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| WasapiError::Platform(e.to_string()))?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
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

            let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;

            client
                .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 200_000, 0, &fmt, None)
                .map_err(|e| WasapiError::Platform(e.to_string()))?;

            let capture: IAudioCaptureClient = client
                .GetService()
                .map_err(|e| WasapiError::Platform(e.to_string()))?;
            client
                .Start()
                .map_err(|e| WasapiError::Platform(e.to_string()))?;

            let mut session_err = Ok(());
            while !stop.load(Ordering::Acquire) {
                let mut packet_len = match capture.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(e) => {
                        session_err = Err(WasapiError::Platform(e.to_string()));
                        break;
                    }
                };
                if packet_len == 0 {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                while packet_len > 0 && !stop.load(Ordering::Acquire) {
                    let mut data_ptr = std::ptr::null_mut();
                    let mut num_frames = 0u32;
                    let mut buf_flags = 0u32;
                    if let Err(e) = capture.GetBuffer(
                        &mut data_ptr,
                        &mut num_frames,
                        &mut buf_flags,
                        None,
                        None,
                    ) {
                        session_err = Err(WasapiError::Platform(e.to_string()));
                        break;
                    }

                    let count = num_frames as usize;
                    let free = producer.slots().min(count);
                    if free > 0
                        && let Ok(chunk) = producer.write_chunk_uninit(free)
                    {
                        if (buf_flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)) != 0
                            || data_ptr.is_null()
                        {
                            chunk.fill_from_iter(std::iter::repeat_n(0.0f32, free));
                        } else {
                            let samples = std::slice::from_raw_parts(data_ptr.cast::<f32>(), count);
                            chunk.fill_from_iter(samples[..free].iter().copied());
                        }
                    }

                    if let Err(e) = capture.ReleaseBuffer(num_frames) {
                        session_err = Err(WasapiError::Platform(e.to_string()));
                        break;
                    }
                    packet_len = capture.GetNextPacketSize().unwrap_or(0);
                }
            }

            let _ = client.Stop();
            session_err
        })();

        if should_uninit {
            CoUninitialize();
        }
        res
    }
}
