//! NV12 status slate generator ("Waiting for Phone", "Camera Paused",
//! "Windows Locked") so virtual camera consumers (OBS, Teams, Zoom, Chrome)
//! always receive a clean, valid NV12 frame instead of a frozen or black image.

use super::shm::CameraState;

/// Exact byte length of a tightly-packed NV12 frame (`width * height * 3 / 2`).
pub const fn nv12_byte_len(width: u32, height: u32) -> usize {
    (width as usize) * (height as usize) * 3 / 2
}

/// Fill `dst` with a clean NV12 privacy/status slate for `state`.
///
/// Uses BT.709 limited-range YUV colors with a centered subtle status bar so
/// users can immediately tell whether the camera is disconnected, paused, or
/// locked by Windows `Win+L`.
pub fn render_nv12_slate(width: u32, height: u32, state: CameraState, dst: &mut [u8]) {
    let w = width as usize;
    let h = height as usize;
    let y_len = w * h;
    let uv_len = y_len / 2;
    assert!(
        dst.len() >= y_len + uv_len,
        "NV12 destination buffer too small"
    );

    let (bg_y, bg_u, bg_v, bar_y, bar_u, bar_v) = match state {
        CameraState::Disconnected => (24u8, 138u8, 120u8, 72u8, 160u8, 112u8), // Deep slate blue
        CameraState::Paused => (28u8, 118u8, 138u8, 130u8, 92u8, 168u8),       // Warm amber-slate
        CameraState::Locked => (18u8, 128u8, 128u8, 46u8, 128u8, 128u8),       // Privacy charcoal
        CameraState::Streaming => (32u8, 128u8, 128u8, 96u8, 128u8, 128u8),
    };

    let (y_plane, uv_plane) = dst[..y_len + uv_len].split_at_mut(y_len);
    y_plane.fill(bg_y);

    // Fill interleaved UV plane (U, V pairs).
    for pair in uv_plane.as_chunks_mut::<2>().0 {
        pair[0] = bg_u;
        pair[1] = bg_v;
    }

    // Draw a subtle centered horizontal status pill (30% width, 6% height).
    let bar_x0 = (w * 35 / 100) & !1;
    let bar_x1 = (w * 65 / 100) & !1;
    let bar_y0 = (h * 47 / 100) & !1;
    let bar_y1 = (h * 53 / 100) & !1;

    for row in bar_y0..bar_y1 {
        let start = row * w + bar_x0;
        let end = row * w + bar_x1;
        y_plane[start..end].fill(bar_y);
    }
    for uv_row in (bar_y0 / 2)..(bar_y1 / 2) {
        let row_base = uv_row * w;
        for x in (bar_x0..bar_x1).step_by(2) {
            uv_plane[row_base + x] = bar_u;
            uv_plane[row_base + x + 1] = bar_v;
        }
    }
}
