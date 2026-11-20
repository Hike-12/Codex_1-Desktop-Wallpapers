use core_engine::Result;
use core_foundation::{
    base::{CFType, CFTypeRef, TCFType},
    string::{CFString, CFStringRef},
};
use objc2::rc::Retained;
use objc2_app_kit::{NSScreen, NSView, NSWindow, NSWindowCollectionBehavior};
use objc2_core_graphics::{CGWindowLevelForKey, CGWindowLevelKey};
use objc2_foundation::{MainThreadMarker, NSNumber, NSPoint, NSRect, ns_string};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

#[derive(Clone, Copy)]
pub struct Display {
    pub id: u32,
    pub frame: NSRect,
}

impl Display {
    pub fn cursor(self, point: NSPoint) -> Option<[f32; 2]> {
        let x = (point.x - self.frame.origin.x) / self.frame.size.width;
        let y = (point.y - self.frame.origin.y) / self.frame.size.height;
        if (0.0..1.0).contains(&x) && (0.0..1.0).contains(&y) {
            Some([x as f32, (1.0 - y) as f32])
        } else {
            None
        }
    }
}

pub fn displays() -> Result<Vec<Display>> {
    let main_thread = MainThreadMarker::new().ok_or("AppKit requires the main thread")?;
    Ok(NSScreen::screens(main_thread)
        .into_iter()
        .filter_map(|screen| {
            let info = screen.deviceDescription();
            let id = info.get(ns_string!("NSScreenNumber"))?;
            // AppKit defines NSScreenNumber as an NSNumber holding the CGDirectDisplayID.
            let id = unsafe { &*std::ptr::from_ref(id).cast::<NSNumber>() }.as_u32();
            Some(Display {
                id,
                frame: screen.frame(),
            })
        })
        .collect())
}

pub fn place_window(window: &Window, display: Display) -> Result<()> {
    let window = native_window(window)?;
    window.setFrame_display(display.frame, false);
    window.setLevel(CGWindowLevelForKey(CGWindowLevelKey::DesktopWindowLevelKey) as isize);
    window.setIgnoresMouseEvents(true);
    // The window is live on the AppKit thread and these flags use one behavior
    // per collection category (Spaces, Mission Control, and window cycling).
    unsafe {
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        window.setCanHide(false);
        window.setHidesOnDeactivate(false);
        window.orderFrontRegardless();
    }
    Ok(())
}

pub fn move_window(window: &Window, display: Display) -> Result<()> {
    native_window(window)?.setFrame_display(display.frame, false);
    Ok(())
}

fn native_window(window: &Window) -> Result<Retained<NSWindow>> {
    let RawWindowHandle::AppKit(handle) = window.window_handle()?.as_raw() else {
        return Err("Expected an AppKit window".into());
    };
    // Winit owns the NSView. Its window stays alive while the Window is borrowed.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window()
        .ok_or_else(|| "The wallpaper view has no AppKit window".into())
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSGetProvidingPowerSourceType(blob: CFTypeRef) -> CFStringRef;
}

pub fn on_battery() -> bool {
    // IOKit owns the returned type string and the source snapshot follows the
    // Create Rule. Retain the string before either object goes out of scope.
    unsafe {
        let snapshot = IOPSCopyPowerSourcesInfo();
        if snapshot.is_null() {
            return false;
        }
        let snapshot = CFType::wrap_under_create_rule(snapshot);
        let source = IOPSGetProvidingPowerSourceType(snapshot.as_CFTypeRef());
        !source.is_null()
            && CFString::wrap_under_get_rule(source)
                == CFString::from_static_string("Battery Power")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::NSSize;

    #[test]
    fn pointer_coordinates_are_local_to_the_display_and_top_left() {
        let display = Display {
            id: 1,
            frame: NSRect::new(NSPoint::new(-1600.0, 100.0), NSSize::new(1600.0, 900.0)),
        };
        assert_eq!(
            display.cursor(NSPoint::new(-1200.0, 775.0)),
            Some([0.25, 0.25])
        );
        assert_eq!(display.cursor(NSPoint::new(0.0, 775.0)), None);
    }
}
