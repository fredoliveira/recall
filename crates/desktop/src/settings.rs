//! The Settings window (⌘,): the 17Lands token and the log file to follow. Both are saved
//! the moment they're set, and the follower restarts with the change.

use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::*;

use crate::feed::Feed;
use crate::state::AppState;
use crate::{Shared, apply_appearance, style};

/// The open Settings window, so ⌘, brings it forward instead of opening a second one.
struct SettingsWindow(AnyWindowHandle, WeakEntity<SettingsView>);

impl Global for SettingsWindow {}

pub fn open(cx: &mut App) {
    // The view is gone once the window closes, and then ⌘, opens a new one.
    let open = cx.try_global::<SettingsWindow>().filter(|w| w.1.upgrade().is_some()).map(|w| w.0);
    if let Some(handle) = open
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some("Settings".into()), ..Default::default() }),
        window_bounds: Some(WindowBounds::centered(size(px(style::SETTINGS_WIDTH), px(style::SETTINGS_HEIGHT)), cx)),
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        apply_appearance(window, cx);
        cx.new(|cx| SettingsView::new(window, cx))
    });
    match opened {
        Ok((handle, view)) => cx.set_global(SettingsWindow(handle, view.downgrade())),
        Err(err) => eprintln!("[recall] couldn't open Settings: {err:#}"),
    }
}

/// What the last Save did, or why it failed (true for a failure).
type Note = Option<(SharedString, bool)>;

pub struct SettingsView {
    feed: Entity<Feed>,
    state: std::sync::Arc<AppState>,
    token: Entity<InputState>,
    path: Entity<InputState>,
    token_note: Note,
    path_note: Note,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shared = cx.global::<Shared>();
        let (feed, state) = (shared.feed.clone(), shared.state.clone());
        let followed = feed.read(cx).status.log_path.clone().unwrap_or_default();

        let token =
            cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("00000000-0000-4000-8000-000000000000"));
        let path = cx.new(|cx| InputState::new(window, cx).placeholder("/path/to/Player.log").default_value(followed));
        token.update(cx, |state, cx| state.focus(window, cx));

        let subscriptions = vec![
            cx.subscribe_in(&token, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.save_token(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            }),
            cx.subscribe_in(&path, window, |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::PressEnter { .. } => this.save_path(cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            }),
            cx.observe(&feed, |_, _, cx| cx.notify()),
            cx.observe_window_appearance(window, |_, window, cx| apply_appearance(window, cx)),
        ];
        Self { feed, state, token, path, token_note: None, path_note: None, _subscriptions: subscriptions }
    }

    fn save_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let token = self.token.read(cx).value().trim().to_string();
        if token.is_empty() {
            return;
        }
        self.token_note = Some(match self.state.save_token(&token) {
            Ok(()) => {
                self.token.update(cx, |state, cx| state.set_value("", window, cx));
                ("Saved. Recall is following with this token.".into(), false)
            }
            Err(err) => (err.into(), true),
        });
        cx.notify();
    }

    fn save_path(&mut self, cx: &mut Context<Self>) {
        let path = self.path.read(cx).value().trim().to_string();
        if path.is_empty() {
            return;
        }
        self.path_note = Some(match self.state.follow(&path) {
            Ok(()) => ("Recall is following this file.".into(), false),
            Err(err) => (err.into(), true),
        });
        cx.notify();
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg, muted, danger) = (theme.background, theme.foreground, theme.muted_foreground, theme.danger);
        let status = self.feed.read(cx).status.clone();
        let token_empty = self.token.read(cx).value().trim().is_empty();
        let path_value = self.path.read(cx).value().trim().to_string();
        let path_unchanged = path_value.is_empty() || Some(&path_value) == status.log_path.as_ref();

        let about_token = match &status.token_hint {
            Some(tail) => format!("A token ending in {tail} is saved. Enter a new one to replace it."),
            None => "Find it on your 17Lands account page. It's kept in Recall's config file on this Mac.".into(),
        };
        // A note about the last save replaces the description until the window closes.
        let caption = |note: &Note, about: String| {
            let (text, color) = match note {
                Some((text, failed)) => (text.clone(), if *failed { danger } else { muted }),
                None => (about.into(), muted),
            };
            div().text_size(px(style::SMALL_TEXT)).text_color(color).child(text)
        };
        let section = |label: &'static str| {
            v_flex().w_full().gap(px(style::SETTINGS_GAP)).child(div().font_medium().child(label))
        };

        v_flex()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .text_size(px(style::TEXT))
            .p(px(style::SETTINGS_PADDING))
            .gap(px(style::SETTINGS_SECTION_GAP))
            .child(
                section("17Lands token")
                    .child(
                        h_flex()
                            .w_full()
                            .gap(px(style::SETTINGS_GAP))
                            .child(div().flex_1().child(Input::new(&self.token).mask_toggle()))
                            .child(
                                Button::new("save-token")
                                    .primary()
                                    .label("Save")
                                    .disabled(token_empty)
                                    .on_click(cx.listener(|this, _, window, cx| this.save_token(window, cx))),
                            ),
                    )
                    .child(caption(&self.token_note, about_token)),
            )
            .child(
                section("Log file")
                    .child(
                        h_flex()
                            .w_full()
                            .gap(px(style::SETTINGS_GAP))
                            .child(div().flex_1().child(Input::new(&self.path)))
                            .child(
                                Button::new("save-path")
                                    .label("Follow")
                                    .disabled(path_unchanged)
                                    .on_click(cx.listener(|this, _, _, cx| this.save_path(cx))),
                            ),
                    )
                    .child(caption(
                        &self.path_note,
                        "Recall finds Player.log on its own. Change this only if yours lives elsewhere.".into(),
                    )),
            )
            .child(div().mt_auto().text_size(px(style::SMALL_TEXT)).text_color(muted).child(format!(
                "Recall {} · uploading to {}",
                env!("CARGO_PKG_VERSION"),
                status.host
            )))
    }
}
