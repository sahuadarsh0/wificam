package com.wificam.android.video

import android.annotation.SuppressLint
import android.content.Context
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.os.Handler
import android.os.HandlerThread
import android.util.Range
import android.view.Surface

data class CameraLensOption(
    val cameraId: String,
    val label: String,
    val isFront: Boolean,
    val focalLengthMm: Float,
)

/**
 * Pro Camera2 controller with dynamic multi-lens discovery (`0.5x / 1x / Tele / Front`),
 * unlocked `[60, 60]` AE FPS ranges, and 50/60 Hz mains antibanding.
 */
class Camera2Controller(context: Context) {
    private val cameraManager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
    private var bgThread: HandlerThread? = null
    private var bgHandler: Handler? = null
    private var cameraDevice: CameraDevice? = null
    private var captureSession: CameraCaptureSession? = null

    fun discoverLenses(): List<CameraLensOption> {
        val result = mutableListOf<CameraLensOption>()
        val ids = runCatching { cameraManager.cameraIdList }.getOrDefault(emptyArray())
        for (id in ids) {
            val chars = runCatching { cameraManager.getCameraCharacteristics(id) }.getOrNull() ?: continue
            val facing = chars.get(CameraCharacteristics.LENS_FACING) ?: continue
            val focals = chars.get(CameraCharacteristics.LENS_INFO_AVAILABLE_FOCAL_LENGTHS)
            val focal = focals?.firstOrNull() ?: 4.0f
            val isFront = facing == CameraCharacteristics.LENS_FACING_FRONT
            val label = when {
                isFront -> "Front"
                focal < 2.6f -> "0.5x"
                focal > 6.5f -> "Tele"
                else -> "1x"
            }
            if (result.none { it.label == label }) {
                result.add(CameraLensOption(id, label, isFront, focal))
            }
        }
        return result.ifEmpty {
            listOf(CameraLensOption("0", "1x", false, 4.2f))
        }
    }

    @SuppressLint("MissingPermission")
    fun startCamera(
        cameraId: String,
        targetFps: Int,
        antibandingHz: Int,
        previewSurface: Surface?,
        encoderSurface: Surface?,
        analysisSurface: Surface? = null,
        onError: (String) -> Unit = {},
    ) {
        stopCamera()
        val thread = HandlerThread("wificam-camera2").apply { start() }
        val handler = Handler(thread.looper)
        bgThread = thread
        bgHandler = handler

        val outputs = listOfNotNull(previewSurface, encoderSurface, analysisSurface)
        if (outputs.isEmpty()) return

        cameraManager.openCamera(
            cameraId,
            object : CameraDevice.StateCallback() {
                override fun onOpened(device: CameraDevice) {
                    cameraDevice = device
                    createSession(device, outputs, targetFps, antibandingHz, handler, onError)
                }

                override fun onDisconnected(device: CameraDevice) {
                    device.close()
                    cameraDevice = null
                }

                override fun onError(device: CameraDevice, error: Int) {
                    device.close()
                    cameraDevice = null
                    onError("Camera2 error code $error")
                }
            },
            handler,
        )
    }

    @Suppress("DEPRECATION")
    private fun createSession(
        device: CameraDevice,
        outputs: List<Surface>,
        targetFps: Int,
        antibandingHz: Int,
        handler: Handler,
        onError: (String) -> Unit,
    ) {
        val fpsRange = runCatching {
            val chars = cameraManager.getCameraCharacteristics(device.id)
            val ranges = chars.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES).orEmpty()
            ranges.firstOrNull { it.lower == targetFps && it.upper == targetFps }
                ?: ranges.firstOrNull { it.upper == targetFps && it.lower >= 15 }
                ?: ranges.firstOrNull { it.upper == targetFps }
                ?: Range(15, targetFps)
        }.getOrElse { Range(15, targetFps) }

        val reqBuilder = device.createCaptureRequest(CameraDevice.TEMPLATE_RECORD).apply {
            outputs.forEach { addTarget(it) }
            set(CaptureRequest.CONTROL_MODE, CaptureRequest.CONTROL_MODE_AUTO)
            set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO)
            set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, fpsRange)
            set(CaptureRequest.EDGE_MODE, CaptureRequest.EDGE_MODE_HIGH_QUALITY)
            set(CaptureRequest.NOISE_REDUCTION_MODE, CaptureRequest.NOISE_REDUCTION_MODE_HIGH_QUALITY)
            val abMode = when (antibandingHz) {
                50 -> CaptureRequest.CONTROL_AE_ANTIBANDING_MODE_50HZ
                60 -> CaptureRequest.CONTROL_AE_ANTIBANDING_MODE_60HZ
                else -> CaptureRequest.CONTROL_AE_ANTIBANDING_MODE_AUTO
            }
            set(CaptureRequest.CONTROL_AE_ANTIBANDING_MODE, abMode)
        }

        device.createCaptureSession(
            outputs,
            object : CameraCaptureSession.StateCallback() {
                override fun onConfigured(session: CameraCaptureSession) {
                    captureSession = session
                    runCatching {
                        session.setRepeatingRequest(reqBuilder.build(), null, handler)
                    }.onFailure { onError(it.message ?: "Capture start failed") }
                }

                override fun onConfigureFailed(session: CameraCaptureSession) {
                    onError("Camera2 session configuration failed")
                }
            },
            handler,
        )
    }

    fun stopCamera() {
        runCatching { captureSession?.close() }
        runCatching { cameraDevice?.close() }
        captureSession = null
        cameraDevice = null
        bgThread?.quitSafely()
        bgThread = null
        bgHandler = null
    }
}
