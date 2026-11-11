// SPDX-License-Identifier: MIT

//! The stateful log follower (port of the `Follower` class in `mtga_follower.py`): owns
//! the parse state and emits [`GameEvent`]s into a [`Service`].
//!
//! Layout: this file holds the state, constructors and reset lifecycle; `tail` the follow
//! loop and entry accumulation; `dispatch` blob parsing and the dispatch table; `account`,
//! `draft`, `event`, `game` the handlers by domain. Upstream line citations stay on the
//! handlers they port.
//!
//! State modeling: dynamic blobs are `serde_json::Value`; the instance-id maps
//! use `serde_json::Map` (insertion-ordered via the `preserve_order` feature) so derived
//! lists like `opponent_card_ids` keep Python dict order. `defaultdict` semantics are
//! emulated at the read sites (`.get(...).unwrap_or_default()` etc.).

use std::collections::HashMap;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::events::{
    Context, DeckEntry, DeckSubmission, DraftPack, DraftPick, DraftSource, GameData, GameEvent,
    GameHistory, GameResult, MatchResult, Service,
};
use crate::services::seventeenlands::ApiClient;
use crate::services::seventeenlands::SeventeenLands;
use crate::time_parse::{epoch_zero, extract_time, isoformat, maybe_get_utc_timestamp};

mod account;
mod dispatch;
mod draft;
mod event;
mod game;
mod tail;

const FILE_UPDATED_FORCE_REFRESH_SECONDS: u64 = 60;
const SLEEP_TIME: Duration = Duration::from_millis(500);

/// Log target marking low-signal, periodic background-sync lines (mastery/inventory/
/// collection/ongoing-event polling). The console formatter (`main::init_logging`) renders
/// these fully dimmed so they recede behind real events; the `--verbose` developer format
/// still shows them in full. Purely cosmetic - no effect on the wire payload.
pub const CHATTER: &str = "recall::chatter";

/// Shared empty-object fallback for `blob.get(..).unwrap_or(&EMPTY_OBJECT)` in hot paths,
/// replacing per-message `json!({})` allocations and clones.
static EMPTY_OBJECT: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| json!({}));

// ---------------------------------------------------------------------------------------
// Regexes (port of mtga_follower.py:132-143). Python `.match` is start-anchored; patterns
// keep the `^` / leading `.*` from the source so `captures()` reproduces it.
// ---------------------------------------------------------------------------------------

fn re_log_start_timed() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\[(UnityCrossThreadLogger|Client GRE)\](\d[\d:/ .-]+(AM|PM)?)").unwrap()
    })
}
fn re_log_start_untimed() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\[(UnityCrossThreadLogger|Client GRE)\]").unwrap())
}
fn re_timestamp() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^([\d/.-]+[ T][\d]+:[\d]+:[\d]+( AM| PM)?)").unwrap())
}
fn re_json_start() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\[\{]").unwrap())
}
fn re_account_info() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r".*Updated account\. DisplayName:(.*), AccountID:(.*), Token:.*").unwrap()
    })
}
fn re_login() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r".*Logged in successfully\. Display Name:(.*)").unwrap())
}
fn re_match_account_info() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r".*: ((\w+) to Match|Match to (\w+)):").unwrap())
}

// ---------------------------------------------------------------------------------------
// Free helpers (ports of module-level functions).
// ---------------------------------------------------------------------------------------

/// `contains_log_key` (`mtga_follower.py:228`): substring with and without underscores.
fn contains_log_key(key: &str, full_log: &str) -> bool {
    full_log.contains(key) || full_log.contains(&key.replace('_', ""))
}

/// `json_value_matches` (`mtga_follower.py:189`): the value at `path` equals `expectation`.
fn json_value_matches(expectation: &Value, path: &[&str], blob: &Value) -> bool {
    let mut cur = blob;
    for p in path {
        match cur.get(*p) {
            Some(v) => cur = v,
            None => return false,
        }
    }
    cur == expectation
}

/// WIRE: Python `int(value)` - integer numbers as-is, floats truncated, all-integer strings parsed.
fn to_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().map(|u| u as i64))
            .or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// WIRE: `[int(x) for x in arr]` - coerce each element of a JSON array to an integer `Value`.
fn int_array(v: &Value) -> Vec<Value> {
    v.as_array()
        .map(|a| a.iter().filter_map(to_i64).map(Value::from).collect())
        .unwrap_or_default()
}

/// `Optional[Value]` → JSON (`None` → `null`).
fn opt_val(v: &Option<Value>) -> Value {
    v.clone().unwrap_or(Value::Null)
}

/// WIRE: Python `str(x)` for the rank-string components (`None` → "None", bools title-cased,
/// floats rendered with a trailing `.0` when integral).
fn python_str(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "None".to_string(),
        Some(Value::Bool(true)) => "True".to_string(),
        Some(Value::Bool(false)) => "False".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else if let Some(f) = n.as_f64() {
                if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e16 {
                    format!("{f:.1}")
                } else {
                    format!("{f}")
                }
            } else {
                n.to_string()
            }
        }
        Some(other) => other.to_string(),
    }
}

/// `__try_decode` (`mtga_follower.py:619`): if the value is a JSON-encoded string, decode
/// one value from it; on failure (or if it's already non-string) return it unchanged.
fn try_decode(v: &Value) -> Value {
    match v {
        Value::String(s) => {
            let mut it = serde_json::Deserializer::from_str(s).into_iter::<Value>();
            match it.next() {
                Some(Ok(parsed)) => parsed,
                _ => v.clone(),
            }
        }
        other => other.clone(),
    }
}

/// `__extract_payload` (`mtga_follower.py:626`): recursively unwrap `payload`/`Payload`/
/// `request`, except for `clientToMatchServiceMessageType` blobs which are returned as-is.
fn extract_payload(blob: Value) -> Value {
    if !blob.is_object() {
        return blob;
    }
    if blob.get("clientToMatchServiceMessageType").is_some() {
        return blob;
    }
    for key in ["payload", "Payload", "request"] {
        if let Some(v) = blob.get(key) {
            return extract_payload(try_decode(v));
        }
    }
    blob
}

/// `get_rank_string` (`mtga_follower.py:207`): `"-".join(str(x) for x in [...])`.
fn get_rank_string(
    rank_class: Option<&Value>,
    level: Option<&Value>,
    percentile: Option<&Value>,
    place: Option<&Value>,
    step: Option<&Value>,
) -> String {
    [rank_class, level, percentile, place, step]
        .iter()
        .map(|x| python_str(*x))
        .collect::<Vec<_>>()
        .join("-")
}

// ---------------------------------------------------------------------------------------
// Follower
// ---------------------------------------------------------------------------------------

/// Identity of the logged-in player. Kept as a snapshot pair so logout stashes the whole
/// thing and reconnect restores it (`__reset_current_user` / `__handle_reconnect_result`).
#[derive(Debug, Clone, Default)]
struct UserInfo {
    id: Option<String>,
    screen_name: Option<String>,
    full_screen_name: Option<String>,
    rank: Option<Value>,
}

/// Per-match state. `clear_match_data` drops it wholesale (port of `__clear_match_data`).
#[derive(Debug, Default)]
struct MatchState {
    screen_names: HashMap<i64, String>,
    match_id: Option<Value>,
    event_id: Option<Value>,
    seat_id: Option<i64>,
}

/// Per-game state. `clear_game_data` drops it wholesale (port of `__clear_game_data`);
/// the field set *is* that method's reset list, so nothing can be forgotten.
#[derive(Debug, Default)]
struct GameState {
    starting_team_id: Option<i64>,
    turn_count: i64,
    /// Instance id → card object, per owner seat. Insertion-ordered (`Map`) so derived
    /// lists like `opponent_card_ids` keep Python dict order.
    objects_by_owner: HashMap<i64, Map<String, Value>>,
    opening_hand_count_by_seat: HashMap<i64, i64>,
    opening_hand: HashMap<i64, Vec<Value>>,
    drawn_hands: HashMap<i64, Vec<Vec<Value>>>,
    drawn_cards_by_instance_id: HashMap<i64, Map<String, Value>>,
    maindeck: Option<Value>,
    sideboard: Option<Value>,
    additional_deck_info: Option<Value>,
    service_metadata: Option<Value>,
    client_metadata: Option<Value>,
    history_events: Vec<Value>,
    /// Result of the game in progress, once known (Python: dict, empty == absent).
    pending_result: Option<GameResult>,
    pending_match_result: Option<MatchResult>,
}

/// The parser state (port of the `Follower` class). Reset wholesale on each outer
/// tail-loop pass.
pub struct Follower<S: Service> {
    /// The event sink (the 17Lands adapter in production, a fan-out, or a recorder in
    /// tests). Public for test inspection.
    pub sink: S,

    // Timing
    cur_log_time: chrono::NaiveDateTime,
    last_utc_time: chrono::NaiveDateTime,
    last_event_time: Option<Value>,
    last_raw_time: String,

    // Identity
    user: UserInfo,
    disconnected: UserInfo,

    // Draft / opponent
    cur_draft_event: Option<Value>,
    cur_opponent_level: Option<String>,
    cur_opponent_match_id: Option<Value>,

    match_state: MatchState,
    game: GameState,
    /// Hand contents by owner seat. WIRE: upstream never clears this per game (only on
    /// reinitialize); entries are overwritten on each zone update.
    cards_in_hand: HashMap<i64, Vec<Value>>,
    /// A completed game's data awaiting its result (`__enqueue_game_data`).
    pending_game: Option<GameData>,

    // Buffering
    buffer: Vec<String>,
    last_blob: String,
}

impl Follower<SeventeenLands<ApiClient>> {
    /// Build a Follower whose only service is 17Lands over HTTP.
    pub fn new(token: String, host: String) -> Self {
        Self::with_sink(SeventeenLands::new(token, ApiClient::new(host)))
    }
}

impl<S: Service> Follower<S> {
    /// Build a Follower emitting into an arbitrary event sink (the test / fan-out seam).
    pub fn with_sink(sink: S) -> Self {
        let mut f = Self {
            sink,
            cur_log_time: epoch_zero(),
            last_utc_time: epoch_zero(),
            last_event_time: None,
            last_raw_time: String::new(),
            user: UserInfo::default(),
            disconnected: UserInfo::default(),
            cur_draft_event: None,
            cur_opponent_level: None,
            cur_opponent_match_id: None,
            match_state: MatchState::default(),
            game: GameState::default(),
            cards_in_hand: HashMap::new(),
            pending_game: None,
            buffer: Vec::new(),
            last_blob: String::new(),
        };
        f.reinitialize();
        f
    }

    /// Port of `_reinitialize`: reset all state, ending with `__clear_match_data`.
    fn reinitialize(&mut self) {
        self.buffer.clear();
        self.cur_log_time = epoch_zero();
        self.last_utc_time = epoch_zero();
        self.last_event_time = None;
        self.last_raw_time = String::new();
        self.user = UserInfo::default();
        self.disconnected = UserInfo::default();
        self.cur_draft_event = None;
        self.cur_opponent_level = None;
        self.cur_opponent_match_id = None;
        self.cards_in_hand.clear();
        self.pending_game = None;
        self.last_blob = String::new();
        self.clear_match_data(false);
    }

    /// The parse-time [`Context`] for an event emitted right now.
    fn context(&self) -> Context {
        Context {
            player_id: self.user.id.clone(),
            log_time: self.cur_log_time,
            utc_time: self.last_utc_time,
            event_time: self.last_event_time.clone(),
            raw_time: self.last_raw_time.clone(),
        }
    }

    /// Port of `__clear_game_data`: optionally flush a pending game, then drop the
    /// per-game state.
    fn clear_game_data(&mut self, submit_pending_game: bool) {
        if submit_pending_game {
            self.maybe_submit_pending_game();
        }
        self.game = GameState::default();
    }

    /// Port of `__clear_match_data`: drop the per-match state, then the game's.
    fn clear_match_data(&mut self, submit_pending_game: bool) {
        self.match_state = MatchState::default();
        self.clear_game_data(submit_pending_game);
    }
}
