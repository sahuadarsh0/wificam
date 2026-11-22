package com.wificam.android.core

/**
 * Centralized branding & protocol identifiers mirroring `pc-core/src/brand.rs`.
 */
object Brand {
    const val APP_DISPLAY_NAME = "WifiCam"
    const val APP_SLUG = "wificam"
    const val PAIRING_URI_SCHEME = "wificam"
    const val MDNS_SERVICE_TYPE = "_wificam._tcp."
    const val DEFAULT_CONTROL_PORT = 7890
    const val DEFAULT_MEDIA_UDP_PORT = 7891
    const val BT_RFCOMM_SERVICE_UUID = "6e400001-b5a3-f393-e0a9-e50e24dcca9e"
}
