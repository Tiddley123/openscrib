//! OS-specific window lookups. Everything is in physical pixels, which needs
//! per-monitor DPI awareness set before any hook or capture starts.

use openscrib_core::{Point, WindowInfo};

#[cfg(windows)]
pub use self::windows_impl::*;

#[cfg(not(windows))]
mod fallback {
    use super::*;

    pub fn init_dpi_awareness() {}

    pub fn window_at(_p: Point) -> Option<WindowInfo> {
        None
    }

    pub fn foreground_window() -> Option<WindowInfo> {
        None
    }

    pub fn cursor_pos() -> Option<Point> {
        None
    }
}

#[cfg(not(windows))]
pub use self::fallback::*;

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use openscrib_core::Rect;
    use windows::Win32::Foundation::{CloseHandle, HWND, POINT, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowRect,
        GetWindowTextW, GetWindowThreadProcessId, WindowFromPoint,
    };
    use windows::core::PWSTR;

    pub fn init_dpi_awareness() {
        // Fails harmlessly if a manifest already set it.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }

    /// The top-level window under a screen point (the click target).
    pub fn window_at(p: Point) -> Option<WindowInfo> {
        let hwnd = unsafe { WindowFromPoint(POINT { x: p.x, y: p.y }) };
        if hwnd.is_invalid() {
            return None;
        }
        let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
        describe(if root.is_invalid() { hwnd } else { root })
    }

    pub fn foreground_window() -> Option<WindowInfo> {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_invalid() { None } else { describe(hwnd) }
    }

    pub fn cursor_pos() -> Option<Point> {
        let mut p = POINT::default();
        unsafe { GetCursorPos(&mut p).ok()? };
        Some(Point::new(p.x, p.y))
    }

    fn describe(hwnd: HWND) -> Option<WindowInfo> {
        Some(WindowInfo {
            rect: frame_bounds(hwnd)?,
            client: client_bounds(hwnd),
            title: title(hwnd),
            process: process_name(hwnd).unwrap_or_default(),
        })
    }

    fn to_rect(r: RECT) -> Rect {
        Rect {
            x: r.left,
            y: r.top,
            w: (r.right - r.left).max(0) as u32,
            h: (r.bottom - r.top).max(0) as u32,
        }
    }

    /// The visible frame, excluding the invisible resize borders that
    /// `GetWindowRect` includes on Windows 10/11.
    fn frame_bounds(hwnd: HWND) -> Option<Rect> {
        let mut r = RECT::default();
        let ok = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut r as *mut RECT as *mut _,
                size_of::<RECT>() as u32,
            )
        };
        if ok.is_err() {
            unsafe { GetWindowRect(hwnd, &mut r).ok()? };
        }
        Some(to_rect(r))
    }

    fn client_bounds(hwnd: HWND) -> Option<Rect> {
        let mut r = RECT::default();
        unsafe { GetClientRect(hwnd, &mut r).ok()? };
        let mut origin = POINT { x: 0, y: 0 };
        if !unsafe { ClientToScreen(hwnd, &mut origin) }.as_bool() {
            return None;
        }
        Some(Rect { x: origin.x, y: origin.y, w: r.right.max(0) as u32, h: r.bottom.max(0) as u32 })
    }

    fn title(hwnd: HWND) -> String {
        let mut buf = [0u16; 512];
        let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    fn process_name(hwnd: HWND) -> Option<String> {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == 0 {
            return None;
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let res = unsafe {
            QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
        };
        unsafe {
            let _ = CloseHandle(handle);
        }
        res.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        Some(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string())
    }
}
