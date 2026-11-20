use std::{io, ptr};

use windows_sys::{
    Win32::{
        Foundation::{GetLastError, HWND, LPARAM, POINT, RECT, SetLastError},
        Graphics::{
            Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
            Gdi::ScreenToClient,
        },
        System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS},
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
    layer: DesktopLayer,
}

#[derive(Clone, Copy)]
pub struct DesktopLayer {
    parent: HWND,
    above: HWND,
}

impl DesktopLayer {
    pub fn find() -> io::Result<Self> {
        let (parent, above) = wallpaper_layer()?;
        Ok(Self { parent, above })
    }
}

impl Desktop {
    pub fn attach(window: &Window, layer: DesktopLayer, bounds: RECT) -> core_engine::Result<Self> {
        let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
            return Err("Expected a Win32 window".into());
        };
        let window = handle.hwnd.get() as HWND;
        // The HWND belongs to winit. Explorer's window handles are borrowed for
        // the lifetime of this attachment and must never be destroyed by us.
        unsafe {
            let style = GetWindowLongPtrW(window, GWL_STYLE);
            set_style(
                window,
                GWL_STYLE,
                (style & !(WS_POPUP as isize)) | WS_CHILD as isize | WS_CLIPSIBLINGS as isize,
            )?;
            let style = GetWindowLongPtrW(window, GWL_EXSTYLE);
            let flags = WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_LAYERED;
            set_style(
                window,
                GWL_EXSTYLE,
                (style & !(WS_EX_APPWINDOW as isize)) | flags as isize,
            )?;
            if SetLayeredWindowAttributes(window, 0, 255, LWA_ALPHA) == 0 {
                return Err(io::Error::last_os_error().into());
            }
            SetLastError(0);
            if SetParent(window, layer.parent).is_null() && GetLastError() != 0 {
                return Err(io::Error::last_os_error().into());
            }
        }
        let desktop = Self { window, layer };
        desktop.fit(bounds)?;
        Ok(desktop)
    }

    pub fn fit(&self, bounds: RECT) -> io::Result<()> {
        let mut origin = POINT {
            x: bounds.left,
            y: bounds.top,
        };
        unsafe {
            if ScreenToClient(self.layer.parent, &mut origin) == 0
                || SetWindowPos(
                    self.window,
                    self.layer.above,
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
        unsafe { IsWindow(self.window) != 0 && GetParent(self.window) == self.layer.parent }
    }
}

pub fn covered_displays(monitors: &[RECT]) -> Vec<bool> {
    let foreground = unsafe { GetForegroundWindow() };
    let bounds = visible_bounds(foreground);
    let mut covered: Vec<_> = monitors
        .iter()
        .map(|&monitor| bounds.is_some_and(|window| covers(window, monitor)))
        .collect();
    // A borderless fullscreen window can stay visible on a secondary display
    // after an application on another monitor becomes the foreground window.
    if monitors.len() > 1 && covered.contains(&false) {
        let mut check = CoverageCheck {
            monitors,
            covered: &mut covered,
            foreground,
        };
        unsafe {
            EnumWindows(
                Some(check_window),
                (&mut check as *mut CoverageCheck<'_>) as LPARAM,
            )
        };
    }
    covered
}

struct CoverageCheck<'a> {
    monitors: &'a [RECT],
    covered: &'a mut [bool],
    foreground: HWND,
}

unsafe extern "system" fn check_window(window: HWND, context: LPARAM) -> i32 {
    let check = unsafe { &mut *(context as *mut CoverageCheck<'_>) };
    if window == check.foreground {
        return 1;
    }
    let style = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } as u32;
    let mut frame: RECT = unsafe { std::mem::zeroed() };
    if style & (WS_EX_TRANSPARENT | WS_EX_LAYERED) != 0
        || unsafe { GetWindowRect(window, &mut frame) } == 0
        || !check
            .monitors
            .iter()
            .zip(check.covered.iter())
            .any(|(&monitor, &covered)| !covered && covers(frame, monitor))
    {
        return 1;
    }
    if let Some(bounds) = visible_bounds(window) {
        for (&monitor, covered) in check.monitors.iter().zip(check.covered.iter_mut()) {
            *covered |= covers(bounds, monitor);
        }
    }
    i32::from(check.covered.contains(&false))
}

fn visible_bounds(window: HWND) -> Option<RECT> {
    let mut process = 0;
    let mut cloaked = 0u32;
    unsafe {
        if window.is_null()
            || IsWindowVisible(window) == 0
            || IsIconic(window) != 0
            || window == GetDesktopWindow()
            || window == GetShellWindow()
            || !child(window, ICONS).is_null()
            || is_wallpaper_layer(window)
        {
            return None;
        }
        GetWindowThreadProcessId(window, &mut process);
        if process == std::process::id() {
            return None;
        }
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED as u32,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        );
        if cloaked != 0 {
            return None;
        }
        let mut bounds: RECT = std::mem::zeroed();
        let status = DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&mut bounds as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        );
        (status >= 0 || GetWindowRect(window, &mut bounds) != 0).then_some(bounds)
    }
}

fn is_wallpaper_layer(window: HWND) -> bool {
    let mut name = [0u16; 9];
    let length = unsafe { GetClassNameW(window, name.as_mut_ptr(), name.len() as i32) };
    ["Progman", "WorkerW"].iter().any(|class| {
        name[..length as usize]
            .iter()
            .copied()
            .eq(class.encode_utf16())
    })
}

fn covers(window: RECT, monitor: RECT) -> bool {
    window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

pub fn cursor_in(monitor: RECT) -> Option<[f32; 2]> {
    let mut position = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut position) } == 0 {
        return None;
    }
    normalize_cursor(position, monitor)
}

fn normalize_cursor(position: POINT, monitor: RECT) -> Option<[f32; 2]> {
    if position.x < monitor.left
        || position.x >= monitor.right
        || position.y < monitor.top
        || position.y >= monitor.bottom
    {
        return None;
    }
    Some([
        (position.x - monitor.left) as f32 / (monitor.right - monitor.left) as f32,
        (position.y - monitor.top) as f32 / (monitor.bottom - monitor.top) as f32,
    ])
}

pub fn on_battery() -> bool {
    let mut status = std::mem::MaybeUninit::<SYSTEM_POWER_STATUS>::uninit();
    unsafe {
        GetSystemPowerStatus(status.as_mut_ptr()) != 0 && status.assume_init().ACLineStatus == 0
    }
}

fn wallpaper_layer() -> io::Result<(HWND, HWND)> {
    unsafe {
        let progman = FindWindowW(PROGMAN, ptr::null());
        if progman.is_null() {
            return Err(io::Error::other("Explorer's Progman window was not found"));
        }

        // Explorer's undocumented 0x052C message requests Explorer to prepare its
        // wallpaper rendering layer behind the desktop icons.
        // Assumptions about Explorer's desktop hierarchy:
        // 1. Raised desktops and Windows 11 host the icon view (SHELLDLL_DefView)
        //    directly inside Progman. On this hierarchy, the wallpaper window must
        //    be parented directly to Progman, placed immediately behind the icon view
        //    in Z-order.
        // 2. Classic Windows 10 hierarchy hosts SHELLDLL_DefView inside a top-level
        //    WorkerW window, and 0x052C spawns a separate WorkerW sibling immediately
        //    behind it in Z-order. On this hierarchy, the wallpaper window is parented
        //    to that sibling WorkerW at HWND_BOTTOM.
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
        if !icons.is_null() {
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

    #[test]
    fn cursor_uses_monitor_relative_coordinates_and_excludes_neighbor() {
        let monitor = RECT {
            left: -1920,
            top: 120,
            right: 0,
            bottom: 1200,
        };
        assert_eq!(
            normalize_cursor(POINT { x: -960, y: 660 }, monitor),
            Some([0.5, 0.5])
        );
        assert_eq!(normalize_cursor(POINT { x: 0, y: 660 }, monitor), None);
    }
}
