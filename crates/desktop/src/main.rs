// Menu-bar (notification-area) desktop app for Recall, the MTG Arena log parser.
// On Windows, don't pop a console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod logbridge;
mod observer;
mod state;

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

use state::AppState;

fn main() {
    // Install the log->webview bridge before anything logs.
    logbridge::WebviewLogger::install();

    // Dev safety: never POST to the live API during development. Set RECALL_HOST
    // (e.g. the local mock server) to override the configured host.
    let host_override = std::env::var("RECALL_HOST").ok();

    tauri::Builder::default()
        .manage(AppState::new(host_override))
        .invoke_handler(tauri::generate_handler![
            commands::token_present,
            commands::save_token,
            commands::get_status,
            commands::recent_logs,
            commands::set_log_path,
        ])
        .setup(|app| {
            logbridge::attach(app.handle().clone());
            app.state::<AppState>().attach(app.handle().clone());

            // Show the crate version in the window title (e.g. "Recall v0.1.1"). Done at
            // runtime since a static tauri.conf.json title can't interpolate the version.
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_title(&format!("Recall v{}", env!("CARGO_PKG_VERSION")));
            }

            // Menu-bar app: no dock icon on macOS.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            build_tray(app.handle())?;

            // First run: if no token yet, open the window on Settings for onboarding.
            // Otherwise start following immediately.
            let state = app.state::<AppState>();
            let following_started =
                state.config().seventeenlands_token().is_some() && state.start().is_ok();

            if !following_started {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
                let _ = app.emit("show-settings", ());
            }
            // From here on the status is pushed, not polled: see `AppState::notify`.
            status_changed(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Closing the window only hides it; the app keeps living in the menu bar.
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            // The one change no in-process event announces is a hand-edit of config.toml;
            // pick it up when the user comes back to the window.
            WindowEvent::Focused(true) => {
                let state = window.state::<AppState>();
                state.reload_config();
                state.notify();
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running Recall desktop app");
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    // Disabled header: app name + version.
    let title = MenuItem::with_id(
        app,
        "title",
        format!("Recall v{}", env!("CARGO_PKG_VERSION")),
        false,
        None::<&str>,
    )?;
    let header_sep = PredefinedMenuItem::separator(app)?;

    // One window holds both the log and the settings, so one item opens it.
    let show = MenuItem::with_id(app, "show", "Open Recall…", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Recall", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&title, &header_sep, &show, &sep, &quit])?;

    // Standard macOS menu-bar behavior: a left-click opens the menu. The menu's
    // "Open Recall…" item keeps the window reachable.
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip(format!("Recall v{}", env!("CARGO_PKG_VERSION")))
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(handle_menu);

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone()).icon_as_template(true);
    }

    builder.build(app)?;
    Ok(())
}

fn handle_menu(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "show" => show_window(app),
        "quit" => {
            app.state::<AppState>().stop();
            app.exit(0);
        }
        _ => {}
    }
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// The displayed status changed: tell the webview to refresh. Callable from any thread, and
/// never blocks (the follower thread calls this while the main thread may be joining it in
/// `AppState::stop`).
pub(crate) fn status_changed(app: &AppHandle) {
    let _ = app.emit("status-changed", ());
}
