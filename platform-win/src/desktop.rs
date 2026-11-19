use std::{io, ptr};

use windows_sys::{
    Win32::{
        Foundation::{GetLastError, HWND, LPARAM, POINT, RECT, SetLastError},
        Graphics::{
            Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
            Gdi::ScreenToClient,
        },
        UI::WindowsAndMessaging::*,
    },
    core::PCWSTR,
    w,
};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

const PROGMAN: PCWSTR = w!("Progman");
const WORKER: PCWSTR = w!("WorkerW");
const ICONS: PCWSTR = w!("SHELLDLL_DefView");

pub struct Desktop {
    window: HWND,
    parent: HWND,
    above: HWND,
}

impl Desktop {
    pub fn attach(window: &Window) -> core_engine::Result<Self> {
        let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
            return Err("Expected a Win32 window".into());
        };
        let window = handle.hwnd.get() as HWND;
        let (parent, above) = wallpaper_layer()?;
        // The HWND belongs to winit. Explorer's window handles are borrowed for
        // the lifetime of this attachment and must never be destroyed by us.
        unsafe {
            let style = GetWindowLongPtrW(window, GWL_STYLE);
            set_style(
                window,
                GWL_STYLE,
                (style & !(WS_POPUP as isize)) | WS_CHILD as isize,
            )?;
            let style = GetWindowLongPtrW(window, GWL_EXSTYLE);
            let mut flags = WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW;
            if above != HWND_BOTTOM {
                flags |= WS_EX_LAYERED;
            }
            set_style(
                window,
                GWL_EXSTYLE,
                (style & !(WS_EX_APPWINDOW as isize)) | flags as isize,
            )?;
            if above != HWND_BOTTOM && SetLayeredWindowAttributes(window, 0, 255, LWA_ALPHA) == 0 {
                return Err(io::Error::last_os_error().into());
            }
            SetLastError(0);
            if SetParent(window, parent).is_null() && GetLastError() != 0 {
                return Err(io::Error::last_os_error().into());
            }
        }
        let desktop = Self {
            window,
            parent,
            above,
        };
        desktop.fit(primary_bounds())?;
        Ok(desktop)
    }

    pub fn fit(&self, bounds: RECT) -> io::Result<()> {
        let mut origin = POINT {
            x: bounds.left,
            y: bounds.top,
        };
        unsafe {
            if ScreenToClient(self.parent, &mut origin) == 0
                || SetWindowPos(
                    self.window,
                    self.above,
                    origin.x,
                    origin.y,
                    bounds.right - bounds.left,
                    bounds.bottom - bounds.top,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_FRAMECHANGED,
                ) == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub fn is_attached(&self) -> bool {
        unsafe { IsWindow(self.window) != 0 && GetParent(self.window) == self.parent }
    }

    pub fn covered(&self, monitor: RECT) -> bool {
        unsafe {
            let foreground = GetForegroundWindow();
            if foreground.is_null()
                || foreground == self.window
                || foreground == GetDesktopWindow()
                || foreground == GetShellWindow()
                || !child(foreground, ICONS).is_null()
                || IsIconic(foreground) != 0
            {
                return false;
            }
            let mut bounds = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let status = DwmGetWindowAttribute(
                foreground,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                (&mut bounds as *mut RECT).cast(),
                std::mem::size_of::<RECT>() as u32,
            );
            (status >= 0 || GetWindowRect(foreground, &mut bounds) != 0) && covers(bounds, monitor)
        }
    }
}

pub fn primary_bounds() -> RECT {
    unsafe {
        RECT {
            left: 0,
            top: 0,
            right: GetSystemMetrics(SM_CXSCREEN),
            bottom: GetSystemMetrics(SM_CYSCREEN),
        }
    }
}

fn covers(window: RECT, monitor: RECT) -> bool {
    window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

fn wallpaper_layer() -> io::Result<(HWND, HWND)> {
    unsafe {
        let progman = FindWindowW(PROGMAN, ptr::null());
        if progman.is_null() {
            return Err(io::Error::other("Explorer's Progman window was not found"));
        }

        // Explorer's undocumented message requests a wallpaper WorkerW.
        SendMessageTimeoutW(
            progman,
            0x052C,
            0xD,
            1,
            SMTO_ABORTIFHUNG,
            1_000,
            ptr::null_mut(),
        );

        let icons = child(progman, ICONS);
        // Raised desktops put the icon and wallpaper windows inside Progman.
        if !icons.is_null() && !child(progman, WORKER).is_null() {
            return Ok((progman, icons));
        }

        let mut worker: HWND = ptr::null_mut();
        EnumWindows(Some(find_worker), (&mut worker as *mut HWND) as LPARAM);
        if worker.is_null() {
            return Err(io::Error::other("Explorer's wallpaper layer was not found"));
        }
        Ok((worker, HWND_BOTTOM))
    }
}

unsafe extern "system" fn find_worker(hwnd: HWND, context: LPARAM) -> i32 {
    unsafe {
        if !child(hwnd, ICONS).is_null() {
            let worker = FindWindowExW(ptr::null_mut(), hwnd, WORKER, ptr::null());
            if !worker.is_null() && child(worker, ICONS).is_null() {
                *(context as *mut HWND) = worker;
                return 0;
            }
        }
    }
    1
}

unsafe fn child(parent: HWND, class: PCWSTR) -> HWND {
    unsafe { FindWindowExW(parent, ptr::null_mut(), class, ptr::null()) }
}

unsafe fn set_style(window: HWND, index: i32, style: isize) -> io::Result<()> {
    unsafe {
        SetLastError(0);
        if SetWindowLongPtrW(window, index, style) == 0 && GetLastError() != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_must_cover_the_entire_monitor() {
        let monitor = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert!(covers(monitor, monitor));
        assert!(!covers(
            RECT {
                bottom: 1040,
                ..monitor
            },
            monitor
        ));
        assert!(!covers(
            RECT {
                left: 0,
                right: 1920,
                ..monitor
            },
            monitor
        ));
    }
}
