package com.wificam.android

import android.Manifest
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.graphics.ImageFormat
import android.hardware.usb.UsbAccessory
import android.hardware.usb.UsbManager
import android.media.ImageReader
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.view.Surface
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.core.content.ContextCompat
import androidx.lifecycle.lifecycleScope
import com.wificam.android.audio.OboeAudioEngine
import com.wificam.android.core.Brand
import com.wificam.android.protocol.PacketFlags
import com.wificam.android.protocol.PairingInfo
import com.wificam.android.protocol.StreamType
import com.wificam.android.protocol.WirePacket
import com.wificam.android.qr.QrPairingScanner
import com.wificam.android.service.StreamForegroundService
import com.wificam.android.transport.ActiveTransportTier
import com.wificam.android.transport.AndroidTransportManager
import com.wificam.android.ui.PreviewCropMode
import com.wificam.android.ui.StudioCockpitScreen
import com.wificam.android.ui.StudioUiState
import com.wificam.android.ui.VideoQualityPreset
import com.wificam.android.ui.VideoResolutionPreset
import com.wificam.android.ui.theme.WifiCamStudioTheme
import com.wificam.android.video.Camera2Controller
import com.wificam.android.video.HardwareVideoEncoder
import com.wificam.android.video.VideoEncoderConfig
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {

    private lateinit var cameraController: Camera2Controller
    private lateinit var transportManager: AndroidTransportManager
    private var audioEngine: OboeAudioEngine? = null
    private var videoEncoder: HardwareVideoEncoder? = null
    private var previewSurface: Surface? = null
    private var qrImageReader: ImageReader? = null
    private var qrThread: HandlerThread? = null
    private var lastQrScanMs: Long = 0L
    private var rememberedControlPort: Int = Brand.DEFAULT_CONTROL_PORT
    private var rememberedUdpPort: Int = Brand.DEFAULT_MEDIA_UDP_PORT
    private var controlSeq: Int = 1

    private val notificationStopReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            if (intent?.action == StreamForegroundService.ACTION_STOP_STREAM_BROADCAST) {
                stopStreaming(statusNote = "Stopped from notification.")
            }
        }
    }

    private val uiState = MutableStateFlow(StudioUiState())

    private fun sendVideoTransformToPc() {
        val st = uiState.value
        val zoomX100 = (st.previewCropMode.zoom * 100f).toInt().coerceIn(100, 250)
        val json = """{"cmd":"video_transform","rotation":${st.previewRotationDeg},"flip_h":${st.previewFlipH},"flip_v":${st.previewFlipV},"zoom_x100":$zoomX100}"""
        transportManager.enqueuePacket(
            WirePacket(
                streamType = StreamType.Control,
                flags = PacketFlags.NONE,
                seq = (controlSeq++).toUInt(),
                ptsUs = System.nanoTime() / 1000L,
                payload = json.toByteArray(Charsets.UTF_8),
            )
        )
    }

    private fun saveRememberedHost(name: String, ip: String, controlPort: Int, udpPort: Int) {
        rememberedControlPort = controlPort
        rememberedUdpPort = udpPort
        getSharedPreferences("wificam_pairing", Context.MODE_PRIVATE).edit()
            .putString("host_name", name)
            .putString("host_ip", ip)
            .putInt("control_port", controlPort)
            .putInt("udp_port", udpPort)
            .apply()
    }

    private fun loadRememberedHost() {
        val prefs = getSharedPreferences("wificam_pairing", Context.MODE_PRIVATE)
        val bgEnabled = prefs.getBoolean("background_streaming", true)
        val ip = prefs.getString("host_ip", null)?.takeIf { it.isNotBlank() }
        if (ip == null) {
            uiState.update { it.copy(backgroundStreamingEnabled = bgEnabled) }
            return
        }
        val name = prefs.getString("host_name", "Last PC") ?: "Last PC"
        rememberedControlPort = prefs.getInt("control_port", Brand.DEFAULT_CONTROL_PORT)
        rememberedUdpPort = prefs.getInt("udp_port", Brand.DEFAULT_MEDIA_UDP_PORT)
        uiState.update {
            it.copy(
                isPaired = true,
                pairedHostName = name,
                pairedHostIp = ip,
                backgroundStreamingEnabled = bgEnabled,
            )
        }
    }

    private fun clearRememberedHost() {
        rememberedControlPort = Brand.DEFAULT_CONTROL_PORT
        rememberedUdpPort = Brand.DEFAULT_MEDIA_UDP_PORT
        getSharedPreferences("wificam_pairing", Context.MODE_PRIVATE).edit().clear().apply()
    }

    private val permissionLauncher = registerForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions()
    ) { results ->
        val granted = results[Manifest.permission.CAMERA] == true &&
            results[Manifest.permission.RECORD_AUDIO] == true
        uiState.update { it.copy(hasPermissions = granted) }
        if (granted) {
            refreshLensesAndPreview()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        cameraController = Camera2Controller(this)
        transportManager = AndroidTransportManager(this, lifecycleScope)

        lifecycleScope.launch {
            transportManager.tier.collect { tier ->
                val wasStreaming = uiState.value.isStreaming
                uiState.update {
                    it.copy(
                        tier = tier,
                        isPaired = it.isPaired || tier != ActiveTransportTier.DISCONNECTED,
                    )
                }
                // If transport drops while streaming, shut camera & stream off cleanly so camera is never stuck on.
                if (tier == ActiveTransportTier.DISCONNECTED && wasStreaming) {
                    stopStreaming(statusNote = "Disconnected from PC. Tap RECONNECT when PC is ready.")
                }
            }
        }
        lifecycleScope.launch {
            transportManager.discoveredHosts.collect { hosts ->
                uiState.update { it.copy(discoveredHosts = hosts) }
            }
        }

        val hasPerms = hasRequiredPermissions()
        uiState.update { it.copy(hasPermissions = hasPerms) }
        if (hasPerms) {
            uiState.update { it.copy(lenses = cameraController.discoverLenses()) }
        }
        loadRememberedHost()
        ContextCompat.registerReceiver(
            this,
            notificationStopReceiver,
            IntentFilter(StreamForegroundService.ACTION_STOP_STREAM_BROADCAST),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        transportManager.startLanDiscovery()
        handleUsbAccessoryIntent(intent)

        setContent {
            WifiCamStudioTheme {
                val state by uiState.collectAsState()
                StudioCockpitScreen(
                    state = state,
                    onPreviewSurfaceReady = { surface ->
                        previewSurface = surface
                        restartCameraPipeline()
                    },
                    onPreviewSurfaceDestroyed = {
                        previewSurface = null
                        val st = uiState.value
                        val keepInBg = st.isStreaming &&
                            st.tier != ActiveTransportTier.DISCONNECTED &&
                            st.backgroundStreamingEnabled &&
                            !st.isVideoPaused &&
                            !st.isQrScannerOpen
                        if (keepInBg) {
                            // Continue streaming headless directly into hardware MediaCodec Surface in background!
                            restartCameraPipeline()
                        } else {
                            cameraController.stopCamera()
                        }
                    },
                    onToggleStream = ::toggleStreaming,
                    onToggleVideoPause = {
                        val next = !uiState.value.isVideoPaused
                        uiState.update { it.copy(isVideoPaused = next) }
                        videoEncoder?.setPaused(next)
                        restartCameraPipeline()
                    },
                    onToggleMicMute = {
                        val next = !uiState.value.isMicMuted
                        uiState.update { it.copy(isMicMuted = next) }
                        audioEngine?.setMuted(next)
                    },
                    onSelectLens = { lensId ->
                        uiState.update { it.copy(activeLensId = lensId) }
                        restartCameraPipeline()
                    },
                    onSelectResolution = { res ->
                        val adjustedFps = if (res == VideoResolutionPreset.R4K && uiState.value.fps > 30) 30 else uiState.value.fps
                        uiState.update { it.copy(resolution = res, fps = adjustedFps) }
                        restartStreamIfActive()
                    },
                    onSelectFps = { fps ->
                        uiState.update { it.copy(fps = fps) }
                        restartStreamIfActive()
                    },
                    onSelectQuality = { quality ->
                        uiState.update { it.copy(quality = quality) }
                        restartStreamIfActive()
                    },
                    onSelectCodec = { codec ->
                        uiState.update { it.copy(codec = codec) }
                        restartStreamIfActive()
                    },
                    onSelectAntibanding = { hz ->
                        uiState.update { it.copy(antibandingHz = hz) }
                        restartCameraPipeline()
                    },
                    onRotatePreview = {
                        uiState.update { it.copy(previewRotationDeg = (it.previewRotationDeg + 90) % 360) }
                        sendVideoTransformToPc()
                    },
                    onToggleFlipH = {
                        uiState.update { it.copy(previewFlipH = !it.previewFlipH) }
                        sendVideoTransformToPc()
                    },
                    onToggleFlipV = {
                        uiState.update { it.copy(previewFlipV = !it.previewFlipV) }
                        sendVideoTransformToPc()
                    },
                    onCycleCropMode = {
                        val all = PreviewCropMode.entries
                        uiState.update {
                            val next = all[(all.indexOf(it.previewCropMode) + 1) % all.size]
                            it.copy(previewCropMode = next)
                        }
                        sendVideoTransformToPc()
                    },
                    onOpenSettings = {
                        uiState.update { it.copy(isSettingsOpen = true, isQrScannerOpen = false) }
                    },
                    onCloseSettings = {
                        uiState.update { it.copy(isSettingsOpen = false) }
                    },
                    onUnpair = {
                        stopStreaming()
                        transportManager.disconnect()
                        clearRememberedHost()
                        uiState.update {
                            it.copy(
                                isPaired = false,
                                pairedHostName = "",
                                pairedHostIp = "",
                                tier = ActiveTransportTier.DISCONNECTED,
                            )
                        }
                    },
                    onQuickReconnect = {
                        val st = uiState.value
                        if (st.pairedHostIp.isNotBlank() && st.pairedHostIp != "USB AOA") {
                            connectAndStartStreaming(st.pairedHostIp, rememberedControlPort, rememberedUdpPort)
                        } else {
                            uiState.update { it.copy(isQrScannerOpen = true, isSettingsOpen = false, pendingConsent = null) }
                            startQrAnalyzer()
                        }
                    },
                    onOpenQrScanner = {
                        uiState.update {
                            it.copy(isQrScannerOpen = true, isSettingsOpen = false, pendingConsent = null)
                        }
                        startQrAnalyzer()
                    },
                    onCloseQrScanner = {
                        // Dismiss overlay immediately before tearing down Camera2 & ImageReader so UI never freezes.
                        uiState.update { it.copy(isQrScannerOpen = false, pendingConsent = null) }
                        stopQrAnalyzer()
                        restartCameraPipeline()
                    },
                    onApproveQrPairing = { info ->
                        saveRememberedHost(info.hostName, info.ip, info.controlPort, info.mediaUdpPort)
                        uiState.update {
                            it.copy(
                                isQrScannerOpen = false,
                                pendingConsent = null,
                                isPaired = true,
                                pairedHostName = info.hostName,
                                pairedHostIp = info.ip,
                                connectionMessage = "Connecting to ${info.hostName}...",
                            )
                        }
                        stopQrAnalyzer()
                        connectAndStartStreaming(info.ip, info.controlPort, info.mediaUdpPort)
                    },
                    onRejectQrPairing = {
                        uiState.update { it.copy(pendingConsent = null) }
                    },
                    onManualIpConnect = { ip ->
                        saveRememberedHost("LAN PC", ip, Brand.DEFAULT_CONTROL_PORT, Brand.DEFAULT_MEDIA_UDP_PORT)
                        uiState.update {
                            it.copy(
                                isQrScannerOpen = false,
                                pendingConsent = null,
                                isPaired = true,
                                pairedHostName = "LAN PC",
                                pairedHostIp = ip,
                                connectionMessage = "Connecting to $ip...",
                            )
                        }
                        stopQrAnalyzer()
                        connectAndStartStreaming(ip, Brand.DEFAULT_CONTROL_PORT, Brand.DEFAULT_MEDIA_UDP_PORT)
                    },
                    onConnectDiscoveredHost = { host ->
                        saveRememberedHost(host.name, host.ip, host.controlPort, host.mediaUdpPort)
                        uiState.update {
                            it.copy(
                                isQrScannerOpen = false,
                                pendingConsent = null,
                                isPaired = true,
                                pairedHostName = host.name,
                                pairedHostIp = host.ip,
                                connectionMessage = "Connecting to ${host.name}...",
                            )
                        }
                        stopQrAnalyzer()
                        connectAndStartStreaming(host.ip, host.controlPort, host.mediaUdpPort)
                    },
                    onToggleOledCurtain = {
                        uiState.update { it.copy(isOledCurtainActive = !it.isOledCurtainActive) }
                    },
                    onToggleBackgroundStreaming = { enabled ->
                        getSharedPreferences("wificam_pairing", Context.MODE_PRIVATE)
                            .edit()
                            .putBoolean("background_streaming", enabled)
                            .apply()
                        uiState.update { it.copy(backgroundStreamingEnabled = enabled) }
                    },
                    onMinimizeToBackground = {
                        moveTaskToBack(true)
                    },
                    onRequestPermissions = ::requestRequiredPermissions,
                )
            }
        }
    }

    override fun onStop() {
        super.onStop()
        val st = uiState.value
        if (st.isStreaming && !st.backgroundStreamingEnabled && !isChangingConfigurations) {
            stopStreaming(statusNote = "Stream stopped when app went to background.")
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleUsbAccessoryIntent(intent)
    }

    override fun onDestroy() {
        runCatching { unregisterReceiver(notificationStopReceiver) }
        stopQrAnalyzer()
        stopStreaming()
        transportManager.stopLanDiscovery()
        transportManager.disconnect()
        cameraController.stopCamera()
        super.onDestroy()
    }

    private fun handleUsbAccessoryIntent(intent: Intent?) {
        if (intent?.action != UsbManager.ACTION_USB_ACCESSORY_ATTACHED) return
        val accessory: UsbAccessory? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(UsbManager.EXTRA_ACCESSORY, UsbAccessory::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(UsbManager.EXTRA_ACCESSORY)
        }
        if (accessory != null && transportManager.connectUsbAccessory(accessory)) {
            uiState.update { it.copy(isPaired = true, pairedHostName = "USB PC", pairedHostIp = "USB AOA") }
            if (!uiState.value.isStreaming && hasRequiredPermissions()) {
                toggleStreaming()
            }
        }
    }

    private fun toggleStreaming() {
        val st = uiState.value
        if (st.isStreaming) {
            stopStreaming()
        } else if (!st.isPaired) {
            // If not paired yet, open QR scanner directly instead of starting an unrouted stream.
            uiState.update { it.copy(isQrScannerOpen = true, isSettingsOpen = false, pendingConsent = null) }
            startQrAnalyzer()
        } else if (st.pairedHostIp.isNotBlank() && st.pairedHostIp != "USB AOA") {
            connectAndStartStreaming(st.pairedHostIp, rememberedControlPort, rememberedUdpPort)
        } else {
            startStreamingMedia()
        }
    }

    private fun restartStreamIfActive() {
        if (uiState.value.isStreaming && transportManager.tier.value != ActiveTransportTier.DISCONNECTED) {
            // Restart encoder/camera cleanly while keeping the active PC connection alive.
            cameraController.stopCamera()
            videoEncoder?.stop()
            videoEncoder = null
            audioEngine?.stop()
            audioEngine?.release()
            audioEngine = null
            startStreamingMedia()
        }
    }

    /**
     * Connect to the PC FIRST; only activate camera & microphone if the PC actually accepts the link!
     * If the PC is closed or unreachable, camera stays completely OFF.
     */
    private fun connectAndStartStreaming(ip: String, controlPort: Int, udpPort: Int) {
        if (!hasRequiredPermissions()) {
            requestRequiredPermissions()
            return
        }
        uiState.update { it.copy(connectionMessage = "Connecting to $ip...") }
        transportManager.connectWifi(ip, controlPort, udpPort) { connected ->
            runOnUiThread {
                if (connected) {
                    uiState.update { it.copy(connectionMessage = "") }
                    startStreamingMedia()
                } else {
                    cameraController.stopCamera()
                    uiState.update {
                        it.copy(
                            isStreaming = false,
                            tier = ActiveTransportTier.DISCONNECTED,
                            connectionMessage = "Could not reach PC ($ip). Open WifiCam Studio on Windows and tap RECONNECT.",
                        )
                    }
                }
            }
        }
    }

    private fun startStreamingMedia() {
        if (!hasRequiredPermissions()) {
            requestRequiredPermissions()
            return
        }
        if (transportManager.tier.value == ActiveTransportTier.DISCONNECTED) {
            return
        }
        ContextCompat.startForegroundService(this, Intent(this, StreamForegroundService::class.java))

        val st = uiState.value
        val bitrateKbps = (st.resolution.defaultBitrateKbps * st.quality.multiplier).toInt().coerceAtLeast(1_500)
        val enc = HardwareVideoEncoder(
            config = VideoEncoderConfig(
                width = st.resolution.width,
                height = st.resolution.height,
                fps = st.fps,
                bitrateKbps = bitrateKbps,
                codec = st.codec,
            ),
            onPacket = transportManager::enqueuePacket,
        )
        videoEncoder = enc

        val audio = OboeAudioEngine(
            onAudioPacket = transportManager::enqueuePacket,
            onVuLevelUpdate = { dbfs -> uiState.update { it.copy(rmsDbfsX10 = dbfs) } },
        ).also {
            it.setMuted(st.isMicMuted)
            it.start(unprocessedMic = false)
        }
        audioEngine = audio

        uiState.update { it.copy(isStreaming = true, connectionMessage = "") }
        restartCameraPipeline()
        sendVideoTransformToPc()
    }

    private fun stopStreaming(statusNote: String = "") {
        // Notify PC immediately over UDP so Windows removes the virtual camera without waiting for timeout.
        runCatching {
            transportManager.enqueuePacket(
                WirePacket(
                    streamType = StreamType.Control,
                    flags = PacketFlags.NONE,
                    seq = (controlSeq++).toUInt(),
                    ptsUs = System.nanoTime() / 1_000L,
                    payload = """{"cmd":"disconnect"}""".toByteArray(Charsets.UTF_8),
                )
            )
        }
        cameraController.stopCamera()
        videoEncoder?.stop()
        videoEncoder = null
        audioEngine?.stop()
        audioEngine?.release()
        audioEngine = null
        transportManager.disconnect()
        uiState.update {
            it.copy(
                isStreaming = false,
                rmsDbfsX10 = -960,
                tier = ActiveTransportTier.DISCONNECTED,
                connectionMessage = statusNote,
            )
        }
        startService(Intent(this, StreamForegroundService::class.java).apply {
            action = StreamForegroundService.ACTION_STOP
        })
    }

    private fun startQrAnalyzer() {
        stopQrAnalyzer()
        val thread = HandlerThread("wificam-qr-scan").apply { start() }
        val handler = Handler(thread.looper)
        qrThread = thread

        val reader = ImageReader.newInstance(640, 480, ImageFormat.YUV_420_888, 2)
        reader.setOnImageAvailableListener({ r ->
            val img = r.acquireLatestImage() ?: return@setOnImageAvailableListener
            try {
                val now = System.currentTimeMillis()
                if (now - lastQrScanMs < 120L || uiState.value.pendingConsent != null) {
                    return@setOnImageAvailableListener
                }
                lastQrScanMs = now
                val yPlane = img.planes[0]
                val buf = yPlane.buffer
                val bytes = ByteArray(buf.remaining())
                buf.get(bytes)
                val parsed: PairingInfo? = QrPairingScanner.decodeFromLuma(bytes, img.width, img.height)
                if (parsed != null) {
                    uiState.update { it.copy(pendingConsent = parsed) }
                }
            } finally {
                img.close()
            }
        }, handler)
        qrImageReader = reader
        restartCameraPipeline()
    }

    private fun stopQrAnalyzer() {
        // CRITICAL: Always stop Camera2 BEFORE closing ImageReader to avoid Camera2 surface deadlock!
        cameraController.stopCamera()
        runCatching { qrImageReader?.close() }
        qrImageReader = null
        qrThread?.quitSafely()
        qrThread = null
    }

    private fun restartCameraPipeline() {
        if (!hasRequiredPermissions()) return
        val st = uiState.value
        val isActivelyConnected = st.isStreaming && st.tier != ActiveTransportTier.DISCONNECTED
        val cameraShouldRun = (isActivelyConnected && !st.isVideoPaused) || st.isQrScannerOpen
        if (!cameraShouldRun) {
            cameraController.stopCamera()
            return
        }
        // ALWAYS use the rear 1x lens when scanning QR codes, even if Front camera was selected earlier!
        val chosenCameraId = if (st.isQrScannerOpen) {
            st.lenses.firstOrNull { !it.isFront && it.label == "1x" }?.cameraId
                ?: st.lenses.firstOrNull { !it.isFront }?.cameraId
                ?: st.activeLensId
        } else {
            st.activeLensId
        }
        val encSurface = if (isActivelyConnected && !st.isVideoPaused && !st.isQrScannerOpen) {
            runCatching { videoEncoder?.start() }.getOrNull()
        } else {
            null
        }
        cameraController.startCamera(
            cameraId = chosenCameraId,
            targetFps = if (st.isQrScannerOpen) 30 else st.fps,
            antibandingHz = st.antibandingHz,
            previewSurface = previewSurface,
            encoderSurface = encSurface,
            analysisSurface = if (st.isQrScannerOpen) qrImageReader?.surface else null,
        )
    }

    private fun refreshLensesAndPreview() {
        val lenses = cameraController.discoverLenses()
        uiState.update {
            it.copy(
                lenses = lenses,
                activeLensId = lenses.firstOrNull()?.cameraId ?: "0",
            )
        }
        restartCameraPipeline()
    }

    private fun hasRequiredPermissions(): Boolean {
        val cam = ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
        val mic = ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        return cam && mic
    }

    private fun requestRequiredPermissions() {
        val perms = buildList {
            add(Manifest.permission.CAMERA)
            add(Manifest.permission.RECORD_AUDIO)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                add(Manifest.permission.POST_NOTIFICATIONS)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                add(Manifest.permission.BLUETOOTH_CONNECT)
            }
        }
        permissionLauncher.launch(perms.toTypedArray())
    }
}
