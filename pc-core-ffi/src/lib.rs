//! C-ABI FFI bridge (`pc_core_ffi.dll`) called by `pc-ui` (WinUI 3) via
//! .NET P/Invoke (`[LibraryImport("pc_core_ffi.dll")]`).
//!
//! All strings returned by this DLL are UTF-8 null-terminated C strings
//! allocated on the Rust heap and freed with [`wificam_free_string`].

use std::ffi::{CString, c_char};
use std::ptr;

use pc_core::audio::DenoiseMode;
use pc_core::session::StudioEngine;

/// Opaque handle representing a live [`StudioEngine`] instance.
pub struct WifiCamEngineHandle {
    engine: StudioEngine,
}

fn to_c_string_ptr(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// Allocate and initialize a new [`StudioEngine`]. Returns null on error.
#[unsafe(no_mangle)]
pub extern "C" fn wificam_engine_create() -> *mut WifiCamEngineHandle {
    match StudioEngine::new() {
        Ok(engine) => Box::into_raw(Box::new(WifiCamEngineHandle { engine })),
        Err(_) => ptr::null_mut(),
    }
}

/// Destroy a [`StudioEngine`] handle cleanly (stops threads & mDNS).
///
/// # Safety
/// `handle` must be a pointer previously returned by [`wificam_engine_create`]
/// and must not be used again after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wificam_engine_destroy(handle: *mut WifiCamEngineHandle) {
    if !handle.is_null() {
        let _ = unsafe { Box::from_raw(handle) };
    }
}

/// Start listening on TCP/UDP, mDNS advertisement, framebus, and WASAPI virtual mic.
/// Returns `0` on success, `-1` on failure.
///
/// # Safety
/// `handle` must be a valid pointer returned by [`wificam_engine_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wificam_engine_start(handle: *mut WifiCamEngineHandle) -> i32 {
    if handle.is_null() {
        return -1;
    }
    let h = unsafe { &mut *handle };
    if h.engine.start().is_ok() { 0 } else { -1 }
}

/// Stop active streaming and listeners.
///
/// # Safety
/// `handle` must be a valid pointer returned by [`wificam_engine_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wificam_engine_stop(handle: *mut WifiCamEngineHandle) {
    if !handle.is_null() {
        let h = unsafe { &mut *handle };
        h.engine.stop();
    }
}

/// Return a freshly allocated JSON string of [`pc_core::session::StudioTelemetry`].
/// Caller must free with [`wificam_free_string`].
///
/// # Safety
/// `handle` must be a valid pointer returned by [`wificam_engine_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wificam_get_telemetry_json(
    handle: *const WifiCamEngineHandle,
) -> *mut c_char {
    if handle.is_null() {
        return ptr::null_mut();
    }
    let h = unsafe { &*handle };
    let snap = h.engine.telemetry();
    match serde_json::to_string(&snap) {
        Ok(json) => to_c_string_ptr(json),
        Err(_) => ptr::null_mut(),
    }
}

/// Trigger `adb reverse` for the first online USB Android device.
/// Returns a UTF-8 status message (freed via [`wificam_free_string`]).
///
/// # Safety
/// `handle` must be a valid pointer returned by [`wificam_engine_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wificam_trigger_adb_reverse(
    handle: *const WifiCamEngineHandle,
) -> *mut c_char {
    if handle.is_null() {
        return ptr::null_mut();
    }
    let h = unsafe { &*handle };
    let msg = match h.engine.trigger_adb_reverse() {
        Ok(dev) => format!("OK: USB ADB tunnel active for {dev}"),
        Err(err) => format!("ERR: {err}"),
    };
    to_c_string_ptr(msg)
}

/// Toggle microphone mute atomically.
///
/// # Safety
/// `handle` must be a valid pointer returned by [`wificam_engine_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wificam_set_muted(handle: *const WifiCamEngineHandle, muted: bool) {
    if !handle.is_null() {
        unsafe { (*handle).engine.set_muted(muted) };
    }
}

/// Toggle video pause (displays clean NV12 "Camera Paused" slate).
///
/// # Safety
/// `handle` must be a valid pointer returned by [`wificam_engine_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wificam_set_video_paused(
    handle: *const WifiCamEngineHandle,
    paused: bool,
) {
    if !handle.is_null() {
        unsafe { (*handle).engine.set_video_paused(paused) };
    }
}

/// Notify engine that the Windows desktop session locked or unlocked (`Win+L`).
///
/// # Safety
/// `handle` must be a valid pointer returned by [`wificam_engine_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wificam_set_session_locked(
    handle: *const WifiCamEngineHandle,
    locked: bool,
) {
    if !handle.is_null() {
        unsafe { (*handle).engine.set_session_locked(locked) };
    }
}

/// Toggle WebRTC AEC3 echo cancellation atomically.
///
/// # Safety
/// `handle` must be a valid pointer returned by [`wificam_engine_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wificam_set_aec_enabled(
    handle: *const WifiCamEngineHandle,
    enabled: bool,
) {
    if !handle.is_null() {
        unsafe { (*handle).engine.set_aec_enabled(enabled) };
    }
}

/// Set AI noise suppression mode (`0 = Off`, `1 = RNNoise`, `2 = DeepFilterNet3`).
///
/// # Safety
/// `handle` must be a valid pointer returned by [`wificam_engine_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wificam_set_denoise_mode(handle: *const WifiCamEngineHandle, mode: u8) {
    if !handle.is_null() {
        unsafe {
            (*handle)
                .engine
                .set_denoise_mode(DenoiseMode::from_u8(mode))
        };
    }
}

/// Set microphone input gain in tenths of a dB (`-240..=240`).
///
/// # Safety
/// `handle` must be a valid pointer returned by [`wificam_engine_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wificam_set_gain_db_x10(
    handle: *const WifiCamEngineHandle,
    gain_db_x10: i16,
) {
    if !handle.is_null() {
        unsafe { (*handle).engine.set_gain_db_x10(gain_db_x10) };
    }
}

/// Free a UTF-8 C string previously returned by this DLL.
///
/// # Safety
/// `ptr` must be null or a pointer returned by one of this DLL's string functions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wificam_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        let _ = unsafe { CString::from_raw(ptr) };
    }
}
