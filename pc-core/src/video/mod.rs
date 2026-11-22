//! Hardware video decoding, lock-free NV12 framebus IPC, and Windows 11
//! virtual camera hosting.

pub mod shm;
pub mod slate;

pub use shm::{
    CameraState, FRAMEBUS_MAGIC, FRAMEBUS_SLOTS, FRAMEBUS_VERSION, FrameDescriptor, FramebusError,
    FramebusHeader, MAX_FRAME_HEIGHT, MAX_FRAME_WIDTH, MAX_SLOT_BYTES, PixelFormat, ReadFrameMeta,
    TOTAL_SHM_BYTES,
};
#[cfg(windows)]
pub use shm::{FramebusReader, FramebusWriter};
pub use slate::{nv12_byte_len, render_nv12_slate};
