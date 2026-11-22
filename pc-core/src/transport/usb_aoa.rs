//! Plug-and-play USB streaming via Android Open Accessory (AOA v1/v2) over
//! pure-Rust `nusb` (zero Developer Options or ADB required).
//!
//! Protocol summary (AOSP):
//! 1. Detect an Android USB device (or one already in AOA mode `0x18D1:0x2D00/0x2D01`).
//! 2. Send vendor control transfer `AOA_GET_PROTOCOL` (51) -> returns version `>= 1`.
//! 3. Send 6 identifying strings via `AOA_SEND_IDENT` (52) so Android auto-launches
//!    our companion app and shows the one-tap USB accessory consent dialog.
//! 4. Send `AOA_START` (53) -> device re-enumerates as `0x18D1:0x2D00` / `0x2D01`.
//! 5. Open Bulk IN/OUT endpoints (`16 KiB` packet reads; zero-length packets are
//!    treated as frame boundaries, never EOF).

use std::time::Duration;

use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};
use nusb::{DeviceInfo, MaybeFuture};
use thiserror::Error;

use crate::brand::{APP_DISPLAY_NAME, APP_SLUG};

/// Google USB Vendor ID used when Android re-enumerates in AOA mode.
pub const AOA_GOOGLE_VID: u16 = 0x18D1;
/// AOA Accessory-only Product ID.
pub const AOA_PID_ACCESSORY: u16 = 0x2D00;
/// AOA Accessory + ADB composite Product ID.
pub const AOA_PID_ACCESSORY_ADB: u16 = 0x2D01;

/// Standard AOA USB control request codes.
pub const AOA_REQ_GET_PROTOCOL: u8 = 51;
pub const AOA_REQ_SEND_IDENT: u8 = 52;
pub const AOA_REQ_START: u8 = 53;

/// Recommended USB bulk transfer packet size (16 KiB — matches Android
/// `UsbAccessory` FileInputStream buffer size).
pub const AOA_BULK_PACKET_SIZE: usize = 16 * 1024;

/// AOA string descriptor indices sent via `AOA_REQ_SEND_IDENT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AoaIdentity {
    pub manufacturer: String,
    pub model: String,
    pub description: String,
    pub version: String,
    pub uri: String,
    pub serial: String,
}

impl Default for AoaIdentity {
    fn default() -> Self {
        Self {
            manufacturer: format!("{APP_DISPLAY_NAME} Open Source"),
            model: APP_DISPLAY_NAME.to_string(),
            description: "Low-Latency PC Webcam & Studio Microphone".to_string(),
            version: "2.0".to_string(),
            uri: format!("https://github.com/sahuadarsh0/{APP_SLUG}"),
            serial: "WIFICAM-AOA-0001".to_string(),
        }
    }
}

/// Summary of an attached USB device discovered on the host bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDeviceSummary {
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    pub is_aoa_active: bool,
}

#[derive(Debug, Error)]
pub enum UsbAoaError {
    #[error("USB enumeration failed: {0}")]
    Enumerate(String),

    #[error("failed to open USB device: {0}")]
    Open(String),

    #[error("device does not support Android Open Accessory (protocol={0})")]
    UnsupportedProtocol(u16),

    #[error("AOA control transfer failed: {0}")]
    ControlTransfer(String),

    #[error("no AOA bulk IN/OUT endpoints found on interface 0")]
    MissingBulkEndpoints,
}

/// True if `(vid, pid)` corresponds to an Android device already enumerated in
/// AOA Accessory mode (`0x18D1:0x2D00` or `0x18D1:0x2D01`).
#[inline]
pub const fn is_aoa_vid_pid(vid: u16, pid: u16) -> bool {
    vid == AOA_GOOGLE_VID && (pid == AOA_PID_ACCESSORY || pid == AOA_PID_ACCESSORY_ADB)
}

/// List connected USB devices on the host bus, flagging any already in AOA mode.
pub fn list_usb_devices() -> Result<Vec<UsbDeviceSummary>, UsbAoaError> {
    let iter = nusb::list_devices()
        .wait()
        .map_err(|e| UsbAoaError::Enumerate(e.to_string()))?;

    let mut out = Vec::new();
    for dev in iter {
        let vid = dev.vendor_id();
        let pid = dev.product_id();
        out.push(UsbDeviceSummary {
            vendor_id: vid,
            product_id: pid,
            manufacturer: dev.manufacturer_string().map(str::to_string),
            product: dev.product_string().map(str::to_string),
            serial: dev.serial_number().map(str::to_string),
            is_aoa_active: is_aoa_vid_pid(vid, pid),
        });
    }
    Ok(out)
}

/// Perform the AOA v1/v2 control handshake to switch an Android USB device into
/// Accessory mode (`0x18D1:0x2D00` / `0x2D01`).
pub fn switch_device_to_aoa(info: &DeviceInfo, ident: &AoaIdentity) -> Result<u16, UsbAoaError> {
    let dev = info
        .open()
        .wait()
        .map_err(|e| UsbAoaError::Open(e.to_string()))?;
    let iface = dev
        .claim_interface(0)
        .wait()
        .map_err(|e| UsbAoaError::Open(e.to_string()))?;
    let timeout = Duration::from_millis(500);

    // 1. Query AOA protocol version (little-endian u16).
    let version_bytes = iface
        .control_in(
            ControlIn {
                control_type: ControlType::Vendor,
                recipient: Recipient::Device,
                request: AOA_REQ_GET_PROTOCOL,
                value: 0,
                index: 0,
                length: 2,
            },
            timeout,
        )
        .wait()
        .map_err(|e| UsbAoaError::ControlTransfer(e.to_string()))?;

    if version_bytes.len() < 2 {
        return Err(UsbAoaError::UnsupportedProtocol(0));
    }
    let protocol = u16::from_le_bytes([version_bytes[0], version_bytes[1]]);
    if protocol == 0 {
        return Err(UsbAoaError::UnsupportedProtocol(0));
    }

    // 2. Send null-terminated identifying strings (indices 0..=5).
    let fields = [
        ident.manufacturer.as_str(),
        ident.model.as_str(),
        ident.description.as_str(),
        ident.version.as_str(),
        ident.uri.as_str(),
        ident.serial.as_str(),
    ];
    for (idx, val) in fields.iter().enumerate() {
        let mut z = val.as_bytes().to_vec();
        z.push(0);
        iface
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: AOA_REQ_SEND_IDENT,
                    value: 0,
                    index: idx as u16,
                    data: &z,
                },
                timeout,
            )
            .wait()
            .map_err(|e| UsbAoaError::ControlTransfer(e.to_string()))?;
    }

    // 3. Request device to switch into Accessory mode and re-enumerate.
    iface
        .control_out(
            ControlOut {
                control_type: ControlType::Vendor,
                recipient: Recipient::Device,
                request: AOA_REQ_START,
                value: 0,
                index: 0,
                data: &[],
            },
            timeout,
        )
        .wait()
        .map_err(|e| UsbAoaError::ControlTransfer(e.to_string()))?;

    Ok(protocol)
}

/// Discovered Bulk IN & Bulk OUT endpoint addresses for an AOA accessory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AoaBulkEndpoints {
    pub interface_number: u8,
    pub bulk_in: u8,
    pub bulk_out: u8,
}

/// Inspect active USB configuration on an AOA device and locate the first
/// Bulk IN + Bulk OUT endpoint pair on interface 0.
pub fn find_aoa_bulk_endpoints(device: &nusb::Device) -> Result<AoaBulkEndpoints, UsbAoaError> {
    let cfg = device
        .active_configuration()
        .map_err(|e| UsbAoaError::Open(e.to_string()))?;

    for iface in cfg.interfaces() {
        for alt in iface.alt_settings() {
            let mut bulk_in = None;
            let mut bulk_out = None;
            for ep in alt.endpoints() {
                if ep.transfer_type() == nusb::descriptors::TransferType::Bulk {
                    match ep.direction() {
                        nusb::transfer::Direction::In => bulk_in = Some(ep.address()),
                        nusb::transfer::Direction::Out => bulk_out = Some(ep.address()),
                    }
                }
            }
            if let (Some(bin), Some(bout)) = (bulk_in, bulk_out) {
                return Ok(AoaBulkEndpoints {
                    interface_number: iface.interface_number(),
                    bulk_in: bin,
                    bulk_out: bout,
                });
            }
        }
    }
    Err(UsbAoaError::MissingBulkEndpoints)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_aoa_vid_pid_pairs() {
        assert!(is_aoa_vid_pid(0x18D1, 0x2D00));
        assert!(is_aoa_vid_pid(0x18D1, 0x2D01));
        assert!(!is_aoa_vid_pid(0x18D1, 0x4EE7)); // Normal Pixel MTP/ADB
        assert!(!is_aoa_vid_pid(0x04E8, 0x2D00)); // Non-Google VID
    }

    #[test]
    fn enumerates_real_host_usb_bus_cleanly() {
        let devices = list_usb_devices().expect("nusb host bus enumeration should succeed");
        // Every desktop/laptop has at least root USB hubs / peripherals.
        let _ = devices.len();
    }
}
