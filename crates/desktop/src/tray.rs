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

/// The app icon's temple, drawn here rather than shipped as a file: white with the shape in the
/// alpha channel, which is all a template image uses. A shrine with its door on top of three
/// sloped tiers, split by the stairway up the middle.
fn icon() -> Result<Icon, String> {
    // 22pt at 2x; coordinates are in those pixels, y down.
    const SIZE: u32 = 44;
    const MID: f32 = SIZE as f32 / 2.;
    // A band from `top` to `bottom`, centred, `w0` wide at the top and `w1` at the bottom.
    struct Band {
        top: f32,
        bottom: f32,
        w0: f32,
        w1: f32,
    }
    impl Band {
        fn contains(&self, x: f32, y: f32) -> bool {
            let t = (y - self.top) / (self.bottom - self.top);
            (0. ..1.).contains(&t) && (x - MID).abs() < (self.w0 + (self.w1 - self.w0) * t) / 2.
        }
    }
    let band = |top, bottom, w0, w1| Band { top, bottom, w0, w1 };
    let solid = [
        band(6., 14., 12., 12.), // shrine
        band(16., 24., 16., 24.),
        band(26., 32., 26., 32.),
        band(34., 40., 34., 40.),
    ];
    // The door, right of centre as in the app icon, then the stairway.
    let door = |x: f32, y: f32| (23. ..26.).contains(&x) && (9. ..14.).contains(&y);
    let stairs = band(16., 40., 4., 8.);
    let inside = |x, y| solid.iter().any(|b| b.contains(x, y)) && !door(x, y) && !stairs.contains(x, y);

    // Coverage from an 8×8 grid of samples per pixel, for antialiased slopes.
    const SAMPLES: u32 = 8;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut hits = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let at = |p: u32, s: u32| p as f32 + (s as f32 + 0.5) / SAMPLES as f32;
                    hits += u32::from(inside(at(x, sx), at(y, sy)));
                }
            }
            let alpha = (hits * 255 + SAMPLES * SAMPLES / 2) / (SAMPLES * SAMPLES);
            rgba.extend_from_slice(&[255, 255, 255, alpha as u8]);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).map_err(|e| e.to_string())
}
