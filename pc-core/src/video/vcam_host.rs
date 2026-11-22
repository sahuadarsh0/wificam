//! Windows 11 `MFCreateVirtualCamera` lifecycle host controller & shared
//! COM CLSID constants for both `pc-vcam-mf.dll` (Win11) and `pc-vcam-ds.dll` (Win10).

use thiserror::Error;

/// COM CLSID implemented by `pc-vcam-mf.dll` (Windows 11 Frame Server Media Source).
///
/// Must match `kVcamMfClsid` in `pc-vcam-mf/src/dllmain.cpp`.
pub const VCAM_MF_CLSID_STR: &str = "{B7E4F9A1-6C2D-4F18-9E3A-1D4C5B6E7F80}";

/// COM CLSID implemented by `pc-vcam-ds.dll` (Windows 10 DirectShow fallback filter).
///
/// Must match `kVcamDsClsid` in `pc-vcam-ds/src/dllmain.cpp`.
pub const VCAM_DS_CLSID_STR: &str = "{C8F5A0B2-7D3E-4A29-8F4B-2E5D6C7F8091}";

#[derive(Debug, Error)]
pub enum VirtualCameraError {
    #[error("Windows 11 MFCreateVirtualCamera API is not supported on this OS build")]
    UnsupportedOs,

    #[error("failed to create/start virtual camera: {0}")]
    Platform(String),
}

/// Check dynamically whether `mfsensorgroup.dll` exports `MFCreateVirtualCamera`
/// (true on Windows 11 build 22000+, false on Windows 10).
#[cfg(windows)]
pub fn is_mf_virtual_camera_supported() -> bool {
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::{s, w};

    unsafe {
        let Ok(module) = LoadLibraryW(w!("mfsensorgroup.dll")) else {
            return false;
        };
        let has_api = GetProcAddress(module, s!("MFCreateVirtualCamera")).is_some();
        let _ = FreeLibrary(module);
        has_api
    }
}

/// RAII handle for a live Windows 11 Media Foundation Virtual Camera backed by
/// `pc-vcam-mf.dll`.
#[cfg(windows)]
pub struct Win11VirtualCamera {
    vcam: windows::Win32::Media::MediaFoundation::IMFVirtualCamera,
    started: bool,
}

#[cfg(windows)]
impl Win11VirtualCamera {
    /// Register a session-scoped software camera with Windows 11 Frame Server.
    ///
    /// Prerequisite: `pc-vcam-mf.dll` must be registered in `HKLM` via
    /// `regsvr32` from a path readable by `Local Service` (e.g. `C:\Program Files\WifiCam\`).
    pub fn create(friendly_name: &str) -> Result<Self, VirtualCameraError> {
        use windows::Win32::Media::MediaFoundation::{
            MFCreateVirtualCamera, MFVirtualCameraAccess_CurrentUser,
            MFVirtualCameraLifetime_Session, MFVirtualCameraType_SoftwareCameraSource,
        };
        use windows::core::HSTRING;

        if !is_mf_virtual_camera_supported() {
            return Err(VirtualCameraError::UnsupportedOs);
        }

        let name_h = HSTRING::from(friendly_name);
        let clsid_h = HSTRING::from(VCAM_MF_CLSID_STR);

        let vcam = unsafe {
            MFCreateVirtualCamera(
                MFVirtualCameraType_SoftwareCameraSource,
                MFVirtualCameraLifetime_Session,
                MFVirtualCameraAccess_CurrentUser,
                &name_h,
                &clsid_h,
                None,
            )
        }
        .map_err(|e| VirtualCameraError::Platform(e.to_string()))?;

        Ok(Self {
            vcam,
            started: false,
        })
    }

    pub fn start(&mut self) -> Result<(), VirtualCameraError> {
        unsafe { self.vcam.Start(None) }
            .map_err(|e| VirtualCameraError::Platform(e.to_string()))?;
        self.started = true;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), VirtualCameraError> {
        if self.started {
            unsafe { self.vcam.Stop() }.map_err(|e| VirtualCameraError::Platform(e.to_string()))?;
            self.started = false;
        }
        Ok(())
    }

    pub fn remove(mut self) -> Result<(), VirtualCameraError> {
        let _ = self.stop();
        unsafe { self.vcam.Remove() }.map_err(|e| VirtualCameraError::Platform(e.to_string()))?;
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for Win11VirtualCamera {
    fn drop(&mut self) {
        if self.started {
            let _ = unsafe { self.vcam.Stop() };
            self.started = false;
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn detects_win11_mf_virtual_camera_api_on_this_host() {
        // This development machine runs Windows 11 (build 26300), so
        // mfsensorgroup.dll!MFCreateVirtualCamera MUST be present.
        assert!(
            is_mf_virtual_camera_supported(),
            "MFCreateVirtualCamera should be available on Windows 11"
        );
    }
}
