package com.wificam.android.ui

import android.graphics.Matrix
import android.graphics.SurfaceTexture
import android.view.Surface
import android.view.TextureView
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.DarkMode
import androidx.compose.material.icons.rounded.Mic
import androidx.compose.material.icons.rounded.MicOff
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material.icons.rounded.Tune
import androidx.compose.material.icons.rounded.Videocam
import androidx.compose.material.icons.rounded.VideocamOff
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.wificam.android.protocol.PairingInfo
import com.wificam.android.transport.ActiveTransportTier
import com.wificam.android.transport.DiscoveredHost
import com.wificam.android.ui.theme.StudioTelemetryStyle
import com.wificam.android.ui.theme.StudioTokens
import com.wificam.android.video.AndroidVideoCodec
import com.wificam.android.video.CameraLensOption
import kotlin.math.roundToInt

data class StudioUiState(
    val isStreaming: Boolean = false,
    val tier: ActiveTransportTier = ActiveTransportTier.DISCONNECTED,
    val isPaired: Boolean = false,
    val pairedHostName: String = "",
    val pairedHostIp: String = "",
    val lenses: List<CameraLensOption> = listOf(CameraLensOption("0", "1x", false, 4.2f)),
    val activeLensId: String = "0",
    val resolution: VideoResolutionPreset = VideoResolutionPreset.R1080P,
    val quality: VideoQualityPreset = VideoQualityPreset.BALANCED,
    val fps: Int = 60,
    val codec: AndroidVideoCodec = AndroidVideoCodec.H264,
    val isVideoPaused: Boolean = false,
    val isMicMuted: Boolean = false,
    val rmsDbfsX10: Int = -960,
    val isOledCurtainActive: Boolean = false,
    val isQrScannerOpen: Boolean = false,
    val isSettingsOpen: Boolean = false,
    val pendingConsent: PairingInfo? = null,
    val discoveredHosts: List<DiscoveredHost> = emptyList(),
    val antibandingHz: Int = 50,
    val previewRotationDeg: Int = 0,
    val previewFlipH: Boolean = false,
    val previewFlipV: Boolean = false,
    val previewCropMode: PreviewCropMode = PreviewCropMode.FIT,
    val hasPermissions: Boolean = false,
)

enum class PreviewCropMode(val label: String, val zoom: Float) {
    FIT("Fit", 1.0f),
    FILL("Fill", 1.0f),
    ZOOM_1_5("1.5x", 1.5f),
}

private fun applyPreviewTransform(
    textureView: TextureView,
    viewW: Int,
    viewH: Int,
    bufferW: Int,
    bufferH: Int,
    rotationDeg: Int,
    flipH: Boolean,
    flipV: Boolean,
    cropMode: PreviewCropMode,
) {
    if (viewW <= 0 || viewH <= 0 || bufferW <= 0 || bufferH <= 0) return

    val normRot = ((rotationDeg % 360) + 360) % 360
    val isPortraitView = viewH >= viewW
    var contentW = if (isPortraitView) bufferH.toFloat() else bufferW.toFloat()
    var contentH = if (isPortraitView) bufferW.toFloat() else bufferH.toFloat()
    if (normRot == 90 || normRot == 270) {
        val tmp = contentW
        contentW = contentH
        contentH = tmp
    }

    val contentAspect = contentW / contentH
    val viewAspect = viewW.toFloat() / viewH.toFloat()

    val baseScaleX: Float
    val baseScaleY: Float
    if (cropMode == PreviewCropMode.FIT) {
        // Letterbox/pillarbox cleanly inside the framed preview stage with ZERO distortion.
        if (contentAspect > viewAspect) {
            baseScaleX = 1f
            baseScaleY = viewAspect / contentAspect
        } else {
            baseScaleX = contentAspect / viewAspect
            baseScaleY = 1f
        }
    } else {
        // Center-crop fill + optional 1.5x zoom with ZERO distortion.
        if (contentAspect > viewAspect) {
            baseScaleX = (contentAspect / viewAspect) * cropMode.zoom
            baseScaleY = cropMode.zoom
        } else {
            baseScaleX = cropMode.zoom
            baseScaleY = (viewAspect / contentAspect) * cropMode.zoom
        }
    }

    val sx = if (flipH) -baseScaleX else baseScaleX
    val sy = if (flipV) -baseScaleY else baseScaleY

    val matrix = Matrix()
    val cx = viewW / 2f
    val cy = viewH / 2f
    matrix.setScale(sx, sy, cx, cy)
    if (normRot != 0) {
        matrix.postRotate(normRot.toFloat(), cx, cy)
    }
    textureView.setTransform(matrix)
}

@Composable
fun StudioCockpitScreen(
    state: StudioUiState,
    onPreviewSurfaceReady: (Surface) -> Unit,
    onPreviewSurfaceDestroyed: () -> Unit,
    onToggleStream: () -> Unit,
    onToggleVideoPause: () -> Unit,
    onToggleMicMute: () -> Unit,
    onSelectLens: (String) -> Unit,
    onSelectResolution: (VideoResolutionPreset) -> Unit,
    onSelectFps: (Int) -> Unit,
    onSelectQuality: (VideoQualityPreset) -> Unit,
    onSelectCodec: (AndroidVideoCodec) -> Unit,
    onSelectAntibanding: (Int) -> Unit,
    onRotatePreview: () -> Unit,
    onToggleFlipH: () -> Unit,
    onToggleFlipV: () -> Unit,
    onCycleCropMode: () -> Unit,
    onOpenSettings: () -> Unit,
    onCloseSettings: () -> Unit,
    onUnpair: () -> Unit,
    onQuickReconnect: () -> Unit,
    onOpenQrScanner: () -> Unit,
    onCloseQrScanner: () -> Unit,
    onApproveQrPairing: (PairingInfo) -> Unit,
    onRejectQrPairing: () -> Unit,
    onManualIpConnect: (String) -> Unit,
    onConnectDiscoveredHost: (DiscoveredHost) -> Unit,
    onToggleOledCurtain: () -> Unit,
    onRequestPermissions: () -> Unit,
) {
    val previewBufferW = state.resolution.width.coerceAtMost(1920)
    val previewBufferH = state.resolution.height.coerceAtMost(1080)

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(StudioTokens.SurfaceBase),
    ) {
        // Clean 3-Zone Studio Layout: Top Header | Framed Preview Stage | Bottom Control Deck
        // (Zero HUD overlapping the camera frame; switches to full camera only while QR Scanner is open)
        Column(
            modifier = Modifier
                .fillMaxSize()
                .statusBarsPadding()
                .navigationBarsPadding(),
        ) {
            // 1. Top Studio Header Bar (unobstructed)
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .background(StudioTokens.SurfaceBase)
                    .padding(horizontal = 16.dp, vertical = 12.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    PairingStatusPill(
                        isPaired = state.isPaired,
                        isStreaming = state.isStreaming,
                        tier = state.tier,
                        hostName = state.pairedHostName,
                        onClick = if (state.isPaired) onOpenSettings else onOpenQrScanner,
                        modifier = Modifier.weight(1f, fill = false),
                    )

                    Spacer(modifier = Modifier.width(12.dp))

                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        TopActionIcon(
                            icon = Icons.Rounded.QrCodeScanner,
                            contentDescription = "Connect to PC via QR",
                            tint = StudioTokens.AccentPrimary,
                            onClick = onOpenQrScanner,
                        )
                        TopActionIcon(
                            icon = Icons.Rounded.DarkMode,
                            contentDescription = "OLED Curtain Mode",
                            tint = StudioTokens.InkPrimary,
                            onClick = onToggleOledCurtain,
                        )
                    }
                }

                // Compact Quality & Codec Pill -> opens full Video Settings Sheet
                Row(
                    modifier = Modifier
                        .clip(RoundedCornerShape(StudioTokens.RadiusMd))
                        .background(StudioTokens.SurfaceCard)
                        .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(StudioTokens.RadiusMd))
                        .clickable(onClick = onOpenSettings)
                        .padding(horizontal = 12.dp, vertical = 7.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Icon(
                        imageVector = Icons.Rounded.Tune,
                        contentDescription = null,
                        tint = StudioTokens.AccentPrimary,
                        modifier = Modifier.size(16.dp),
                    )
                    Text(
                        text = "${state.resolution.label} · ${state.fps}fps · ${state.quality.label} · ${state.codec.label}",
                        style = StudioTelemetryStyle.copy(fontSize = 12.sp),
                        color = StudioTokens.InkPrimary,
                    )
                }

                // 1-Tap Fast Reconnect banner for last remembered PC
                if (state.isPaired && !state.isStreaming && state.pairedHostIp.isNotBlank()) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(StudioTokens.RadiusMd))
                            .background(StudioTokens.AccentPrimary.copy(alpha = 0.14f))
                            .border(1.dp, StudioTokens.AccentPrimary.copy(alpha = 0.65f), RoundedCornerShape(StudioTokens.RadiusMd))
                            .clickable(onClick = onQuickReconnect)
                            .padding(horizontal = 14.dp, vertical = 10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.SpaceBetween,
                    ) {
                        Text(
                            text = "Reconnect · ${state.pairedHostName}",
                            style = MaterialTheme.typography.labelMedium,
                            color = StudioTokens.InkPrimary,
                        )
                        Text(
                            text = "${state.pairedHostIp} · Tap",
                            style = StudioTelemetryStyle.copy(fontSize = 11.sp),
                            color = StudioTokens.AccentPrimary,
                        )
                    }
                }

                val firstHost = state.discoveredHosts.firstOrNull()
                if (firstHost != null && !state.isStreaming && !state.isPaired) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(StudioTokens.RadiusMd))
                            .background(StudioTokens.SurfaceElevated)
                            .border(1.dp, StudioTokens.AccentPrimary.copy(alpha = 0.55f), RoundedCornerShape(StudioTokens.RadiusMd))
                            .clickable { onConnectDiscoveredHost(firstHost) }
                            .padding(horizontal = 14.dp, vertical = 9.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.SpaceBetween,
                    ) {
                        Text(
                            text = "PC Found: ${firstHost.name}",
                            style = MaterialTheme.typography.labelMedium,
                            color = StudioTokens.InkPrimary,
                        )
                        Text(
                            text = "${firstHost.ip} · Tap to connect",
                            style = StudioTelemetryStyle.copy(fontSize = 11.sp),
                            color = StudioTokens.AccentPrimary,
                        )
                    }
                }
            }

            // 2. Framed Center Camera Preview Stage (not hidden behind HUD!)
            Box(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp)
                    .clip(RoundedCornerShape(18.dp))
                    .background(StudioTokens.OledBlack)
                    .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(18.dp)),
                contentAlignment = Alignment.Center,
            ) {
                if (state.hasPermissions) {
                    AndroidView(
                        modifier = Modifier.fillMaxSize(),
                        factory = { ctx ->
                            TextureView(ctx).apply {
                                surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                                    override fun onSurfaceTextureAvailable(st: SurfaceTexture, w: Int, h: Int) {
                                        st.setDefaultBufferSize(previewBufferW, previewBufferH)
                                        applyPreviewTransform(
                                            this@apply,
                                            w,
                                            h,
                                            previewBufferW,
                                            previewBufferH,
                                            if (state.isQrScannerOpen) 0 else state.previewRotationDeg,
                                            if (state.isQrScannerOpen) false else state.previewFlipH,
                                            if (state.isQrScannerOpen) false else state.previewFlipV,
                                            if (state.isQrScannerOpen) PreviewCropMode.FILL else state.previewCropMode,
                                        )
                                        onPreviewSurfaceReady(Surface(st))
                                    }

                                    override fun onSurfaceTextureSizeChanged(st: SurfaceTexture, w: Int, h: Int) {
                                        st.setDefaultBufferSize(previewBufferW, previewBufferH)
                                        applyPreviewTransform(
                                            this@apply,
                                            w,
                                            h,
                                            previewBufferW,
                                            previewBufferH,
                                            if (state.isQrScannerOpen) 0 else state.previewRotationDeg,
                                            if (state.isQrScannerOpen) false else state.previewFlipH,
                                            if (state.isQrScannerOpen) false else state.previewFlipV,
                                            if (state.isQrScannerOpen) PreviewCropMode.FILL else state.previewCropMode,
                                        )
                                    }

                                    override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
                                        onPreviewSurfaceDestroyed()
                                        return true
                                    }

                                    override fun onSurfaceTextureUpdated(st: SurfaceTexture) {}
                                }
                            }
                        },
                        update = { tv ->
                            if (tv.isAvailable && tv.width > 0 && tv.height > 0) {
                                tv.surfaceTexture?.setDefaultBufferSize(previewBufferW, previewBufferH)
                                applyPreviewTransform(
                                    tv,
                                    tv.width,
                                    tv.height,
                                    previewBufferW,
                                    previewBufferH,
                                    if (state.isQrScannerOpen) 0 else state.previewRotationDeg,
                                    if (state.isQrScannerOpen) false else state.previewFlipH,
                                    if (state.isQrScannerOpen) false else state.previewFlipV,
                                    if (state.isQrScannerOpen) PreviewCropMode.FILL else state.previewCropMode,
                                )
                            }
                        },
                    )
                } else {
                    PermissionGate(onRequestPermissions = onRequestPermissions)
                }
            }

            // 2b. Compact Rotate / Flip H / Flip V / Crop Preview Toolbar
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp, vertical = 10.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                TransformChip(
                    label = "Rot ${state.previewRotationDeg}°",
                    active = state.previewRotationDeg != 0,
                    onClick = onRotatePreview,
                    modifier = Modifier.weight(1f),
                )
                TransformChip(
                    label = "Mirror H",
                    active = state.previewFlipH,
                    onClick = onToggleFlipH,
                    modifier = Modifier.weight(1f),
                )
                TransformChip(
                    label = "Flip V",
                    active = state.previewFlipV,
                    onClick = onToggleFlipV,
                    modifier = Modifier.weight(1f),
                )
                TransformChip(
                    label = state.previewCropMode.label,
                    active = state.previewCropMode != PreviewCropMode.FIT,
                    onClick = onCycleCropMode,
                    modifier = Modifier.weight(1f),
                )
            }

            // 3. Bottom Studio Control Deck
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(start = 16.dp, end = 16.dp, bottom = 14.dp)
                    .background(StudioTokens.SurfaceElevated, RoundedCornerShape(StudioTokens.RadiusLg))
                    .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(StudioTokens.RadiusLg))
                    .padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                BroadcastVuMeter16(
                    rmsDbfsX10 = if (state.isMicMuted) -960 else state.rmsDbfsX10,
                    isMuted = state.isMicMuted,
                )

                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterHorizontally),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    state.lenses.forEach { lens ->
                        val selected = lens.cameraId == state.activeLensId
                        Box(
                            modifier = Modifier
                                .clip(CircleShape)
                                .background(if (selected) StudioTokens.AccentPrimary else StudioTokens.SurfaceCard)
                                .border(
                                    1.dp,
                                    if (selected) StudioTokens.AccentPrimary else StudioTokens.BorderSubtle,
                                    CircleShape,
                                )
                                .clickable { onSelectLens(lens.cameraId) }
                                .padding(horizontal = 16.dp, vertical = 8.dp),
                        ) {
                            Text(
                                text = lens.label,
                                style = StudioTelemetryStyle.copy(fontSize = 12.sp),
                                color = if (selected) StudioTokens.InkOnAccent else StudioTokens.InkPrimary,
                            )
                        }
                    }
                }

                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    DockToggleIcon(
                        active = !state.isVideoPaused,
                        activeIcon = Icons.Rounded.Videocam,
                        inactiveIcon = Icons.Rounded.VideocamOff,
                        inactiveTint = StudioTokens.StatusWarn,
                        contentDescription = "Pause video",
                        onClick = onToggleVideoPause,
                    )

                    Button(
                        onClick = onToggleStream,
                        modifier = Modifier
                            .weight(1f)
                            .height(52.dp),
                        shape = RoundedCornerShape(StudioTokens.RadiusMd),
                        colors = ButtonDefaults.buttonColors(
                            containerColor = if (state.isStreaming) StudioTokens.StatusAlert else StudioTokens.AccentPrimary,
                            contentColor = if (state.isStreaming) StudioTokens.InkPrimary else StudioTokens.InkOnAccent,
                        ),
                    ) {
                        Text(
                            text = if (state.isStreaming) "STOP" else "GO LIVE",
                            style = MaterialTheme.typography.labelMedium.copy(fontSize = 14.sp),
                        )
                    }

                    DockToggleIcon(
                        active = !state.isMicMuted,
                        activeIcon = Icons.Rounded.Mic,
                        inactiveIcon = Icons.Rounded.MicOff,
                        inactiveTint = StudioTokens.StatusAlert,
                        contentDescription = "Mute microphone",
                        onClick = onToggleMicMute,
                    )

                    IconButton(
                        onClick = onOpenSettings,
                        modifier = Modifier
                            .size(52.dp)
                            .clip(RoundedCornerShape(StudioTokens.RadiusMd))
                            .background(StudioTokens.SurfaceCard)
                            .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(StudioTokens.RadiusMd)),
                    ) {
                        Icon(
                            imageVector = Icons.Rounded.Tune,
                            contentDescription = "Video & Quality Settings",
                            tint = StudioTokens.InkPrimary,
                        )
                    }
                }
            }
        }

        if (state.isSettingsOpen) {
            SettingsSheet(
                state = state,
                onSelectResolution = onSelectResolution,
                onSelectFps = onSelectFps,
                onSelectQuality = onSelectQuality,
                onSelectCodec = onSelectCodec,
                onSelectAntibanding = onSelectAntibanding,
                onOpenQrScanner = onOpenQrScanner,
                onUnpair = onUnpair,
                onDismiss = onCloseSettings,
            )
        }

        if (state.isQrScannerOpen) {
            QrScannerOverlay(
                pendingConsent = state.pendingConsent,
                onApprovePairing = onApproveQrPairing,
                onRejectPairing = onRejectQrPairing,
                onManualIpConnect = onManualIpConnect,
                onClose = onCloseQrScanner,
            )
        }

        if (state.isOledCurtainActive) {
            OledCurtainOverlay(
                tierBadge = state.tier.badge,
                isMuted = state.isMicMuted,
                onWake = onToggleOledCurtain,
            )
        }
    }
}

@Composable
private fun TransformChip(
    label: String,
    active: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(
        modifier = modifier
            .clip(RoundedCornerShape(StudioTokens.RadiusSm))
            .background(if (active) StudioTokens.AccentPrimary.copy(alpha = 0.2f) else StudioTokens.SurfaceCard)
            .border(
                1.dp,
                if (active) StudioTokens.AccentPrimary else StudioTokens.BorderSubtle,
                RoundedCornerShape(StudioTokens.RadiusSm),
            )
            .clickable(onClick = onClick)
            .padding(vertical = 8.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = label,
            style = StudioTelemetryStyle.copy(fontSize = 11.sp),
            color = if (active) StudioTokens.AccentPrimary else StudioTokens.InkPrimary,
            maxLines = 1,
        )
    }
}

@Composable
private fun PairingStatusPill(
    isPaired: Boolean,
    isStreaming: Boolean,
    tier: ActiveTransportTier,
    hostName: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val dotColor = when {
        isStreaming && tier != ActiveTransportTier.DISCONNECTED -> StudioTokens.StatusLive
        isPaired -> StudioTokens.AccentPrimary
        else -> StudioTokens.StatusWarn
    }
    val label = when {
        isStreaming && tier != ActiveTransportTier.DISCONNECTED -> {
            if (hostName.isNotEmpty()) "LIVE · ${tier.badge} · $hostName" else "LIVE · ${tier.badge}"
        }
        isPaired -> if (hostName.isNotEmpty()) "PAIRED · $hostName" else "PAIRED"
        else -> "NOT PAIRED · Tap QR"
    }
    val textColor = when {
        isStreaming && tier != ActiveTransportTier.DISCONNECTED -> StudioTokens.StatusLive
        isPaired -> StudioTokens.AccentPrimary
        else -> StudioTokens.InkPrimary
    }

    Row(
        modifier = modifier
            .clip(CircleShape)
            .background(StudioTokens.SurfaceCard.copy(alpha = 0.92f))
            .border(1.dp, dotColor.copy(alpha = 0.55f), CircleShape)
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            modifier = Modifier
                .size(9.dp)
                .clip(CircleShape)
                .background(dotColor),
        )
        Spacer(modifier = Modifier.width(8.dp))
        Text(
            text = label,
            style = StudioTelemetryStyle.copy(fontSize = 12.sp),
            color = textColor,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun TopActionIcon(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    contentDescription: String,
    tint: androidx.compose.ui.graphics.Color,
    onClick: () -> Unit,
) {
    IconButton(
        onClick = onClick,
        modifier = Modifier
            .size(44.dp)
            .clip(RoundedCornerShape(StudioTokens.RadiusMd))
            .background(StudioTokens.SurfaceCard.copy(alpha = 0.92f))
            .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(StudioTokens.RadiusMd)),
    ) {
        Icon(imageVector = icon, contentDescription = contentDescription, tint = tint, modifier = Modifier.size(22.dp))
    }
}

@Composable
private fun DockToggleIcon(
    active: Boolean,
    activeIcon: androidx.compose.ui.graphics.vector.ImageVector,
    inactiveIcon: androidx.compose.ui.graphics.vector.ImageVector,
    inactiveTint: androidx.compose.ui.graphics.Color,
    contentDescription: String,
    onClick: () -> Unit,
) {
    IconButton(
        onClick = onClick,
        modifier = Modifier
            .size(52.dp)
            .clip(RoundedCornerShape(StudioTokens.RadiusMd))
            .background(if (active) StudioTokens.SurfaceCard else inactiveTint.copy(alpha = 0.18f))
            .border(
                1.dp,
                if (active) StudioTokens.BorderSubtle else inactiveTint,
                RoundedCornerShape(StudioTokens.RadiusMd),
            ),
    ) {
        Icon(
            imageVector = if (active) activeIcon else inactiveIcon,
            contentDescription = contentDescription,
            tint = if (active) StudioTokens.InkPrimary else inactiveTint,
        )
    }
}

@Composable
private fun BroadcastVuMeter16(rmsDbfsX10: Int, isMuted: Boolean) {
    val db = rmsDbfsX10 / 10f
    val litSegments = (((db + 60f) / 60f).coerceIn(0f, 1f) * 16f).roundToInt()

    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(
            text = if (isMuted) "MUTE" else "MIC",
            style = StudioTelemetryStyle.copy(fontSize = 11.sp),
            color = if (isMuted) StudioTokens.StatusAlert else StudioTokens.InkSecondary,
        )
        Row(
            modifier = Modifier.weight(1f),
            horizontalArrangement = Arrangement.spacedBy(3.dp),
        ) {
            for (seg in 1..16) {
                val segColor = when {
                    seg > litSegments -> StudioTokens.SurfaceCard
                    seg <= 10 -> StudioTokens.VuNominal
                    seg <= 14 -> StudioTokens.VuHot
                    else -> StudioTokens.VuClip
                }
                Box(
                    modifier = Modifier
                        .weight(1f)
                        .height(8.dp)
                        .clip(RoundedCornerShape(2.dp))
                        .background(segColor),
                )
            }
        }
        Text(
            text = if (isMuted || rmsDbfsX10 <= -900) "-∞ dB" else "${db.roundToInt()} dB",
            style = StudioTelemetryStyle.copy(fontSize = 11.sp),
            color = StudioTokens.InkSecondary,
        )
    }
}

@Composable
private fun PermissionGate(onRequestPermissions: () -> Unit) {
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(StudioTokens.SurfaceBase)
            .padding(24.dp),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            modifier = Modifier
                .background(StudioTokens.SurfaceElevated, RoundedCornerShape(StudioTokens.RadiusLg))
                .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(StudioTokens.RadiusLg))
                .padding(20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Camera & Studio Mic Access",
                style = MaterialTheme.typography.titleMedium,
                color = StudioTokens.InkPrimary,
            )
            Text(
                text = "WifiCam streams directly to your local PC over USB, Wi-Fi, or Bluetooth with zero cloud and zero telemetry.",
                style = MaterialTheme.typography.bodyMedium,
                color = StudioTokens.InkSecondary,
            )
            Button(
                onClick = onRequestPermissions,
                shape = RoundedCornerShape(StudioTokens.RadiusMd),
                colors = ButtonDefaults.buttonColors(
                    containerColor = StudioTokens.AccentPrimary,
                    contentColor = StudioTokens.InkOnAccent,
                ),
            ) {
                Text("Grant Permissions")
            }
        }
    }
}
