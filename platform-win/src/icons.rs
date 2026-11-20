use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    ptr::{self, NonNull},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
};

use core_engine::icons::{Icon, IconLayout};
use windows::{
    Win32::{
        System::{
            Com::{
                CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize, IServiceProvider,
            },
            Variant::VARIANT,
        },
        UI::Shell::{
            Common::ITEMIDLIST, IFolderView, ILGetSize, IShellBrowser, IShellWindows,
            SID_STopLevelBrowser, SVGIO_ALLVIEW, SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellWindows,
        },
    },
    core::Interface,
};
use windows_sys::{
    Win32::{
        Foundation::{POINT, RECT},
        Graphics::Gdi::ClientToScreen,
        UI::WindowsAndMessaging::{FindWindowExW, IsWindowVisible},
    },
    w,
};

const MAX_ICONS: i32 = 1024;

pub struct Reader {
    request: SyncSender<()>,
    results: Receiver<Snapshot>,
}

impl Reader {
    pub fn start() -> std::io::Result<Self> {
        let (request, pending) = mpsc::sync_channel(1);
        let (report, results) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("wallp-icons".into())
            .spawn(move || {
                let apartment = match Apartment::enter() {
                    Ok(apartment) => apartment,
                    Err(error) => {
                        eprintln!("wallp: desktop icons are unavailable: {error}");
                        return;
                    }
                };
                let mut failed = false;
                while pending.recv().is_ok() {
                    match apartment.sample() {
                        Ok(snapshot) => {
                            failed = false;
                            let _ = report.try_send(snapshot);
                        }
                        Err(error) if !failed => {
                            failed = true;
                            eprintln!("wallp: could not read desktop icons: {error}");
                        }
                        Err(_) => {}
                    }
                }
            })?;
        Ok(Self { request, results })
    }

    pub fn poll(&self) -> Option<Snapshot> {
        let snapshot = self.results.try_iter().last();
        let _ = self.request.try_send(());
        snapshot
    }
}

struct Apartment;

impl Apartment {
    fn enter() -> windows::core::Result<Self> {
        // The worker exclusively owns its COM objects and releases them before uninitializing.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }

    fn sample(&self) -> core_engine::Result<Snapshot> {
        // These read-only Shell calls run off the window event loop. PIDLs returned by
        // Item are owned by this thread and freed on every path by ItemId.
        unsafe {
            let shell: IShellWindows =
                CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)
                    .map_err(|error| format!("connect to Shell Windows: {error}"))?;
            let location = VARIANT::from(0_i32);
            let empty = VARIANT::default();
            let mut hwnd = 0;
            let services: IServiceProvider = shell
                .FindWindowSW(&location, &empty, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)
                .map_err(|error| format!("find the Explorer desktop: {error}"))?
                .cast()?;
            let browser: IShellBrowser = services
                .QueryService(&SID_STopLevelBrowser)
                .map_err(|error| format!("open the desktop browser: {error}"))?;
            let shell_view = browser
                .QueryActiveShellView()
                .map_err(|error| format!("open the desktop view: {error}"))?;
            let coords = shell_view
                .GetWindow()
                .map_err(|error| format!("find the desktop view window: {error}"))?
                .0;
            let list = FindWindowExW(coords, ptr::null_mut(), w!("SysListView32"), ptr::null());
            let coords = if list.is_null() { coords } else { list };
            let mut origin = POINT { x: 0, y: 0 };
            if ClientToScreen(coords, &mut origin) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let folder: IFolderView = shell_view.cast()?;
            let mut spacing = windows::Win32::Foundation::POINT::default();
            folder
                .GetSpacing(&mut spacing)
                .map_err(|error| format!("read desktop icon spacing: {error}"))?;
            let spacing = [spacing.x.max(32), spacing.y.max(32)];
            let mut snapshot = Snapshot {
                spacing,
                icons: Vec::new(),
            };
            if IsWindowVisible(coords) == 0 {
                return Ok(snapshot);
            }
            let count = folder
                .ItemCount(SVGIO_ALLVIEW)
                .map_err(|error| format!("enumerate desktop icons: {error}"))?
                .clamp(0, MAX_ICONS);
            for index in 0..count {
                let Ok(pidl) = folder.Item(index) else {
                    continue;
                };
                let Some(pidl) = NonNull::new(pidl).map(ItemId) else {
                    continue;
                };
                let Ok(point) = folder.GetItemPosition(pidl.0.as_ptr()) else {
                    continue;
                };
                let bytes = std::slice::from_raw_parts(
                    pidl.0.as_ptr().cast::<u8>(),
                    ILGetSize(Some(pidl.0.as_ptr())) as usize,
                );
                let mut identity = DefaultHasher::new();
                bytes.hash(&mut identity);
                snapshot.icons.push((
                    [
                        origin.x + point.x + spacing[0] / 2,
                        origin.y + point.y + spacing[1] / 3,
                    ],
                    (identity.finish() % 5) as u8,
                ));
            }
            Ok(snapshot)
        }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // Corresponds to the successful initialization on this worker.
        unsafe { CoUninitialize() };
    }
}

struct ItemId(NonNull<ITEMIDLIST>);

impl Drop for ItemId {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.as_ptr().cast())) };
    }
}

pub struct Snapshot {
    spacing: [i32; 2],
    icons: Vec<([i32; 2], u8)>,
}

impl Snapshot {
    pub fn for_monitor(&self, bounds: RECT) -> IconLayout {
        let width = (bounds.right - bounds.left).max(1) as f32;
        let height = (bounds.bottom - bounds.top).max(1) as f32;
        IconLayout {
            radius: [
                self.spacing[0] as f32 * 1.08 / width,
                self.spacing[1] as f32 * 1.08 / height,
            ],
            icons: self
                .icons
                .iter()
                .filter(|&([x, y], _)| {
                    (bounds.left..bounds.right).contains(x)
                        && (bounds.top..bounds.bottom).contains(y)
                })
                .map(|&([x, y], kind)| Icon {
                    center: [
                        (x - bounds.left) as f32 / width,
                        (y - bounds.top) as f32 / height,
                    ],
                    kind,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_are_local_even_to_a_monitor_left_of_primary() {
        let snapshot = Snapshot {
            spacing: [80, 96],
            icons: vec![([-100, 25], 2), ([100, 25], 3)],
        };
        let layout = snapshot.for_monitor(RECT {
            left: -400,
            top: 0,
            right: 0,
            bottom: 100,
        });
        assert_eq!(
            layout.icons,
            [Icon {
                center: [0.75, 0.25],
                kind: 2
            }]
        );
    }
}
