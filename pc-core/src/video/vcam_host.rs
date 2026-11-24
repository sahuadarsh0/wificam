//! Windows 11 `MFCreateVirtualCamera` lifecycle host controller & COM CLSID
//! for `pc-vcam-mf.dll`.

use thiserror::Error;

/// COM CLSID implemented by `pc-vcam-mf.dll` (Windows 11 Frame Server Media Source).
///
/// Must match `kVcamMfClsid` in `pc-vcam-mf/src/dllmain.cpp`.
pub const VCAM_MF_CLSID_STR: &str = "{B7E4F9A1-6C2D-4F18-9E3A-1D4C5B6E7F80}";

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
    vcam: Option<windows::Win32::Media::MediaFoundation::IMFVirtualCamera>,
    started: bool,
    com_initialized: bool,
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
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
        use windows::core::HSTRING;

        if !is_mf_virtual_camera_supported() {
            return Err(VirtualCameraError::UnsupportedOs);
        }

        let com_initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();

        let name_h = HSTRING::from(friendly_name);
        let clsid_h = HSTRING::from(VCAM_MF_CLSID_STR);

        let vcam = match unsafe {
            MFCreateVirtualCamera(
                MFVirtualCameraType_SoftwareCameraSource,
                MFVirtualCameraLifetime_Session,
                MFVirtualCameraAccess_CurrentUser,
                &name_h,
                &clsid_h,
                None,
            )
        } {
            Ok(v) => v,
            Err(e) => {
                if com_initialized {
                    unsafe { CoUninitialize() };
                }
                return Err(VirtualCameraError::Platform(e.to_string()));
            }
        };

        Ok(Self {
            vcam: Some(vcam),
            started: false,
            com_initialized,
        })
    }

    pub fn start(&mut self) -> Result<(), VirtualCameraError> {
        if let Some(ref vcam) = self.vcam {
            unsafe { vcam.Start(None) }.map_err(|e| VirtualCameraError::Platform(e.to_string()))?;
            self.started = true;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), VirtualCameraError> {
        if self.started
            && let Some(ref vcam) = self.vcam
        {
            // For MFVirtualCameraLifetime_Session, Shutdown() unregisters & removes the camera cleanly.
            let _ = unsafe { vcam.Stop() };
            self.started = false;
        }
        Ok(())
    }

    pub fn remove(mut self) -> Result<(), VirtualCameraError> {
        if let Some(vcam) = self.vcam.take() {
            let res =
                unsafe { vcam.Shutdown() }.map_err(|e| VirtualCameraError::Platform(e.to_string()));
            self.started = false;
            drop(vcam);
            return res;
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for Win11VirtualCamera {
    fn drop(&mut self) {
        // Release the COM pointer BEFORE CoUninitialize() so Release() runs while COM is alive!
        if let Some(vcam) = self.vcam.take() {
            let _ = unsafe { vcam.Shutdown() };
            self.started = false;
            drop(vcam);
        }
        if self.com_initialized {
            unsafe { windows::Win32::System::Com::CoUninitialize() };
            self.com_initialized = false;
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn detects_win11_mf_virtual_camera_api_on_this_host() {
        let supported = is_mf_virtual_camera_supported();
        // Headless Windows Server CI runners may lack Desktop Experience / mfsensorgroup.dll.
        if std::env::var_os("GITHUB_ACTIONS").is_none() {
            assert!(
                supported,
                "MFCreateVirtualCamera should be available on Windows 11"
            );
        }
    }

    #[test]
    #[ignore = "requires registered pc-vcam-mf.dll in HKLM (run locally on Windows 11)"]
    fn live_win11_virtual_camera_starts_and_removes_cleanly() {
        let mut vcam =
            Win11VirtualCamera::create("WifiCam Camera").expect("create Win11 virtual camera");
        vcam.start()
            .expect("start Win11 virtual camera via FrameServer");
        let while_live = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                "Get-PnpDevice | Where-Object { $_.FriendlyName -match 'WifiCam' } | Select-Object Status, Class, FriendlyName | Format-Table -AutoSize",
            ])
            .output()
            .expect("query pnp while live");
        println!(
            "WHILE LIVE:\n{}",
            String::from_utf8_lossy(&while_live.stdout)
        );
        vcam.remove().expect("remove Win11 virtual camera");
        std::thread::sleep(std::time::Duration::from_millis(500));
        let after_stop = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                "Get-PnpDevice | Where-Object { $_.FriendlyName -match 'WifiCam' } | Select-Object Status, Class, FriendlyName | Format-Table -AutoSize",
            ])
            .output()
            .expect("query pnp after stop");
        println!(
            "AFTER STOP:\n{}",
            String::from_utf8_lossy(&after_stop.stdout)
        );
    }
}
