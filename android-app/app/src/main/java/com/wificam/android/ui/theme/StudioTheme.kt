package com.wificam.android.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * Exact WCAG-validated tokens from root `DESIGN.md` ("WifiCam Studio").
 */
object StudioTokens {
    val OledBlack = Color(0xFF000000)
    val SurfaceBase = Color(0xFF0D0F12)
    val SurfaceElevated = Color(0xFF161A20)
    val SurfaceCard = Color(0xFF1D222B)
    val BorderSubtle = Color(0xFF2B3240)

    val InkPrimary = Color(0xFFF8FAFC)
    val InkSecondary = Color(0xFF9CA8BA)
    val InkOnAccent = Color(0xFF061018)

    val AccentPrimary = Color(0xFF38BDF8)
    val AccentPrimaryHover = Color(0xFF7DD3FC)

    val StatusLive = Color(0xFF22C55E)
    val StatusWarn = Color(0xFFF59E0B)
    val StatusAlert = Color(0xFFF43F5E)

    val VuNominal = Color(0xFF22C55E)
    val VuHot = Color(0xFFF59E0B)
    val VuClip = Color(0xFFEF4444)

    val RadiusSm = 8.dp
    val RadiusMd = 12.dp
    val RadiusLg = 18.dp
}

private val StudioColorScheme = darkColorScheme(
    primary = StudioTokens.AccentPrimary,
    onPrimary = StudioTokens.InkOnAccent,
    secondary = StudioTokens.StatusLive,
    onSecondary = StudioTokens.InkOnAccent,
    background = StudioTokens.OledBlack,
    onBackground = StudioTokens.InkPrimary,
    surface = StudioTokens.SurfaceBase,
    onSurface = StudioTokens.InkPrimary,
    surfaceVariant = StudioTokens.SurfaceElevated,
    onSurfaceVariant = StudioTokens.InkSecondary,
    outline = StudioTokens.BorderSubtle,
    error = StudioTokens.StatusAlert,
    onError = StudioTokens.InkPrimary,
)

val StudioTelemetryStyle = TextStyle(
    fontFamily = FontFamily.Monospace,
    fontWeight = FontWeight.SemiBold,
    fontSize = 13.sp,
    lineHeight = 16.sp,
    fontFeatureSettings = "tnum",
)

private val StudioTypography = Typography(
    headlineSmall = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Bold,
        fontSize = 22.sp,
        lineHeight = 26.sp,
        letterSpacing = (-0.4).sp,
    ),
    titleMedium = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.SemiBold,
        fontSize = 16.sp,
        lineHeight = 20.sp,
    ),
    bodyMedium = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.Normal,
        fontSize = 15.sp,
        lineHeight = 22.sp,
    ),
    labelMedium = TextStyle(
        fontFamily = FontFamily.SansSerif,
        fontWeight = FontWeight.SemiBold,
        fontSize = 12.sp,
        lineHeight = 14.sp,
        letterSpacing = 0.5.sp,
    ),
)

@Composable
fun WifiCamStudioTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = StudioColorScheme,
        typography = StudioTypography,
        content = content,
    )
}
