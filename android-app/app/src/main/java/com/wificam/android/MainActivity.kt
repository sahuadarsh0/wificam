package com.wificam.android

import android.Manifest
import android.content.Context
import android.content.Intent
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
import com.wificam.android.protocol.PairingInfo
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

    private val uiState = MutableStateFlow(StudioUiState())

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
        val ip = prefs.getString("host_ip", null)?.takeIf { it.isNotBlank() } ?: return
        val name = prefs.getString("host_name", "Last PC") ?: "Last PC"
        rememberedControlPort = prefs.getInt("control_port", Brand.DEFAULT_CONTROL_PORT)
        rememberedUdpPort = prefs.getInt("udp_port", Brand.DEFAULT_MEDIA_UDP_PORT)
        uiState.update {
            it.copy(
                isPaired = true,
                pairedHostName = name,
                pairedHostIp = ip,
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
                uiState.update {
                    it.copy(
                        tier = tier,
                        isPaired = it.isPaired || tier != ActiveTransportTier.DISCONNECTED,
                    )
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
                        cameraController.stopCamera()
                    },
                    onToggleStream = ::toggleStreaming,
                    onToggleVideoPause = {
                        val next = !uiState.value.isVideoPaused
                        uiState.update { it.copy(isVideoPaused = next) }
                        videoEncoder?.setPaused(next)
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
                    },
                    onToggleFlipH = {
                        uiState.update { it.copy(previewFlipH = !it.previewFlipH) }
                    },
                    onToggleFlipV = {
                        uiState.update { it.copy(previewFlipV = !it.previewFlipV) }
                    },
                    onCycleCropMode = {
                        val all = PreviewCropMode.entries
                        uiState.update {
                            val next = all[(all.indexOf(it.previewCropMode) + 1) % all.size]
                            it.copy(previewCropMode = next)
                        }
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
                            transportManager.connectWifi(st.pairedHostIp, rememberedControlPort, rememberedUdpPort)
                            if (!st.isStreaming) {
                                startStreaming()
                            }
                        }
                    },
                    onOpenQrScanner = {
                        uiState.update {
                            it.copy(isQrScannerOpen = true, isSettingsOpen = false, pendingConsent = null)
                        }
                        startQrAnalyzer()
                    },
                    onCloseQrScanner = {
                        stopQrAnalyzer()
                        uiState.update { it.copy(isQrScannerOpen = false, pendingConsent = null) }
                        restartCameraPipeline()
                    },
                    onApproveQrPairing = { info ->
                        stopQrAnalyzer()
                        saveRememberedHost(info.hostName, info.ip, info.controlPort, info.mediaUdpPort)
                        uiState.update {
                            it.copy(
                                isQrScannerOpen = false,
                                pendingConsent = null,
                                isPaired = true,
                                pairedHostName = info.hostName,
                                pairedHostIp = info.ip,
                            )
                        }
                        transportManager.connectFromQrPairing(info)
                        if (!uiState.value.isStreaming) {
                            toggleStreaming()
                        }
                    },
                    onRejectQrPairing = {
                        uiState.update { it.copy(pendingConsent = null) }
                    },
                    onManualIpConnect = { ip ->
                        stopQrAnalyzer()
                        saveRememberedHost("LAN PC", ip, Brand.DEFAULT_CONTROL_PORT, Brand.DEFAULT_MEDIA_UDP_PORT)
                        uiState.update {
                            it.copy(
                                isQrScannerOpen = false,
                                pendingConsent = null,
                                isPaired = true,
                                pairedHostName = "LAN PC",
                                pairedHostIp = ip,
                            )
                        }
                        transportManager.connectWifi(ip)
                        if (!uiState.value.isStreaming) {
                            toggleStreaming()
                        }
                    },
                    onConnectDiscoveredHost = { host ->
                        saveRememberedHost(host.name, host.ip, host.controlPort, host.mediaUdpPort)
                        uiState.update {
                            it.copy(
                                isPaired = true,
                                pairedHostName = host.name,
                                pairedHostIp = host.ip,
                            )
                        }
                        transportManager.connectWifi(host.ip, host.controlPort, host.mediaUdpPort)
                        if (!uiState.value.isStreaming) {
                            toggleStreaming()
                        }
                    },
                    onToggleOledCurtain = {
                        uiState.update { it.copy(isOledCurtainActive = !it.isOledCurtainActive) }
                    },
                    onRequestPermissions = ::requestRequiredPermissions,
                )
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleUsbAccessoryIntent(intent)
    }

    override fun onDestroy() {
        stopStreaming()
        stopQrAnalyzer()
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
        if (uiState.value.isStreaming) {
            stopStreaming()
        } else {
            startStreaming()
        }
    }

    private fun restartStreamIfActive() {
        if (uiState.value.isStreaming) {
            stopStreaming()
            startStreaming()
        }
    }

    private fun startStreaming() {
        if (!hasRequiredPermissions()) {
            requestRequiredPermissions()
            return
        }
        ContextCompat.startForegroundService(this, Intent(this, StreamForegroundService::class.java))

        val st = uiState.value
        if (transportManager.tier.value == ActiveTransportTier.DISCONNECTED) {
            if (st.isPaired && st.pairedHostIp.isNotBlank() && st.pairedHostIp != "USB AOA") {
                transportManager.connectWifi(st.pairedHostIp, rememberedControlPort, rememberedUdpPort)
            } else {
                transportManager.connectUsbAdb()
            }
        }
        val bitrateKbps = (st.resolution.defaultBitrateKbps * st.quality.multiplier).toInt().coerceAtLeast(500)
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

        uiState.update { it.copy(isStreaming = true) }
        restartCameraPipeline()
    }

    private fun stopStreaming() {
        videoEncoder?.stop()
        videoEncoder = null
        audioEngine?.stop()
        audioEngine?.release()
        audioEngine = null
        uiState.update { it.copy(isStreaming = false, rmsDbfsX10 = -960) }
        startService(Intent(this, StreamForegroundService::class.java).apply {
            action = StreamForegroundService.ACTION_STOP
        })
        restartCameraPipeline()
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
        runCatching { qrImageReader?.close() }
        qrImageReader = null
        qrThread?.quitSafely()
        qrThread = null
    }

    private fun restartCameraPipeline() {
        if (!hasRequiredPermissions()) return
        val st = uiState.value
        val encSurface = if (st.isStreaming) runCatching { videoEncoder?.start() }.getOrNull() else null
        cameraController.startCamera(
            cameraId = st.activeLensId,
            targetFps = st.fps,
            antibandingHz = st.antibandingHz,
            previewSurface = previewSurface,
            encoderSurface = encSurface,
            analysisSurface = qrImageReader?.surface,
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
