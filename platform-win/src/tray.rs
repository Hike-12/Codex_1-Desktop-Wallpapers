use std::{fs, path::PathBuf, process::Command};

use core_engine::{Result, shader::installed_packs};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuId, MenuItem, PredefinedMenuItem},
};
use windows_sys::{
    Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
    w,
};

const BUILT_IN: &str = "wallp.built-in";
const OPEN: &str = "wallp.open-folder";
const REFRESH: &str = "wallp.refresh";
const QUIT: &str = "wallp.quit";

pub enum Action {
    BuiltIn,
    Select(PathBuf),
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    folder: PathBuf,
    selections: Vec<(MenuId, PathBuf)>,
}

impl Tray {
    pub fn new() -> Result<Self> {
        let folder = PathBuf::from(std::env::var_os("APPDATA").ok_or("APPDATA is unavailable")?)
            .join("wallp")
            .join("wallpapers");
        let (menu, selections) = build_menu(installed_packs(&folder)?)?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("Desktop Wallpapers")
            .with_icon(wallpaper_icon()?)
            .with_menu(Box::new(menu))
            .build()?;
        Ok(Self {
            icon,
            folder,
            selections,
        })
    }

    pub fn action(&mut self, id: &MenuId) -> Result<Option<Action>> {
        let action = match id.as_ref() {
            BUILT_IN => Some(Action::BuiltIn),
            QUIT => Some(Action::Quit),
            OPEN => {
                fs::create_dir_all(&self.folder)?;
                Command::new("explorer.exe").arg(&self.folder).spawn()?;
                None
            }
            REFRESH => {
                let (menu, selections) = build_menu(installed_packs(&self.folder)?)?;
                self.icon.set_menu(Some(Box::new(menu)));
                self.selections = selections;
                None
            }
            _ => self
                .selections
                .iter()
                .find(|(item, _)| item == id)
                .map(|(_, folder)| Action::Select(folder.clone())),
        };
        Ok(action)
    }
}

fn build_menu(packs: Vec<(String, PathBuf)>) -> Result<(Menu, Vec<(MenuId, PathBuf)>)> {
    let menu = Menu::new();
    menu.append(&MenuItem::with_id(
        BUILT_IN,
        "Use built-in wallpaper",
        true,
        None,
    ))?;
    let mut selections = Vec::new();
    for (name, path) in packs {
        let item = MenuItem::new(name.replace('&', "&&"), true, None);
        menu.append(&item)?;
        selections.push((item.id().clone(), path));
    }
    menu.append_items(&[
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(OPEN, "Open wallpapers folder", true, None),
        &MenuItem::with_id(REFRESH, "Refresh wallpapers", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(QUIT, "Quit", true, None),
    ])?;
    Ok((menu, selections))
}

pub fn show_error(message: &str) {
    let message: Vec<u16> = message.encode_utf16().chain([0]).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            w!("Desktop Wallpapers"),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn wallpaper_icon() -> Result<Icon> {
    const PIXELS: [&[u8; 16]; 16] = [
        b"................",
        b".##############.",
        b".#oooooooooooo#.",
        b".#oooooooooooo#.",
        b".#ooooo++ooooo#.",
        b".#oooo+oo+oooo#.",
        b".#o+++oooo++++#.",
        b".#oooooooooooo#.",
        b".#oooooooooooo#.",
        b".##############.",
        b"......####......",
        b"......####......",
        b"....########....",
        b"................",
        b"................",
        b"................",
    ];
    let rgba = PIXELS
        .into_iter()
        .flatten()
        .flat_map(|&pixel| match pixel {
            b'#' => [217, 239, 246, 255],
            b'o' => [17, 31, 45, 255],
            b'+' => [65, 214, 191, 255],
            _ => [0, 0, 0, 0],
        })
        .collect();
    Ok(Icon::from_rgba(rgba, 16, 16)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_item_points_to_its_pack_folder() {
        let folder = PathBuf::from(r"C:\Wallpapers\Dusk");
        let (menu, selections) = build_menu(vec![("Dusk & night".into(), folder.clone())]).unwrap();
        let items = menu.items();
        assert_eq!(items[0].id().as_ref(), BUILT_IN);
        assert_eq!(items[1].id(), &selections[0].0);
        assert_eq!(selections[0].1, folder);
        assert_eq!(items.last().unwrap().id().as_ref(), QUIT);
    }
}
