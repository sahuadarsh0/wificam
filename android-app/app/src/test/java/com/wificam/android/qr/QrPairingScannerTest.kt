package com.wificam.android.qr

import com.google.zxing.BarcodeFormat
import com.google.zxing.qrcode.QRCodeWriter
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test
import java.util.Base64

class QrPairingScannerTest {

    @Test
    fun decodesGeneratedPcPairingQrFromLumaBuffer() {
        val nameB64 = Base64.getUrlEncoder().withoutPadding().encodeToString("Adarsh-Win11".toByteArray())
        val uri = "wificam://pair?ip=192.168.1.99&port=7890&udp=7891&pub=PUBKEY_B64&token=TOK123&name=$nameB64"

        val size = 320
        val matrix = QRCodeWriter().encode(uri, BarcodeFormat.QR_CODE, size, size)
        val luma = ByteArray(size * size) { idx ->
            val x = idx % size
            val y = idx / size
            if (matrix.get(x, y)) 16.toByte() else 240.toByte()
        }

        val decoded = QrPairingScanner.decodeFromLuma(luma, size, size)
        assertNotNull("QR code should decode cleanly from Y-plane", decoded)
        assertEquals("192.168.1.99", decoded!!.ip)
        assertEquals(7890, decoded.controlPort)
        assertEquals(7891, decoded.mediaUdpPort)
        assertEquals("Adarsh-Win11", decoded.hostName)
        assertEquals("PUBKEY_B64", decoded.hostPubB64)
        assertEquals("TOK123", decoded.tokenB64)
    }

    @Test
    fun ignoresUnrelatedNonWifiCamQrCodes() {
        val size = 256
        val matrix = QRCodeWriter().encode("https://example.com/not-wificam", BarcodeFormat.QR_CODE, size, size)
        val luma = ByteArray(size * size) { idx ->
            if (matrix.get(idx % size, idx / size)) 10.toByte() else 245.toByte()
        }
        assertNull(QrPairingScanner.decodeFromLuma(luma, size, size))
    }
}
