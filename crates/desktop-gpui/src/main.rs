//! Recall's desktop app on GPUI: a log window, a Settings window and a menu-bar icon in front
//! of the same follower core as the CLI.

mod feed;
mod log_window;
mod observer;
mod settings;
mod state;
mod style;
mod tray;

use std::sync::Arc;

use gpui_kit::*;

use feed::{Feed, Signal};
use state::AppState;

actions!(recall, [Quit, OpenLog, OpenSettings]);

/// What every window needs: the follower, and the feed of what it reports.
pub struct Shared {
    pub state: Arc<AppState>,
    pub feed: Entity<Feed>,
}

impl Global for Shared {}

/// Follow the system's light or dark mode, unless `RECALL_APPEARANCE` is `light` or `dark`
/// (for screenshots and checking both themes without changing the system setting).
pub fn apply_appearance(window: &mut Window, cx: &mut App) {
    use gpui_kit::component::{Theme, ThemeMode};
    match std::env::var("RECALL_APPEARANCE").as_deref() {
        Ok("light") => Theme::change(ThemeMode::Light, Some(window), cx),
        Ok("dark") => Theme::change(ThemeMode::Dark, Some(window), cx),
        _ => Theme::sync_system_appearance(Some(window), cx),
    }
}

fn main() {
    let (signals, incoming) = smol::channel::unbounded::<Signal>();
    // Before anything logs.
    feed::install_logger(signals.clone());

    // Dev safety: never POST to the live API during development. Set RECALL_HOST (the local
    // mock server, say) to override the configured host.
    let state = Arc::new(AppState::new(std::env::var("RECALL_HOST").ok(), signals.clone()));

    // The menu-bar icon. With it the app needs no Dock icon; without it (RECALL_TRAY=0, or if
    // it can't be made) Recall is an ordinary Dock app. The policy is set on the platform
    // before the app launches: set any later, the Dock icon shows for a moment first.
    let wants_tray = std::env::var("RECALL_TRAY").as_deref() != Ok("0");
    let platform = gpui_kit::platform::current_platform(false);
    if wants_tray {
        platform.set_activation_policy(ActivationPolicy::Accessory);
    }
    let app = gpui_kit::Application::with_platform(platform).with_assets(gpui_kit::assets::Assets);
    // Clicking the Dock icon with no window open brings the log back.
    app.on_reopen(log_window::open);
    app.run(move |cx| {
        gpui_kit::init(cx);

        let feed = cx.new(|_| Feed { status: state.status(), ..Default::default() });
        cx.set_global(Shared { state: state.clone(), feed: feed.clone() });

        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-0", OpenLog, None),
        ]);
        cx.on_action(|_: &Quit, cx: &mut App| quit(cx));
        cx.on_action(|_: &OpenLog, cx: &mut App| log_window::open(cx));
        cx.on_action(|_: &OpenSettings, cx: &mut App| settings::open(cx));
        cx.set_menus(vec![
            Menu {
                name: "Recall".into(),
                disabled: false,
                items: vec![
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::action("Quit Recall", Quit),
                ],
            },
            Menu { name: "Window".into(), disabled: false, items: vec![MenuItem::action("Log", OpenLog)] },
        ]);

        let tray = wants_tray.then(|| tray::install(signals.clone())).and_then(|made| {
            made.inspect_err(|err| log::warn!(target: "recall_desktop", "No menu-bar icon: {err}")).ok()
        });
        if wants_tray && tray.is_none() {
            // No way back to the windows without a Dock icon.
            cx.set_activation_policy(ActivationPolicy::Regular);
        }

        // Everything the follower reports arrives here, on the main thread.
        cx.spawn(async move |cx| {
            let _tray = tray; // lives as long as the app
            while let Ok(first) = incoming.recv().await {
                // Take what has queued up in one go: a busy log is one redraw, not hundreds.
                let mut batch = vec![first];
                while let Ok(more) = incoming.try_recv() {
                    batch.push(more);
                }
                cx.update(|cx| receive(batch, cx));
            }
        })
        .detach();

        // First run: with no token there's nothing to follow, so ask for one.
        let following = state.config().seventeenlands_token().is_some() && state.start().is_ok();
        log_window::open(cx);
        if !following {
            settings::open(cx);
        }
        cx.activate(true);
    });
}

fn receive(batch: Vec<Signal>, cx: &mut App) {
    let shared = cx.global::<Shared>();
    let (state, feed) = (shared.state.clone(), shared.feed.clone());
    let mut menu = Vec::new();
    feed.update(cx, |feed, cx| {
        let mut status_changed = false;
        for signal in batch {
            match signal {
                Signal::Log(line) => feed.push(line),
                Signal::Status => status_changed = true,
                Signal::Menu(id) => menu.push(id),
            }
        }
        if status_changed {
            feed.status = state.status();
        }
        cx.notify();
    });
    for id in menu {
        match id.as_str() {
            tray::OPEN => {
                log_window::open(cx);
                cx.activate(true);
            }
            tray::SETTINGS => {
                settings::open(cx);
                cx.activate(true);
            }
            tray::QUIT => quit(cx),
            _ => {}
        }
    }
}

fn quit(cx: &mut App) {
    cx.global::<Shared>().state.stop();
    cx.quit();
}
