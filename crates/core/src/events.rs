// SPDX-License-Identifier: MIT

//! The service-agnostic event layer between the parser and the services.
//!
//! The [`Follower`](crate::follower::Follower) emits [`GameEvent`]s: typed, credential-free
//! descriptions of what happened in the log, defined by **what the log contains**, not by
//! what any service's API wants. Each service (see [`crate::services`]) implements
//! [`Service`] and translates events into its own wire format; events are passed by
//! reference so any number of services can consume the same stream.
//!
//! Field typing follows the log: numbers the log always carries as integers are `i64`;
//! ids and names the log carries loosely are passed through as `Value` (strings in
//! practice), so no adapter ever loses information the upstream client would have sent.
//! Every event also carries the raw MTGA blob it was parsed from where one exists.

use chrono::NaiveDateTime;
use serde_json::{Map, Value};

use crate::time_parse::isoformat;

/// Parse-time context attached to every event: who and when, as known to the parser at
/// emission time. Mirrors the context half of upstream's `_add_base_api_data`.
#[derive(Debug, Clone)]
pub struct Context {
    /// The MTGA account id, when known.
    pub player_id: Option<String>,
    /// Timestamp of the log line that produced the event (naive, as logged).
    pub log_time: NaiveDateTime,
    /// Last UTC timestamp seen in the log.
    pub utc_time: NaiveDateTime,
    /// Upstream quirk: the blob's `EventTime`, usually absent.
    pub event_time: Option<Value>,
    /// The raw timestamp text of the log line.
    pub raw_time: String,
}

impl Context {
    /// The five context fields as an ordered JSON map (`player_id`, `time`, `utc_time`,
    /// `event_time`, `raw_time`), formatted exactly as the wire contract requires -
    /// shared by service adapters and local sinks. Fields are emitted even when null.
    pub fn to_map(&self) -> Map<String, Value> {
        // WIRE: key order and null-not-absent are part of the 17Lands contract.
        let mut m = Map::new();
        m.insert("player_id".into(), opt_string(&self.player_id));
        m.insert("time".into(), Value::String(isoformat(&self.log_time)));
        m.insert("utc_time".into(), Value::String(isoformat(&self.utc_time)));
        m.insert("event_time".into(), opt_value(&self.event_time));
        m.insert("raw_time".into(), Value::String(self.raw_time.clone()));
        m
    }
}

#[cfg(test)]
impl Context {
    /// A fixed context for unit tests across the crate.
    pub fn test_fixture() -> Self {
        let t = |s| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap();
        Context {
            player_id: Some("P1".into()),
            log_time: t("2024-02-02 13:00:00"),
            utc_time: t("2024-02-02 13:00:01"),
            event_time: None,
            raw_time: "2/2/2024 1:00:00 PM".into(),
        }
    }
}

/// `Some(v)` → `v`, `None` → `null`.
pub fn opt_value(v: &Option<Value>) -> Value {
    v.clone().unwrap_or(Value::Null)
}

/// `Some(s)` → string, `None` → `null`.
pub fn opt_string(v: &Option<String>) -> Value {
    v.as_ref()
        .map(|s| Value::String(s.clone()))
        .unwrap_or(Value::Null)
}

/// `Some(n)` → number, `None` → `null`.
pub fn opt_i64(v: Option<i64>) -> Value {
    v.map(Value::from).unwrap_or(Value::Null)
}

/// Which log message a draft event was parsed from. The sources differ in what they
/// carry (see the field docs on [`DraftPack`] / [`DraftPick`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftSource {
    /// Bot (quick) draft: `DraftStatus` packs, `BotDraft_DraftPick` picks.
    BotDraft,
    /// Human draft, `LogBusinessEvents` entry carrying pack and pick together.
    LogBusiness,
    /// Human draft pack from `Draft.Notify`.
    DraftNotify,
    /// Human draft pick from `EventPlayerDraftMakePick`.
    PlayerDraftMakePick,
}

/// A draft pack as presented to the player.
#[derive(Debug, Clone)]
pub struct DraftPack {
    pub source: DraftSource,
    /// Absent for bot drafts.
    pub draft_id: Option<Value>,
    pub event_name: Option<Value>,
    pub pack_number: Option<i64>,
    pub pick_number: Option<i64>,
    /// The card ids in the pack: an array of integers, or `null` when the source lacked
    /// them (`LogBusiness` passes the logged value through as-is).
    pub card_ids: Value,
    /// The log blob the pack was parsed from.
    pub raw: Value,
    pub ctx: Context,
}

/// A draft pick made by the player.
#[derive(Debug, Clone)]
pub struct DraftPick {
    pub source: DraftSource,
    /// Absent for bot drafts.
    pub draft_id: Option<Value>,
    pub event_name: Option<Value>,
    pub pack_number: Option<i64>,
    pub pick_number: Option<i64>,
    /// The picked card (`BotDraft`, `LogBusiness`); `PlayerDraftMakePick` reports
    /// `card_ids` instead.
    pub card_id: Option<i64>,
    /// `BotDraft`: the pack contents at pick time; `PlayerDraftMakePick`: the picked
    /// card(s) (`GrpIds`, passed through as logged).
    pub card_ids: Option<Value>,
    /// `LogBusiness` only.
    pub auto_pick: Option<Value>,
    /// `LogBusiness` only.
    pub time_remaining: Option<Value>,
    /// The log blob the pick was parsed from; absent for bot picks (upstream never
    /// echoed it).
    pub raw: Option<Value>,
    pub ctx: Context,
}

/// One `(cardId, quantity)` line of a submitted deck, as logged.
#[derive(Debug, Clone)]
pub struct DeckEntry {
    pub card_id: Value,
    pub quantity: i64,
}

/// A deck submitted for an event (`Event_SetDeck`).
#[derive(Debug, Clone)]
pub struct DeckSubmission {
    pub event_name: Option<Value>,
    pub maindeck: Vec<DeckEntry>,
    pub sideboard: Vec<DeckEntry>,
    /// The companion card id, if one was submitted.
    pub companion: Option<Value>,
    pub raw: Value,
    pub ctx: Context,
}

/// Per-game replay data assembled by the parser's game-state machine.
///
/// This is log-derived data any service wanting game records needs (hands, draws, decks,
/// the GRE event history); it is the shared representation, not a 17Lands shape -
/// though its field set was naturally first established by that client.
#[derive(Debug, Clone)]
pub struct GameData {
    pub event_name: Option<Value>,
    pub match_id: Option<Value>,
    pub on_play: bool,
    /// Card ids in the kept opening hand.
    pub opening_hand: Vec<Value>,
    /// Each hand mulliganed away (card ids), in order.
    pub mulligans: Vec<Vec<Value>>,
    /// Every hand drawn (mulligans + kept), in order.
    pub drawn_hands: Vec<Vec<Value>>,
    /// Card ids drawn during the game, in draw order.
    pub drawn_cards: Vec<Value>,
    pub mulligan_count: i64,
    pub opponent_mulligan_count: i64,
    pub turns: i64,
    /// Card ids the opponent revealed, in first-seen order.
    pub opponent_card_ids: Vec<Value>,
    /// The player's rank info at game time, if known.
    pub rank_data: Option<Value>,
    /// The opponent's rank class, if known.
    pub opponent_rank: Option<String>,
    pub maindeck_card_ids: Option<Value>,
    pub sideboard_card_ids: Option<Value>,
    pub additional_deck_info: Option<Value>,
    pub service_metadata: Option<Value>,
    pub client_metadata: Option<Value>,
    pub history: GameHistory,
}

/// The GRE message history of a game, with the seats it was recorded from.
#[derive(Debug, Clone)]
pub struct GameHistory {
    pub seat_id: Option<i64>,
    pub opponent_seat_id: i64,
    pub screen_name: String,
    pub opponent_screen_name: String,
    /// Per-message history entries as built by the parser (see `follower`).
    pub events: Vec<Value>,
}

/// How one game ended. Comes from either the GRE `GameStage_GameOver` results or the
/// `LogBusinessEvents` game-end entry (which additionally carries its payload).
#[derive(Debug, Clone)]
pub struct GameResult {
    /// The `LogBusinessEvents` payload, when that was the source.
    pub game_end_payload: Option<Value>,
    /// Game number within the match: the logged value (`LogBusinessEvents`) or the
    /// count of game-scope results so far (GRE), as the source provides.
    pub game_number: Value,
    pub won: bool,
    pub win_type: Option<Value>,
    pub game_end_reason: Option<Value>,
}

/// How the enclosing match ended, when the game that just ended also ended the match.
#[derive(Debug, Clone)]
pub struct MatchResult {
    pub won_match: bool,
    pub match_result_type: Option<Value>,
    pub match_end_reason: Option<Value>,
    /// The `matchGameRoomStateChangedEvent` blob, when that was the source.
    pub match_result_payload: Option<Value>,
}

/// A parsed, submittable occurrence from the log - the neutral currency between the
/// parser and all services.
#[derive(Debug, Clone)]
pub enum GameEvent {
    /// The player's screen name became known or changed.
    ScreenNameUpdated {
        screen_name: Option<String>,
        full_screen_name: Option<String>,
        ctx: Context,
    },
    /// The player joined an event (`Event_Join` response carrying a `Course`).
    EventJoined {
        raw: Value,
        ctx: Context,
    },
    DraftPack(DraftPack),
    DraftPick(DraftPick),
    DeckSubmitted(DeckSubmission),
    /// `Event_GetCourses`: the player's ongoing event courses, as logged.
    OngoingEvents {
        courses: Option<Value>,
        ctx: Context,
    },
    /// `Event_ClaimPrize`: an event finished.
    EventEnded {
        event_name: Option<Value>,
        ctx: Context,
    },
    /// `Draft_CompleteDraft`: a draft's final card pool.
    EventCourse {
        event_name: Option<Value>,
        draft_id: Option<Value>,
        course_id: Option<Value>,
        card_pool: Option<Value>,
        raw: Value,
        ctx: Context,
    },
    /// `Rank_GetCombinedRankInfo`: the player's combined rank info blob, as logged.
    RankUpdated {
        rank_data: Value,
        ctx: Context,
    },
    /// `PlayerInventory.GetPlayerCardsV3`: owned card counts keyed by card id.
    CollectionUpdated {
        card_counts: Value,
        ctx: Context,
    },
    /// `DTO_InventoryInfo`: the full inventory blob, as logged. Services filter what
    /// they want (e.g. 17Lands keeps only currency/wildcard/booster fields).
    InventoryUpdated {
        inventory: Value,
        ctx: Context,
    },
    /// Mastery-pass progress (`NodeStates`/`RewardTierUpgrade`), as logged.
    MasteryProgress {
        progress: Value,
        ctx: Context,
    },
    /// A game finished and its record is complete (data + result, plus the match
    /// result when the game also ended the match). Boxed: the record dwarfs every other
    /// variant and `GameEvent` is passed around by value.
    GameCompleted {
        game: Box<GameData>,
        result: GameResult,
        match_result: Option<MatchResult>,
        ctx: Context,
    },
}

impl GameEvent {
    /// Stable snake_case name of the variant - the event's identity for generic
    /// consumers (archives, logs).
    pub fn name(&self) -> &'static str {
        match self {
            GameEvent::ScreenNameUpdated { .. } => "screen_name_updated",
            GameEvent::EventJoined { .. } => "event_joined",
            GameEvent::DraftPack(_) => "draft_pack",
            GameEvent::DraftPick(_) => "draft_pick",
            GameEvent::DeckSubmitted(_) => "deck_submitted",
            GameEvent::OngoingEvents { .. } => "ongoing_events",
            GameEvent::EventEnded { .. } => "event_ended",
            GameEvent::EventCourse { .. } => "event_course",
            GameEvent::RankUpdated { .. } => "rank_updated",
            GameEvent::CollectionUpdated { .. } => "collection_updated",
            GameEvent::InventoryUpdated { .. } => "inventory_updated",
            GameEvent::MasteryProgress { .. } => "mastery_progress",
            GameEvent::GameCompleted { .. } => "game_completed",
        }
    }

    /// The who/when context every variant carries.
    pub fn context(&self) -> &Context {
        match self {
            GameEvent::DraftPack(p) => &p.ctx,
            GameEvent::DraftPick(p) => &p.ctx,
            GameEvent::DeckSubmitted(d) => &d.ctx,
            GameEvent::ScreenNameUpdated { ctx, .. }
            | GameEvent::EventJoined { ctx, .. }
            | GameEvent::OngoingEvents { ctx, .. }
            | GameEvent::EventEnded { ctx, .. }
            | GameEvent::EventCourse { ctx, .. }
            | GameEvent::RankUpdated { ctx, .. }
            | GameEvent::CollectionUpdated { ctx, .. }
            | GameEvent::InventoryUpdated { ctx, .. }
            | GameEvent::MasteryProgress { ctx, .. }
            | GameEvent::GameCompleted { ctx, .. } => ctx,
        }
    }

    /// Generic JSON view (context fields + event data) for consumers that want events
    /// as-is rather than a service's wire format - archives, logs.
    pub fn to_json(&self) -> Value {
        fn with<F: FnOnce(&mut Map<String, Value>)>(ctx: &Context, f: F) -> Value {
            let mut m = ctx.to_map();
            f(&mut m);
            Value::Object(m)
        }
        fn entries(v: &[DeckEntry]) -> Value {
            Value::Array(
                v.iter()
                    .map(|e| {
                        let mut m = Map::new();
                        m.insert("card_id".into(), e.card_id.clone());
                        m.insert("quantity".into(), Value::from(e.quantity));
                        Value::Object(m)
                    })
                    .collect(),
            )
        }
        match self {
            GameEvent::ScreenNameUpdated {
                screen_name,
                full_screen_name,
                ctx,
            } => with(ctx, |m| {
                m.insert("screen_name".into(), opt_string(screen_name));
                m.insert("full_screen_name".into(), opt_string(full_screen_name));
            }),
            GameEvent::EventJoined { raw, ctx } => with(ctx, |m| {
                m.insert("payload".into(), raw.clone());
            }),
            GameEvent::DraftPack(p) => with(&p.ctx, |m| {
                m.insert("source".into(), Value::String(format!("{:?}", p.source)));
                m.insert("draft_id".into(), opt_value(&p.draft_id));
                m.insert("event_name".into(), opt_value(&p.event_name));
                m.insert("pack_number".into(), opt_i64(p.pack_number));
                m.insert("pick_number".into(), opt_i64(p.pick_number));
                m.insert("card_ids".into(), p.card_ids.clone());
                m.insert("payload".into(), p.raw.clone());
            }),
            GameEvent::DraftPick(p) => with(&p.ctx, |m| {
                m.insert("source".into(), Value::String(format!("{:?}", p.source)));
                m.insert("draft_id".into(), opt_value(&p.draft_id));
                m.insert("event_name".into(), opt_value(&p.event_name));
                m.insert("pack_number".into(), opt_i64(p.pack_number));
                m.insert("pick_number".into(), opt_i64(p.pick_number));
                m.insert("card_id".into(), opt_i64(p.card_id));
                m.insert("card_ids".into(), opt_value(&p.card_ids));
                m.insert("auto_pick".into(), opt_value(&p.auto_pick));
                m.insert("time_remaining".into(), opt_value(&p.time_remaining));
                m.insert("payload".into(), opt_value(&p.raw));
            }),
            GameEvent::DeckSubmitted(d) => with(&d.ctx, |m| {
                m.insert("event_name".into(), opt_value(&d.event_name));
                m.insert("maindeck".into(), entries(&d.maindeck));
                m.insert("sideboard".into(), entries(&d.sideboard));
                m.insert("companion".into(), opt_value(&d.companion));
                m.insert("payload".into(), d.raw.clone());
            }),
            GameEvent::OngoingEvents { courses, ctx } => with(ctx, |m| {
                m.insert("courses".into(), opt_value(courses));
            }),
            GameEvent::EventEnded { event_name, ctx } => with(ctx, |m| {
                m.insert("event_name".into(), opt_value(event_name));
            }),
            GameEvent::EventCourse {
                event_name,
                draft_id,
                course_id,
                card_pool,
                raw,
                ctx,
            } => with(ctx, |m| {
                m.insert("event_name".into(), opt_value(event_name));
                m.insert("draft_id".into(), opt_value(draft_id));
                m.insert("course_id".into(), opt_value(course_id));
                m.insert("card_pool".into(), opt_value(card_pool));
                m.insert("payload".into(), raw.clone());
            }),
            GameEvent::RankUpdated { rank_data, ctx } => with(ctx, |m| {
                m.insert("rank_data".into(), rank_data.clone());
            }),
            GameEvent::CollectionUpdated { card_counts, ctx } => with(ctx, |m| {
                m.insert("card_counts".into(), card_counts.clone());
            }),
            GameEvent::InventoryUpdated { inventory, ctx } => with(ctx, |m| {
                m.insert("inventory".into(), inventory.clone());
            }),
            GameEvent::MasteryProgress { progress, ctx } => with(ctx, |m| {
                m.insert("progress".into(), progress.clone());
            }),
            GameEvent::GameCompleted {
                game,
                result,
                match_result,
                ctx,
            } => with(ctx, |m| {
                m.insert("game".into(), game.to_json());
                m.insert("result".into(), result.to_json());
                m.insert(
                    "match_result".into(),
                    match_result
                        .as_ref()
                        .map(MatchResult::to_json)
                        .unwrap_or(Value::Null),
                );
            }),
        }
    }
}

fn nested(v: &[Vec<Value>]) -> Value {
    Value::Array(v.iter().cloned().map(Value::Array).collect())
}

impl GameData {
    /// Ordered JSON view of the record (the field order above; `history` last).
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("event_name".into(), opt_value(&self.event_name));
        m.insert("match_id".into(), opt_value(&self.match_id));
        m.insert("on_play".into(), Value::Bool(self.on_play));
        m.insert(
            "opening_hand".into(),
            Value::Array(self.opening_hand.clone()),
        );
        m.insert("mulligans".into(), nested(&self.mulligans));
        m.insert("drawn_hands".into(), nested(&self.drawn_hands));
        m.insert("drawn_cards".into(), Value::Array(self.drawn_cards.clone()));
        m.insert("mulligan_count".into(), Value::from(self.mulligan_count));
        m.insert(
            "opponent_mulligan_count".into(),
            Value::from(self.opponent_mulligan_count),
        );
        m.insert("turns".into(), Value::from(self.turns));
        m.insert(
            "opponent_card_ids".into(),
            Value::Array(self.opponent_card_ids.clone()),
        );
        m.insert("rank_data".into(), opt_value(&self.rank_data));
        m.insert("opponent_rank".into(), opt_string(&self.opponent_rank));
        m.insert(
            "maindeck_card_ids".into(),
            opt_value(&self.maindeck_card_ids),
        );
        m.insert(
            "sideboard_card_ids".into(),
            opt_value(&self.sideboard_card_ids),
        );
        m.insert(
            "additional_deck_info".into(),
            opt_value(&self.additional_deck_info),
        );
        m.insert("service_metadata".into(), opt_value(&self.service_metadata));
        m.insert("client_metadata".into(), opt_value(&self.client_metadata));
        m.insert("history".into(), self.history.to_json());
        Value::Object(m)
    }
}

impl GameHistory {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("seat_id".into(), opt_i64(self.seat_id));
        m.insert(
            "opponent_seat_id".into(),
            Value::from(self.opponent_seat_id),
        );
        m.insert(
            "screen_name".into(),
            Value::String(self.screen_name.clone()),
        );
        m.insert(
            "opponent_screen_name".into(),
            Value::String(self.opponent_screen_name.clone()),
        );
        m.insert("events".into(), Value::Array(self.events.clone()));
        Value::Object(m)
    }
}

impl GameResult {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        if let Some(p) = &self.game_end_payload {
            m.insert("game_end_payload".into(), p.clone());
        }
        m.insert("game_number".into(), self.game_number.clone());
        m.insert("won".into(), Value::Bool(self.won));
        m.insert("win_type".into(), opt_value(&self.win_type));
        m.insert("game_end_reason".into(), opt_value(&self.game_end_reason));
        Value::Object(m)
    }
}

impl MatchResult {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("won_match".into(), Value::Bool(self.won_match));
        m.insert(
            "match_result_type".into(),
            opt_value(&self.match_result_type),
        );
        m.insert("match_end_reason".into(), opt_value(&self.match_end_reason));
        if let Some(p) = &self.match_result_payload {
            m.insert("match_result_payload".into(), p.clone());
        }
        Value::Object(m)
    }
}

/// A consumer of the event stream. Implemented by service adapters
/// ([`crate::services::seventeenlands::SeventeenLands`]),
/// [`crate::services::fanout::Fanout`], and test recorders. Implementations translate (or ignore) events as they see fit; they never
/// receive another service's credentials.
pub trait Service {
    fn handle(&mut self, event: &GameEvent);
}

impl Service for Box<dyn Service> {
    fn handle(&mut self, event: &GameEvent) {
        (**self).handle(event)
    }
}
