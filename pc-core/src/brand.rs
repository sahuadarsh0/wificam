//! Centralized branding & protocol identifiers.
//!
//! Keep every user-facing product name, URI scheme, mDNS service type,
//! shared-memory object name, and default port here so renaming the app
//! later is a single-file change.

/// Display name shown in UI, virtual camera friendly name, and pairing prompts.
pub const APP_DISPLAY_NAME: &str = "WifiCam";

/// Lowercase slug used in URI schemes, install paths, and identifiers.
pub const APP_SLUG: &str = "wificam";

/// Pairing QR URI scheme (e.g. `wificam://pair?...`).
pub const PAIRING_URI_SCHEME: &str = "wificam";

/// mDNS service type advertised on LAN.
pub const MDNS_SERVICE_TYPE: &str = "_wificam._tcp.local.";

/// Default TCP control / USB-ADB port.
pub const DEFAULT_CONTROL_PORT: u16 = 7890;

/// Default UDP media port (audio + video over LAN).
pub const DEFAULT_MEDIA_UDP_PORT: u16 = 7891;

/// Windows Global shared memory name for the virtual camera framebus.
pub const VCAM_SHM_GLOBAL_NAME: &str = r"Global\WifiCamVcamShm";

/// Windows Global named event for signaling new virtual camera frames.
pub const VCAM_EVENT_GLOBAL_NAME: &str = r"Global\WifiCamVcamEvent";

/// Bluetooth Classic RFCOMM custom service UUID (128-bit).
pub const BT_RFCOMM_SERVICE_UUID: &str = "6e400001-b5a3-f393-e0a9-e50e24dcca9e";
