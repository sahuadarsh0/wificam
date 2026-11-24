#include <jni.h>
#include <oboe/Oboe.h>
#include <android/log.h>

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <memory>
#include <mutex>
#include <vector>

namespace {

constexpr const char* kTag = "WifiCamOboe";
constexpr int32_t kSampleRate = 48000;
constexpr int32_t kChannelCount = 1;
constexpr int32_t kFrameSamples10ms = 480;

class OboeCaptureEngine final : public oboe::AudioStreamDataCallback,
                                public oboe::AudioStreamErrorCallback {
public:
    OboeCaptureEngine(JavaVM* jvm, jobject callback_global_ref, jmethodID on_pcm_mid)
        : jvm_(jvm), callback_ref_(callback_global_ref), on_pcm_mid_(on_pcm_mid) {
        accum_.reserve(kFrameSamples10ms * 2);
    }

    ~OboeCaptureEngine() override {
        Stop();
        if (callback_ref_ && jvm_) {
            JNIEnv* env = nullptr;
            bool attached = false;
            if (jvm_->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) == JNI_EDETACHED) {
                if (jvm_->AttachCurrentThread(&env, nullptr) == JNI_OK) {
                    attached = true;
                }
            }
            if (env) {
                env->DeleteGlobalRef(callback_ref_);
            }
            if (attached) {
                jvm_->DetachCurrentThread();
            }
            callback_ref_ = nullptr;
        }
    }

    bool Start(bool unprocessed_preset) {
        std::lock_guard lock(stream_lock_);
        if (stream_) {
            return true;
        }

        oboe::AudioStreamBuilder builder;
        builder.setDirection(oboe::Direction::Input)
            ->setPerformanceMode(oboe::PerformanceMode::LowLatency)
            ->setSharingMode(oboe::SharingMode::Exclusive)
            ->setFormat(oboe::AudioFormat::I16)
            ->setChannelCount(kChannelCount)
            ->setSampleRate(kSampleRate)
            ->setInputPreset(unprocessed_preset
                                 ? oboe::InputPreset::Unprocessed
                                 : oboe::InputPreset::VoiceCommunication)
            ->setDataCallback(this)
            ->setErrorCallback(this);

        oboe::Result res = builder.openStream(stream_);
        if (res != oboe::Result::OK || !stream_) {
            __android_log_print(
                ANDROID_LOG_WARN,
                kTag,
                "Exclusive low-latency open failed (%s), retrying Shared mode",
                oboe::convertToText(res));
            builder.setSharingMode(oboe::SharingMode::Shared);
            res = builder.openStream(stream_);
            if (res != oboe::Result::OK || !stream_) {
                __android_log_print(
                    ANDROID_LOG_ERROR,
                    kTag,
                    "Failed to open Oboe input stream: %s",
                    oboe::convertToText(res));
                return false;
            }
        }

        running_.store(true, std::memory_order_release);
        res = stream_->requestStart();
        if (res != oboe::Result::OK) {
            __android_log_print(
                ANDROID_LOG_ERROR,
                kTag,
                "Oboe requestStart failed: %s",
                oboe::convertToText(res));
            stream_->close();
            stream_.reset();
            running_.store(false, std::memory_order_release);
            return false;
        }
        return true;
    }

    void Stop() {
        running_.store(false, std::memory_order_release);
        std::lock_guard lock(stream_lock_);
        if (stream_) {
            stream_->requestStop();
            stream_->close();
            stream_.reset();
        }
    }

    void SetMuted(bool muted) {
        muted_.store(muted, std::memory_order_relaxed);
    }

    int32_t RmsDbfsX10() const {
        return rms_dbfs_x10_.load(std::memory_order_relaxed);
    }

    oboe::DataCallbackResult onAudioReady(
        oboe::AudioStream* /*stream*/,
        void* audio_data,
        int32_t num_frames) override {
        if (!running_.load(std::memory_order_acquire) || !audio_data || num_frames <= 0) {
            return oboe::DataCallbackResult::Continue;
        }

        const auto* samples = static_cast<const int16_t*>(audio_data);
        const bool is_muted = muted_.load(std::memory_order_relaxed);

        for (int32_t i = 0; i < num_frames; ++i) {
            accum_.push_back(is_muted ? int16_t{0} : samples[i]);
            if (static_cast<int32_t>(accum_.size()) >= kFrameSamples10ms) {
                EmitFrame10ms(accum_.data());
                accum_.erase(accum_.begin(), accum_.begin() + kFrameSamples10ms);
            }
        }
        return oboe::DataCallbackResult::Continue;
    }

    void onErrorAfterClose(oboe::AudioStream* /*stream*/, oboe::Result error) override {
        __android_log_print(
            ANDROID_LOG_WARN,
            kTag,
            "Oboe stream disconnected (%s); waiting for host restart",
            oboe::convertToText(error));
    }

private:
    void EmitFrame10ms(const int16_t* frame) {
        double sum_sq = 0.0;
        for (int i = 0; i < kFrameSamples10ms; ++i) {
            const double norm = static_cast<double>(frame[i]) / 32768.0;
            sum_sq += norm * norm;
        }
        const double rms = std::sqrt(sum_sq / kFrameSamples10ms);
        const int32_t db_x10 = (rms <= 1e-5)
            ? -960
            : static_cast<int32_t>(std::clamp(20.0 * std::log10(rms) * 10.0, -960.0, 0.0));
        rms_dbfs_x10_.store(db_x10, std::memory_order_relaxed);

        if (!jvm_ || !callback_ref_ || !on_pcm_mid_) {
            return;
        }

        JNIEnv* env = nullptr;
        bool attached = false;
        const jint get_res = jvm_->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6);
        if (get_res == JNI_EDETACHED) {
            if (jvm_->AttachCurrentThread(&env, nullptr) != JNI_OK || !env) {
                return;
            }
            attached = true;
        } else if (get_res != JNI_OK || !env) {
            return;
        }

        const jsize byte_len = kFrameSamples10ms * 2;
        jbyteArray arr = env->NewByteArray(byte_len);
        if (arr) {
            env->SetByteArrayRegion(
                arr, 0, byte_len, reinterpret_cast<const jbyte*>(frame));
            env->CallVoidMethod(callback_ref_, on_pcm_mid_, arr, db_x10);
            env->DeleteLocalRef(arr);
        }

        if (attached) {
            jvm_->DetachCurrentThread();
        }
    }

    JavaVM* jvm_ = nullptr;
    jobject callback_ref_ = nullptr;
    jmethodID on_pcm_mid_ = nullptr;

    std::mutex stream_lock_;
    std::shared_ptr<oboe::AudioStream> stream_;
    std::vector<int16_t> accum_;
    std::atomic<bool> running_{false};
    std::atomic<bool> muted_{false};
    std::atomic<int32_t> rms_dbfs_x10_{-960};
};

} // namespace

extern "C" {

JNIEXPORT jlong JNICALL
Java_com_wificam_android_audio_OboeAudioEngine_nativeCreate(
    JNIEnv* env,
    jobject thiz) {
    JavaVM* jvm = nullptr;
    if (env->GetJavaVM(&jvm) != JNI_OK || !jvm) {
        return 0;
    }
    jclass cls = env->GetObjectClass(thiz);
    jmethodID mid = env->GetMethodID(cls, "onNativePcmFrame", "([BI)V");
    if (!mid) {
        return 0;
    }
    jobject global_ref = env->NewGlobalRef(thiz);
    auto* engine = new OboeCaptureEngine(jvm, global_ref, mid);
    return reinterpret_cast<jlong>(engine);
}

JNIEXPORT jboolean JNICALL
Java_com_wificam_android_audio_OboeAudioEngine_nativeStart(
    JNIEnv* /*env*/,
    jobject /*thiz*/,
    jlong handle,
    jboolean unprocessed_preset) {
    auto* engine = reinterpret_cast<OboeCaptureEngine*>(handle);
    if (!engine) return JNI_FALSE;
    return engine->Start(unprocessed_preset == JNI_TRUE) ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT void JNICALL
Java_com_wificam_android_audio_OboeAudioEngine_nativeStop(
    JNIEnv* /*env*/,
    jobject /*thiz*/,
    jlong handle) {
    auto* engine = reinterpret_cast<OboeCaptureEngine*>(handle);
    if (engine) engine->Stop();
}

JNIEXPORT void JNICALL
Java_com_wificam_android_audio_OboeAudioEngine_nativeSetMuted(
    JNIEnv* /*env*/,
    jobject /*thiz*/,
    jlong handle,
    jboolean muted) {
    auto* engine = reinterpret_cast<OboeCaptureEngine*>(handle);
    if (engine) engine->SetMuted(muted == JNI_TRUE);
}

JNIEXPORT void JNICALL
Java_com_wificam_android_audio_OboeAudioEngine_nativeDestroy(
    JNIEnv* /*env*/,
    jobject /*thiz*/,
    jlong handle) {
    auto* engine = reinterpret_cast<OboeCaptureEngine*>(handle);
    delete engine;
}

} // extern "C"
