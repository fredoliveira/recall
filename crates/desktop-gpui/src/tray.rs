//! The menu-bar icon. GPUI has no status-item API, so this uses the `tray-icon` crate on
//! GPUI's own `NSApplication`. Menu choices come back as [`Signal::Menu`] on the same channel
//! as everything else, so they are handled on the main thread with the app in hand.

use smol::channel::Sender;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::feed::Signal;

pub const OPEN: &str = "open";
pub const SETTINGS: &str = "settings";
pub const QUIT: &str = "quit";

/// Must be called on the main thread, once the app has launched. Keep the returned icon
/// alive: dropping it removes it from the menu bar.
pub fn install(signals: Sender<Signal>) -> Result<TrayIcon, String> {
    let menu = Menu::new();
    menu.append_items(&[
        &MenuItem::with_id(OPEN, "Open Recall", true, None),
        &MenuItem::with_id(SETTINGS, "Settings…", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(QUIT, "Quit Recall", true, None),
    ])
    .map_err(|e| e.to_string())?;

    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = signals.try_send(Signal::Menu(event.id.0));
    }));

    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(concat!("Recall v", env!("CARGO_PKG_VERSION")))
        // A template image: macOS draws it in the menu bar's own colour.
        .with_icon_templated(icon()?)
        .build()
        .map_err(|e| e.to_string())
}

/// The app icon's ring, drawn here rather than shipped as a file: white with the ring in the
/// alpha channel, which is all a template image uses.
fn icon() -> Result<Icon, String> {
    // 22pt at 2x. The ring leaves the margin other menu-bar icons do.
    const SIZE: u32 = 44;
    const RADIUS: f32 = 13.5;
    const THICKNESS: f32 = 5.;
    let centre = (SIZE as f32 - 1.) / 2.;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let from_centre = ((x as f32 - centre).powi(2) + (y as f32 - centre).powi(2)).sqrt();
            // Distance outside the ring's band, turned into one pixel of soft edge.
            let outside = (from_centre - RADIUS).abs() - THICKNESS / 2.;
            let alpha = (0.5 - outside).clamp(0., 1.);
            rgba.extend_from_slice(&[255, 255, 255, (alpha * 255.) as u8]);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).map_err(|e| e.to_string())
}
