//! Windows Bluetooth Classic RFCOMM (Serial Port Profile) transport for
//! wireless Opus microphone failover (~45 ms).
//!
//! Platform note: Windows does NOT expose a public BLE L2CAP CoC API in WinRT,
//! so Bluetooth Classic RFCOMM (`AF_BTH` / `BTHPROTO_RFCOMM`) is the proven,
//! reliable high-throughput Bluetooth audio transport on Windows 10/11.

use thiserror::Error;

/// Paired Bluetooth Classic device discovered on the Windows host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedBluetoothDevice {
    pub name: String,
    /// 48-bit Bluetooth MAC address packed in the low 6 bytes.
    pub address: u64,
    pub connected: bool,
    pub authenticated: bool,
}

impl PairedBluetoothDevice {
    /// Format MAC address as `AA:BB:CC:DD:EE:FF`.
    pub fn mac_string(&self) -> String {
        format_bt_mac(self.address)
    }
}

/// Format a 48-bit packed Bluetooth address into standard `AA:BB:CC:DD:EE:FF`.
pub fn format_bt_mac(addr: u64) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        (addr >> 40) & 0xFF,
        (addr >> 32) & 0xFF,
        (addr >> 24) & 0xFF,
        (addr >> 16) & 0xFF,
        (addr >> 8) & 0xFF,
        addr & 0xFF,
    )
}

#[derive(Debug, Error)]
pub enum BluetoothError {
    #[error("Bluetooth Win32 / WinSock error: {0}")]
    Platform(String),

    #[error("invalid Bluetooth service UUID: {0}")]
    InvalidUuid(String),
}

/// Enumerate paired/remembered Bluetooth Classic devices on Windows using
/// `BluetoothFindFirstDevice` / `BluetoothFindNextDevice`.
#[cfg(windows)]
pub fn list_paired_bluetooth_devices() -> Result<Vec<PairedBluetoothDevice>, BluetoothError> {
    use windows::Win32::Devices::Bluetooth::{
        BLUETOOTH_DEVICE_INFO, BLUETOOTH_DEVICE_SEARCH_PARAMS, BluetoothFindDeviceClose,
        BluetoothFindFirstDevice, BluetoothFindNextDevice,
    };

    unsafe {
        let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
            fReturnAuthenticated: true.into(),
            fReturnRemembered: true.into(),
            fReturnUnknown: false.into(),
            fReturnConnected: true.into(),
            fIssueInquiry: false.into(),
            cTimeoutMultiplier: 0,
            hRadio: Default::default(),
        };

        let mut info = BLUETOOTH_DEVICE_INFO {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
            ..Default::default()
        };

        let Ok(handle) = BluetoothFindFirstDevice(&params, &mut info) else {
            // No paired devices or no Bluetooth radio present -> empty list.
            return Ok(Vec::new());
        };

        let mut devices = Vec::new();
        loop {
            let name_end = info
                .szName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(info.szName.len());
            let name = String::from_utf16_lossy(&info.szName[..name_end]);
            let address = info.Address.Anonymous.ullLong;

            devices.push(PairedBluetoothDevice {
                name,
                address,
                connected: info.fConnected.as_bool(),
                authenticated: info.fAuthenticated.as_bool(),
            });

            info.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
            if BluetoothFindNextDevice(handle, &mut info).is_err() {
                break;
            }
        }

        let _ = BluetoothFindDeviceClose(handle);
        Ok(devices)
    }
}

/// Parse a canonical hyphenated UUID string (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`)
/// into a Windows `GUID` for WinSock `SOCKADDR_BTH::serviceClassId`.
#[cfg(windows)]
pub fn parse_uuid_to_guid(uuid_str: &str) -> Result<windows::core::GUID, BluetoothError> {
    let clean: String = uuid_str.chars().filter(|&c| c != '-').collect();
    if clean.len() != 32 {
        return Err(BluetoothError::InvalidUuid(uuid_str.to_string()));
    }
    let val = u128::from_str_radix(&clean, 16)
        .map_err(|_| BluetoothError::InvalidUuid(uuid_str.to_string()))?;
    Ok(windows::core::GUID::from_u128(val))
}

/// Packed Win32 `SOCKADDR_BTH` layout matching `<ws2bth.h>` (`#pragma pack(push, 1)`).
#[cfg(windows)]
#[repr(C, packed)]
struct SockAddrBth {
    address_family: u16, // AF_BTH = 32
    bt_addr: u64,
    service_class_id: windows::core::GUID,
    port: u32, // 0 = resolve via SDP from service_class_id
}

/// Active Windows Bluetooth Classic RFCOMM byte stream (`AF_BTH`, `BTHPROTO_RFCOMM`).
#[cfg(windows)]
pub struct WindowsRfcommStream {
    socket: windows::Win32::Networking::WinSock::SOCKET,
}

#[cfg(windows)]
impl WindowsRfcommStream {
    /// Connect to an Android phone's RFCOMM service UUID via SDP resolution.
    pub fn connect(bt_addr: u64, service_uuid: &str) -> Result<Self, BluetoothError> {
        use windows::Win32::Networking::WinSock::{
            INVALID_SOCKET, SOCK_STREAM, WSADATA, WSAStartup, closesocket, connect, socket,
        };

        const AF_BTH: i32 = 32;
        const BTHPROTO_RFCOMM: i32 = 3;

        let guid = parse_uuid_to_guid(service_uuid)?;

        unsafe {
            let mut wsa = WSADATA::default();
            let rc = WSAStartup(0x0202, &mut wsa);
            if rc != 0 {
                return Err(BluetoothError::Platform(format!("WSAStartup failed: {rc}")));
            }

            let sock = socket(AF_BTH, SOCK_STREAM, BTHPROTO_RFCOMM)
                .map_err(|e| BluetoothError::Platform(e.to_string()))?;
            if sock == INVALID_SOCKET {
                return Err(BluetoothError::Platform("socket(AF_BTH) failed".into()));
            }

            let addr = SockAddrBth {
                address_family: AF_BTH as u16,
                bt_addr,
                service_class_id: guid,
                port: 0, // Resolve RFCOMM channel dynamically via SDP
            };

            let conn_rc = connect(
                sock,
                (&addr as *const SockAddrBth).cast(),
                std::mem::size_of::<SockAddrBth>() as i32,
            );
            if conn_rc != 0 {
                let _ = closesocket(sock);
                return Err(BluetoothError::Platform(format!(
                    "RFCOMM connect failed (rc={conn_rc})"
                )));
            }

            Ok(Self { socket: sock })
        }
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, BluetoothError> {
        use windows::Win32::Networking::WinSock::{SEND_RECV_FLAGS, recv};
        let n = unsafe { recv(self.socket, buf, SEND_RECV_FLAGS(0)) };
        if n < 0 {
            return Err(BluetoothError::Platform("RFCOMM recv error".into()));
        }
        Ok(n as usize)
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize, BluetoothError> {
        use windows::Win32::Networking::WinSock::{SEND_RECV_FLAGS, send};
        let n = unsafe { send(self.socket, buf, SEND_RECV_FLAGS(0)) };
        if n < 0 {
            return Err(BluetoothError::Platform("RFCOMM send error".into()));
        }
        Ok(n as usize)
    }
}

#[cfg(windows)]
impl Drop for WindowsRfcommStream {
    fn drop(&mut self) {
        use windows::Win32::Networking::WinSock::closesocket;
        unsafe {
            let _ = closesocket(self.socket);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::BT_RFCOMM_SERVICE_UUID;

    #[test]
    fn formats_bt_mac_address_correctly() {
        assert_eq!(format_bt_mac(0x0011_2233_4455), "00:11:22:33:44:55");
        assert_eq!(format_bt_mac(0xAABB_CCDD_EEFF), "AA:BB:CC:DD:EE:FF");
    }

    #[cfg(windows)]
    #[test]
    fn parses_service_uuid_and_enumerates_paired_bt_devices() {
        let guid = parse_uuid_to_guid(BT_RFCOMM_SERVICE_UUID).expect("parse RFCOMM UUID");
        assert_eq!(guid.data1, 0x6e40_0001);

        let devices = list_paired_bluetooth_devices().expect("BT enumeration should succeed");
        let _ = devices.len();
    }
}
