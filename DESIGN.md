---
version: alpha
name: WifiCam Studio
description: Hardware-instrument precision meets true 0-nit OLED efficiency for Android Compose and Windows 11 WinUI 3.
colors:
  oled-black: "#000000"
  surface-base: "#0D0F12"
  surface-elevated: "#161A20"
  surface-card: "#1D222B"
  border-subtle: "#2B3240"
  ink-primary: "#F8FAFC"
  ink-secondary: "#9CA8BA"
  ink-on-accent: "#061018"
  accent-primary: "#38BDF8"
  accent-primary-hover: "#7DD3FC"
  status-live: "#22C55E"
  status-warn: "#F59E0B"
  status-alert: "#F43F5E"
  vu-nominal: "#22C55E"
  vu-hot: "#F59E0B"
  vu-clip: "#EF4444"
typography:
  display:
    fontFamily: Inter
    fontSize: 1.75rem
    fontWeight: 700
    lineHeight: 1.15
    letterSpacing: "-0.02em"
  heading:
    fontFamily: Inter
    fontSize: 1.125rem
    fontWeight: 600
    lineHeight: 1.25
    letterSpacing: "-0.01em"
  body:
    fontFamily: Inter
    fontSize: 0.9375rem
    fontWeight: 400
    lineHeight: 1.45
  label:
    fontFamily: Inter
    fontSize: 0.75rem
    fontWeight: 600
    lineHeight: 1.2
    letterSpacing: "0.04em"
  telemetry:
    fontFamily: JetBrains Mono
    fontSize: 0.875rem
    fontWeight: 600
    lineHeight: 1.2
    fontFeature: "'tnum' 1"
rounded:
  sm: 8px
  md: 12px
  lg: 18px
  pill: 999px
spacing:
  xs: 4px
  sm: 8px
  md: 16px
  lg: 24px
  xl: 32px
components:
  button-primary:
    backgroundColor: "{colors.accent-primary}"
    textColor: "{colors.ink-on-accent}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: 14px
  button-primary-hover:
    backgroundColor: "{colors.accent-primary-hover}"
    textColor: "{colors.ink-on-accent}"
    rounded: "{rounded.md}"
    padding: 14px
  button-danger:
    backgroundColor: "{colors.status-alert}"
    textColor: "{colors.ink-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: 14px
  card-control:
    backgroundColor: "{colors.surface-elevated}"
    textColor: "{colors.ink-primary}"
    rounded: "{rounded.lg}"
    padding: 16px
  pill-live:
    backgroundColor: "{colors.surface-card}"
    textColor: "{colors.status-live}"
    typography: "{typography.label}"
    rounded: "{rounded.pill}"
    padding: 8px
  pill-telemetry:
    backgroundColor: "{colors.surface-card}"
    textColor: "{colors.ink-secondary}"
    typography: "{typography.telemetry}"
    rounded: "{rounded.sm}"
    padding: 8px
  curtain-oled:
    backgroundColor: "{colors.oled-black}"
    textColor: "{colors.ink-secondary}"
    typography: "{typography.telemetry}"
    padding: 24px
---

## Overview

WifiCam Studio is a precision broadcast-instrument design system shared across the Android Jetpack Compose companion app and the Windows 11 WinUI 3 desktop controller. Every visual decision prioritizes instant legibility during live calls, low thermal output on OLED phones, and zero visual clutter.

## Colors

- **OLED Black (`#000000`)**: True 0-nit canvas for the Android viewfinder backdrop and OLED Curtain battery-saver mode, cutting display power draw and reducing phone temperature by 8-12 C on long 4K/60 fps sessions.
- **Carbon Surfaces (`#0D0F12`, `#161A20`, `#1D222B`)**: Layered dark surfaces on Android and WinUI 3 (paired with Windows 11 Mica backdrop on desktop) with crisp `1px` `#2B3240` hairlines instead of heavy drop shadows.
- **Studio Cyan (`#38BDF8`)**: Sole primary action accent (Connect, Start Stream, Active Toggle). High contrast against `#061018` ink (exceeds WCAG AAA).
- **Signal Semantics**: Emerald (`#22C55E`) strictly means live/connected; Amber (`#F59E0B`) means paused/caution; Coral (`#F43F5E`) means muted/disconnect/error.
- **Broadcast 16-Segment VU Meter**: Segments 1-10 use `#22C55E` (-60 to -12 dBFS), segments 11-14 use `#F59E0B` (-12 to -3 dBFS), and segments 15-16 use `#EF4444` (-3 to 0 dBFS peak clip).

## Typography

- **UI Sans (`Inter` / Android `Roboto` / Win11 `Segoe UI Variable`)**: Clean geometric readability for controls, lens chips, and DSP toggles.
- **Tabular Monospace (`JetBrains Mono` / `Cascadia Mono`, `tnum` enabled)**: Mandatory for all live numbers (`60.0 FPS`, `18 ms`, `48 kHz`, `+12 ppm`, `37 C`) so values never jitter horizontally during streaming.

## Layout

- **Android (Camera-First Cockpit)**: Full-bleed edge-to-edge 16:9 / sensor preview with floating top status strip (Transport Tier badge, Bitrate/FPS/Temp telemetry, OLED Curtain toggle) and a thumb-reachable bottom control dock (Lens switcher `0.5x / 1x / 2x / Front`, primary Stream CTA, Video Pause, Mic Mute, Settings sheet).
- **Windows 11 WinUI 3 (Compact Studio Console)**: Two-column Mica window (left: live camera preview + quick-action bar + 16-segment LED VU meter; right: 4-tier transport selector, QR pairing card, AI Denoiser selector `Off / RNNoise / DeepFilterNet3`, WebRTC AEC3 toggle, Virtual Cam/Mic status).

## Elevation & Depth

Prefer subtle tonal surface stepping (`#0D0F12` -> `#161A20` -> `#1D222B`) plus `1px` `#2B3240` borders over heavy blur or drop shadows. On Windows 11 WinUI 3, let the native Mica backdrop shine through the root shell.

## Shapes

- **Control Cards**: `18px` radius (`rounded.lg`).
- **Buttons & Inputs**: `12px` radius (`rounded.md`).
- **Telemetry Chips**: `8px` radius (`rounded.sm`).
- **Status Pills & Lens Chips**: Full pill (`999px`).

## Components

- **`button-primary`**: Single clear call-to-action per view (Start Streaming / Pair Device).
- **`button-danger`**: Stop Streaming or Revoke Trusted Device.
- **`pill-live`**: Displays active transport tier (`USB AOA`, `ADB`, `WI-FI`, `BT`) with pulsing emerald dot.
- **`pill-telemetry`**: Monospace chip displaying live FPS, latency, temperature, or audio skew.
- **`curtain-oled`**: Full-screen `#000000` overlay with subtle drifting minimal clock & "Double-tap to wake" hint to prevent AMOLED burn-in during long streams.

## Do's and Don'ts

- **Do** use tabular monospace (`tnum`) for every changing metric.
- **Do** keep OLED Curtain mode 100% pure `#000000` with minimal dim text drifting gently every 60 seconds.
- **Do** require an explicit two-button consent sheet on Android before transmitting live camera or microphone frames to a newly paired PC.
- **Don't** place bright white cards next to a live camera preview or show a frozen/black frame when paused (always show a clear status badge/slate).
- **Don't** use raw red/green text without icons or explicit labels (maintain color-blind and WCAG AA legibility).
