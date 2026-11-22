# WifiCam Studio *(working title)*

> **Turn your Android phone into a high-quality Windows 11/10 webcam & studio microphone — 1080p60 / 4K30 hardware H.264/HEVC/AV1 video, 48 kHz studio audio with RNNoise / DeepFilterNet3 AI denoising & WebRTC AEC3 echo cancellation, fast wireless QR pairing, and 4-tier USB/Wi-Fi/Bluetooth failover.**
>
> **100% open source · Privacy-first · Zero cloud · Zero accounts · Zero watermarks · Zero telemetry.**

---

## Why WifiCam?

| Capability | WifiCam Studio | Typical Proprietary Alternatives |
|---|---|---|
| **Video Resolution & Codecs** | 1080p60 / 4K30 hardware **H.264 / HEVC / AV1** (`MediaCodec` Surface -> Windows Media Foundation) | Paywalled 720p/1080p, watermarks |
| **Virtual Camera Driver** | Native **Windows 11 `MFCreateVirtualCamera`** (works in Windows Camera, Teams, Zoom, OBS, Chrome, Discord) + Windows 10 DirectShow fallback | Legacy DirectShow only (fails in UWP/Store apps) |
| **Audio Pipeline** | **48 kHz** Oboe AAudio exclusive capture -> raw PCM16 over USB / **Opus 1.6.1** with In-Band FEC & PLC over Wi-Fi -> **WebRTC AEC3** + **RNNoise** / DeepFilterNet3 -> PI clock-drift micro-resampler -> MMCSS Pro Audio WASAPI | Robotic audio, clipping, speaker echo, long-call clock drift |
| **Transports & Failover** | 4-tier: **USB AOA** (zero-setup plug-and-play) -> **USB `adb reverse`** -> **Wi-Fi LAN** (E2E encrypted) -> **Bluetooth Classic RFCOMM** audio failover | Single transport or manual IP entry |
| **Fast Wireless Pairing** | **Instant Local QR Code Scan** (`wificam://pair?...` via Camera2 + open-source ZXing Core, zero Play Services) + explicit TOFU consent + mDNS auto-discovery | Manual IP typing or cloud sign-in |
| **Security & Privacy** | **P-256 ECDH + HKDF-SHA256 + AES-256-GCM** with header AAD & 64-packet replay window; **Win+L** auto-pause/mute lock guard; **0-nit `#000000` OLED Curtain Mode** | Cleartext LAN streams or proprietary cloud relays |

---

## Architecture Overview

```text
Android App (Kotlin 2.2 + Jetpack Compose + C++20 Oboe JNI)
 ├── Camera2 (60fps, multi-lens, 50/60 Hz antibanding) ──► MediaCodec (H.264 / HEVC / AV1)
 ├── Camera2 Y-plane ──► ZXing Core QR Pairing Scanner (wificam://pair?...)
 ├── Oboe AAudio Exclusive (48 kHz mono 10 ms) ──► Raw PCM16 (USB) / Opus+FEC (Wi-Fi)
 └── 4-Tier Transport: USB AOA | USB ADB | Wi-Fi TCP+UDP (AES-256-GCM) | BT RFCOMM
                                   │
                                   ▼
Windows PC Core (Rust 1.100, Edition 2024 — pc-core + pc-core-ffi)
 ├── Security: P-256 ECDH, HKDF-SHA256, AES-256-GCM AEAD, TOFU Trust Store, QR URI Gen
 ├── Audio DSP: Adaptive Jitter Buffer -> Opus 1.6.1 -> WebRTC AEC3 -> RNNoise/DFN3 -> PI Drift Servo -> rtrb -> MMCSS WASAPI Virtual Mic
 ├── Video: Media Foundation Hardware NV12 Decoder -> Lock-Free Seqlock Shared Memory Framebus
 ├── Privacy: Win32 WTS_SESSION_LOCK (Win+L) Guard -> NV12 Privacy Slate + Mic Mute
 └── C-ABI DLL (pc_core_ffi.dll)
        ├──► WinUI 3 Desktop Studio Console (pc-ui — .NET 10, Windows App SDK 2.7, Mica, QRCoder)
        ├──► Windows 11 Media Foundation Virtual Camera COM DLL (pc-vcam-mf — C++23)
        └──► Windows 10 DirectShow Virtual Camera Fallback DLL (pc-vcam-ds — C++23)
```

---

## Repository Layout

- `DESIGN.md` — Shared WCAG-validated Google `DESIGN.md` token spec ("WifiCam Studio") used by both Android Compose and Windows 11 WinUI 3.
- `proto/PROTOCOL.md` & `proto/golden_vectors.json` — Wire Protocol v2 specification and cross-language Rust <-> Kotlin golden byte vectors.
- `pc-core/` — Rust 2024 real-time engine (protocol, P-256/AES-256-GCM security, audio DSP, Media Foundation video decoder, seqlock framebus, 4-tier transports, `StudioEngine`, `SessionLockMonitor`).
- `pc-core-ffi/` — Rust C-ABI `cdylib` (`pc_core_ffi.dll`) bridging `pc-core` into WinUI 3.
- `pc-vcam-mf/` — C++23 Windows 11 Media Foundation Virtual Camera COM source (`IMFMediaSourceEx` / `MFCreateVirtualCamera`).
- `pc-vcam-ds/` — C++23 Windows 10 DirectShow Virtual Camera fallback filter (`IBaseFilter` / `IAMStreamConfig`).
- `pc-vcam-common/` — Shared C/C++/Rust lock-free seqlock shared-memory framebus header (`framebus_shm.h`).
- `pc-ui/` — Native Windows 11 **WinUI 3** Studio Console (`.NET 10`, `Windows App SDK 2.7.0`, Mica backdrop, live `QRCoder` QR pairing code, 16-segment broadcast LED VU meter, DSP & privacy controls).
- `android-app/` — Android Studio Gradle 9.7 / Kotlin 2.2 / Jetpack Compose Material 3 app with C++20 Oboe JNI, Camera2 + MediaCodec H.264/HEVC/AV1, ZXing Core QR Scanner Mode, and 0-nit OLED Curtain Mode.
- `installer/` — Inno Setup 6 production installer (`wificam.iss`) and elevated developer registration script (`register-vcam.ps1`).

---

## Building from Source

### Prerequisites
- **Rust** 1.100+ (`rustup`)
- **Visual Studio Build Tools 2026** (MSVC v145 / Windows 11 SDK `10.0.26100.0`) + **CMake 4.x**
- **.NET 10 SDK** (for WinUI 3 `pc-ui`)
- **Android Studio / OpenJDK 25 + Android SDK 37 + NDK 28.2** (for `android-app`)

### 1. Rust Core + C-ABI DLL & Tests
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p pc-core-ffi
```

### 2. Virtual Camera COM DLLs (C++23)
```bash
cmake -S pc-vcam-mf -B pc-vcam-mf/build -G "Visual Studio 18 2026" -A x64
cmake --build pc-vcam-mf/build --config Release

cmake -S pc-vcam-ds -B pc-vcam-ds/build -G "Visual Studio 18 2026" -A x64
cmake --build pc-vcam-ds/build --config Release
```
Register locally for testing in an elevated PowerShell window:
```powershell
.\installer\register-vcam.ps1
```

### 3. Windows 11 WinUI 3 Desktop Studio App
```bash
dotnet build pc-ui/WifiCam.Desktop.csproj -c Release -p:Platform=x64
```
Run `pc-ui\bin\x64\Release\net10.0-windows10.0.26100.0\win-x64\WifiCam.Desktop.exe`.

### 4. Android App (APK + Cross-Language Parity & QR Tests)
```bash
cd android-app
./gradlew :app:testDebugUnitTest :app:assembleDebug
```
Install on a connected phone:
```bash
adb install -r android-app/app/build/outputs/apk/debug/app-debug.apk
```

---

## Wireless QR Pairing Flow

1. Launch **WifiCam Studio** on Windows — the right-hand **Fast Wireless QR Pairing** card displays a freshly generated `wificam://pair?...` QR code containing your PC's LAN IP, control/media ports, P-256 public key, and single-use pairing token.
2. Open **WifiCam** on Android and tap the **QR Scanner** icon in the top bar.
3. Point your phone at the PC screen — Camera2 Y-plane luma frames are decoded locally in under 10 ms via open-source ZXing Core (no Play Services, no internet).
4. Review the **Allow PC to connect?** TOFU consent sheet and tap **Allow & Connect** to begin encrypted LAN video & studio mic streaming immediately.

---

## License

MIT License — see [LICENSE](LICENSE).
