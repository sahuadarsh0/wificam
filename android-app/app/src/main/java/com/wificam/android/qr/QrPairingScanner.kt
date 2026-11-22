package com.wificam.android.qr

import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import com.wificam.android.protocol.PairingInfo

/**
 * 100% open-source, zero-cloud, zero-Play-Services QR pairing decoder powered
 * by ZXing Core. Reads directly from Camera2 Y-plane luma buffers in <10 ms.
 */
object QrPairingScanner {
    private val hints: Map<DecodeHintType, Any> = mapOf(
        DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE),
        DecodeHintType.TRY_HARDER to true,
    )

    /**
     * Attempt to decode a WifiCam pairing QR code (`wificam://pair?...`) from
     * an 8-bit grayscale / Camera2 Y-plane buffer.
     *
     * Returns `null` if no valid pairing QR code is visible in the frame.
     */
    fun decodeFromLuma(
        yPlane: ByteArray,
        width: Int,
        height: Int,
    ): PairingInfo? {
        if (width <= 0 || height <= 0 || yPlane.size < width * height) {
            return null
        }
        val source = PlanarYUVLuminanceSource(
            yPlane,
            width,
            height,
            0,
            0,
            width,
            height,
            false,
        )
        val bitmap = BinaryBitmap(HybridBinarizer(source))
        val rawText = runCatching {
            QRCodeReader().decode(bitmap, hints).text
        }.getOrNull() ?: return null

        return runCatching { PairingInfo.parse(rawText) }.getOrNull()
    }
}
