package com.wificam.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Computer
import androidx.compose.material.icons.rounded.Keyboard
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.wificam.android.protocol.PairingInfo
import com.wificam.android.ui.theme.StudioTelemetryStyle
import com.wificam.android.ui.theme.StudioTokens

/**
 * Clean, bright cutout QR pairing scanner designed specifically for connecting
 * this Android phone to WifiCam Studio on a Windows PC.
 */
@Composable
fun QrScannerOverlay(
    pendingConsent: PairingInfo?,
    discoveredHosts: List<com.wificam.android.transport.DiscoveredHost>,
    onConnectDiscoveredHost: (com.wificam.android.transport.DiscoveredHost) -> Unit,
    onApprovePairing: (PairingInfo) -> Unit,
    onRejectPairing: () -> Unit,
    onManualIpConnect: (String) -> Unit,
    onClose: () -> Unit,
) {
    var manualIp by remember { mutableStateOf("") }
    var showManualEntry by remember { mutableStateOf(false) }
    val haptics = LocalHapticFeedback.current

    LaunchedEffect(pendingConsent) {
        if (pendingConsent != null) {
            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
        }
    }

    val infiniteTransition = rememberInfiniteTransition(label = "qr-scan-beam")
    val beamFraction by infiniteTransition.animateFloat(
        initialValue = 0.08f,
        targetValue = 0.92f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 1900, easing = LinearEasing),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "beam-y",
    )

    BackHandler(onBack = onClose)

    Box(modifier = Modifier.fillMaxSize()) {
        // 1. Dimmed surround with a crisp transparent square cutout window + animated scan beam
        Canvas(
            modifier = Modifier
                .fillMaxSize()
                .graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen },
        ) {
            drawRect(color = Color.Black.copy(alpha = 0.72f))

            val boxSize = 256.dp.toPx()
            val left = (size.width - boxSize) / 2f
            val top = (size.height - boxSize) / 2f - 28.dp.toPx()

            // Punch a clean transparent hole so the QR code on the PC screen stays 100% bright & clear
            drawRoundRect(
                color = Color.Transparent,
                topLeft = Offset(left, top),
                size = Size(boxSize, boxSize),
                cornerRadius = CornerRadius(20.dp.toPx(), 20.dp.toPx()),
                blendMode = BlendMode.Clear,
            )

            // Corner brackets
            val stroke = 4.dp.toPx()
            val corner = 30.dp.toPx()
            val accent = if (pendingConsent != null) StudioTokens.StatusLive else StudioTokens.AccentPrimary
            val right = left + boxSize
            val bottom = top + boxSize

            drawLine(accent, Offset(left, top + corner), Offset(left, top), stroke, StrokeCap.Round)
            drawLine(accent, Offset(left, top), Offset(left + corner, top), stroke, StrokeCap.Round)
            drawLine(accent, Offset(right - corner, top), Offset(right, top), stroke, StrokeCap.Round)
            drawLine(accent, Offset(right, top), Offset(right, top + corner), stroke, StrokeCap.Round)
            drawLine(accent, Offset(left, bottom - corner), Offset(left, bottom), stroke, StrokeCap.Round)
            drawLine(accent, Offset(left, bottom), Offset(left + corner, bottom), stroke, StrokeCap.Round)
            drawLine(accent, Offset(right - corner, bottom), Offset(right, bottom), stroke, StrokeCap.Round)
            drawLine(accent, Offset(right, bottom - corner), Offset(right, bottom), stroke, StrokeCap.Round)

            // Smooth animated scan laser while actively searching
            if (pendingConsent == null) {
                val beamY = top + boxSize * beamFraction
                drawLine(
                    color = StudioTokens.AccentPrimary.copy(alpha = 0.85f),
                    start = Offset(left + 16.dp.toPx(), beamY),
                    end = Offset(right - 16.dp.toPx(), beamY),
                    strokeWidth = 2.5.dp.toPx(),
                    cap = StrokeCap.Round,
                )
            }
        }

        // 2. Clean Top Header Bar
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .statusBarsPadding()
                .padding(horizontal = 16.dp, vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(
                    modifier = Modifier
                        .size(40.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(StudioTokens.SurfaceCard)
                        .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(12.dp)),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(
                        imageVector = Icons.Rounded.Computer,
                        contentDescription = null,
                        tint = StudioTokens.AccentPrimary,
                    )
                }
                Spacer(modifier = Modifier.width(12.dp))
                Column {
                    Text(
                        text = "Connect to Your PC",
                        style = MaterialTheme.typography.titleMedium,
                        color = StudioTokens.InkPrimary,
                    )
                    Text(
                        text = "Scan the QR in WifiCam Studio on Windows",
                        style = StudioTelemetryStyle.copy(fontSize = 11.sp),
                        color = StudioTokens.InkSecondary,
                    )
                }
            }
            IconButton(
                onClick = onClose,
                modifier = Modifier
                    .size(44.dp)
                    .clip(RoundedCornerShape(12.dp))
                    .background(StudioTokens.SurfaceCard)
                    .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(12.dp)),
            ) {
                Icon(
                    imageVector = Icons.Rounded.Close,
                    contentDescription = "Close scanner",
                    tint = StudioTokens.InkPrimary,
                )
            }
        }

        // 3. Bottom Sheet: PC Found Consent Card OR Clean Collapsible Manual IP Entry
        Box(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .navigationBarsPadding()
                .padding(16.dp)
                .clip(RoundedCornerShape(20.dp))
                .background(StudioTokens.SurfaceElevated)
                .border(
                    1.dp,
                    if (pendingConsent != null) StudioTokens.StatusLive.copy(alpha = 0.6f) else StudioTokens.BorderSubtle,
                    RoundedCornerShape(20.dp),
                )
                .padding(18.dp),
        ) {
            if (pendingConsent != null) {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Icon(
                            imageVector = Icons.Rounded.Computer,
                            contentDescription = null,
                            tint = StudioTokens.StatusLive,
                            modifier = Modifier.size(22.dp),
                        )
                        Spacer(modifier = Modifier.width(10.dp))
                        Column {
                            Text(
                                text = "PC Found: ${pendingConsent.hostName}",
                                style = MaterialTheme.typography.titleMedium,
                                color = StudioTokens.InkPrimary,
                            )
                            Text(
                                text = "${pendingConsent.ip}:${pendingConsent.controlPort}",
                                style = StudioTelemetryStyle.copy(fontSize = 12.sp),
                                color = StudioTokens.AccentPrimary,
                            )
                        }
                    }
                    Text(
                        text = "Tap Connect to start encrypted camera & studio microphone streaming to this PC.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = StudioTokens.InkSecondary,
                    )
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        OutlinedButton(
                            onClick = onRejectPairing,
                            modifier = Modifier
                                .weight(1f)
                                .height(48.dp),
                            shape = RoundedCornerShape(StudioTokens.RadiusMd),
                        ) {
                            Text("Cancel", color = StudioTokens.InkPrimary)
                        }
                        Button(
                            onClick = { onApprovePairing(pendingConsent) },
                            modifier = Modifier
                                .weight(1.4f)
                                .height(48.dp),
                            shape = RoundedCornerShape(StudioTokens.RadiusMd),
                            colors = ButtonDefaults.buttonColors(
                                containerColor = StudioTokens.AccentPrimary,
                                contentColor = StudioTokens.InkOnAccent,
                            ),
                        ) {
                            Text("Connect to PC")
                        }
                    }
                }
            } else {
                Column(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Text(
                        text = "Align the PC QR code inside the frame above",
                        style = MaterialTheme.typography.bodyMedium,
                        color = StudioTokens.InkPrimary,
                        textAlign = TextAlign.Center,
                    )
                    val discovered = discoveredHosts.firstOrNull()
                    if (discovered != null) {
                        OutlinedButton(
                            onClick = { onConnectDiscoveredHost(discovered) },
                            modifier = Modifier.fillMaxWidth(),
                            shape = RoundedCornerShape(StudioTokens.RadiusMd),
                        ) {
                            Text(
                                text = "Tap to connect: ${discovered.name} (${discovered.ip})",
                                color = StudioTokens.AccentPrimary,
                            )
                        }
                    }
                    if (!showManualEntry) {
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            TextButton(
                                onClick = { showManualEntry = true },
                                modifier = Modifier.weight(1f),
                            ) {
                                Icon(
                                    imageVector = Icons.Rounded.Keyboard,
                                    contentDescription = null,
                                    tint = StudioTokens.AccentPrimary,
                                    modifier = Modifier.size(18.dp),
                                )
                                Spacer(modifier = Modifier.width(6.dp))
                                Text("Manual IP", color = StudioTokens.AccentPrimary)
                            }
                            OutlinedButton(
                                onClick = onClose,
                                modifier = Modifier.weight(1f),
                                shape = RoundedCornerShape(StudioTokens.RadiusMd),
                            ) {
                                Text("Close", color = StudioTokens.InkPrimary)
                            }
                        }
                    } else {
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            OutlinedTextField(
                                value = manualIp,
                                onValueChange = { manualIp = it.trim() },
                                placeholder = {
                                    Text("192.168.1.42", style = StudioTelemetryStyle, color = StudioTokens.InkSecondary)
                                },
                                singleLine = true,
                                textStyle = StudioTelemetryStyle.copy(color = StudioTokens.InkPrimary),
                                colors = OutlinedTextFieldDefaults.colors(
                                    focusedBorderColor = StudioTokens.AccentPrimary,
                                    unfocusedBorderColor = StudioTokens.BorderSubtle,
                                ),
                                modifier = Modifier.weight(1f),
                            )
                            Button(
                                onClick = { if (manualIp.isNotEmpty()) onManualIpConnect(manualIp) },
                                shape = RoundedCornerShape(StudioTokens.RadiusMd),
                                colors = ButtonDefaults.buttonColors(
                                    containerColor = StudioTokens.AccentPrimary,
                                    contentColor = StudioTokens.InkOnAccent,
                                ),
                                modifier = Modifier.height(54.dp),
                            ) {
                                Text("Connect")
                            }
                        }
                        TextButton(onClick = onClose) {
                            Text("Close Scanner", color = StudioTokens.InkSecondary)
                        }
                    }
                }
            }
        }
    }
}
