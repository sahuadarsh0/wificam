# WifiCam (Working Title)

Open-source, privacy-first, zero-cloud Android-to-PC webcam and studio microphone connector.

## Highlights
- **Video:** Hardware-encoded 1080p60 / 4K30 H.264 & HEVC -> Windows 11 Media Foundation Virtual Camera (`MFCreateVirtualCamera`) + Windows 10 DirectShow fallback.
- **Audio:** 48 kHz studio microphone (raw PCM over USB, Opus + In-Band FEC over Wi-Fi/Bluetooth), lock-free MMCSS Pro Audio pipeline, PI clock-drift micro-resampler, RNNoise (default) + optional DeepFilterNet3 AI noise suppression, and WebRTC AEC3.
- **Transports:** 4-tier automatic failover — USB AOA (plug-and-play, no ADB), USB `adb reverse`, Wi-Fi LAN (P-256 ECDH + AES-256-GCM + mDNS), and Bluetooth Classic RFCOMM audio failover.
- **Zero Cloud:** No accounts, no telemetry, no watermarks.

## Workspace Layout
- `pc-core/` — Rust core engine (protocol v2, security, audio DSP, video decoder, transports)
- `proto/` — Cross-language wire protocol specs and golden test vectors (Rust + Kotlin)
- `pc-vcam-mf/` — Windows 11 Media Foundation virtual camera COM DLL (Phase 3)
- `pc-vcam-ds/` — Windows 10 DirectShow virtual camera fallback DLL (Phase 3)
- `android-app/` — Kotlin + Jetpack Compose + Camera2/MediaCodec + Oboe/Opus app (Phase 5)
- `pc-ui/` — Native lightweight Windows tray & control UI (Phase 6)
