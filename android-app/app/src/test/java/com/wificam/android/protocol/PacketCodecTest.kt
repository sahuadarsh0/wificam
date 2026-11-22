package com.wificam.android.protocol

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.util.Base64

class PacketCodecTest {

    @Test
    fun goldenVectorsMatchRustWireFormatByteForByte() {
        val vectorFile = findGoldenVectorsFile()
        val root = JSONObject(vectorFile.readText(Charsets.UTF_8))
        val vectors = root.getJSONArray("vectors")
        assertTrue("golden_vectors.json must not be empty", vectors.length() > 0)

        for (i in 0 until vectors.length()) {
            val v = vectors.getJSONObject(i)
            val name = v.getString("name")
            val pkt = WirePacket(
                streamType = StreamType.fromByte(v.getInt("stream_type").toByte()),
                flags = v.getInt("flags"),
                seq = v.getLong("seq").toUInt(),
                ptsUs = v.getLong("pts_us"),
                payload = decodeHex(v.getString("payload_hex")),
            )
            val expectedWire = decodeHex(v.getString("wire_hex"))

            val encoded = PacketCodec.encode(pkt)
            assertArrayEquals("Encode mismatch on vector '$name'", expectedWire, encoded)

            val decoded = PacketCodec.decode(expectedWire)
            assertEquals("Decode mismatch on vector '$name'", pkt, decoded)
        }
    }

    @Test
    fun fragmentsLarge4kVideoInto8KiBChunks() {
        val frame = ByteArray(25_000) { (it % 251).toByte() }
        val (frags, nextSeq) = PacketCodec.fragmentVideo(
            streamType = StreamType.VideoAv1,
            baseFlags = PacketFlags.KEYFRAME,
            startSeq = 50u,
            ptsUs = 1_000_000L,
            frame = frame,
        )
        assertEquals(4, frags.size)
        assertEquals(54u, nextSeq)
        assertTrue((frags.first().flags and PacketFlags.FRAG_START) != 0)
        assertTrue((frags.last().flags and PacketFlags.FRAG_END) != 0)

        val reassembled = ByteArray(frame.size)
        var offset = 0
        for (f in frags) {
            System.arraycopy(f.payload, 0, reassembled, offset, f.payload.size)
            offset += f.payload.size
        }
        assertArrayEquals(frame, reassembled)
    }

    @Test
    fun parsesPcPairingQrUri() {
        val hostNameB64 = Base64.getUrlEncoder().withoutPadding().encodeToString("Studio PC".toByteArray())
        val uri = "wificam://pair?ip=192.168.1.55&port=7890&udp=7891&pub=ABCD1234&token=TOK999&name=$hostNameB64"
        val info = PairingInfo.parse(uri)
        assertEquals("192.168.1.55", info.ip)
        assertEquals(7890, info.controlPort)
        assertEquals(7891, info.mediaUdpPort)
        assertEquals("Studio PC", info.hostName)
        assertEquals("ABCD1234", info.hostPubB64)
        assertEquals("TOK999", info.tokenB64)
    }

    private fun findGoldenVectorsFile(): File {
        val candidates = listOf(
            File("../../proto/golden_vectors.json"),
            File("../proto/golden_vectors.json"),
            File("proto/golden_vectors.json"),
        )
        return candidates.firstOrNull { it.isFile }
            ?: error("Could not locate proto/golden_vectors.json from ${File(".").absolutePath}")
    }

    private fun decodeHex(hex: String): ByteArray {
        require(hex.length % 2 == 0)
        return ByteArray(hex.length / 2) { i ->
            hex.substring(i * 2, i * 2 + 2).toInt(16).toByte()
        }
    }
}
