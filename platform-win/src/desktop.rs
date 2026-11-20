use std::{io, num::NonZeroIsize, ptr};

use core_engine::{Renderer, Result};
use windows::{
    Win32::{
        Foundation::HWND as CompositionHwnd,
        Graphics::{
            DirectComposition::{
                DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget,
            },
            Dxgi::IDXGIDevice,
        },
    },
    core::Interface,
};
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
    platform::windows::WindowAttributesExtWindows,
    raw_window_handle::{HasWindowHandle, RawWindowHandle, Win32WindowHandle},
    window::{Window, WindowAttributes},
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
    icon_anchor: HWND,
}

impl DesktopLayer {
    pub fn find() -> io::Result<Self> {
        wallpaper_layer()
    }

    pub fn window_attributes(self) -> io::Result<WindowAttributes> {
        let parent = NonZeroIsize::new(self.parent as isize)
            .filter(|_| unsafe { IsWindow(self.parent) != 0 })
            .ok_or_else(|| io::Error::other("Explorer's wallpaper layer was closed"))?;
        let handle = RawWindowHandle::Win32(Win32WindowHandle::new(parent));
        // Explorer owns this HWND. Create the hidden render window as its child
        // so it never has a top-level window that can be promoted on activation.
        Ok(unsafe {
            Window::default_attributes()
                .with_title("Stilltide")
                .with_visible(false)
                .with_active(false)
                .with_decorations(false)
                .with_resizable(false)
                // Windows requires this style at creation. Adding it later to
                // an Explorer child fails with ERROR_INVALID_PARAMETER.
                .with_no_redirection_bitmap(true)
                .with_parent_window(Some(handle))
        })
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
                style | (WS_DISABLED | WS_CLIPSIBLINGS) as isize,
            )
            .map_err(|error| format!("Set wallpaper child styles: {error}"))?;
            let style = GetWindowLongPtrW(window, GWL_EXSTYLE);
            let flags = WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW;
            set_style(
                window,
                GWL_EXSTYLE,
                (style & !(WS_EX_APPWINDOW as isize)) | flags as isize,
            )
            .map_err(|error| format!("Set wallpaper composition styles: {error}"))?;
            if GetParent(window) != layer.parent {
                return Err("The wallpaper window is outside Explorer's desktop layer".into());
            }
        }
        let desktop = Self { window, layer };
        desktop
            .fit(bounds)
            .map_err(|error| format!("Position wallpaper window: {error}"))?;
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
                    SWP_NOACTIVATE | SWP_FRAMECHANGED,
                ) == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub fn show_behind_icons(&self) -> bool {
        let surface_layer = if self.layer.above == HWND_BOTTOM {
            self.layer.parent
        } else {
            self.window
        };
        let safe = self.is_attached() && follows(self.layer.icon_anchor, surface_layer);
        unsafe {
            let shown = GetWindowLongPtrW(self.window, GWL_STYLE) as u32 & WS_VISIBLE != 0;
            if shown != safe {
                ShowWindow(self.window, if safe { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
        safe
    }

    pub fn renderer(&self, width: u32, height: u32) -> Result<(Renderer<'static>, Composition)> {
        Composition::renderer(self.window, width, height)
    }

    pub fn is_attached(&self) -> bool {
        unsafe { IsWindow(self.window) != 0 && GetParent(self.window) == self.layer.parent }
    }
}

fn follows(anchor: HWND, candidate: HWND) -> bool {
    let mut sibling = unsafe { GetWindow(anchor, GW_HWNDNEXT) };
    while !sibling.is_null() {
        if sibling == candidate {
            return true;
        }
        sibling = unsafe { GetWindow(sibling, GW_HWNDNEXT) };
    }
    false
}

pub struct Composition {
    device: IDCompositionDevice,
    _target: IDCompositionTarget,
}

impl Composition {
    fn renderer(window: HWND, width: u32, height: u32) -> Result<(Renderer<'static>, Self)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            ..Default::default()
        });
        // Use a composition swap chain. It has no HWND to promote to fullscreen
        // and DWM clips the visual to our disabled child window.
        unsafe {
            let device: IDCompositionDevice = DCompositionCreateDevice(None::<&IDXGIDevice>)
                .map_err(|error| format!("Create DirectComposition device: {error}"))?;
            let target = device
                .CreateTargetForHwnd(CompositionHwnd(window), false)
                .map_err(|error| {
                    format!("Bind DirectComposition to the wallpaper child: {error}")
                })?;
            let visual = device
                .CreateVisual()
                .map_err(|error| format!("Create wallpaper visual: {error}"))?;
            target
                .SetRoot(&visual)
                .map_err(|error| format!("Attach wallpaper visual: {error}"))?;
            // wgpu takes its own reference to the visual.
            let surface = instance.create_surface_unsafe(
                wgpu::SurfaceTargetUnsafe::CompositionVisual(visual.as_raw()),
            )?;
            let renderer =
                pollster::block_on(Renderer::from_surface(&instance, surface, width, height))?;
            device
                .Commit()
                .map_err(|error| format!("Commit wallpaper composition: {error}"))?;
            Ok((
                renderer,
                Self {
                    device,
                    _target: target,
                },
            ))
        }
    }

    pub fn commit(&self) -> Result<()> {
        unsafe { self.device.Commit()? };
        Ok(())
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
        {
            return None;
        }
        let root = GetAncestor(window, GA_ROOT);
        if root.is_null()
            || root == GetShellWindow()
            || !child(root, ICONS).is_null()
            || is_wallpaper_layer(root)
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
    class_is(window, "Progman") || class_is(window, "WorkerW")
}

fn class_is(window: HWND, class: &str) -> bool {
    let mut name = [0u16; 9];
    let length = unsafe { GetClassNameW(window, name.as_mut_ptr(), name.len() as i32) };
    name[..length as usize]
        .iter()
        .copied()
        .eq(class.encode_utf16())
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

fn wallpaper_layer() -> io::Result<DesktopLayer> {
    find_layer(true)
}

fn find_layer(prepare: bool) -> io::Result<DesktopLayer> {
    unsafe {
        let progman = FindWindowW(PROGMAN, ptr::null());
        if progman.is_null() {
            return Err(io::Error::other("Explorer's Progman window was not found"));
        }

        // Explorer's undocumented 0x052C message requests Explorer to prepare its
        // wallpaper rendering layer behind the desktop icons.
        // Assumptions about Explorer's desktop hierarchy:
        // 1. Raised desktops host the icon view (SHELLDLL_DefView) inside Progman.
        //    Prefer the visible WorkerW sibling behind the icon view. Its children
        //    stay behind the icon view even when Explorer activates the desktop.
        //    Some Explorer versions have no usable WorkerW and need a direct child.
        // 2. Classic Windows 10 hierarchy hosts SHELLDLL_DefView inside a top-level
        //    WorkerW window, and 0x052C spawns a separate WorkerW sibling immediately
        //    behind it in Z-order. On this hierarchy, the wallpaper window is parented
        //    to that sibling WorkerW at HWND_BOTTOM.
        if prepare {
            SendMessageTimeoutW(
                progman,
                0x052C,
                0xD,
                1,
                SMTO_ABORTIFHUNG,
                1_000,
                ptr::null_mut(),
            );
        }

        let icons = child(progman, ICONS);
        if !icons.is_null() {
            let mut sibling = GetWindow(icons, GW_HWNDNEXT);
            while !sibling.is_null() {
                if class_is(sibling, "WorkerW") && IsWindowVisible(sibling) != 0 {
                    let mut shell: RECT = std::mem::zeroed();
                    let mut worker: RECT = std::mem::zeroed();
                    if GetWindowRect(progman, &mut shell) != 0
                        && GetWindowRect(sibling, &mut worker) != 0
                        && covers(worker, shell)
                    {
                        return Ok(DesktopLayer {
                            parent: sibling,
                            above: HWND_BOTTOM,
                            icon_anchor: icons,
                        });
                    }
                }
                sibling = GetWindow(sibling, GW_HWNDNEXT);
            }
            return Ok(DesktopLayer {
                parent: progman,
                above: icons,
                icon_anchor: icons,
            });
        }

        let mut layer = None;
        EnumWindows(
            Some(find_worker),
            (&mut layer as *mut Option<DesktopLayer>) as LPARAM,
        );
        layer.ok_or_else(|| io::Error::other("Explorer's wallpaper layer was not found"))
    }
}

unsafe extern "system" fn find_worker(hwnd: HWND, context: LPARAM) -> i32 {
    unsafe {
        if !child(hwnd, ICONS).is_null() {
            let worker = FindWindowExW(ptr::null_mut(), hwnd, WORKER, ptr::null());
            if !worker.is_null() && child(worker, ICONS).is_null() {
                *(context as *mut Option<DesktopLayer>) = Some(DesktopLayer {
                    parent: worker,
                    above: HWND_BOTTOM,
                    icon_anchor: hwnd,
                });
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
    #[ignore = "requires an interactive Explorer session and a DirectX 12 GPU"]
    fn composition_presents_to_a_hidden_explorer_child() -> Result<()> {
        use winit::{
            application::ApplicationHandler,
            event::WindowEvent,
            event_loop::{ActiveEventLoop, EventLoop},
            platform::windows::EventLoopBuilderExtWindows,
            window::WindowId,
        };

        #[derive(Default)]
        struct Probe(Option<Result<()>>);
        impl ApplicationHandler for Probe {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.0 = Some((|| {
                    let layer = find_layer(false)?;
                    let window = event_loop.create_window(layer.window_attributes()?)?;
                    let mut bounds: RECT = unsafe { std::mem::zeroed() };
                    if unsafe { GetWindowRect(layer.parent, &mut bounds) } == 0 {
                        return Err(io::Error::last_os_error().into());
                    }
                    bounds.right = bounds.left + 64;
                    bounds.bottom = bounds.top + 64;
                    let desktop = Desktop::attach(&window, layer, bounds)?;
                    let (mut renderer, _composition) = desktop.renderer(64, 64)?;
                    assert!(renderer.render(64, 64, 0.0)?);
                    Ok(())
                })());
                event_loop.exit();
            }

            fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
        }
        let mut probe = Probe::default();
        EventLoop::builder()
            .with_any_thread(true)
            .build()?
            .run_app(&mut probe)?;
        probe.0.expect("desktop probe did not run")
    }

    #[test]
    #[ignore = "requires a Windows session with a DirectX 12 GPU"]
    fn composition_presents_to_a_hidden_window() -> Result<()> {
        struct HiddenWindow(HWND);
        impl Drop for HiddenWindow {
            fn drop(&mut self) {
                unsafe { DestroyWindow(self.0) };
            }
        }
        // A private hidden window exercises the compositor without changing Explorer
        // or taking focus from the user's desktop.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("Stilltide composition test"),
                WS_POPUP | WS_DISABLED,
                0,
                0,
                64,
                64,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        let window = HiddenWindow(hwnd);
        let (mut renderer, _composition) = Composition::renderer(window.0, 64, 64)?;
        assert!(renderer.render(64, 64, 0.0)?);
        Ok(())
    }

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
