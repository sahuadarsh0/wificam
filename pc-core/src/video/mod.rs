//! Hardware video decoding, lock-free NV12 framebus IPC, and Windows 11
//! virtual camera hosting.

pub mod decoder_mf;
pub mod shm;
pub mod slate;
pub mod vcam_host;

#[cfg(windows)]
pub use decoder_mf::MfVideoDecoder;
pub use decoder_mf::{DecodedNv12Frame, VideoDecodeError};
pub use shm::{
    CameraState, FRAMEBUS_MAGIC, FRAMEBUS_SLOTS, FRAMEBUS_VERSION, FrameDescriptor, FramebusError,
    FramebusHeader, MAX_FRAME_HEIGHT, MAX_FRAME_WIDTH, MAX_SLOT_BYTES, PixelFormat, ReadFrameMeta,
    TOTAL_SHM_BYTES,
};
#[cfg(windows)]
pub use shm::{FramebusReader, FramebusWriter};
pub use slate::{nv12_byte_len, render_nv12_slate};
pub use vcam_host::{VCAM_DS_CLSID_STR, VCAM_MF_CLSID_STR, VirtualCameraError};
#[cfg(windows)]
pub use vcam_host::{Win11VirtualCamera, is_mf_virtual_camera_supported};
