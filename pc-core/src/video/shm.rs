//! Lock-free cross-session triple-buffered NV12 shared-memory framebus
//! (`Global\WifiCamVcamShm`) matching `pc-vcam-common/include/framebus_shm.h`.
//!
//! Uses a 64-byte aligned atomic seqlock (zero cross-process mutexes) and an
//! explicit Windows DACL granting `Local Service` (`svchost.exe -k Camera`
//! Frame Server) and `ALL APPLICATION PACKAGES` read access.

use std::mem::{align_of, offset_of, size_of};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

use thiserror::Error;

use crate::brand::{VCAM_EVENT_GLOBAL_NAME, VCAM_SHM_GLOBAL_NAME};

pub const FRAMEBUS_MAGIC: u32 = 0x5643_414D; // "VCAM"
pub const FRAMEBUS_VERSION: u32 = 1;
pub const FRAMEBUS_SLOTS: usize = 3;

pub const MAX_FRAME_WIDTH: u32 = 3840;
pub const MAX_FRAME_HEIGHT: u32 = 2160;
/// 4K UHD NV12 slot size (`3840 * 2160 * 3 / 2 = 12,441,600` bytes).
pub const MAX_SLOT_BYTES: usize = (MAX_FRAME_WIDTH as usize) * (MAX_FRAME_HEIGHT as usize) * 3 / 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PixelFormat {
    Nv12 = 1,
    Bgra32 = 2,
}

impl PixelFormat {
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::Nv12),
            2 => Some(Self::Bgra32),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CameraState {
    Disconnected = 0,
    Streaming = 1,
    Paused = 2,
    Locked = 3,
}

impl CameraState {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Streaming,
            2 => Self::Paused,
            3 => Self::Locked,
            _ => Self::Disconnected,
        }
    }
}

/// Exact 64-byte aligned header shared with C++ `pc-vcam-common/include/framebus_shm.h`.
#[repr(C, align(64))]
pub struct FramebusHeader {
    pub magic: u32,
    pub version: u32,
    pub seq: AtomicU64,
    pub active_slot: AtomicU32,
    pub state: AtomicU32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixel_format: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub pts_us: AtomicU64,
    pub payload_bytes: u32,
    pub reserved: u32,
}

// Compile-time binary layout verification against `framebus_shm.h`.
const _: () = {
    assert!(size_of::<FramebusHeader>() == 64);
    assert!(align_of::<FramebusHeader>() == 64);
    assert!(offset_of!(FramebusHeader, seq) == 8);
    assert!(offset_of!(FramebusHeader, active_slot) == 16);
    assert!(offset_of!(FramebusHeader, state) == 20);
    assert!(offset_of!(FramebusHeader, width) == 24);
    assert!(offset_of!(FramebusHeader, pts_us) == 48);
    assert!(offset_of!(FramebusHeader, payload_bytes) == 56);
};

/// Total shared memory mapping size (~37.3 MiB for 3× 4K NV12 slots).
pub const TOTAL_SHM_BYTES: usize = size_of::<FramebusHeader>() + MAX_SLOT_BYTES * FRAMEBUS_SLOTS;

/// Metadata describing a single video frame published onto the framebus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameDescriptor {
    pub state: CameraState,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixel_format: PixelFormat,
    pub fps_num: u32,
    pub fps_den: u32,
    pub pts_us: u64,
}

/// Snapshot metadata read back by a consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadFrameMeta {
    pub seq: u64,
    pub desc: FrameDescriptor,
    pub payload_bytes: usize,
}

#[derive(Debug, Error)]
pub enum FramebusError {
    #[error("frame payload {len} bytes exceeds max 4K NV12 slot size {max}")]
    FrameTooLarge { len: usize, max: usize },

    #[error("destination buffer too small ({got} < {needed})")]
    BufferTooSmall { got: usize, needed: usize },

    #[error("OS shared memory error: {0}")]
    Platform(String),
}

/// Writer side of the lock-free seqlock shared-memory framebus (owned by `pc-core`).
#[cfg(windows)]
pub struct FramebusWriter {
    mapping: windows::Win32::Foundation::HANDLE,
    event: Option<windows::Win32::Foundation::HANDLE>,
    base: NonNull<u8>,
    name: String,
    next_slot: usize,
}

// SAFETY: `FramebusWriter` owns a process-wide Win32 memory mapping handle and
// pointer that may be moved across threads (single-writer ownership).
#[cfg(windows)]
unsafe impl Send for FramebusWriter {}

#[cfg(windows)]
impl FramebusWriter {
    /// Create the production framebus, trying `Global\WifiCamVcamShm` first
    /// (required by Windows 11 Frame Server `svchost.exe -k Camera`), and
    /// falling back to `Local\WifiCamVcamShm` if running without
    /// `SeCreateGlobalPrivilege`.
    pub fn create_default() -> Result<Self, FramebusError> {
        let local_shm = VCAM_SHM_GLOBAL_NAME.replace(r"Global\", r"Local\");
        let local_evt = VCAM_EVENT_GLOBAL_NAME.replace(r"Global\", r"Local\");
        match Self::create_named(VCAM_SHM_GLOBAL_NAME, Some(VCAM_EVENT_GLOBAL_NAME)) {
            Ok(w) => Ok(w),
            Err(_) => Self::create_named(&local_shm, Some(&local_evt)),
        }
    }

    /// Create or open a named Win32 shared-memory framebus with an explicit
    /// DACL granting `Local Service` & `ALL APPLICATION PACKAGES` read access.
    pub fn create_named(shm_name: &str, event_name: Option<&str>) -> Result<Self, FramebusError> {
        use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, LocalFree};
        use windows::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
        use windows::Win32::System::Memory::{
            CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE,
        };
        use windows::Win32::System::Threading::CreateEventW;
        use windows::core::{HSTRING, w};

        // Owner/System/Admins: Full; Local Service (Frame Server) & AppContainer: Read.
        let sddl = w!("D:(A;;GA;;;BA)(A;;GA;;;SY)(A;;GA;;;OW)(A;;GR;;;LS)(A;;GR;;;AC)");
        let shm_h = HSTRING::from(shm_name);

        unsafe {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            let sd_ok = ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl,
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .is_ok();

            let sa = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: if sd_ok { sd.0 } else { std::ptr::null_mut() },
                bInheritHandle: false.into(),
            };
            let sa_opt = if sd_ok { Some(&sa as *const _) } else { None };

            let size_u64 = TOTAL_SHM_BYTES as u64;
            let mapping_res = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                sa_opt,
                PAGE_READWRITE,
                (size_u64 >> 32) as u32,
                (size_u64 & 0xFFFF_FFFF) as u32,
                &shm_h,
            );

            let event = event_name.and_then(|en| {
                let eh = HSTRING::from(en);
                CreateEventW(sa_opt, false, false, &eh).ok()
            });

            if sd_ok && !sd.0.is_null() {
                let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(sd.0)));
            }

            let mapping = mapping_res.map_err(|e| FramebusError::Platform(e.to_string()))?;
            let view = MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, TOTAL_SHM_BYTES);
            let Some(base) = NonNull::new(view.Value.cast::<u8>()) else {
                let _ = CloseHandle(mapping);
                if let Some(ev) = event {
                    let _ = CloseHandle(ev);
                }
                return Err(FramebusError::Platform(
                    "MapViewOfFile returned null".into(),
                ));
            };

            // Initialize header cleanly.
            let hdr = base.as_ptr().cast::<FramebusHeader>();
            std::ptr::write_bytes(hdr, 0, 1);
            (*hdr).magic = FRAMEBUS_MAGIC;
            (*hdr).version = FRAMEBUS_VERSION;
            (*hdr).seq.store(0, Ordering::Release);
            (*hdr).active_slot.store(0, Ordering::Release);
            (*hdr)
                .state
                .store(CameraState::Disconnected as u32, Ordering::Release);
            (*hdr).pixel_format = PixelFormat::Nv12 as u32;
            (*hdr).fps_num = 30;
            (*hdr).fps_den = 1;

            Ok(Self {
                mapping,
                event,
                base,
                name: shm_name.to_string(),
                next_slot: 0,
            })
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Publish a new video frame into the next inactive triple-buffer slot
    /// using an atomic seqlock (zero locks, zero torn reads).
    pub fn write_frame(
        &mut self,
        desc: FrameDescriptor,
        payload: &[u8],
    ) -> Result<u64, FramebusError> {
        if payload.len() > MAX_SLOT_BYTES {
            return Err(FramebusError::FrameTooLarge {
                len: payload.len(),
                max: MAX_SLOT_BYTES,
            });
        }

        let slot = self.next_slot % FRAMEBUS_SLOTS;
        self.next_slot = (slot + 1) % FRAMEBUS_SLOTS;

        let slot_offset = size_of::<FramebusHeader>() + slot * MAX_SLOT_BYTES;
        unsafe {
            // 1. Copy pixels into the currently-inactive slot BEFORE bumping seq.
            let dst = self.base.as_ptr().add(slot_offset);
            std::ptr::copy_nonoverlapping(payload.as_ptr(), dst, payload.len());

            // 2. Begin seqlock critical section (odd seq) to update metadata + active_slot.
            let hdr = &mut *self.base.as_ptr().cast::<FramebusHeader>();
            let cur_seq = hdr.seq.load(Ordering::Relaxed);
            hdr.seq.store(cur_seq.wrapping_add(1), Ordering::Release);
            fence(Ordering::Release);

            hdr.width = desc.width;
            hdr.height = desc.height;
            hdr.stride = desc.stride;
            hdr.pixel_format = desc.pixel_format as u32;
            hdr.fps_num = desc.fps_num;
            hdr.fps_den = desc.fps_den.max(1);
            hdr.payload_bytes = payload.len() as u32;
            hdr.pts_us.store(desc.pts_us, Ordering::Relaxed);
            hdr.state.store(desc.state as u32, Ordering::Relaxed);
            hdr.active_slot.store(slot as u32, Ordering::Release);

            // 3. Commit seqlock (even seq).
            let new_seq = cur_seq.wrapping_add(2);
            fence(Ordering::Release);
            hdr.seq.store(new_seq, Ordering::Release);

            if let Some(ev) = self.event {
                let _ = windows::Win32::System::Threading::SetEvent(ev);
            }
            Ok(new_seq)
        }
    }
}

#[cfg(windows)]
impl Drop for FramebusWriter {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Memory::{MEMORY_MAPPED_VIEW_ADDRESS, UnmapViewOfFile};
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.base.as_ptr().cast(),
            });
            let _ = CloseHandle(self.mapping);
            if let Some(ev) = self.event {
                let _ = CloseHandle(ev);
            }
        }
    }
}

/// Read-only consumer view of the shared-memory framebus (mirrors C++ virtual
/// camera DLL reader logic for testing and preview).
#[cfg(windows)]
pub struct FramebusReader {
    mapping: windows::Win32::Foundation::HANDLE,
    base: NonNull<u8>,
}

#[cfg(windows)]
unsafe impl Send for FramebusReader {}

#[cfg(windows)]
impl FramebusReader {
    pub fn open_named(shm_name: &str) -> Result<Self, FramebusError> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Memory::{FILE_MAP_READ, MapViewOfFile, OpenFileMappingW};
        use windows::core::HSTRING;

        let shm_h = HSTRING::from(shm_name);
        unsafe {
            let mapping = OpenFileMappingW(FILE_MAP_READ.0, false, &shm_h)
                .map_err(|e| FramebusError::Platform(e.to_string()))?;
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, TOTAL_SHM_BYTES);
            let Some(base) = NonNull::new(view.Value.cast::<u8>()) else {
                let _ = CloseHandle(mapping);
                return Err(FramebusError::Platform(
                    "MapViewOfFile returned null".into(),
                ));
            };
            Ok(Self { mapping, base })
        }
    }

    /// Perform a lock-free atomic seqlock snapshot read into `dst`.
    pub fn try_read_latest(&self, dst: &mut [u8]) -> Result<Option<ReadFrameMeta>, FramebusError> {
        unsafe {
            let hdr = &*self.base.as_ptr().cast::<FramebusHeader>();
            if hdr.magic != FRAMEBUS_MAGIC || hdr.version != FRAMEBUS_VERSION {
                return Ok(None);
            }

            for _ in 0..4 {
                let s1 = hdr.seq.load(Ordering::Acquire);
                if (s1 & 1) != 0 {
                    continue;
                }
                let slot = hdr.active_slot.load(Ordering::Acquire) as usize;
                if slot >= FRAMEBUS_SLOTS {
                    return Ok(None);
                }
                let payload_bytes = hdr.payload_bytes as usize;
                if payload_bytes == 0 || payload_bytes > MAX_SLOT_BYTES {
                    return Ok(None);
                }
                if dst.len() < payload_bytes {
                    return Err(FramebusError::BufferTooSmall {
                        got: dst.len(),
                        needed: payload_bytes,
                    });
                }

                let desc = FrameDescriptor {
                    state: CameraState::from_u32(hdr.state.load(Ordering::Acquire)),
                    width: hdr.width,
                    height: hdr.height,
                    stride: hdr.stride,
                    pixel_format: PixelFormat::from_u32(hdr.pixel_format)
                        .unwrap_or(PixelFormat::Nv12),
                    fps_num: hdr.fps_num,
                    fps_den: hdr.fps_den.max(1),
                    pts_us: hdr.pts_us.load(Ordering::Acquire),
                };

                let offset = size_of::<FramebusHeader>() + slot * MAX_SLOT_BYTES;
                let src = self.base.as_ptr().add(offset);
                std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), payload_bytes);

                fence(Ordering::Acquire);
                let s2 = hdr.seq.load(Ordering::Acquire);
                if s1 == s2 {
                    return Ok(Some(ReadFrameMeta {
                        seq: s1,
                        desc,
                        payload_bytes,
                    }));
                }
            }
            Ok(None)
        }
    }
}

#[cfg(windows)]
impl Drop for FramebusReader {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Memory::{MEMORY_MAPPED_VIEW_ADDRESS, UnmapViewOfFile};
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.base.as_ptr().cast(),
            });
            let _ = CloseHandle(self.mapping);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::thread;

    use super::*;
    use crate::video::slate::{nv12_byte_len, render_nv12_slate};

    #[test]
    fn seqlock_framebus_concurrent_reads_never_tear() {
        let shm_name = format!(r"Local\WifiCamTestShm_{}", std::process::id());
        let mut writer = FramebusWriter::create_named(&shm_name, None).expect("create writer");
        let reader = FramebusReader::open_named(&shm_name).expect("open reader");

        let w = 640u32;
        let h = 360u32;
        let frame_len = nv12_byte_len(w, h);

        // Publish initial paused slate.
        let mut slate = vec![0u8; frame_len];
        render_nv12_slate(w, h, CameraState::Paused, &mut slate);
        writer
            .write_frame(
                FrameDescriptor {
                    state: CameraState::Paused,
                    width: w,
                    height: h,
                    stride: w,
                    pixel_format: PixelFormat::Nv12,
                    fps_num: 60,
                    fps_den: 1,
                    pts_us: 0,
                },
                &slate,
            )
            .unwrap();

        let done = Arc::new(AtomicBool::new(false));
        let done_reader = done.clone();

        let reader_thread = thread::spawn(move || {
            let mut buf = vec![0u8; frame_len];
            let mut verified = 0usize;
            while !done_reader.load(Ordering::Acquire) {
                if let Some(meta) = reader.try_read_latest(&mut buf).unwrap()
                    && meta.desc.state == CameraState::Streaming
                {
                    // Every byte of a coherent streaming test frame equals (pts_us & 0xFF)!
                    let expected = (meta.desc.pts_us & 0xFF) as u8;
                    assert!(
                        buf[..meta.payload_bytes].iter().all(|&b| b == expected),
                        "TORN FRAME DETECTED at seq={} pts={}",
                        meta.seq,
                        meta.desc.pts_us
                    );
                    verified += 1;
                }
            }
            verified
        });

        let mut frame = vec![0u8; frame_len];
        for i in 1u64..=200 {
            frame.fill((i & 0xFF) as u8);
            writer
                .write_frame(
                    FrameDescriptor {
                        state: CameraState::Streaming,
                        width: w,
                        height: h,
                        stride: w,
                        pixel_format: PixelFormat::Nv12,
                        fps_num: 60,
                        fps_den: 1,
                        pts_us: i,
                    },
                    &frame,
                )
                .unwrap();
        }

        done.store(true, Ordering::Release);
        let verified = reader_thread.join().unwrap();
        assert!(verified > 0, "reader should have verified coherent frames");
    }
}
