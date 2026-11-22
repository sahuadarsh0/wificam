//! Windows session lock/unlock privacy guard (`WM_WTSSESSION_CHANGE`).
//!
//! Spawns a lightweight hidden message-only window (`HWND_MESSAGE`) registered
//! with `WTSRegisterSessionNotification`. The instant the user locks Windows
//! (`Win+L`), the callback triggers so `StudioSession` switches the virtual
//! camera to the Privacy Locked NV12 slate and mutes microphone output.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

/// Handle to the running Windows session-lock monitor thread.
pub struct SessionLockMonitor {
    is_locked: Arc<AtomicBool>,
    #[cfg(windows)]
    hwnd_raw: std::sync::atomic::AtomicIsize,
    join: Option<JoinHandle<()>>,
}

impl SessionLockMonitor {
    /// True while the Windows desktop session is locked (`Win+L`).
    pub fn is_locked(&self) -> bool {
        self.is_locked.load(Ordering::Acquire)
    }

    pub fn locked_flag(&self) -> Arc<AtomicBool> {
        self.is_locked.clone()
    }

    /// Start monitoring Windows lock/unlock events, invoking `on_change(locked)`
    /// whenever the desktop session transitions.
    #[cfg(windows)]
    pub fn start<F>(on_change: F) -> Self
    where
        F: Fn(bool) + Send + Sync + 'static,
    {
        use std::sync::atomic::AtomicIsize;

        let is_locked = Arc::new(AtomicBool::new(false));
        let hwnd_raw = Arc::new(AtomicIsize::new(0));

        let locked_bg = is_locked.clone();
        let hwnd_bg = hwnd_raw.clone();
        let cb: Arc<dyn Fn(bool) + Send + Sync> = Arc::new(on_change);

        let join = thread::Builder::new()
            .name("wificam-session-lock".into())
            .spawn(move || {
                run_wts_message_loop(locked_bg, hwnd_bg, cb);
            })
            .ok();

        Self {
            is_locked,
            hwnd_raw: AtomicIsize::new(hwnd_raw.load(Ordering::Acquire)),
            join,
        }
    }

    #[cfg(not(windows))]
    pub fn start<F>(_on_change: F) -> Self
    where
        F: Fn(bool) + Send + Sync + 'static,
    {
        Self {
            is_locked: Arc::new(AtomicBool::new(false)),
            join: None,
        }
    }
}

impl Drop for SessionLockMonitor {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

            let raw = self.hwnd_raw.load(Ordering::Acquire);
            if raw != 0 {
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(raw as *mut std::ffi::c_void)),
                        WM_CLOSE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        }
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

#[cfg(windows)]
fn run_wts_message_loop(
    locked: Arc<AtomicBool>,
    hwnd_out: Arc<std::sync::atomic::AtomicIsize>,
    on_change: Arc<dyn Fn(bool) + Send + Sync>,
) {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::RemoteDesktop::{
        NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
        GetMessageW, GetWindowLongPtrW, HWND_MESSAGE, MSG, PostQuitMessage, RegisterClassW,
        SetWindowLongPtrW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_DESTROY,
        WM_WTSSESSION_CHANGE, WNDCLASSW, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
    };
    use windows::core::w;

    struct HookCtx {
        locked: Arc<AtomicBool>,
        on_change: Arc<dyn Fn(bool) + Send + Sync>,
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_WTSSESSION_CHANGE => {
                let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const HookCtx;
                if !ptr.is_null() {
                    let ctx = unsafe { &*ptr };
                    match wparam.0 as u32 {
                        WTS_SESSION_LOCK => {
                            ctx.locked.store(true, Ordering::Release);
                            (ctx.on_change)(true);
                        }
                        WTS_SESSION_UNLOCK => {
                            ctx.locked.store(false, Ordering::Release);
                            (ctx.on_change)(false);
                        }
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = unsafe { DestroyWindow(hwnd) };
                LRESULT(0)
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let class_name = w!("WifiCamSessionLockWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("WifiCamLockGuard"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(hinst.into()),
            None,
        ) else {
            return;
        };

        let ctx = Box::new(HookCtx { locked, on_change });
        let ctx_ptr = Box::into_raw(ctx);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx_ptr as isize);
        hwnd_out.store(hwnd.0 as isize, Ordering::Release);

        let registered = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION).is_ok();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        if registered {
            let _ = WTSUnRegisterSessionNotification(hwnd);
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(Box::from_raw(ctx_ptr));
    }
}
