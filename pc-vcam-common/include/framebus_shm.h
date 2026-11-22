#pragma once

// Shared C++23 / Rust binary contract for the lock-free triple-buffered
// virtual camera framebus (`Global\WifiCamVcamShm`).
//
// Both `pc-vcam-mf.dll` (Windows 11 Frame Server) and `pc-vcam-ds.dll`
// (Windows 10 DirectShow fallback) read frames written by Rust `pc-core`
// using an atomic seqlock — zero cross-process mutexes, zero deadlocks.

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>

namespace wificam::framebus {

inline constexpr uint32_t kMagic = 0x5643414Du; // "VCAM"
inline constexpr uint32_t kVersion = 1u;
inline constexpr uint32_t kSlotCount = 3u;

// Maximum 4K UHD (3840x2160) NV12 frame size: Y (3840*2160) + UV (3840*1080).
inline constexpr uint32_t kMaxWidth = 3840u;
inline constexpr uint32_t kMaxHeight = 2160u;
inline constexpr size_t kMaxSlotBytes =
    static_cast<size_t>(kMaxWidth) * static_cast<size_t>(kMaxHeight) * 3u / 2u;

inline constexpr wchar_t kGlobalShmName[] = L"Global\\WifiCamVcamShm";
inline constexpr wchar_t kLocalShmName[] = L"Local\\WifiCamVcamShm";
inline constexpr wchar_t kGlobalEventName[] = L"Global\\WifiCamVcamEvent";
inline constexpr wchar_t kLocalEventName[] = L"Local\\WifiCamVcamEvent";

enum class PixelFormat : uint32_t {
    Nv12 = 1u,
    Bgra32 = 2u,
};

enum class CameraState : uint32_t {
    Disconnected = 0u,
    Streaming = 1u,
    Paused = 2u,
    Locked = 3u,
};

// Exact 64-byte aligned header matching `pc-core/src/video/shm.rs::FramebusHeader`.
struct alignas(64) FramebusHeader {
    uint32_t magic;
    uint32_t version;
    std::atomic<uint64_t> seq;         // Even = stable, odd = write in progress
    std::atomic<uint32_t> active_slot; // 0..2
    std::atomic<uint32_t> state;       // CameraState
    uint32_t width;
    uint32_t height;
    uint32_t stride;
    uint32_t pixel_format;             // PixelFormat
    uint32_t fps_num;
    uint32_t fps_den;
    std::atomic<uint64_t> pts_us;
    uint32_t payload_bytes;
    uint32_t reserved;
};

static_assert(sizeof(FramebusHeader) == 64, "FramebusHeader must be exactly 64 bytes");
static_assert(alignof(FramebusHeader) == 64, "FramebusHeader must be 64-byte aligned");
static_assert(offsetof(FramebusHeader, seq) == 8);
static_assert(offsetof(FramebusHeader, active_slot) == 16);
static_assert(offsetof(FramebusHeader, state) == 20);
static_assert(offsetof(FramebusHeader, width) == 24);
static_assert(offsetof(FramebusHeader, pts_us) == 48);
static_assert(offsetof(FramebusHeader, payload_bytes) == 56);

inline constexpr size_t kTotalShmBytes =
    sizeof(FramebusHeader) + (kMaxSlotBytes * kSlotCount);

struct FrameSnapshotMeta {
    uint64_t seq = 0;
    CameraState state = CameraState::Disconnected;
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t stride = 0;
    PixelFormat pixel_format = PixelFormat::Nv12;
    uint32_t fps_num = 30;
    uint32_t fps_den = 1;
    uint64_t pts_us = 0;
    uint32_t payload_bytes = 0;
};

// Lock-free seqlock reader copy into `dst`. Returns true if a coherent,
// non-torn frame snapshot was read.
inline bool TryReadLatestFrame(
    const void* mapped_base,
    uint8_t* dst,
    size_t dst_capacity,
    FrameSnapshotMeta& out_meta) {
    if (!mapped_base || !dst) {
        return false;
    }
    const auto* hdr = reinterpret_cast<const FramebusHeader*>(mapped_base);
    if (hdr->magic != kMagic || hdr->version != kVersion) {
        return false;
    }

    const auto* base_bytes = reinterpret_cast<const uint8_t*>(mapped_base);

    for (int attempt = 0; attempt < 4; ++attempt) {
        const uint64_t s1 = hdr->seq.load(std::memory_order_acquire);
        if ((s1 & 1u) != 0u) {
            continue; // Writer currently flipping header/slot
        }

        const uint32_t slot = hdr->active_slot.load(std::memory_order_acquire);
        if (slot >= kSlotCount) {
            return false;
        }

        FrameSnapshotMeta meta{};
        meta.seq = s1;
        meta.state = static_cast<CameraState>(hdr->state.load(std::memory_order_acquire));
        meta.width = hdr->width;
        meta.height = hdr->height;
        meta.stride = hdr->stride;
        meta.pixel_format = static_cast<PixelFormat>(hdr->pixel_format);
        meta.fps_num = hdr->fps_num;
        meta.fps_den = hdr->fps_den ? hdr->fps_den : 1u;
        meta.pts_us = hdr->pts_us.load(std::memory_order_acquire);
        meta.payload_bytes = hdr->payload_bytes;

        if (meta.payload_bytes == 0 || meta.payload_bytes > kMaxSlotBytes ||
            meta.payload_bytes > dst_capacity) {
            return false;
        }

        const size_t offset = sizeof(FramebusHeader) + static_cast<size_t>(slot) * kMaxSlotBytes;
        std::memcpy(dst, base_bytes + offset, meta.payload_bytes);

        std::atomic_thread_fence(std::memory_order_acquire);
        const uint64_t s2 = hdr->seq.load(std::memory_order_acquire);
        if (s1 == s2) {
            out_meta = meta;
            return true;
        }
    }
    return false;
}

} // namespace wificam::framebus
