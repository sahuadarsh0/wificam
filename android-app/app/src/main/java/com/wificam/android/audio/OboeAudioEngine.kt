package com.wificam.android.audio

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.Process
import android.os.SystemClock
import com.wificam.android.protocol.PacketFlags
import com.wificam.android.protocol.StreamType
import com.wificam.android.protocol.WirePacket
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlin.math.log10
import kotlin.math.sqrt

/**
 * Low-latency 48 kHz mono audio capture engine backed by Google Oboe (AAudio
 * exclusive/shared low-latency), with an automatic Android [AudioRecord] fallback
 * so studio microphone audio is guaranteed on every Android device even when
 * Camera2 holds exclusive hardware resources.
 */
class OboeAudioEngine(
    private val onAudioPacket: (WirePacket) -> Unit,
    private val onVuLevelUpdate: (rmsDbfsX10: Int) -> Unit,
) {
    private var nativeHandle: Long = 0L
    private val seqCounter = AtomicInteger(0)
    private val mutedFlag = AtomicBoolean(false)

    private var fallbackRecord: AudioRecord? = null
    private var fallbackThread: Thread? = null
    private val fallbackRunning = AtomicBoolean(false)

    fun start(unprocessedMic: Boolean = false): Boolean {
        val oboeOk = runCatching {
            if (nativeHandle == 0L) {
                nativeHandle = nativeCreate()
            }
            if (nativeHandle != 0L) {
                nativeSetMuted(nativeHandle, mutedFlag.get())
                nativeStart(nativeHandle, unprocessedMic)
            } else {
                false
            }
        }.getOrDefault(false)

        if (oboeOk) {
            return true
        }
        return startAudioRecordFallback()
    }

    fun setMuted(muted: Boolean) {
        mutedFlag.set(muted)
        if (nativeHandle != 0L) {
            runCatching { nativeSetMuted(nativeHandle, muted) }
        }
    }

    fun stop() {
        stopAudioRecordFallback()
        if (nativeHandle != 0L) {
            runCatching { nativeStop(nativeHandle) }
        }
    }

    fun release() {
        stopAudioRecordFallback()
        if (nativeHandle != 0L) {
            runCatching { nativeDestroy(nativeHandle) }
            nativeHandle = 0L
        }
    }

    @SuppressLint("MissingPermission")
    private fun startAudioRecordFallback(): Boolean {
        stopAudioRecordFallback()
        val sampleRate = 48_000
        val channelCfg = AudioFormat.CHANNEL_IN_MONO
        val encoding = AudioFormat.ENCODING_PCM_16BIT
        val minBuf = AudioRecord.getMinBufferSize(sampleRate, channelCfg, encoding)
        if (minBuf <= 0) return false
        val bufBytes = maxOf(minBuf * 2, FRAME_SAMPLES_10MS * 2 * 4)

        val sources = intArrayOf(
            MediaRecorder.AudioSource.VOICE_COMMUNICATION,
            MediaRecorder.AudioSource.MIC,
            MediaRecorder.AudioSource.VOICE_RECOGNITION,
        )
        var rec: AudioRecord? = null
        for (src in sources) {
            val candidate = runCatching {
                AudioRecord(src, sampleRate, channelCfg, encoding, bufBytes)
            }.getOrNull()
            if (candidate != null && candidate.state == AudioRecord.STATE_INITIALIZED) {
                rec = candidate
                break
            }
            runCatching { candidate?.release() }
        }
        val activeRec = rec ?: return false
        runCatching { activeRec.startRecording() }.onFailure {
            runCatching { activeRec.release() }
            return false
        }

        fallbackRecord = activeRec
        fallbackRunning.set(true)
        fallbackThread = Thread({
            Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
            val shortBuf = ShortArray(FRAME_SAMPLES_10MS)
            val byteBuf = ByteArray(FRAME_SAMPLES_10MS * 2)
            val bb = ByteBuffer.wrap(byteBuf).order(ByteOrder.LITTLE_ENDIAN)

            while (fallbackRunning.get()) {
                val read = activeRec.read(shortBuf, 0, FRAME_SAMPLES_10MS)
                if (read == FRAME_SAMPLES_10MS) {
                    val isMuted = mutedFlag.get()
                    var sumSq = 0.0
                    bb.clear()
                    for (i in 0 until FRAME_SAMPLES_10MS) {
                        val s: Short = if (isMuted) 0 else shortBuf[i]
                        bb.putShort(s)
                        val norm = s.toDouble() / 32768.0
                        sumSq += norm * norm
                    }
                    val rms = sqrt(sumSq / FRAME_SAMPLES_10MS)
                    val dbX10 = if (rms <= 1e-5) {
                        -960
                    } else {
                        (20.0 * log10(rms) * 10.0).toInt().coerceIn(-960, 0)
                    }
                    onNativePcmFrame(byteBuf.copyOf(), dbX10)
                } else if (read < 0) {
                    break
                }
            }
        }, "wificam-audiorecord").apply { start() }
        return true
    }

    private fun stopAudioRecordFallback() {
        fallbackRunning.set(false)
        runCatching { fallbackRecord?.stop() }
        fallbackThread?.join(300)
        fallbackThread = null
        runCatching { fallbackRecord?.release() }
        fallbackRecord = null
    }

    // Invoked from the native Oboe audio thread (or AudioRecord fallback) every 10 ms (480 samples).
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
        private const val FRAME_SAMPLES_10MS = 480

        init {
            runCatching { System.loadLibrary("wificam_audio") }
        }
    }
}
