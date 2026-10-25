// SPDX-License-Identifier: MIT
//
// Inspired by the original MTGA log client by 17Lands
// (https://github.com/rconroy293/mtga-log-client): this adapter reproduces its
// payloads byte-for-byte so Recall is a drop-in replacement on the wire.

//! The 17Lands service adapter - **the parity surface**.
//!
//! Translates each [`GameEvent`] into the exact payload the upstream Python client POSTs:
//! builds the handler body for typed variants (field names, order, null-vs-absent - all
//! 17Lands API choices live here), completes upstream's `_add_base_api_data` envelope by
//! prepending `token` and `client_version`, routes the kind to its `api/client/*`
//! endpoint, and marks `add_game` for gzip. Endpoint mapping mirrors upstream
//! `api_client.py`'s `submit_*` methods.
//!
//! Wire rules pinned by `tests/parity.rs` / `tests/parity_game.rs`: envelope key order is
//! `token, client_version, player_id, time, utc_time, event_time, raw_time, <handler
//! fields>`; fields are emitted even when null; only `add_game` is gzipped.

use serde_json::{Map, Value};

use crate::events::{DeckEntry, DraftSource, GameEvent, Service, opt_i64, opt_string, opt_value};

pub mod transport;

pub use transport::{
    ApiClient, RecordedCall, RecordingSubmitter, SubmitOutcome, Submitter, to_python_json_string,
    to_python_json_vec,
};

pub const DEFAULT_HOST: &str = "https://api.17lands.com";

/// Client version string sent in every payload.
///
/// Defaults to impersonating the Python client for guaranteed acceptance; revisit after
/// live testing (switch to "0.1.44.r" only if 17Lands accepts a distinct identifier).
pub const CLIENT_VERSION: &str = "0.1.44.p";

/// Endpoint paths. Kept subset only.
pub mod endpoints {
    pub const UPDATE_CARD_COLLECTION: &str = "api/client/update_card_collection";
    pub const ADD_DECK: &str = "api/client/add_deck";
    pub const ADD_PACK: &str = "api/client/add_pack";
    pub const ADD_PICK: &str = "api/client/add_pick";
    pub const UPDATE_EVENT_COURSE: &str = "api/client/update_event_course";
    pub const RECORD_EVENT_JOIN: &str = "api/client/record_event_join";
    pub const MARK_EVENT_ENDED: &str = "api/client/mark_event_ended";
    pub const ADD_GAME: &str = "api/client/add_game";
    pub const ADD_HUMAN_DRAFT_PACK: &str = "api/client/add_human_draft_pack";
    pub const ADD_HUMAN_DRAFT_PICK: &str = "api/client/add_human_draft_pick";
    pub const UPDATE_INVENTORY: &str = "api/client/update_inventory";
    pub const UPDATE_ONGOING_EVENTS: &str = "api/client/update_ongoing_events";
    pub const UPDATE_PLAYER_PROGRESS: &str = "api/client/update_player_progress";
    pub const ADD_RANK: &str = "api/client/add_rank";
    pub const ADD_MTGA_ACCOUNT: &str = "api/client/add_mtga_account";
}

/// Maps an event to its 17Lands endpoint and gzip flag (upstream `api_client.py`'s
/// `submit_*` methods; `add_game` is the only gzipped one).
fn route(event: &GameEvent) -> (&'static str, bool) {
    match event {
        GameEvent::ScreenNameUpdated { .. } => (endpoints::ADD_MTGA_ACCOUNT, false),
        GameEvent::EventJoined { .. } => (endpoints::RECORD_EVENT_JOIN, false),
        GameEvent::DraftPack(p) => match p.source {
            DraftSource::BotDraft => (endpoints::ADD_PACK, false),
            _ => (endpoints::ADD_HUMAN_DRAFT_PACK, false),
        },
        GameEvent::DraftPick(p) => match p.source {
            DraftSource::BotDraft => (endpoints::ADD_PICK, false),
            _ => (endpoints::ADD_HUMAN_DRAFT_PICK, false),
        },
        GameEvent::DeckSubmitted(_) => (endpoints::ADD_DECK, false),
        GameEvent::OngoingEvents { .. } => (endpoints::UPDATE_ONGOING_EVENTS, false),
        GameEvent::EventEnded { .. } => (endpoints::MARK_EVENT_ENDED, false),
        GameEvent::EventCourse { .. } => (endpoints::UPDATE_EVENT_COURSE, false),
        GameEvent::RankUpdated { .. } => (endpoints::ADD_RANK, false),
        GameEvent::CollectionUpdated { .. } => (endpoints::UPDATE_CARD_COLLECTION, false),
        GameEvent::InventoryUpdated { .. } => (endpoints::UPDATE_INVENTORY, false),
        GameEvent::MasteryProgress { .. } => (endpoints::UPDATE_PLAYER_PROGRESS, false),
        GameEvent::GameCompleted { .. } => (endpoints::ADD_GAME, true),
    }
}

/// The inventory fields 17Lands accepts (upstream `__handle_inventory`). Filtering is a
/// 17Lands API choice, so it lives here - the neutral event carries the full blob.
const INVENTORY_KEEP: &[&str] = &[
    "Gems",
    "Gold",
    "TotalVaultProgress",
    "wcTrackPosition",
    "WildCardCommons",
    "WildCardUnCommons",
    "WildCardRares",
    "WildCardMythics",
    "DraftTokens",
    "SealedTokens",
    "Boosters",
    "Changes",
];

/// 17Lands sink: owns the upload token and the wire client (live [`ApiClient`], a recorder
/// in tests, or a decorator like the desktop's observer). Public `wire` mirrors the old `Follower.api` test seam.
pub struct SeventeenLands<S: Submitter> {
    pub wire: S,
    token: String,
}

impl<S: Submitter> SeventeenLands<S> {
    pub fn new(token: String, wire: S) -> Self {
        Self { wire, token }
    }

    /// The 17Lands handler body for an event: context fields + handler fields, in the
    /// exact upstream order. Each arm mirrors the upstream handler it is named after.
    fn body(event: &GameEvent) -> Map<String, Value> {
        // WIRE: every arm reproduces its upstream handler's field names, order, nulls and
        // constants byte-for-byte (tests/parity.rs, tests/parity_game.rs, `just golden`).
        let mut m = match event {
            GameEvent::ScreenNameUpdated { ctx, .. }
            | GameEvent::EventJoined { ctx, .. }
            | GameEvent::OngoingEvents { ctx, .. }
            | GameEvent::EventEnded { ctx, .. }
            | GameEvent::EventCourse { ctx, .. }
            | GameEvent::RankUpdated { ctx, .. }
            | GameEvent::CollectionUpdated { ctx, .. }
            | GameEvent::InventoryUpdated { ctx, .. }
            | GameEvent::MasteryProgress { ctx, .. }
            | GameEvent::GameCompleted { ctx, .. } => ctx.to_map(),
            GameEvent::DraftPack(p) => p.ctx.to_map(),
            GameEvent::DraftPick(p) => p.ctx.to_map(),
            GameEvent::DeckSubmitted(d) => d.ctx.to_map(),
        };
        match event {
            GameEvent::ScreenNameUpdated {
                screen_name,
                full_screen_name,
                ..
            } => {
                // `__update_screen_name` (its `player_id` re-insert is a no-op: same key,
                // same value, same position).
                m.insert("screen_name".into(), opt_string(screen_name));
                m.insert("full_screen_name".into(), opt_string(full_screen_name));
            }
            GameEvent::EventJoined { raw, .. } => {
                // `__handle_joined_event_response`
                m.insert("payload".into(), raw.clone());
            }
            GameEvent::DraftPack(p) => match p.source {
                DraftSource::BotDraft => {
                    // `__handle_bot_draft_pack`
                    m.insert("payload".into(), p.raw.clone());
                    m.insert("event_name".into(), opt_value(&p.event_name));
                    m.insert("pack_number".into(), opt_i64(p.pack_number));
                    m.insert("pick_number".into(), opt_i64(p.pick_number));
                    m.insert("card_ids".into(), p.card_ids.clone());
                }
                DraftSource::LogBusiness
                | DraftSource::DraftNotify
                | DraftSource::PlayerDraftMakePick => {
                    // `__handle_human_draft_combined` / `__handle_human_draft_pack`
                    m.insert("payload".into(), p.raw.clone());
                    m.insert("draft_id".into(), opt_value(&p.draft_id));
                    m.insert("event_name".into(), opt_value(&p.event_name));
                    m.insert("pack_number".into(), opt_i64(p.pack_number));
                    m.insert("pick_number".into(), opt_i64(p.pick_number));
                    m.insert("card_ids".into(), p.card_ids.clone());
                    let method = match p.source {
                        DraftSource::DraftNotify => "Draft.Notify",
                        _ => "LogBusiness",
                    };
                    m.insert("method".into(), Value::String(method.into()));
                }
            },
            GameEvent::DraftPick(p) => match p.source {
                DraftSource::BotDraft => {
                    // `__handle_bot_draft_pick` (no payload echo upstream)
                    m.insert("event_name".into(), opt_value(&p.event_name));
                    m.insert("pack_number".into(), opt_i64(p.pack_number));
                    m.insert("pick_number".into(), opt_i64(p.pick_number));
                    m.insert("card_id".into(), opt_i64(p.card_id));
                    m.insert("card_ids".into(), opt_value(&p.card_ids));
                }
                DraftSource::LogBusiness => {
                    // `__handle_human_draft_combined` (pick half)
                    m.insert("payload".into(), opt_value(&p.raw));
                    m.insert("draft_id".into(), opt_value(&p.draft_id));
                    m.insert("event_name".into(), opt_value(&p.event_name));
                    m.insert("pack_number".into(), opt_i64(p.pack_number));
                    m.insert("pick_number".into(), opt_i64(p.pick_number));
                    m.insert("card_id".into(), opt_i64(p.card_id));
                    m.insert("auto_pick".into(), opt_value(&p.auto_pick));
                    m.insert("time_remaining".into(), opt_value(&p.time_remaining));
                }
                DraftSource::PlayerDraftMakePick | DraftSource::DraftNotify => {
                    // `__handle_player_draft_pick`
                    m.insert("payload".into(), opt_value(&p.raw));
                    m.insert("draft_id".into(), opt_value(&p.draft_id));
                    m.insert("event_name".into(), opt_value(&p.event_name));
                    m.insert("pack_number".into(), opt_i64(p.pack_number));
                    m.insert("pick_number".into(), opt_i64(p.pick_number));
                    m.insert("card_ids".into(), opt_value(&p.card_ids));
                }
            },
            GameEvent::DeckSubmitted(d) => {
                // `__handle_deck_submission`: quantities expanded to repeated ids,
                // companion defaults to 0, `is_during_match` is always false here.
                fn expand(entries: &[DeckEntry]) -> Value {
                    Value::Array(
                        entries
                            .iter()
                            .flat_map(|e| {
                                std::iter::repeat_n(e.card_id.clone(), e.quantity.max(0) as usize)
                            })
                            .collect(),
                    )
                }
                m.insert("payload".into(), d.raw.clone());
                m.insert("event_name".into(), opt_value(&d.event_name));
                m.insert("maindeck_card_ids".into(), expand(&d.maindeck));
                m.insert("sideboard_card_ids".into(), expand(&d.sideboard));
                m.insert(
                    "companion".into(),
                    d.companion.clone().unwrap_or(Value::from(0)),
                );
                m.insert("is_during_match".into(), Value::Bool(false));
            }
            GameEvent::OngoingEvents { courses, .. } => {
                // `__handle_ongoing_events`
                m.insert("courses".into(), opt_value(courses));
            }
            GameEvent::EventEnded { event_name, .. } => {
                // `__handle_claim_prize`
                m.insert("event_name".into(), opt_value(event_name));
            }
            GameEvent::EventCourse {
                event_name,
                draft_id,
                course_id,
                card_pool,
                raw,
                ..
            } => {
                // `__handle_event_course`
                m.insert("payload".into(), raw.clone());
                m.insert("event_name".into(), opt_value(event_name));
                m.insert("draft_id".into(), opt_value(draft_id));
                m.insert("course_id".into(), opt_value(course_id));
                m.insert("card_pool".into(), opt_value(card_pool));
            }
            GameEvent::RankUpdated { rank_data, .. } => {
                // `__handle_self_rank_info`: rank_data + always-null rank splits.
                m.insert("rank_data".into(), rank_data.clone());
                m.insert("limited_rank".into(), Value::Null);
                m.insert("constructed_rank".into(), Value::Null);
            }
            GameEvent::CollectionUpdated { card_counts, .. } => {
                // `__handle_collection`
                m.insert("card_counts".into(), card_counts.clone());
            }
            GameEvent::InventoryUpdated { inventory, .. } => {
                // `__handle_inventory`: keep-list filter, original key order.
                let mut filtered = Map::new();
                if let Some(map) = inventory.as_object() {
                    for (k, v) in map {
                        if INVENTORY_KEEP.contains(&k.as_str()) {
                            filtered.insert(k.clone(), v.clone());
                        }
                    }
                }
                m.insert("inventory".into(), Value::Object(filtered));
            }
            GameEvent::MasteryProgress { progress, .. } => {
                // `__handle_player_progress`
                m.insert("progress".into(), progress.clone());
            }
            GameEvent::GameCompleted {
                game,
                result,
                match_result,
                ..
            } => {
                // `__maybe_submit_pending_game`: game result, then match result, then the
                // game data - with upstream's never-computed `duration: -1` after `turns`.
                fn extend(m: &mut Map<String, Value>, v: Value) {
                    if let Value::Object(o) = v {
                        for (k, v) in o {
                            m.insert(k, v);
                        }
                    }
                }
                extend(&mut m, result.to_json());
                if let Some(mr) = match_result {
                    extend(&mut m, mr.to_json());
                }
                if let Value::Object(g) = game.to_json() {
                    for (k, v) in g {
                        let is_turns = k == "turns";
                        m.insert(k, v);
                        if is_turns {
                            m.insert("duration".into(), Value::from(-1));
                        }
                    }
                }
            }
        }
        m
    }
}

impl<S: Submitter> Service for SeventeenLands<S> {
    fn handle(&mut self, event: &GameEvent) {
        // WIRE: envelope head is `token, client_version` then the context fields.
        let mut payload = Map::new();
        payload.insert("token".into(), Value::String(self.token.clone()));
        payload.insert(
            "client_version".into(),
            Value::String(CLIENT_VERSION.into()),
        );
        for (k, v) in Self::body(event) {
            payload.insert(k, v);
        }
        let (endpoint, use_gzip) = route(event);
        // The outcome is for observers; the client has already logged any failure.
        self.wire.submit(endpoint, Value::Object(payload), use_gzip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Context;
    use serde_json::json;

    fn ctx() -> Context {
        Context::test_fixture()
    }

    #[test]
    fn routes_and_injects_envelope_head() {
        let mut svc = SeventeenLands::new("tok".into(), RecordingSubmitter::new());
        svc.handle(&GameEvent::EventEnded {
            event_name: Some(json!("PremierDraft_X")),
            ctx: ctx(),
        });
        svc.handle(&GameEvent::MasteryProgress {
            progress: json!({"CurrentLevel": 1}),
            ctx: ctx(),
        });

        let calls = &svc.wire.calls;
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].endpoint, endpoints::MARK_EVENT_ENDED);
        assert_eq!(calls[1].endpoint, endpoints::UPDATE_PLAYER_PROGRESS);
        assert!(calls.iter().all(|c| !c.use_gzip));

        // token + client_version lead, context follows, then handler fields.
        let keys: Vec<&str> = calls[0]
            .payload
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            keys,
            [
                "token",
                "client_version",
                "player_id",
                "time",
                "utc_time",
                "event_time",
                "raw_time",
                "event_name"
            ]
        );
        assert_eq!(calls[0].payload["token"], json!("tok"));
        assert_eq!(calls[0].payload["client_version"], json!(CLIENT_VERSION));
    }

    #[test]
    fn rank_event_builds_upstream_body() {
        let mut svc = SeventeenLands::new("tok".into(), RecordingSubmitter::new());
        svc.handle(&GameEvent::RankUpdated {
            rank_data: json!({"limitedClass": "Gold"}),
            ctx: ctx(),
        });
        let p = svc.wire.calls[0].payload.as_object().unwrap();
        let keys: Vec<&str> = p.keys().map(|s| s.as_str()).collect();
        assert_eq!(
            keys,
            [
                "token",
                "client_version",
                "player_id",
                "time",
                "utc_time",
                "event_time",
                "raw_time",
                "rank_data",
                "limited_rank",
                "constructed_rank"
            ]
        );
        assert_eq!(p["limited_rank"], Value::Null);
        assert_eq!(p["time"], json!("2024-02-02T13:00:00"));
    }

    #[test]
    fn inventory_event_filters_to_keep_list_preserving_order() {
        let mut svc = SeventeenLands::new("tok".into(), RecordingSubmitter::new());
        svc.handle(&GameEvent::InventoryUpdated {
            inventory: json!({"Gems": 100, "SecretField": "x", "Gold": 5, "Boosters": []}),
            ctx: ctx(),
        });
        assert_eq!(
            svc.wire.calls[0].payload["inventory"],
            json!({"Gems": 100, "Gold": 5, "Boosters": []})
        );
    }
}
