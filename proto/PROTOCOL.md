# Binary Wire Protocol v2 Specification

Every packet transmitted over USB (AOA / `adb reverse`), Wi-Fi (UDP / TCP), or Bluetooth Classic RFCOMM uses the exact same 24-byte big-endian header followed by `payload_len` bytes.

## Header Layout (24 bytes, Big-Endian)

| Offset | Size | Field | Description |
|---|---|---|---|
| `0..2` | `u16` | `magic` | Fixed `0x434D` (`"CM"`) |
| `2` | `u8` | `version` | Fixed `2` |
| `3` | `u8` | `stream_type` | `1=VideoH264`, `2=VideoH265`, `3=AudioPcm16`, `4=AudioOpus`, `5=Control`, `6=Heartbeat` |
| `4..6` | `u16` | `flags` | Bitmask: `0x01=KEYFRAME`, `0x02=CONFIG`, `0x04=HAS_FEC`, `0x08=FRAG_START`, `0x10=FRAG_END`, `0x20=ENCRYPTED`, `0x40=CRC_PRESENT` |
| `6..10` | `u32` | `seq` | Per-stream monotonic sequence number |
| `10..18` | `u64` | `pts_us` | Capture timestamp in microseconds |
| `18..22` | `u32` | `payload_len` | Payload byte length (`<= 4 MiB`) |
| `22..24` | `u16` | `crc16` | `0x0000` unless `CRC_PRESENT` is set; CRC-16/CCITT-FALSE (`0x1021`, init `0xFFFF`) over `header[0..22] || payload` |

## Audio Priority & Video Fragmentation

Video access units exceeding 8 KiB are split into 8 KiB chunks (`FRAG_START` on chunk 0, `FRAG_END` on final chunk, shared `pts_us`, consecutive `seq`). Audio, Control, and Heartbeat packets preempt queued video chunks so 4K keyframes never stall 10 ms voice frames.

## Cross-Language Verification

Both Rust (`pc-core`) and Kotlin (`android-app`) tests load `proto/golden_vectors.json` and assert byte-for-byte encode/decode parity.
