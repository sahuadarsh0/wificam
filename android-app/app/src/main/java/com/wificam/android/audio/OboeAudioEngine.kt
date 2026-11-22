package com.wificam.android.audio

import android.os.SystemClock
import com.wificam.android.protocol.PacketFlags
import com.wificam.android.protocol.StreamType
import com.wificam.android.protocol.WirePacket
import java.util.concurrent.atomic.AtomicInteger

/**
 * Low-latency 48 kHz mono audio capture engine backed by Google Oboe (AAudio
 * exclusive low-latency on API 27+, OpenSL ES fallback on API 26).
 *
 * Emits 10 ms (480-sample) PCM16 frames tagged with monotonic microsecond PTS
 * and live RMS dBFS telemetry for the 16-segment studio VU meter.
 */
class OboeAudioEngine(
    private val onAudioPacket: (WirePacket) -> Unit,
    private val onVuLevelUpdate: (rmsDbfsX10: Int) -> Unit,
) {
    private var nativeHandle: Long = 0L
    private val seqCounter = AtomicInteger(0)

    fun start(unprocessedMic: Boolean = false): Boolean {
        if (nativeHandle == 0L) {
            nativeHandle = nativeCreate()
        }
        if (nativeHandle == 0L) return false
        return nativeStart(nativeHandle, unprocessedMic)
    }

    fun setMuted(muted: Boolean) {
        if (nativeHandle != 0L) {
            nativeSetMuted(nativeHandle, muted)
        }
    }

    fun stop() {
        if (nativeHandle != 0L) {
            nativeStop(nativeHandle)
        }
    }

    fun release() {
        if (nativeHandle != 0L) {
            nativeDestroy(nativeHandle)
            nativeHandle = 0L
        }
    }

    // Invoked from the native Oboe audio thread every 10 ms (480 samples).
    @Suppress("unused")
    private fun onNativePcmFrame(pcm16LeBytes: ByteArray, rmsDbfsX10: Int) {
        onVuLevelUpdate(rmsDbfsX10)
        val seq = seqCounter.getAndIncrement().toUInt()
        val ptsUs = SystemClock.elapsedRealtimeNanos() / 1_000L
        onAudioPacket(
            WirePacket(
                streamType = StreamType.AudioPcm16,
                flags = PacketFlags.NONE,
                seq = seq,
                ptsUs = ptsUs,
                payload = pcm16LeBytes,
            )
        )
    }

    private external fun nativeCreate(): Long
    private external fun nativeStart(handle: Long, unprocessedPreset: Boolean): Boolean
    private external fun nativeStop(handle: Long)
    private external fun nativeSetMuted(handle: Long, muted: Boolean)
    private external fun nativeDestroy(handle: Long)

    companion object {
        init {
            System.loadLibrary("wificam_audio")
        }
    }
}
