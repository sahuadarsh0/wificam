package com.wificam.android.ui

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
import androidx.compose.material.icons.rounded.Settings
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
    val lenses: List<CameraLensOption> = listOf(CameraLensOption("0", "1x", false, 4.2f)),
    val activeLensId: String = "0",
    val resolutionLabel: String = "1080p",
    val fps: Int = 60,
    val codec: AndroidVideoCodec = AndroidVideoCodec.H264,
    val isVideoPaused: Boolean = false,
    val isMicMuted: Boolean = false,
    val rmsDbfsX10: Int = -960,
    val isOledCurtainActive: Boolean = false,
    val isQrScannerOpen: Boolean = false,
    val pendingConsent: PairingInfo? = null,
    val discoveredHosts: List<DiscoveredHost> = emptyList(),
    val antibandingHz: Int = 50,
    val hasPermissions: Boolean = false,
)

/**
 * Hardware-instrument camera-first cockpit implementing the WCAG-validated
 * `DESIGN.md` visual system (OLED black canvas, Studio Cyan accent, tabular
 * monospace telemetry, 16-segment LED VU meter, thumb-reachable dock).
 */
@Composable
fun StudioCockpitScreen(
    state: StudioUiState,
    onPreviewSurfaceReady: (Surface) -> Unit,
    onPreviewSurfaceDestroyed: () -> Unit,
    onToggleStream: () -> Unit,
    onToggleVideoPause: () -> Unit,
    onToggleMicMute: () -> Unit,
    onSelectLens: (String) -> Unit,
    onCycleResolution: () -> Unit,
    onCycleCodec: () -> Unit,
    onOpenQrScanner: () -> Unit,
    onCloseQrScanner: () -> Unit,
    onApproveQrPairing: (PairingInfo) -> Unit,
    onRejectQrPairing: () -> Unit,
    onManualIpConnect: (String) -> Unit,
    onConnectDiscoveredHost: (DiscoveredHost) -> Unit,
    onToggleOledCurtain: () -> Unit,
    onRequestPermissions: () -> Unit,
) {
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(StudioTokens.OledBlack),
    ) {
        // 1. Edge-to-edge Camera2 Preview Surface
        if (state.hasPermissions) {
            AndroidView(
                modifier = Modifier.fillMaxSize(),
                factory = { ctx ->
                    TextureView(ctx).apply {
                        surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                            override fun onSurfaceTextureAvailable(st: SurfaceTexture, w: Int, h: Int) {
                                onPreviewSurfaceReady(Surface(st))
                            }

                            override fun onSurfaceTextureSizeChanged(st: SurfaceTexture, w: Int, h: Int) {}

                            override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
                                onPreviewSurfaceDestroyed()
                                return true
                            }

                            override fun onSurfaceTextureUpdated(st: SurfaceTexture) {}
                        }
                    }
                },
            )
        } else {
            PermissionGate(onRequestPermissions = onRequestPermissions)
        }

        // 2. Floating Top Telemetry Strip
        Column(
            modifier = Modifier
                .align(Alignment.TopCenter)
                .fillMaxWidth()
                .statusBarsPadding()
                .padding(horizontal = 14.dp, vertical = 10.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                TransportTierPill(tier = state.tier, isStreaming = state.isStreaming)

                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    TelemetryChip(
                        text = "${state.resolutionLabel}${state.fps}",
                        onClick = onCycleResolution,
                    )
                    TelemetryChip(
                        text = state.codec.label,
                        onClick = onCycleCodec,
                    )
                    TopActionIcon(
                        icon = Icons.Rounded.QrCodeScanner,
                        contentDescription = "Scan PC QR code",
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

            // Quick-connect LAN chip when a PC is auto-discovered via mDNS
            val firstHost = state.discoveredHosts.firstOrNull()
            if (firstHost != null && !state.isStreaming) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(StudioTokens.RadiusMd))
                        .background(StudioTokens.SurfaceElevated.copy(alpha = 0.92f))
                        .border(1.dp, StudioTokens.AccentPrimary.copy(alpha = 0.5f), RoundedCornerShape(StudioTokens.RadiusMd))
                        .clickable { onConnectDiscoveredHost(firstHost) }
                        .padding(horizontal = 12.dp, vertical = 9.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Text(
                        text = "PC Ready: ${firstHost.name}",
                        style = MaterialTheme.typography.labelMedium,
                        color = StudioTokens.InkPrimary,
                    )
                    Text(
                        text = "${firstHost.ip} • Tap to pair",
                        style = StudioTelemetryStyle.copy(fontSize = 11.sp),
                        color = StudioTokens.AccentPrimary,
                    )
                }
            }
        }

        // 3. Thumb-Reachable Bottom Studio Control Dock
        Column(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(horizontal = 14.dp, vertical = 14.dp)
                .background(StudioTokens.SurfaceElevated.copy(alpha = 0.94f), RoundedCornerShape(StudioTokens.RadiusLg))
                .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(StudioTokens.RadiusLg))
                .padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            // Broadcast 16-segment LED VU Meter
            BroadcastVuMeter16(
                rmsDbfsX10 = if (state.isMicMuted) -960 else state.rmsDbfsX10,
                isMuted = state.isMicMuted,
            )

            // Multi-lens optical switcher pills
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.Center,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                state.lenses.forEach { lens ->
                    val selected = lens.cameraId == state.activeLensId
                    Box(
                        modifier = Modifier
                            .padding(horizontal = 4.dp)
                            .clip(CircleShape)
                            .background(if (selected) StudioTokens.AccentPrimary else StudioTokens.SurfaceCard)
                            .border(
                                1.dp,
                                if (selected) StudioTokens.AccentPrimary else StudioTokens.BorderSubtle,
                                CircleShape,
                            )
                            .clickable { onSelectLens(lens.cameraId) }
                            .padding(horizontal = 14.dp, vertical = 6.dp),
                    ) {
                        Text(
                            text = lens.label,
                            style = StudioTelemetryStyle.copy(fontSize = 12.sp),
                            color = if (selected) StudioTokens.InkOnAccent else StudioTokens.InkPrimary,
                        )
                    }
                }
            }

            // Primary action bar: Video Pause | Start/Stop CTA | Mic Mute | QR/Settings
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
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
                        .height(50.dp),
                    shape = RoundedCornerShape(StudioTokens.RadiusMd),
                    colors = ButtonDefaults.buttonColors(
                        containerColor = if (state.isStreaming) StudioTokens.StatusAlert else StudioTokens.AccentPrimary,
                        contentColor = if (state.isStreaming) StudioTokens.InkPrimary else StudioTokens.InkOnAccent,
                    ),
                ) {
                    Text(
                        text = if (state.isStreaming) "STOP STREAM" else "START STREAM",
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
                    onClick = onOpenQrScanner,
                    modifier = Modifier
                        .size(48.dp)
                        .clip(RoundedCornerShape(StudioTokens.RadiusMd))
                        .background(StudioTokens.SurfaceCard)
                        .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(StudioTokens.RadiusMd)),
                ) {
                    Icon(
                        imageVector = Icons.Rounded.Settings,
                        contentDescription = "Pair & Settings",
                        tint = StudioTokens.InkSecondary,
                    )
                }
            }
        }

        // 4. Optional Live QR Scanner + TOFU Consent Overlay
        if (state.isQrScannerOpen) {
            QrScannerOverlay(
                pendingConsent = state.pendingConsent,
                onApprovePairing = onApproveQrPairing,
                onRejectPairing = onRejectQrPairing,
                onManualIpConnect = onManualIpConnect,
                onClose = onCloseQrScanner,
            )
        }

        // 5. Optional 0-Nit Pure-Black OLED Curtain Overlay
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
private fun TransportTierPill(tier: ActiveTransportTier, isStreaming: Boolean) {
    val dotColor = when {
        !isStreaming -> StudioTokens.InkSecondary
        tier == ActiveTransportTier.DISCONNECTED -> StudioTokens.StatusWarn
        else -> StudioTokens.StatusLive
    }
    Row(
        modifier = Modifier
            .clip(CircleShape)
            .background(StudioTokens.SurfaceCard.copy(alpha = 0.92f))
            .border(1.dp, StudioTokens.BorderSubtle, CircleShape)
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            modifier = Modifier
                .size(8.dp)
                .clip(CircleShape)
                .background(dotColor),
        )
        Spacer(modifier = Modifier.width(7.dp))
        Text(
            text = if (isStreaming) tier.badge else "STANDBY",
            style = StudioTelemetryStyle.copy(fontSize = 12.sp),
            color = if (isStreaming) StudioTokens.StatusLive else StudioTokens.InkPrimary,
        )
    }
}

@Composable
private fun TelemetryChip(text: String, onClick: () -> Unit) {
    Box(
        modifier = Modifier
            .clip(RoundedCornerShape(StudioTokens.RadiusSm))
            .background(StudioTokens.SurfaceCard.copy(alpha = 0.9f))
            .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(StudioTokens.RadiusSm))
            .clickable(onClick = onClick)
            .padding(horizontal = 9.dp, vertical = 6.dp),
    ) {
        Text(
            text = text,
            style = StudioTelemetryStyle.copy(fontSize = 12.sp),
            color = StudioTokens.InkPrimary,
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
            .size(34.dp)
            .clip(RoundedCornerShape(StudioTokens.RadiusSm))
            .background(StudioTokens.SurfaceCard.copy(alpha = 0.9f))
            .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(StudioTokens.RadiusSm)),
    ) {
        Icon(imageVector = icon, contentDescription = contentDescription, tint = tint, modifier = Modifier.size(18.dp))
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
            .size(48.dp)
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
