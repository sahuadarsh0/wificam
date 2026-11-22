//! Low-latency Windows Media Foundation H.264 / HEVC / AV1 video decoder
//! emitting native NV12 frames directly for [`crate::video::FramebusWriter`].
//!
//! Enables `CODECAPI_AVLowLatencyMode` to eliminate B-frame reordering delay
//! and handles `MF_E_TRANSFORM_STREAM_CHANGE` seamlessly when the Android phone
//! switches resolution (e.g. 1080p60 <-> 4K30) or orientation mid-call.

use thiserror::Error;

use crate::protocol::VideoCodecChoice;

/// Decoded NV12 frame ready to publish into the virtual camera framebus.
#[derive(Debug, Clone)]
pub struct DecodedNv12Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pts_us: u64,
    pub nv12_data: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum VideoDecodeError {
    #[error("no Media Foundation decoder available for codec {0:?}")]
    DecoderUnavailable(VideoCodecChoice),

    #[error("Media Foundation error: {0}")]
    Platform(String),
}

#[cfg(windows)]
mod win_impl {
    use super::{DecodedNv12Frame, VideoDecodeError};
    use crate::protocol::VideoCodecChoice;
    use windows::Win32::Media::MediaFoundation::{
        IMFActivate, IMFMediaType, IMFSample, IMFTransform, MF_E_TRANSFORM_NEED_MORE_INPUT,
        MF_E_TRANSFORM_STREAM_CHANGE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
        MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_VERSION, MFCreateMediaType, MFCreateMemoryBuffer,
        MFCreateSample, MFMediaType_Video, MFSTARTUP_NOSOCKET, MFShutdown, MFStartup,
        MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
        MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_COMMAND_FLUSH, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
        MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER, MFT_REGISTER_TYPE_INFO,
        MFTEnumEx, MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_NV12,
        MFVideoInterlace_Progressive,
    };
    use windows::Win32::System::Com::{
        COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    };
    use windows::core::GUID;

    // `MFVideoFormat_AV1` = FOURCC('A','V','0','1') base Media Foundation GUID.
    const MF_VIDEO_FORMAT_AV1: GUID = GUID::from_u128(0x31305641_0000_0010_8000_00aa00389b71);
    // `CODECAPI_AVLowLatencyMode` GUID (`9c27891a-ed7a-40e1-88e8-b22727a024ee`).
    const CODECAPI_AV_LOW_LATENCY_MODE: GUID =
        GUID::from_u128(0x9c27891a_ed7a_40e1_88e8_b22727a024ee);

    fn codec_subtype(codec: VideoCodecChoice) -> GUID {
        match codec {
            VideoCodecChoice::H264 => MFVideoFormat_H264,
            VideoCodecChoice::H265 => MFVideoFormat_HEVC,
            VideoCodecChoice::Av1 => MF_VIDEO_FORMAT_AV1,
        }
    }

    #[inline]
    fn pack_u64_pair(hi: u32, lo: u32) -> u64 {
        ((hi as u64) << 32) | (lo as u64)
    }

    #[inline]
    fn unpack_u64_pair(v: u64) -> (u32, u32) {
        ((v >> 32) as u32, (v & 0xFFFF_FFFF) as u32)
    }

    /// Low-latency Windows Media Foundation video decoder outputting NV12.
    pub struct MfVideoDecoder {
        codec: VideoCodecChoice,
        transform: Option<IMFTransform>,
        width: u32,
        height: u32,
        fps: u32,
        com_initialized: bool,
    }

    impl MfVideoDecoder {
        pub fn new(
            codec: VideoCodecChoice,
            width: u32,
            height: u32,
            fps: u32,
        ) -> Result<Self, VideoDecodeError> {
            unsafe {
                let com_initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
                if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) {
                    if com_initialized {
                        CoUninitialize();
                    }
                    return Err(VideoDecodeError::Platform(e.to_string()));
                }

                let init_res = (|| -> Result<IMFTransform, VideoDecodeError> {
                    let input_info = MFT_REGISTER_TYPE_INFO {
                        guidMajorType: MFMediaType_Video,
                        guidSubtype: codec_subtype(codec),
                    };

                    let mut activates_ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
                    let mut count = 0u32;
                    let flags = MFT_ENUM_FLAG_SYNCMFT
                        | MFT_ENUM_FLAG_HARDWARE
                        | MFT_ENUM_FLAG_SORTANDFILTER;

                    MFTEnumEx(
                        MFT_CATEGORY_VIDEO_DECODER,
                        flags,
                        Some(&input_info),
                        None,
                        &mut activates_ptr,
                        &mut count,
                    )
                    .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;

                    if count == 0 || activates_ptr.is_null() {
                        return Err(VideoDecodeError::DecoderUnavailable(codec));
                    }

                    let mut activated: Option<IMFTransform> = None;
                    for i in 0..count as usize {
                        // Take ownership of each IMFActivate slot so Rust releases
                        // its COM reference cleanly when dropped (never calling
                        // ShutdownObject on the active MFT!).
                        let act_opt: Option<IMFActivate> = std::ptr::read(activates_ptr.add(i));
                        if activated.is_none()
                            && let Some(ref act) = act_opt
                            && let Ok(mft) = act.ActivateObject::<IMFTransform>()
                        {
                            activated = Some(mft);
                        }
                        drop(act_opt);
                    }
                    CoTaskMemFree(Some(activates_ptr.cast()));

                    let mft = activated.ok_or(VideoDecodeError::DecoderUnavailable(codec))?;

                    // Enable zero-reorder low-latency mode if supported by the MFT.
                    if let Ok(attrs) = mft.GetAttributes() {
                        let _ = attrs.SetUINT32(&CODECAPI_AV_LOW_LATENCY_MODE, 1);
                    }

                    // Configure compressed input type.
                    let in_type = MFCreateMediaType()
                        .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                    in_type
                        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                        .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                    in_type
                        .SetGUID(&MF_MT_SUBTYPE, &codec_subtype(codec))
                        .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                    in_type
                        .SetUINT64(&MF_MT_FRAME_SIZE, pack_u64_pair(width, height))
                        .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                    in_type
                        .SetUINT64(&MF_MT_FRAME_RATE, pack_u64_pair(fps.max(1), 1))
                        .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                    in_type
                        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                        .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;

                    mft.SetInputType(0, &in_type, 0)
                        .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;

                    select_nv12_output_type(&mft)?;

                    mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                        .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                    mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                        .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;

                    Ok(mft)
                })();

                match init_res {
                    Ok(transform) => Ok(Self {
                        codec,
                        transform: Some(transform),
                        width,
                        height,
                        fps: fps.max(1),
                        com_initialized,
                    }),
                    Err(err) => {
                        let _ = MFShutdown();
                        if com_initialized {
                            CoUninitialize();
                        }
                        Err(err)
                    }
                }
            }
        }

        pub fn codec(&self) -> VideoCodecChoice {
            self.codec
        }

        pub fn dimensions(&self) -> (u32, u32) {
            (self.width, self.height)
        }

        pub fn fps(&self) -> u32 {
            self.fps
        }

        fn mft(&self) -> &IMFTransform {
            self.transform
                .as_ref()
                .expect("IMFTransform valid until Drop")
        }

        /// Flush internal decoder state (e.g. after packet loss before feeding a
        /// fresh IDR keyframe).
        pub fn flush(&mut self) {
            unsafe {
                let _ = self.mft().ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            }
        }

        /// Feed one compressed video access unit (Annex-B H.264/HEVC or AV1 OBU)
        /// and drain zero or more decoded NV12 frames.
        pub fn decode_access_unit(
            &mut self,
            bitstream: &[u8],
            pts_us: u64,
        ) -> Result<Vec<DecodedNv12Frame>, VideoDecodeError> {
            unsafe {
                let buffer = MFCreateMemoryBuffer(bitstream.len() as u32)
                    .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                let mut ptr = std::ptr::null_mut();
                buffer
                    .Lock(&mut ptr, None, None)
                    .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                std::ptr::copy_nonoverlapping(bitstream.as_ptr(), ptr, bitstream.len());
                let _ = buffer.Unlock();
                buffer
                    .SetCurrentLength(bitstream.len() as u32)
                    .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;

                let sample =
                    MFCreateSample().map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                sample
                    .AddBuffer(&buffer)
                    .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                // Media Foundation timestamps are in 100-nanosecond ticks.
                let _ = sample.SetSampleTime((pts_us * 10) as i64);

                self.mft()
                    .ProcessInput(0, &sample, 0)
                    .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;

                self.drain_outputs()
            }
        }

        unsafe fn drain_outputs(&mut self) -> Result<Vec<DecodedNv12Frame>, VideoDecodeError> {
            let mut frames = Vec::new();
            loop {
                let out_info = unsafe { self.mft().GetOutputStreamInfo(0) }
                    .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;

                // MFT_OUTPUT_STREAM_PROVIDES_SAMPLES = 0x100
                let mft_provides = (out_info.dwFlags & 0x100) != 0;
                let sample_opt: Option<IMFSample> = if mft_provides {
                    None
                } else {
                    let s = unsafe { MFCreateSample() }
                        .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                    let alloc_len = out_info.cbSize.max((self.width * self.height * 3) / 2);
                    let b = unsafe { MFCreateMemoryBuffer(alloc_len) }
                        .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                    unsafe { s.AddBuffer(&b) }
                        .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                    Some(s)
                };

                let mut out_bufs = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(sample_opt),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(None),
                }];
                let mut status = 0u32;

                let res = unsafe { self.mft().ProcessOutput(0, &mut out_bufs, &mut status) };
                let out_sample = std::mem::ManuallyDrop::into_inner(out_bufs[0].pSample.clone());
                let _events = std::mem::ManuallyDrop::into_inner(out_bufs[0].pEvents.clone());

                match res {
                    Ok(()) => {
                        if let Some(sample) = out_sample {
                            let pts_us = unsafe { sample.GetSampleTime() }
                                .map(|t| (t.max(0) / 10) as u64)
                                .unwrap_or(0);
                            let buf = unsafe { sample.ConvertToContiguousBuffer() }
                                .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                            let mut ptr = std::ptr::null_mut();
                            let mut cur_len = 0u32;
                            unsafe { buf.Lock(&mut ptr, None, Some(&mut cur_len)) }
                                .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                            let data = unsafe {
                                std::slice::from_raw_parts(ptr, cur_len as usize).to_vec()
                            };
                            let _ = unsafe { buf.Unlock() };

                            frames.push(DecodedNv12Frame {
                                width: self.width,
                                height: self.height,
                                stride: self.width,
                                pts_us,
                                nv12_data: data,
                            });
                        }
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => break,
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        let (w, h) = unsafe { select_nv12_output_type(self.mft())? };
                        self.width = w;
                        self.height = h;
                    }
                    Err(e) => return Err(VideoDecodeError::Platform(e.to_string())),
                }
            }
            Ok(frames)
        }
    }

    impl Drop for MfVideoDecoder {
        fn drop(&mut self) {
            // CRITICAL: release the COM IMFTransform BEFORE MFShutdown/CoUninitialize!
            drop(self.transform.take());
            unsafe {
                let _ = MFShutdown();
                if self.com_initialized {
                    CoUninitialize();
                }
            }
        }
    }

    unsafe fn select_nv12_output_type(mft: &IMFTransform) -> Result<(u32, u32), VideoDecodeError> {
        let mut idx = 0u32;
        loop {
            let candidate: IMFMediaType = match unsafe { mft.GetOutputAvailableType(0, idx) } {
                Ok(t) => t,
                Err(_) => {
                    return Err(VideoDecodeError::Platform(
                        "MFT did not offer an NV12 output media type".into(),
                    ));
                }
            };
            if let Ok(sub) = unsafe { candidate.GetGUID(&MF_MT_SUBTYPE) }
                && sub == MFVideoFormat_NV12
            {
                unsafe { mft.SetOutputType(0, &candidate, 0) }
                    .map_err(|e| VideoDecodeError::Platform(e.to_string()))?;
                let (w, h) = unsafe { candidate.GetUINT64(&MF_MT_FRAME_SIZE) }
                    .map(unpack_u64_pair)
                    .unwrap_or((1920, 1080));
                return Ok((w, h));
            }
            idx += 1;
        }
    }
}

#[cfg(windows)]
pub use win_impl::MfVideoDecoder;

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn instantiates_real_windows_mf_h264_nv12_decoder() {
        let mut dec = MfVideoDecoder::new(VideoCodecChoice::H264, 1920, 1080, 60)
            .expect("Windows Media Foundation H.264 decoder should activate");
        assert_eq!(dec.codec(), VideoCodecChoice::H264);
        assert_eq!(dec.dimensions(), (1920, 1080));
        assert_eq!(dec.fps(), 60);
        dec.flush();
    }
}
