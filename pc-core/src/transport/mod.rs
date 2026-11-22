//! 4-tier automatic transport layer (USB AOA -> USB ADB -> USB Tether ->
//! Encrypted Wi-Fi UDP/TCP + mDNS -> Bluetooth Classic RFCOMM).

pub mod bt_rfcomm;
pub mod failover;
pub mod mdns;
pub mod usb_adb;
pub mod usb_aoa;
pub mod wifi;

pub use bt_rfcomm::{BluetoothError, PairedBluetoothDevice, format_bt_mac};
#[cfg(windows)]
pub use bt_rfcomm::{WindowsRfcommStream, list_paired_bluetooth_devices, parse_uuid_to_guid};
pub use failover::{
    FailoverDecision, FailoverManager, HEARTBEAT_INTERVAL, HEARTBEAT_TIMEOUT, TransportTier,
};
pub use mdns::{MdnsAdvertiseConfig, MdnsAdvertiser, MdnsError};
pub use usb_adb::{
    AdbDevice, AdbDeviceState, AdbError, AdbSupervisor, locate_adb_binary, parse_adb_devices_output,
};
pub use usb_aoa::{
    AOA_BULK_PACKET_SIZE, AOA_GOOGLE_VID, AOA_PID_ACCESSORY, AOA_PID_ACCESSORY_ADB,
    AoaBulkEndpoints, AoaIdentity, UsbAoaError, UsbDeviceSummary, find_aoa_bulk_endpoints,
    is_aoa_vid_pid, list_usb_devices, switch_device_to_aoa,
};
pub use wifi::{
    FramedTcpConnection, MAX_UDP_DATAGRAM_BYTES, TcpControlListener, UDP_SOCKET_BUFFER_BYTES,
    UdpMediaSocket, WifiTransportError,
};
