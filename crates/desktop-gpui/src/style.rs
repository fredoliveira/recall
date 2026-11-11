//! Every size in the app's windows, in one place to tweak. All values are in points.
//! Colours come from the theme (light or dark), not from here.

// ── Log window ───────────────────────────────────────────────────────────────

pub const WINDOW_WIDTH: f32 = 760.;
pub const WINDOW_HEIGHT: f32 = 520.;
pub const WINDOW_MIN_WIDTH: f32 = 480.;
pub const WINDOW_MIN_HEIGHT: f32 = 320.;

/// Between the window's sides and everything in it.
pub const MARGIN: f32 = 16.;

// ── Type ─────────────────────────────────────────────────────────────────────

pub const TEXT: f32 = 13.;
pub const SMALL_TEXT: f32 = 11.;
/// "Recall" in the title bar.
pub const TITLE_TEXT: f32 = 14.;
/// The log itself, in the monospaced font.
pub const LOG_TEXT: f32 = 11.;

// ── Title bar ────────────────────────────────────────────────────────────────

/// Space between the settings button and the window's right edge.
pub const TITLE_BAR_PADDING_RIGHT: f32 = 8.;

// ── Status line ──────────────────────────────────────────────────────────────

pub const STATUS_PADDING_Y: f32 = 8.;
/// The coloured dot, and the space between it and the text.
pub const STATUS_DOT: f32 = 8.;
pub const STATUS_GAP: f32 = 8.;

// ── Log ──────────────────────────────────────────────────────────────────────

/// Every line is one row of this height (the list only draws the rows on screen).
pub const LOG_ROW_HEIGHT: f32 = 18.;
/// Between the time and the message.
pub const LOG_GAP: f32 = 10.;
pub const LOG_PADDING_Y: f32 = 6.;
/// How dim background chatter is.
pub const CHATTER_OPACITY: f32 = 0.55;
/// Space above the empty-state line.
pub const EMPTY_TOP: f32 = 120.;

// ── Footer ───────────────────────────────────────────────────────────────────

pub const FOOTER_PADDING_Y: f32 = 8.;
pub const FOOTER_GAP: f32 = 12.;

// ── Notice ───────────────────────────────────────────────────────────────────
// The card that asks for a token.

pub const NOTICE_PADDING_X: f32 = 14.;
pub const NOTICE_PADDING_Y: f32 = 10.;
pub const NOTICE_RADIUS: f32 = 12.;
pub const NOTICE_GAP: f32 = 10.;

// ── Settings window ──────────────────────────────────────────────────────────

pub const SETTINGS_WIDTH: f32 = 500.;
pub const SETTINGS_HEIGHT: f32 = 290.;
/// Around the window's contents.
pub const SETTINGS_PADDING: f32 = 20.;
/// Between the two settings.
pub const SETTINGS_SECTION_GAP: f32 = 22.;
/// Between a setting's label, field and description.
pub const SETTINGS_GAP: f32 = 8.;
