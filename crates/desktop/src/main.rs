// Menu-bar (notification-area) desktop app for Recall, the MTG Arena log parser.
// On Windows, don't pop a console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod logbridge;
mod observer;
mod state;

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WindowEvent, Wry};

use state::AppState;

/// Handle to the disabled tray-menu status line ("Working" / "Stopped" / "Missing token"),
/// kept in managed state so it can be re-rendered as the follower's state changes.
struct TrayStatusItem(MenuItem<Wry>);

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
    // Disabled header: app name + version, and a live status line.
    let title = MenuItem::with_id(
        app,
        "title",
        format!("Recall v{}", env!("CARGO_PKG_VERSION")),
        false,
        None::<&str>,
    )?;
    let status = MenuItem::with_id(app, "status", tray_status_text(app), false, None::<&str>)?;
    let header_sep = PredefinedMenuItem::separator(app)?;

    let show = MenuItem::with_id(app, "show", "Show Log Window", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Recall", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&title, &status, &header_sep, &show, &settings, &sep, &quit],
    )?;

    app.manage(TrayStatusItem(status));

    // Standard macOS menu-bar behavior: a left-click opens the menu. The menu's
    // "Show Log Window" item keeps the window reachable.
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
        "settings" => {
            show_window(app);
            let _ = app.emit("show-settings", ());
        }
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

fn tray_status_text(app: &AppHandle) -> &'static str {
    let state = app.state::<AppState>();
    if state.config().seventeenlands_token().is_none() {
        "Missing token"
    } else if state.is_following() {
        "Working"
    } else {
        "Stopped"
    }
}

/// The displayed status changed: re-render the tray status line and tell the webview to
/// refresh. Callable from any thread, and never blocks: the work is queued onto the main
/// thread rather than awaited, because Tauri's menu setters wait for the main thread and the
/// follower thread calls this while the main thread may be joining it (`AppState::stop`).
pub(crate) fn status_changed(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let text = tray_status_text(&handle);
        if let Some(item) = handle.try_state::<TrayStatusItem>() {
            let _ = item.0.set_text(text);
        }
        let _ = handle.emit("status-changed", ());
    });
}
