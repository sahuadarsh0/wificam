//! Automatic USB Debugging (`adb reverse`) device discovery & tunnel manager.
//!
//! Uses `adb reverse tcp:<port> tcp:<port>` (rather than `adb forward`) so the
//! Android app connects to `127.0.0.1:<port>` with the exact same TCP client
//! state machine used over Wi-Fi LAN.

use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

use crate::brand::DEFAULT_CONTROL_PORT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdbDeviceState {
    Online,
    Unauthorized,
    Offline,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbDevice {
    pub serial: String,
    pub state: AdbDeviceState,
    pub model: Option<String>,
    pub transport_id: Option<String>,
}

#[derive(Debug, Error)]
pub enum AdbError {
    #[error("adb executable not found on PATH or standard Android SDK locations")]
    NotFound,

    #[error("failed to execute adb: {0}")]
    Spawn(#[from] std::io::Error),

    #[error("adb command failed (exit={code:?}): {stderr}")]
    CommandFailed { code: Option<i32>, stderr: String },
}

/// Parse `adb devices -l` stdout into structured [`AdbDevice`] records.
pub fn parse_adb_devices_output(stdout: &str) -> Vec<AdbDevice> {
    let mut devices = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("List of devices") || trimmed.starts_with('*')
        {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(serial) = parts.next() else {
            continue;
        };
        let Some(state_str) = parts.next() else {
            continue;
        };
        let state = match state_str {
            "device" => AdbDeviceState::Online,
            "unauthorized" => AdbDeviceState::Unauthorized,
            "offline" => AdbDeviceState::Offline,
            _ => AdbDeviceState::Other,
        };

        let mut model = None;
        let mut transport_id = None;
        for token in parts {
            if let Some(m) = token.strip_prefix("model:") {
                model = Some(m.replace('_', " "));
            } else if let Some(tid) = token.strip_prefix("transport_id:") {
                transport_id = Some(tid.to_string());
            }
        }

        devices.push(AdbDevice {
            serial: serial.to_string(),
            state,
            model,
            transport_id,
        });
    }
    devices
}

/// Locate `adb` / `adb.exe` across `PATH`, `ANDROID_HOME`, `ANDROID_SDK_ROOT`,
/// and common Windows SDK install locations.
pub fn locate_adb_binary() -> Option<PathBuf> {
    let exe_name = if cfg!(windows) { "adb.exe" } else { "adb" };

    // 1. Check PATH first.
    if let Ok(path_env) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_env) {
            let candidate = dir.join(exe_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // 2. Check ANDROID_HOME / ANDROID_SDK_ROOT.
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Ok(sdk) = std::env::var(var) {
            let candidate = Path::new(&sdk).join("platform-tools").join(exe_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // 3. Common Windows fallback paths.
    if cfg!(windows) {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let candidate = Path::new(&local)
                .join("Android")
                .join("Sdk")
                .join("platform-tools")
                .join(exe_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        let e_sdk = Path::new(r"E:\Android\Sdk\platform-tools").join(exe_name);
        if e_sdk.is_file() {
            return Some(e_sdk);
        }
    }
    None
}

/// Manages `adb devices -l` queries and `adb reverse` tunnels.
#[derive(Debug, Clone)]
pub struct AdbSupervisor {
    adb_path: PathBuf,
    port: u16,
}

impl AdbSupervisor {
    pub fn discover(port: u16) -> Result<Self, AdbError> {
        let adb_path = locate_adb_binary().ok_or(AdbError::NotFound)?;
        Ok(Self { adb_path, port })
    }

    pub fn with_path(adb_path: PathBuf, port: u16) -> Self {
        Self { adb_path, port }
    }

    pub fn adb_path(&self) -> &Path {
        &self.adb_path
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn list_devices(&self) -> Result<Vec<AdbDevice>, AdbError> {
        let output = Command::new(&self.adb_path)
            .args(["devices", "-l"])
            .output()?;
        if !output.status.success() {
            return Err(AdbError::CommandFailed {
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(parse_adb_devices_output(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    /// Set up `adb -s <serial> reverse tcp:<port> tcp:<port>`.
    pub fn setup_reverse(&self, serial: &str) -> Result<(), AdbError> {
        let spec = format!("tcp:{}", self.port);
        let output = Command::new(&self.adb_path)
            .args(["-s", serial, "reverse", &spec, &spec])
            .output()?;
        if !output.status.success() {
            return Err(AdbError::CommandFailed {
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(())
    }

    /// Remove the reverse tunnel cleanly on disconnect/shutdown.
    pub fn remove_reverse(&self, serial: &str) -> Result<(), AdbError> {
        let spec = format!("tcp:{}", self.port);
        let output = Command::new(&self.adb_path)
            .args(["-s", serial, "reverse", "--remove", &spec])
            .output()?;
        if !output.status.success() {
            return Err(AdbError::CommandFailed {
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(())
    }
}

impl Default for AdbSupervisor {
    fn default() -> Self {
        Self {
            adb_path: locate_adb_binary().unwrap_or_else(|| PathBuf::from("adb")),
            port: DEFAULT_CONTROL_PORT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_adb_devices_long_output() {
        let sample = "\
* daemon not running; starting now at tcp:5037
* daemon started successfully
List of devices attached
R5CW30ABCDE            device product:dm1qxxx model:SM_S911B device:dm1q transport_id:4
3A141FDH200012         unauthorized usb:1-3 transport_id:7
emulator-5554          offline transport_id:9
";
        let list = parse_adb_devices_output(sample);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].serial, "R5CW30ABCDE");
        assert_eq!(list[0].state, AdbDeviceState::Online);
        assert_eq!(list[0].model.as_deref(), Some("SM S911B"));
        assert_eq!(list[0].transport_id.as_deref(), Some("4"));

        assert_eq!(list[1].serial, "3A141FDH200012");
        assert_eq!(list[1].state, AdbDeviceState::Unauthorized);

        assert_eq!(list[2].serial, "emulator-5554");
        assert_eq!(list[2].state, AdbDeviceState::Offline);
    }

    #[cfg(windows)]
    #[test]
    fn locates_adb_on_dev_machine() {
        match AdbSupervisor::discover(DEFAULT_CONTROL_PORT) {
            Ok(sup) => assert!(sup.adb_path().exists()),
            Err(_) if std::env::var_os("GITHUB_ACTIONS").is_some() => {}
            Err(e) => panic!("adb.exe should be discovered on this dev machine: {e}"),
        }
    }
}
