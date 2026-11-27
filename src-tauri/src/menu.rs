//! The macOS menu bar.
//!
//! Tauri gives a Mac app a default menu — the app menu, Edit, View, Window,
//! Help — and that default is what makes copy and paste work in the webview at
//! all, so everything in it is kept here exactly. What it lacks is the app: no
//! Settings… under the app's name where every Mac app has it, no New
//! Conversation under File, nothing in View that this window can show. A menu
//! bar with only the system's items is the clearest single sign that a window is
//! a web page in a frame.
//!
//! macOS only. On Windows and Linux a menu bar is drawn inside the window, and
//! an app of this shape does not have one there; the palette is where those
//! platforms find a command.
//!
//! The items the app owns don't do anything here. Each emits `menu` with its id
//! and the frontend runs it (`lib/commands.ts`), through the same function its
//! own keyboard handler calls, so a command reached by the menu and the same
//! command reached by its key cannot drift apart. The accelerators are the keys
//! that handler already answers to; with the menu present it steps aside for
//! them, so a key is handled once, by the menu, rather than twice.

// Built on macOS only (see `lib.rs`), so on the other platforms everything but
// `forward` is unused there by design.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use tauri::menu::Menu;
use tauri::{AppHandle, Emitter, Runtime};

/// The event the frontend listens for. Its payload is the item's id.
pub const EVENT: &str = "menu";

/// `(id, label, accelerator)` for the app's own items, by submenu. The ids are
/// the frontend's command names; `lib/commands.ts` lists the same ones.
const APP: &[(&str, &str, Option<&str>)] = &[("settings", "Settings…", Some("CmdOrCtrl+,"))];
const FILE: &[(&str, &str, Option<&str>)] = &[
    ("new", "New Conversation", Some("CmdOrCtrl+N")),
    // No accelerator: ⌃R is also the shell's history search, and a menu key is
    // taken before the terminal dock could see it. The window's own handler
    // already leaves it to the dock when the dock has focus.
    ("switch", "Switch Workspace…", None),
    ("finder", "Go to File…", Some("CmdOrCtrl+P")),
];
const VIEW: &[(&str, &str, Option<&str>)] = &[
    ("palette", "Command Palette…", Some("CmdOrCtrl+K")),
    ("sidebar", "Toggle Sidebar", Some("CmdOrCtrl+B")),
    // Not the accelerator, for the same reason as Switch Workspace: ⌃` is
    // handled in the page, where it can tell the dock has focus.
    ("terminal", "Toggle Terminal", None),
    ("composer", "Focus the Message Box", Some("CmdOrCtrl+L")),
];

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    use tauri::menu::{
        AboutMetadata, MenuItemBuilder, SubmenuBuilder, HELP_SUBMENU_ID, WINDOW_SUBMENU_ID,
    };

    let pkg = app.package_info();
    let config = app.config();
    // The product's name, not the process's. Left to the system, the app menu
    // said "About taurus-app" and "Quit taurus-app" — the binary's name, which
    // is what macOS reads when an item brings no text of its own, and which the
    // bundle keeps — so every item that names the app is given the name here.
    let name = config.product_name.clone().unwrap_or_else(|| pkg.name.clone());
    let about = AboutMetadata {
        name: Some(name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };

    let items = |list: &[(&str, &str, Option<&str>)]| {
        list.iter()
            .map(|(id, label, key)| {
                let item = MenuItemBuilder::with_id(*id, *label);
                match key {
                    Some(key) => item.accelerator(*key),
                    None => item,
                }
                .build(app)
            })
            .collect::<tauri::Result<Vec<_>>>()
    };
    let app_items = items(APP)?;
    let file_items = items(FILE)?;
    let view_items = items(VIEW)?;

    let mut app_menu = SubmenuBuilder::new(app, &name)
        .about_with_text(format!("About {name}"), Some(about))
        .separator();
    for item in &app_items {
        app_menu = app_menu.item(item);
    }
    let app_menu = app_menu
        .separator()
        .services()
        .separator()
        .hide_with_text(format!("Hide {name}"))
        .hide_others()
        .show_all()
        .separator()
        .quit_with_text(format!("Quit {name}"))
        .build()?;

    let mut file = SubmenuBuilder::new(app, "File");
    for item in &file_items {
        file = file.item(item);
    }
    let file = file.separator().close_window().build()?;

    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let mut view = SubmenuBuilder::new(app, "View");
    for item in &view_items {
        view = view.item(item);
    }
    let view = view.separator().fullscreen().build()?;

    // The ids macOS keys its own behaviour off: the Window menu lists the open
    // windows, and the Help menu gets the system's search field.
    let window = SubmenuBuilder::with_id(app, WINDOW_SUBMENU_ID, "Window")
        .minimize()
        .maximize()
        .separator()
        .close_window()
        .build()?;
    let help = SubmenuBuilder::with_id(app, HELP_SUBMENU_ID, "Help")
        .text("docs", "Taurus Documentation")
        .build()?;

    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &window, &help])
}

/// Hands an item the app owns to the frontend. The system's own items (Copy,
/// Quit, Minimize) are performed by the OS and never reach this.
pub fn forward<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let _ = app.emit(EVENT, id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_id_is_unique() {
        let mut ids: Vec<&str> = APP.iter().chain(FILE).chain(VIEW).map(|(id, ..)| *id).collect();
        ids.push("docs");
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }
}
