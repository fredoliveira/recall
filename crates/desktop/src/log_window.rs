//! The main window: the log of what Recall did, and one status line under it.

use std::rc::Rc;

use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::feed::{Feed, LogLine};
use crate::state::{AppState, Status};
use crate::{OpenSettings, Shared, apply_appearance, style};

/// The open log window, so opening it again brings it forward instead of making a second.
struct LogWindow(AnyWindowHandle, WeakEntity<LogView>);

impl Global for LogWindow {}

pub fn open(cx: &mut App) {
    // The view is gone once the window closes, and then this opens a new one.
    let open = cx.try_global::<LogWindow>().filter(|w| w.1.upgrade().is_some()).map(|w| w.0);
    if let Some(handle) = open
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    // A transparent title bar: the view draws its own, so it can hold the settings button.
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some("Recall".into()), ..TitleBar::title_bar_options() }),
        window_bounds: Some(WindowBounds::centered(size(px(style::WINDOW_WIDTH), px(style::WINDOW_HEIGHT)), cx)),
        window_min_size: Some(size(px(style::WINDOW_MIN_WIDTH), px(style::WINDOW_MIN_HEIGHT))),
        ..TitleBar::window_options()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        apply_appearance(window, cx);
        cx.new(|cx| LogView::new(window, cx))
    });
    match opened {
        Ok((handle, view)) => cx.set_global(LogWindow(handle, view.downgrade())),
        Err(err) => eprintln!("[recall] couldn't open the log window: {err:#}"),
    }
}

pub struct LogView {
    feed: Entity<Feed>,
    state: std::sync::Arc<AppState>,
    /// The lines on screen: a copy of the feed's, shared with the list.
    rows: Rc<Vec<LogLine>>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl LogView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shared = cx.global::<Shared>();
        let (feed, state) = (shared.feed.clone(), shared.state.clone());
        let subscriptions = vec![
            cx.observe(&feed, |this, _, cx| this.refresh(cx)),
            cx.observe_window_appearance(window, |_, window, cx| apply_appearance(window, cx)),
            // The one change nothing announces is a hand-edit of config.toml; pick it up
            // when the user comes back to the window.
            cx.observe_window_activation(window, |this, window, _| {
                if window.is_window_active() {
                    this.state.reload_config();
                    this.state.notify();
                }
            }),
        ];
        let mut view = Self {
            feed,
            state,
            rows: Rc::new(Vec::new()),
            scroll: UniformListScrollHandle::default(),
            _subscriptions: subscriptions,
        };
        view.refresh(cx);
        view
    }

    /// The feed changed: take its lines, and keep the newest in view if it was.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let at_bottom = {
            let handle = &self.scroll.0.borrow().base_handle;
            handle.offset().y <= -handle.max_offset().y + px(style::LOG_ROW_HEIGHT)
        };
        let rows: Vec<LogLine> = self.feed.read(cx).lines.iter().cloned().collect();
        if at_bottom && !rows.is_empty() {
            self.scroll.scroll_to_item(rows.len() - 1, ScrollStrategy::Bottom);
        }
        self.rows = Rc::new(rows);
        cx.notify();
    }
}

/// What the status line reports.
enum Health {
    Following,
    Stopped,
    NoToken,
}

fn health(status: &Status) -> Health {
    if !status.token_present {
        Health::NoToken
    } else if status.following {
        Health::Following
    } else {
        Health::Stopped
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn plural(n: u64, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

impl Render for LogView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg, muted, border) = (theme.background, theme.foreground, theme.muted_foreground, theme.border);
        let (success, warning, danger) = (theme.success, theme.warning, theme.danger);
        let mono = theme.mono_font_family.clone();
        let status = self.feed.read(cx).status.clone();
        let health = health(&status);

        // The title is laid over the whole bar, so it's centred on the window rather than on
        // the space the bar leaves right of the traffic lights. It has no mouse handlers, so
        // clicks and drags pass through to the bar and the button.
        let title_bar = div()
            .relative()
            .child(
                TitleBar::new().border_b_0().bg(bg).child(
                    h_flex().flex_1().justify_end().pr(px(style::TITLE_BAR_PADDING_RIGHT)).child(
                        Button::new("settings")
                            .ghost()
                            .small()
                            .icon(IconName::Settings)
                            .tooltip("Settings (⌘,)")
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx)),
                    ),
                ),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(style::TITLE_TEXT))
                    .font_medium()
                    .text_color(muted)
                    .child("Recall"),
            );

        let needs_token = matches!(health, Health::NoToken).then(|| {
            h_flex()
                .mx(px(style::MARGIN))
                .px(px(style::NOTICE_PADDING_X))
                .py(px(style::NOTICE_PADDING_Y))
                .gap(px(style::NOTICE_GAP))
                .rounded(px(style::NOTICE_RADIUS))
                .border_1()
                .border_color(border)
                .text_size(px(style::TEXT))
                .child(Icon::new(IconName::Info).text_color(muted))
                .child(div().flex_1().child("Recall needs your 17Lands token to upload your drafts and games."))
                .child(
                    Button::new("open-settings")
                        .primary()
                        .small()
                        .label("Open Settings")
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx)),
                )
        });

        let rows = self.rows.clone();
        let log = if rows.is_empty() {
            div()
                .flex_1()
                .w_full()
                .pt(px(style::EMPTY_TOP))
                .flex()
                .justify_center()
                .text_size(px(style::TEXT))
                .text_color(muted)
                .child("Nothing yet. Activity shows up here as you play.")
                .into_any_element()
        } else {
            uniform_list("log", rows.len(), move |range, _, _| {
                range
                    .map(|i| {
                        let line = &rows[i];
                        let color = match line.level {
                            log::Level::Error => danger,
                            log::Level::Warn => warning,
                            log::Level::Debug | log::Level::Trace => muted,
                            log::Level::Info => fg,
                        };
                        h_flex()
                            .h(px(style::LOG_ROW_HEIGHT))
                            .w_full()
                            .px(px(style::MARGIN))
                            .gap(px(style::LOG_GAP))
                            .child(div().flex_none().text_color(muted).child(line.ts.clone()))
                            .child(div().flex_1().min_w_0().truncate().text_color(color).child(line.msg.clone()))
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .flex_1()
            .w_full()
            .py(px(style::LOG_PADDING_Y))
            .font_family(mono)
            .text_size(px(style::LOG_TEXT))
            .into_any_element()
        };

        // The status: how Recall is doing on the left, how much it has uploaded on the right.
        let (dot, text, text_color): (Hsla, SharedString, Hsla) = match health {
            Health::NoToken => (danger, "No 17Lands token".into(), fg),
            Health::Stopped if status.log_path.is_none() => (muted, "Player.log not found".into(), fg),
            Health::Stopped => (muted, "Not following".into(), fg),
            Health::Following if status.upload_failed > 0 => (
                warning,
                format!("Last failure: {}", status.last_failure.as_deref().unwrap_or("unknown")).into(),
                warning,
            ),
            Health::Following => match (&status.last_endpoint, &status.last_time) {
                (Some(endpoint), Some(time)) => {
                    (success, format!("Last upload: {} at {time}", file_name(endpoint)).into(), muted)
                }
                _ => (success, "Nothing uploaded yet".into(), muted),
            },
        };
        let tally: Option<(SharedString, Hsla)> = if status.upload_failed > 0 {
            Some((format!("{} uploaded · {} failed", status.upload_count, status.upload_failed).into(), warning))
        } else if status.upload_count > 0 {
            Some((plural(status.upload_count, "upload").into(), muted))
        } else {
            None
        };
        let footer = h_flex()
            .w_full()
            .px(px(style::MARGIN))
            .py(px(style::FOOTER_PADDING_Y))
            .gap(px(style::STATUS_GAP))
            .border_t_1()
            .border_color(border)
            .text_size(px(style::SMALL_TEXT))
            .child(div().flex_none().size(px(style::STATUS_DOT)).rounded_full().bg(dot))
            .child(div().flex_1().min_w_0().truncate().text_color(text_color).child(text))
            .when_some(tally, |this, (text, color)| this.child(div().flex_none().text_color(color).child(text)));

        v_flex().size_full().bg(bg).text_color(fg).child(title_bar).children(needs_token).child(log).child(footer)
    }
}
