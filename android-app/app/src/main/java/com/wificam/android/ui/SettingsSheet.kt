package com.wificam.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.wificam.android.ui.theme.StudioTelemetryStyle
import com.wificam.android.ui.theme.StudioTokens
import com.wificam.android.video.AndroidVideoCodec

enum class VideoResolutionPreset(
    val label: String,
    val width: Int,
    val height: Int,
    val defaultBitrateKbps: Int,
) {
    R360P("360p", 640, 360, 1_200),
    R480P("480p", 854, 480, 2_200),
    R720P("720p", 1280, 720, 5_000),
    R1080P("1080p", 1920, 1080, 10_000),
    R4K("4K", 3840, 2160, 22_000),
}

enum class VideoQualityPreset(val label: String, val multiplier: Float) {
    LOW("Low", 0.55f),
    BALANCED("Balanced", 1.0f),
    HIGH("High", 1.5f),
    STUDIO("Studio", 2.0f),
}

/**
 * Dismissible Video Quality, Codec, FPS, Antibanding & Pairing Settings sheet.
 * Supports tap-outside scrim, large Close button, Done button, and system Back gesture.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun SettingsSheet(
    state: StudioUiState,
    onSelectResolution: (VideoResolutionPreset) -> Unit,
    onSelectFps: (Int) -> Unit,
    onSelectQuality: (VideoQualityPreset) -> Unit,
    onSelectCodec: (AndroidVideoCodec) -> Unit,
    onSelectAntibanding: (Int) -> Unit,
    onOpenQrScanner: () -> Unit,
    onReconnect: () -> Unit,
    onUnpair: () -> Unit,
    onToggleBackgroundStreaming: (Boolean) -> Unit,
    onMinimizeToBackground: () -> Unit,
    onDismiss: () -> Unit,
) {
    BackHandler(onBack = onDismiss)

    // Dimmed tap-to-dismiss scrim
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(StudioTokens.OledBlack.copy(alpha = 0.68f))
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                onClick = onDismiss,
            ),
        contentAlignment = Alignment.BottomCenter,
    ) {
        // Sheet surface (consumes clicks so tapping inside does not dismiss)
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp))
                .background(StudioTokens.SurfaceElevated)
                .border(
                    1.dp,
                    StudioTokens.BorderSubtle,
                    RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp),
                )
                .clickable(
                    interactionSource = remember { MutableInteractionSource() },
                    indication = null,
                    onClick = {},
                )
                .navigationBarsPadding()
                .padding(horizontal = 20.dp, vertical = 18.dp)
                .verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            // Header with large dismissible Close button
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(
                    text = "Video & Connection Settings",
                    style = MaterialTheme.typography.titleMedium,
                    color = StudioTokens.InkPrimary,
                )
                IconButton(
                    onClick = onDismiss,
                    modifier = Modifier
                        .size(44.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(StudioTokens.SurfaceCard),
                ) {
                    Icon(
                        imageVector = Icons.Rounded.Close,
                        contentDescription = "Close settings",
                        tint = StudioTokens.InkPrimary,
                    )
                }
            }

            // Paired PC Connection Card
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(StudioTokens.RadiusMd))
                    .background(StudioTokens.SurfaceCard)
                    .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(StudioTokens.RadiusMd))
                    .padding(14.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    text = if (state.isPaired) "PAIRED PC" else "PAIRING STATUS",
                    style = StudioTelemetryStyle.copy(fontSize = 11.sp),
                    color = if (state.isPaired) StudioTokens.StatusLive else StudioTokens.StatusWarn,
                )
                Text(
                    text = if (state.isPaired) {
                        "${state.pairedHostName} (${state.pairedHostIp})"
                    } else {
                        "Not paired to any PC yet"
                    },
                    style = MaterialTheme.typography.bodyMedium,
                    color = StudioTokens.InkPrimary,
                )
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    if (state.isPaired && !state.isStreaming) {
                        Button(
                            onClick = {
                                onDismiss()
                                onReconnect()
                            },
                            shape = RoundedCornerShape(StudioTokens.RadiusSm),
                            colors = ButtonDefaults.buttonColors(
                                containerColor = StudioTokens.StatusLive,
                                contentColor = StudioTokens.InkOnAccent,
                            ),
                        ) {
                            Text("Reconnect")
                        }
                    }
                    Button(
                        onClick = {
                            onDismiss()
                            onOpenQrScanner()
                        },
                        shape = RoundedCornerShape(StudioTokens.RadiusSm),
                        colors = ButtonDefaults.buttonColors(
                            containerColor = StudioTokens.AccentPrimary,
                            contentColor = StudioTokens.InkOnAccent,
                        ),
                    ) {
                        Text(if (state.isPaired) "Pair New PC" else "Scan PC QR")
                    }
                    if (state.isPaired) {
                        OutlinedButton(
                            onClick = onUnpair,
                            shape = RoundedCornerShape(StudioTokens.RadiusSm),
                        ) {
                            Text("Unpair", color = StudioTokens.StatusAlert)
                        }
                    }
                }
            }

            // Background Streaming Mode (keeps Camera2 + Mic alive when minimized / screen off)
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(StudioTokens.RadiusMd))
                    .background(StudioTokens.SurfaceCard)
                    .border(1.dp, StudioTokens.BorderSubtle, RoundedCornerShape(StudioTokens.RadiusMd))
                    .padding(14.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Column(modifier = Modifier.weight(1f)) {
                        Text(
                            text = "WORK IN BACKGROUND",
                            style = StudioTelemetryStyle.copy(fontSize = 11.sp),
                            color = StudioTokens.AccentPrimary,
                        )
                        Text(
                            text = if (state.backgroundStreamingEnabled) {
                                "Camera & mic keep streaming when minimized or screen turns off"
                            } else {
                                "Streaming pauses when you leave the app"
                            },
                            style = MaterialTheme.typography.bodySmall,
                            color = StudioTokens.InkSecondary,
                        )
                    }
                    androidx.compose.material3.Switch(
                        checked = state.backgroundStreamingEnabled,
                        onCheckedChange = onToggleBackgroundStreaming,
                    )
                }
                if (state.isStreaming && state.backgroundStreamingEnabled) {
                    OutlinedButton(
                        onClick = {
                            onDismiss()
                            onMinimizeToBackground()
                        },
                        shape = RoundedCornerShape(StudioTokens.RadiusSm),
                    ) {
                        Text("Minimize to Background Now", color = StudioTokens.AccentPrimary)
                    }
                }
            }

            // Resolution picker (360p -> 4K)
            SettingsSection(title = "Resolution (${state.resolution.width}x${state.resolution.height})") {
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    VideoResolutionPreset.entries.forEach { preset ->
                        OptionPill(
                            label = preset.label,
                            selected = state.resolution == preset,
                            onClick = { onSelectResolution(preset) },
                        )
                    }
                }
            }

            // Video Quality / Bitrate picker
            val kbps = (state.resolution.defaultBitrateKbps * state.quality.multiplier).toInt()
            val mbpsText = if (kbps >= 1000) String.format("%.1f Mbps", kbps / 1000f) else "$kbps kbps"
            SettingsSection(title = "Video Quality ($mbpsText)") {
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    VideoQualityPreset.entries.forEach { q ->
                        OptionPill(
                            label = q.label,
                            selected = state.quality == q,
                            onClick = { onSelectQuality(q) },
                        )
                    }
                }
            }

            // Frame rate picker
            SettingsSection(title = "Frame Rate") {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf(24, 30, 60).forEach { fps ->
                        OptionPill(
                            label = "$fps fps",
                            selected = state.fps == fps,
                            onClick = { onSelectFps(fps) },
                        )
                    }
                }
            }

            // Codec picker
            SettingsSection(title = "Video Codec") {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    AndroidVideoCodec.entries.forEach { codec ->
                        OptionPill(
                            label = codec.label,
                            selected = state.codec == codec,
                            onClick = { onSelectCodec(codec) },
                        )
                    }
                }
            }

            // Antibanding
            SettingsSection(title = "Anti-Flicker") {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf(50 to "50 Hz", 60 to "60 Hz").forEach { (hz, label) ->
                        OptionPill(
                            label = label,
                            selected = state.antibandingHz == hz,
                            onClick = { onSelectAntibanding(hz) },
                        )
                    }
                }
            }

            Spacer(modifier = Modifier.height(4.dp))

            Button(
                onClick = onDismiss,
                modifier = Modifier
                    .fillMaxWidth()
                    .height(48.dp),
                shape = RoundedCornerShape(StudioTokens.RadiusMd),
                colors = ButtonDefaults.buttonColors(
                    containerColor = StudioTokens.AccentPrimary,
                    contentColor = StudioTokens.InkOnAccent,
                ),
            ) {
                Text("Done")
            }
        }
    }
}

@Composable
private fun SettingsSection(
    title: String,
    content: @Composable () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            text = title,
            style = MaterialTheme.typography.labelMedium,
            color = StudioTokens.InkSecondary,
        )
        content()
    }
}

@Composable
private fun OptionPill(
    label: String,
    selected: Boolean,
    onClick: () -> Unit,
) {
    Box(
        modifier = Modifier
            .clip(RoundedCornerShape(StudioTokens.RadiusSm))
            .background(if (selected) StudioTokens.AccentPrimary else StudioTokens.SurfaceCard)
            .border(
                1.dp,
                if (selected) StudioTokens.AccentPrimary else StudioTokens.BorderSubtle,
                RoundedCornerShape(StudioTokens.RadiusSm),
            )
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 10.dp),
    ) {
        Text(
            text = label,
            style = StudioTelemetryStyle.copy(fontSize = 12.sp),
            color = if (selected) StudioTokens.InkOnAccent else StudioTokens.InkPrimary,
        )
    }
}
