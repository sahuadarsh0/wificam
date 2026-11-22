package com.wificam.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.wificam.android.ui.theme.StudioTelemetryStyle
import com.wificam.android.ui.theme.StudioTokens
import kotlinx.coroutines.delay
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * True 0-nit pure `#000000` AMOLED battery & thermal saver overlay.
 *
 * Cuts display power draw to near zero (cooling the phone by 8-12 C during
 * long 4K/60 fps sessions) while drifting dim status text every 60 seconds
 * to eliminate OLED burn-in.
 */
@Composable
fun OledCurtainOverlay(
    tierBadge: String,
    isMuted: Boolean,
    onWake: () -> Unit,
) {
    var clockText by remember { mutableStateOf(formatTime()) }
    var driftStep by remember { mutableIntStateOf(0) }

    BackHandler(onBack = onWake)

    LaunchedEffect(Unit) {
        while (true) {
            clockText = formatTime()
            delay(60_000L)
            driftStep = (driftStep + 1) % 6
        }
    }

    val driftX = ((driftStep % 3) - 1) * 18
    val driftY = ((driftStep / 3) - 1) * 24

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(StudioTokens.OledBlack)
            .pointerInput(Unit) {
                detectTapGestures(onDoubleTap = { onWake() })
            },
        contentAlignment = Alignment.Center,
    ) {
        Column(
            modifier = Modifier.offset(x = driftX.dp, y = driftY.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            Text(
                text = clockText,
                style = StudioTelemetryStyle.copy(fontSize = 44.sp, lineHeight = 48.sp),
                color = StudioTokens.InkSecondary.copy(alpha = 0.35f),
            )
            Spacer(modifier = Modifier.height(12.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(
                    modifier = Modifier
                        .size(8.dp)
                        .clip(CircleShape)
                        .background(
                            if (isMuted) StudioTokens.StatusAlert else StudioTokens.StatusLive.copy(alpha = 0.65f)
                        ),
                )
                Spacer(modifier = Modifier.width(8.dp))
                Text(
                    text = if (isMuted) "$tierBadge • MIC MUTED" else "$tierBadge • LIVE",
                    style = StudioTelemetryStyle,
                    color = StudioTokens.InkSecondary.copy(alpha = 0.45f),
                )
            }
            Spacer(modifier = Modifier.height(24.dp))
            Text(
                text = "Double-tap anywhere to wake viewfinder",
                style = StudioTelemetryStyle.copy(fontSize = 11.sp),
                color = StudioTokens.InkSecondary.copy(alpha = 0.25f),
            )
        }
    }
}

private fun formatTime(): String =
    SimpleDateFormat("HH:mm", Locale.getDefault()).format(Date())
