package com.wificam.android.protocol

import com.wificam.android.core.Brand
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.Base64

const val PACKET_MAGIC: Int = 0x434D // "CM"
const val PROTOCOL_VERSION: Int = 2
const val HEADER_LEN: Int = 24
const val MAX_PAYLOAD_LEN: Int = 4 * 1024 * 1024
const val DEFAULT_FRAGMENT_SIZE: Int = 8 * 1024

enum class StreamType(val id: Byte) {
    VideoH264(1),
    VideoH265(2),
    AudioPcm16(3),
    AudioOpus(4),
    Control(5),
    Heartbeat(6),
    VideoAv1(7);

    val isVideo: Boolean
        get() = this == VideoH264 || this == VideoH265 || this == VideoAv1

    val isAudio: Boolean
        get() = this == AudioPcm16 || this == AudioOpus

    companion object {
        fun fromByte(value: Byte): StreamType =
            entries.firstOrNull { it.id == value }
                ?: throw IllegalArgumentException("Unknown stream type: ${value.toInt() and 0xFF}")
    }
}

object PacketFlags {
    const val NONE: Int = 0
    const val KEYFRAME: Int = 1 shl 0
    const val CONFIG: Int = 1 shl 1
    const val HAS_FEC: Int = 1 shl 2
    const val FRAG_START: Int = 1 shl 3
    const val FRAG_END: Int = 1 shl 4
    const val ENCRYPTED: Int = 1 shl 5
    const val CRC_PRESENT: Int = 1 shl 6
}

data class WirePacket(
    val streamType: StreamType,
    val flags: Int,
    val seq: UInt,
    val ptsUs: Long,
    val payload: ByteArray,
) {
    override fun equals(other: Any?): Boolean {
        if (this === other) return true
        if (other !is WirePacket) return false
        return streamType == other.streamType &&
            flags == other.flags &&
            seq == other.seq &&
            ptsUs == other.ptsUs &&
            payload.contentEquals(other.payload)
    }

    override fun hashCode(): Int {
        var result = streamType.hashCode()
        result = 31 * result + flags
        result = 31 * result + seq.hashCode()
        result = 31 * result + ptsUs.hashCode()
        result = 31 * result + payload.contentHashCode()
        return result
    }
}

/**
 * CRC-16/CCITT-FALSE (poly=0x1021, init=0xFFFF) matching `pc-core/src/protocol/crc.rs`.
 */
object Crc16Ccitt {
    fun compute(vararg chunks: ByteArray): Int {
        var crc = 0xFFFF
        for (chunk in chunks) {
            for (b in chunk) {
                crc = crc xor ((b.toInt() and 0xFF) shl 8)
                repeat(8) {
                    crc = if ((crc and 0x8000) != 0) {
                        ((crc shl 1) xor 0x1021) and 0xFFFF
                    } else {
                        (crc shl 1) and 0xFFFF
                    }
                }
            }
        }
        return crc
    }
}

/**
 * Wire Protocol v2 encoder/decoder validated byte-for-byte against
 * `proto/golden_vectors.json` and `pc-core/src/protocol/packet.rs`.
 */
object PacketCodec {
    fun encode(packet: WirePacket): ByteArray {
        require(packet.payload.size <= MAX_PAYLOAD_LEN) {
            "Payload ${packet.payload.size} exceeds max $MAX_PAYLOAD_LEN"
        }
        val out = ByteArray(HEADER_LEN + packet.payload.size)
        val buf = ByteBuffer.wrap(out).order(ByteOrder.BIG_ENDIAN)
        buf.putShort(PACKET_MAGIC.toShort())
        buf.put(PROTOCOL_VERSION.toByte())
        buf.put(packet.streamType.id)
        buf.putShort(packet.flags.toShort())
        buf.putInt(packet.seq.toInt())
        buf.putLong(packet.ptsUs)
        buf.putInt(packet.payload.size)

        val crc = if ((packet.flags and PacketFlags.CRC_PRESENT) != 0) {
            val aad = out.copyOfRange(0, 22)
            Crc16Ccitt.compute(aad, packet.payload)
        } else {
            0
        }
        buf.putShort(crc.toShort())
        buf.put(packet.payload)
        return out
    }

    fun decode(raw: ByteArray): WirePacket {
        require(raw.size >= HEADER_LEN) { "Truncated header: ${raw.size} < $HEADER_LEN" }
        val buf = ByteBuffer.wrap(raw).order(ByteOrder.BIG_ENDIAN)
        val magic = buf.short.toInt() and 0xFFFF
        require(magic == PACKET_MAGIC) { "Invalid magic: 0x${magic.toString(16)}" }

        val version = buf.get().toInt() and 0xFF
        require(version == PROTOCOL_VERSION) { "Unsupported version: $version" }

        val streamType = StreamType.fromByte(buf.get())
        val flags = buf.short.toInt() and 0xFFFF
        val seq = buf.int.toUInt()
        val ptsUs = buf.long
        val payloadLen = buf.int
        val crc16 = buf.short.toInt() and 0xFFFF

        require(payloadLen in 0..MAX_PAYLOAD_LEN) { "Invalid payload length: $payloadLen" }
        require(raw.size >= HEADER_LEN + payloadLen) {
            "Truncated payload: got ${raw.size - HEADER_LEN}, expected $payloadLen"
        }

        val payload = raw.copyOfRange(HEADER_LEN, HEADER_LEN + payloadLen)
        if ((flags and PacketFlags.CRC_PRESENT) != 0) {
            val computed = Crc16Ccitt.compute(raw.copyOfRange(0, 22), payload)
            require(computed == crc16) {
                "CRC mismatch: expected 0x${crc16.toString(16)}, got 0x${computed.toString(16)}"
            }
        }
        return WirePacket(streamType, flags, seq, ptsUs, payload)
    }

    /**
     * Split a large video access unit into 8 KiB fragments so 10 ms voice frames
     * can preempt queued video chunks on shared USB/TCP/BT links.
     */
    fun fragmentVideo(
        streamType: StreamType,
        baseFlags: Int,
        startSeq: UInt,
        ptsUs: Long,
        frame: ByteArray,
        maxChunkSize: Int = DEFAULT_FRAGMENT_SIZE,
    ): Pair<List<WirePacket>, UInt> {
        require(streamType.isVideo) { "fragmentVideo requires a video stream type" }
        val chunkSize = maxChunkSize.coerceAtLeast(256)
        val cleanFlags = baseFlags and (PacketFlags.FRAG_START or PacketFlags.FRAG_END).inv()

        if (frame.size <= chunkSize) {
            return listOf(WirePacket(streamType, cleanFlags, startSeq, ptsUs, frame)) to (startSeq + 1u)
        }

        val totalChunks = (frame.size + chunkSize - 1) / chunkSize
        val result = ArrayList<WirePacket>(totalChunks)
        var seq = startSeq
        var offset = 0
        for (idx in 0 until totalChunks) {
            val end = (offset + chunkSize).coerceAtMost(frame.size)
            val slice = frame.copyOfRange(offset, end)
            offset = end
            var flags = cleanFlags
            if (idx == 0) flags = flags or PacketFlags.FRAG_START
            if (idx == totalChunks - 1) flags = flags or PacketFlags.FRAG_END
            result.add(WirePacket(streamType, flags, seq, ptsUs, slice))
            seq += 1u
        }
        return result to seq
    }
}

/**
 * Parser for PC QR pairing URIs (`wificam://pair?ip=...&port=...&udp=...&pub=...&token=...&name=...`).
 */
data class PairingInfo(
    val ip: String,
    val controlPort: Int,
    val mediaUdpPort: Int,
    val hostName: String,
    val hostPubB64: String,
    val tokenB64: String,
) {
    companion object {
        fun parse(uri: String): PairingInfo {
            val prefix = "${Brand.PAIRING_URI_SCHEME}://pair?"
            require(uri.startsWith(prefix)) { "Invalid pairing URI scheme" }
            val params = uri.removePrefix(prefix).split('&').associate { kv ->
                val parts = kv.split('=', limit = 2)
                require(parts.size == 2) { "Malformed query param: $kv" }
                parts[0] to parts[1]
            }
            val nameBytes = Base64.getUrlDecoder().decode(requireNotNull(params["name"]))
            return PairingInfo(
                ip = requireNotNull(params["ip"]),
                controlPort = requireNotNull(params["port"]).toInt(),
                mediaUdpPort = requireNotNull(params["udp"]).toInt(),
                hostName = String(nameBytes, Charsets.UTF_8),
                hostPubB64 = requireNotNull(params["pub"]),
                tokenB64 = requireNotNull(params["token"]),
            )
        }
    }
}
