//! Quip: a local-first macOS composition layer. Tray-only Tauri shell.
//!
//! Commit 1 scope: the app boots as a menu-bar item with placeholder controls
//! and hidden `composition`, `settings`, and `demo` windows. Behavior wiring
//! (composition flow, learning, demo harness) lands in the next change.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accessibility;
mod commit;
mod composition;
mod inference;
mod learning;
mod settings;

use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Manager, WindowEvent,
};

fn show_window(app: &tauri::AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Placeholder tray controls; the toggles become functional when the
            // settings module lands.
            let enabled = CheckMenuItem::with_id(app, "enabled", "Enabled", true, false, None::<&str>)?;
            let window_context =
                CheckMenuItem::with_id(app, "window_context", "Window context", true, false, None::<&str>)?;
            let pause_learning =
                CheckMenuItem::with_id(app, "pause_learning", "Pause learning", true, false, None::<&str>)?;
            let profile = MenuItem::with_id(app, "profile", "Profile: default", false, None::<&str>)?;
            let open_settings = MenuItem::with_id(app, "open_settings", "Settings…", true, None::<&str>)?;
            let open_demo = MenuItem::with_id(app, "open_demo", "Demo…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Quip", true, None::<&str>)?;

            let menu = Menu::with_items(
                app,
                &[
                    &enabled,
                    &window_context,
                    &pause_learning,
                    &profile,
                    &PredefinedMenuItem::separator(app)?,
                    &open_settings,
                    &open_demo,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?;

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
            TrayIconBuilder::with_id("quip-tray")
                .icon(icon)
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open_settings" => show_window(app, "settings"),
                    "open_demo" => show_window(app, "demo"),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        // Tray apps outlive their windows: closing a window hides it so the
        // tray can reopen it later.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Quip");
}
