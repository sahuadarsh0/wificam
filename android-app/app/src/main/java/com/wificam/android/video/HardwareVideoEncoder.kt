package com.wificam.android.video

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Build
import android.os.Bundle
import android.view.Surface
import com.wificam.android.protocol.PacketCodec
import com.wificam.android.protocol.PacketFlags
import com.wificam.android.protocol.StreamType
import com.wificam.android.protocol.WirePacket
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

enum class AndroidVideoCodec(val mimeType: String, val streamType: StreamType, val label: String) {
    H264(MediaFormat.MIMETYPE_VIDEO_AVC, StreamType.VideoH264, "H.264"),
    H265(MediaFormat.MIMETYPE_VIDEO_HEVC, StreamType.VideoH265, "HEVC"),
    AV1("video/av01", StreamType.VideoAv1, "AV1"),
}

data class VideoEncoderConfig(
    val width: Int = 1920,
    val height: Int = 1080,
    val fps: Int = 60,
    val bitrateKbps: Int = 14_000,
    val codec: AndroidVideoCodec = AndroidVideoCodec.H264,
)

/**
 * Hardware-accelerated zero-copy `MediaCodec` Surface encoder targeting
 * sub-frame latency (`KEY_LATENCY = 0`, 1-second GOP, instant on-demand IDR)
 * with 8 KiB wire packet fragmentation so voice packets never stall behind
 * 4K keyframes.
 */
class HardwareVideoEncoder(
    private val config: VideoEncoderConfig,
    private val onPacket: (WirePacket) -> Unit,
) {
    private var codec: MediaCodec? = null
    private var inputSurface: Surface? = null
    private val running = AtomicBoolean(false)
    private val paused = AtomicBoolean(false)
    private val seq = AtomicInteger(0)
    private var drainThread: Thread? = null

    fun start(): Surface {
        val format = MediaFormat.createVideoFormat(config.codec.mimeType, config.width, config.height).apply {
            setInteger(
                MediaFormat.KEY_COLOR_FORMAT,
                MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface,
            )
            setInteger(MediaFormat.KEY_BIT_RATE, config.bitrateKbps * 1000)
            setInteger(MediaFormat.KEY_FRAME_RATE, config.fps)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
            setInteger(MediaFormat.KEY_PRIORITY, 0) // Real-time priority
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                setInteger(MediaFormat.KEY_LATENCY, 0)
            }
        }

        val enc = MediaCodec.createEncoderByType(config.codec.mimeType)
        enc.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        val surface = enc.createInputSurface()
        enc.start()

        codec = enc
        inputSurface = surface
        running.set(true)
        drainThread = Thread(::drainLoop, "wificam-video-enc").apply { start() }
        return surface
    }

    fun requestKeyFrame() {
        val enc = codec ?: return
        val params = Bundle().apply {
            putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0)
        }
        runCatching { enc.setParameters(params) }
    }

    fun setPaused(isPaused: Boolean) {
        paused.set(isPaused)
        if (!isPaused) {
            requestKeyFrame()
        }
    }

    fun stop() {
        running.set(false)
        drainThread?.join(500)
        drainThread = null
        runCatching { codec?.stop() }
        runCatching { codec?.release() }
        runCatching { inputSurface?.release() }
        codec = null
        inputSurface = null
    }

    private fun drainLoop() {
        val enc = codec ?: return
        val info = MediaCodec.BufferInfo()
        while (running.get()) {
            val outIndex = runCatching { enc.dequeueOutputBuffer(info, 10_000L) }.getOrElse { -1 }
            if (outIndex >= 0) {
                val buf = enc.getOutputBuffer(outIndex)
                if (buf != null && info.size > 0 && !paused.get()) {
                    buf.position(info.offset)
                    buf.limit(info.offset + info.size)
                    val bytes = ByteArray(info.size)
                    buf.get(bytes)

                    var flags = PacketFlags.NONE
                    if ((info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME) != 0) {
                        flags = flags or PacketFlags.KEYFRAME
                    }
                    if ((info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG) != 0) {
                        flags = flags or PacketFlags.CONFIG
                    }

                    val startSeq = seq.get().toUInt()
                    val (frags, nextSeq) = PacketCodec.fragmentVideo(
                        streamType = config.codec.streamType,
                        baseFlags = flags,
                        startSeq = startSeq,
                        ptsUs = info.presentationTimeUs,
                        frame = bytes,
                    )
                    seq.set(nextSeq.toInt())
                    for (pkt in frags) {
                        onPacket(pkt)
                    }
                }
                runCatching { enc.releaseOutputBuffer(outIndex, false) }
            }
        }
    }
}
