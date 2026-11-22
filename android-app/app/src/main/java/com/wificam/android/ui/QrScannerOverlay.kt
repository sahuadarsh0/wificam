package com.wificam.android.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.unit.dp
import com.wificam.android.protocol.PairingInfo
import com.wificam.android.ui.theme.StudioTelemetryStyle
import com.wificam.android.ui.theme.StudioTokens

/**
 * Fast wireless QR pairing overlay sitting directly on top of the live Camera2
 * viewfinder. Pointing the phone at the PC's QR code decodes `wificam://pair?...`
 * locally (zero cloud) and opens the explicit TOFU consent sheet.
 */
@Composable
fun QrScannerOverlay(
    pendingConsent: PairingInfo?,
    onApprovePairing: (PairingInfo) -> Unit,
    onRejectPairing: () -> Unit,
    onManualIpConnect: (String) -> Unit,
    onClose: () -> Unit,
) {
    var manualIp by remember { mutableStateOf("") }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(StudioTokens.OledBlack.copy(alpha = 0.58f)),
    ) {
        // Top header bar
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .statusBarsPadding()
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    imageVector = Icons.Rounded.QrCodeScanner,
                    contentDescription = null,
                    tint = StudioTokens.AccentPrimary,
                )
                Spacer(modifier = Modifier.width(8.dp))
                Text(
                    text = "Scan PC Pairing QR",
                    style = MaterialTheme.typography.titleMedium,
                    color = StudioTokens.InkPrimary,
                )
            }
            IconButton(onClick = onClose) {
                Icon(
                    imageVector = Icons.Rounded.Close,
                    contentDescription = "Close scanner",
                    tint = StudioTokens.InkPrimary,
                )
            }
        }

        // Center Studio Cyan framing reticle
        Box(
            modifier = Modifier
                .align(Alignment.Center)
                .size(256.dp),
        ) {
            Canvas(modifier = Modifier.fillMaxSize()) {
                val stroke = 4.dp.toPx()
                val corner = 32.dp.toPx()
                val color = StudioTokens.AccentPrimary
                val w = size.width
                val h = size.height

                // Top-left
                drawLine(color, Offset(0f, corner), Offset(0f, 0f), stroke, StrokeCap.Round)
                drawLine(color, Offset(0f, 0f), Offset(corner, 0f), stroke, StrokeCap.Round)
                // Top-right
                drawLine(color, Offset(w - corner, 0f), Offset(w, 0f), stroke, StrokeCap.Round)
                drawLine(color, Offset(w, 0f), Offset(w, corner), stroke, StrokeCap.Round)
                // Bottom-left
                drawLine(color, Offset(0f, h - corner), Offset(0f, h), stroke, StrokeCap.Round)
                drawLine(color, Offset(0f, h), Offset(corner, h), stroke, StrokeCap.Round)
                // Bottom-right
                drawLine(color, Offset(w - corner, h), Offset(w, h), stroke, StrokeCap.Round)
                drawLine(color, Offset(w, h - corner), Offset(w, h), stroke, StrokeCap.Round)
            }
        }

        // Bottom card: either Explicit TOFU Consent Sheet OR Manual IP fallback
        Box(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .padding(16.dp)
                .background(StudioTokens.SurfaceElevated, RoundedCornerShape(StudioTokens.RadiusLg))
                .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(StudioTokens.RadiusLg))
                .padding(16.dp),
        ) {
            if (pendingConsent != null) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(
                        text = "Allow PC to connect?",
                        style = MaterialTheme.typography.titleMedium,
                        color = StudioTokens.InkPrimary,
                    )
                    Text(
                        text = "${pendingConsent.hostName} (${pendingConsent.ip}:${pendingConsent.controlPort})",
                        style = StudioTelemetryStyle,
                        color = StudioTokens.AccentPrimary,
                    )
                    Text(
                        text = "QR code is consent to pair. Tap Allow to begin encrypted LAN video & mic streaming.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = StudioTokens.InkSecondary,
                    )
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        OutlinedButton(
                            onClick = onRejectPairing,
                            modifier = Modifier.weight(1f),
                            shape = RoundedCornerShape(StudioTokens.RadiusMd),
                        ) {
                            Text("Deny", color = StudioTokens.InkPrimary)
                        }
                        Button(
                            onClick = { onApprovePairing(pendingConsent) },
                            modifier = Modifier.weight(1f),
                            shape = RoundedCornerShape(StudioTokens.RadiusMd),
                            colors = ButtonDefaults.buttonColors(
                                containerColor = StudioTokens.AccentPrimary,
                                contentColor = StudioTokens.InkOnAccent,
                            ),
                        ) {
                            Text("Allow & Connect")
                        }
                    }
                }
            } else {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(
                        text = "Point at the QR code on your Windows 11 PC app, or enter its LAN IP:",
                        style = MaterialTheme.typography.bodyMedium,
                        color = StudioTokens.InkSecondary,
                    )
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
                }
            }
        }
    }
}
