package com.wificam.android

import android.Manifest
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
import com.wificam.android.protocol.PairingInfo
import com.wificam.android.qr.QrPairingScanner
import com.wificam.android.service.StreamForegroundService
import com.wificam.android.transport.AndroidTransportManager
import com.wificam.android.ui.StudioCockpitScreen
import com.wificam.android.ui.StudioUiState
import com.wificam.android.ui.theme.WifiCamStudioTheme
import com.wificam.android.video.AndroidVideoCodec
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

    private val uiState = MutableStateFlow(StudioUiState())

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
                uiState.update { it.copy(tier = tier) }
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
                    onCycleResolution = ::cycleResolution,
                    onCycleCodec = ::cycleCodec,
                    onOpenQrScanner = {
                        uiState.update { it.copy(isQrScannerOpen = true, pendingConsent = null) }
                        startQrAnalyzer()
                    },
                    onCloseQrScanner = {
                        stopQrAnalyzer()
                        uiState.update { it.copy(isQrScannerOpen = false, pendingConsent = null) }
                        restartCameraPipeline()
                    },
                    onApproveQrPairing = { info ->
                        stopQrAnalyzer()
                        uiState.update { it.copy(isQrScannerOpen = false, pendingConsent = null) }
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
                        uiState.update { it.copy(isQrScannerOpen = false, pendingConsent = null) }
                        transportManager.connectWifi(ip)
                        if (!uiState.value.isStreaming) {
                            toggleStreaming()
                        }
                    },
                    onConnectDiscoveredHost = { host ->
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

    private fun startStreaming() {
        if (!hasRequiredPermissions()) {
            requestRequiredPermissions()
            return
        }
        ContextCompat.startForegroundService(this, Intent(this, StreamForegroundService::class.java))

        if (transportManager.tier.value == com.wificam.android.transport.ActiveTransportTier.DISCONNECTED) {
            // Try USB adb reverse automatically when starting without explicit Wi-Fi pairing.
            transportManager.connectUsbAdb()
        }

        val st = uiState.value
        val (w, h) = when (st.resolutionLabel) {
            "4K" -> 3840 to 2160
            "720p" -> 1280 to 720
            else -> 1920 to 1080
        }
        val enc = HardwareVideoEncoder(
            config = VideoEncoderConfig(
                width = w,
                height = h,
                fps = st.fps,
                bitrateKbps = if (w >= 3840) 24_000 else 14_000,
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

    private fun cycleResolution() {
        val next = when (uiState.value.resolutionLabel) {
            "720p" -> "1080p" to 60
            "1080p" -> "4K" to 30
            else -> "720p" to 60
        }
        uiState.update { it.copy(resolutionLabel = next.first, fps = next.second) }
        if (uiState.value.isStreaming) {
            stopStreaming()
            startStreaming()
        }
    }

    private fun cycleCodec() {
        val all = AndroidVideoCodec.entries
        val next = all[(all.indexOf(uiState.value.codec) + 1) % all.size]
        uiState.update { it.copy(codec = next) }
        if (uiState.value.isStreaming) {
            stopStreaming()
            startStreaming()
        }
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
