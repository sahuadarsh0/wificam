package com.wificam.android.transport

import android.annotation.SuppressLint
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothServerSocket
import android.content.Context
import android.hardware.usb.UsbAccessory
import android.hardware.usb.UsbManager
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import com.wificam.android.core.Brand
import com.wificam.android.protocol.PacketCodec
import com.wificam.android.protocol.PacketFlags
import com.wificam.android.protocol.PairingInfo
import com.wificam.android.protocol.StreamType
import com.wificam.android.protocol.WirePacket
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.io.FileOutputStream
import java.io.OutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.util.UUID
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.atomic.AtomicInteger

enum class ActiveTransportTier(val badge: String) {
    DISCONNECTED("IDLE"),
    USB_AOA("USB AOA"),
    USB_ADB("USB ADB"),
    WIFI_LAN("WI-FI"),
    BLUETOOTH("BT AUDIO"),
}

data class DiscoveredHost(
    val name: String,
    val ip: String,
    val controlPort: Int,
    val mediaUdpPort: Int,
    val fingerprint: String = "",
)

/**
 * 4-tier Android transport controller with strict audio-priority TX scheduling:
 *   1. USB AOA (`UsbAccessory` 16 KiB bulk pipe, zero ADB)
 *   2. USB `adb reverse` (`127.0.0.1:7890` TCP_NODELAY)
 *   3. Wi-Fi LAN (mDNS / QR paired dual TCP control + UDP media)
 *   4. Bluetooth Classic RFCOMM server (audio failover)
 */
class AndroidTransportManager(
    private val context: Context,
    private val scope: CoroutineScope,
) {
    private val _tier = MutableStateFlow(ActiveTransportTier.DISCONNECTED)
    val tier: StateFlow<ActiveTransportTier> = _tier.asStateFlow()

    private val _discoveredHosts = MutableStateFlow<List<DiscoveredHost>>(emptyList())
    val discoveredHosts: StateFlow<List<DiscoveredHost>> = _discoveredHosts.asStateFlow()

    private val highPriorityQueue = LinkedBlockingQueue<WirePacket>(256)
    private val videoQueue = LinkedBlockingQueue<WirePacket>(512)
    private val heartbeatSeq = AtomicInteger(0)

    private var txJob: Job? = null
    private var heartbeatJob: Job? = null
    private var activeTcp: Socket? = null
    private var activeUdp: DatagramSocket? = null
    private var udpTargetAddr: InetAddress? = null
    private var udpTargetPort: Int = Brand.DEFAULT_MEDIA_UDP_PORT
    private var activeAoaFd: ParcelFileDescriptor? = null
    private var activeStreamOut: OutputStream? = null
    private var btServer: BluetoothServerSocket? = null
    private var nsdManager: NsdManager? = null
    private var discoveryListener: NsdManager.DiscoveryListener? = null

    fun enqueuePacket(packet: WirePacket) {
        if (packet.streamType.isVideo) {
            if (!videoQueue.offer(packet)) {
                // Drop oldest stale video fragment rather than stalling live feed.
                videoQueue.poll()
                videoQueue.offer(packet)
            }
        } else {
            highPriorityQueue.offer(packet)
        }
    }

    /** Connect over USB Android Open Accessory (plug-and-play, no ADB). */
    fun connectUsbAccessory(accessory: UsbAccessory): Boolean {
        disconnect()
        val usbManager = context.getSystemService(Context.USB_SERVICE) as UsbManager
        val pfd = usbManager.openAccessory(accessory) ?: return false
        activeAoaFd = pfd
        activeStreamOut = FileOutputStream(pfd.fileDescriptor)
        _tier.value = ActiveTransportTier.USB_AOA
        startTxLoop(useUdp = false)
        return true
    }

    /** Connect over USB `adb reverse tcp:7890 tcp:7890` (`127.0.0.1`). */
    fun connectUsbAdb(port: Int = Brand.DEFAULT_CONTROL_PORT) {
        disconnect()
        scope.launch(Dispatchers.IO) {
            runCatching {
                val sock = Socket().apply {
                    tcpNoDelay = true
                    connect(InetSocketAddress("127.0.0.1", port), 1500)
                }
                activeTcp = sock
                activeStreamOut = sock.getOutputStream()
                _tier.value = ActiveTransportTier.USB_ADB
                startTxLoop(useUdp = false)
            }.onFailure {
                _tier.value = ActiveTransportTier.DISCONNECTED
            }
        }
    }

    /** Connect over Wi-Fi LAN using either a QR-scanned [PairingInfo] or mDNS host. */
    fun connectWifi(ip: String, controlPort: Int = Brand.DEFAULT_CONTROL_PORT, mediaUdpPort: Int = Brand.DEFAULT_MEDIA_UDP_PORT) {
        disconnect()
        scope.launch(Dispatchers.IO) {
            runCatching {
                val addr = InetAddress.getByName(ip)
                val tcp = Socket().apply {
                    tcpNoDelay = true
                    connect(InetSocketAddress(addr, controlPort), 2500)
                }
                val udp = DatagramSocket().apply {
                    sendBufferSize = 2 * 1024 * 1024
                }
                activeTcp = tcp
                activeStreamOut = tcp.getOutputStream()
                activeUdp = udp
                udpTargetAddr = addr
                udpTargetPort = mediaUdpPort
                _tier.value = ActiveTransportTier.WIFI_LAN
                startTxLoop(useUdp = true)
            }.onFailure {
                _tier.value = ActiveTransportTier.DISCONNECTED
            }
        }
    }

    fun connectFromQrPairing(info: PairingInfo) {
        connectWifi(info.ip, info.controlPort, info.mediaUdpPort)
    }

    /** Start Bluetooth Classic RFCOMM audio failover listener. */
    @SuppressLint("MissingPermission")
    fun startBluetoothRfcommServer() {
        disconnect()
        scope.launch(Dispatchers.IO) {
            runCatching {
                val mgr = context.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager
                val adapter = mgr.adapter ?: return@launch
                val server = adapter.listenUsingRfcommWithServiceRecord(
                    Brand.APP_DISPLAY_NAME,
                    UUID.fromString(Brand.BT_RFCOMM_SERVICE_UUID),
                )
                btServer = server
                val client = server.accept()
                activeStreamOut = client.outputStream
                _tier.value = ActiveTransportTier.BLUETOOTH
                startTxLoop(useUdp = false, addCrc = true)
            }
        }
    }

    /** Discover PCs advertising `_wificam._tcp.` on local LAN via Android NsdManager. */
    @Suppress("DEPRECATION")
    fun startLanDiscovery() {
        stopLanDiscovery()
        val mgr = context.getSystemService(Context.NSD_SERVICE) as? NsdManager ?: return
        nsdManager = mgr
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(regType: String) {}
            override fun onServiceFound(service: NsdServiceInfo) {
                mgr.resolveService(
                    service,
                    object : NsdManager.ResolveListener {
                        override fun onResolveFailed(info: NsdServiceInfo, code: Int) {}
                        override fun onServiceResolved(info: NsdServiceInfo) {
                            val host = info.host?.hostAddress ?: return
                            val udpPort = info.attributes["udp"]?.decodeToString()?.toIntOrNull()
                                ?: Brand.DEFAULT_MEDIA_UDP_PORT
                            val fp = info.attributes["fp"]?.decodeToString().orEmpty()
                            val item = DiscoveredHost(info.serviceName, host, info.port, udpPort, fp)
                            _discoveredHosts.value = (_discoveredHosts.value.filterNot { it.ip == host }) + item
                        }
                    },
                )
            }

            override fun onServiceLost(service: NsdServiceInfo) {
                _discoveredHosts.value = _discoveredHosts.value.filterNot { it.name == service.serviceName }
            }

            override fun onDiscoveryStopped(serviceType: String) {}
            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {}
            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
        }
        discoveryListener = listener
        runCatching { mgr.discoverServices(Brand.MDNS_SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener) }
    }

    fun stopLanDiscovery() {
        val mgr = nsdManager
        val listener = discoveryListener
        if (mgr != null && listener != null) {
            runCatching { mgr.stopServiceDiscovery(listener) }
        }
        discoveryListener = null
    }

    private fun startTxLoop(useUdp: Boolean, addCrc: Boolean = false) {
        txJob?.cancel()
        heartbeatJob?.cancel()

        heartbeatJob = scope.launch(Dispatchers.IO) {
            while (isActive && _tier.value != ActiveTransportTier.DISCONNECTED) {
                enqueuePacket(
                    WirePacket(
                        streamType = StreamType.Heartbeat,
                        flags = if (addCrc) PacketFlags.CRC_PRESENT else PacketFlags.NONE,
                        seq = heartbeatSeq.getAndIncrement().toUInt(),
                        ptsUs = SystemClock.elapsedRealtimeNanos() / 1_000L,
                        payload = ByteArray(0),
                    )
                )
                delay(500L)
            }
        }

        txJob = scope.launch(Dispatchers.IO) {
            while (isActive && _tier.value != ActiveTransportTier.DISCONNECTED) {
                val pkt = highPriorityQueue.poll() ?: videoQueue.poll()
                if (pkt == null) {
                    delay(1L)
                    continue
                }
                val finalPkt = if (addCrc) {
                    pkt.copy(flags = pkt.flags or PacketFlags.CRC_PRESENT)
                } else {
                    pkt
                }
                val wire = PacketCodec.encode(finalPkt)
                val ok = runCatching {
                    if (useUdp && (finalPkt.streamType.isAudio || finalPkt.streamType.isVideo)) {
                        val udp = activeUdp
                        val addr = udpTargetAddr
                        if (udp != null && addr != null) {
                            udp.send(DatagramPacket(wire, wire.size, addr, udpTargetPort))
                        }
                    } else {
                        activeStreamOut?.write(wire)
                    }
                    true
                }.getOrDefault(false)

                if (!ok) {
                    disconnect()
                    break
                }
            }
        }
    }

    fun disconnect() {
        txJob?.cancel()
        heartbeatJob?.cancel()
        txJob = null
        heartbeatJob = null
        highPriorityQueue.clear()
        videoQueue.clear()
        runCatching { activeStreamOut?.close() }
        runCatching { activeTcp?.close() }
        runCatching { activeUdp?.close() }
        runCatching { activeAoaFd?.close() }
        runCatching { btServer?.close() }
        activeStreamOut = null
        activeTcp = null
        activeUdp = null
        activeAoaFd = null
        btServer = null
        _tier.value = ActiveTransportTier.DISCONNECTED
    }
}
