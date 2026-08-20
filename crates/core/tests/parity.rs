// SPDX-License-Identifier: MIT

//! Fixture-based parity tests.
//!
//! Feeds raw `Player.log` lines through `Follower` with a recording submitter and asserts
//! the resulting `(endpoint, payload)` sequence. These committed fixtures cover the dispatch
//! branches that are **absent** from the real sample logs (bot draft, combined human draft,
//! claim prize, event course, inventory, collection); the branches that *are* present in the
//! real logs - full games (§8 game-state machine), Draft.Notify packs, EventPlayerDraftMakePick
//! picks, deck submission, rank, account, ongoing events, player progress - were validated
//! byte-for-byte against the live Python client during the port (via a since-removed oracle
//! harness).
//!
//! The expected payloads below were captured from the reference Python client run against the
//! same fixture (so they encode Python's exact field order, null-vs-absent, and int coercion).
//! The seven envelope fields (`token`/`client_version`/`player_id`/`time`/`utc_time`/
//! `event_time`/`raw_time`) are asserted separately from the handler body, because `utc_time`
//! defaults to a local-timezone epoch and would otherwise make the test machine-dependent.

use serde_json::{Value, json};

use recall_core::follower::Follower;
use recall_core::services::seventeenlands::SeventeenLands;
use recall_core::services::seventeenlands::{
    RecordedCall, RecordingSubmitter, to_python_json_string,
};

const TOKEN: &str = "00000000-0000-4000-8000-000000000000";
const ENVELOPE_KEYS: &[&str] = &[
    "token",
    "client_version",
    "player_id",
    "time",
    "utc_time",
    "event_time",
    "raw_time",
];

fn run(fixture: &str) -> Vec<RecordedCall> {
    let data = std::fs::read_to_string(fixture).expect("read fixture");
    let mut f = Follower::with_sink(SeventeenLands::new(TOKEN.into(), RecordingSubmitter::new()));
    f.process_str(&data);
    f.sink.wire.calls
}

/// The handler-specific portion of a payload, with the envelope fields removed (order of the
/// remaining keys is preserved).
fn handler_body(payload: &Value) -> Value {
    let mut m = payload.as_object().expect("object payload").clone();
    for k in ENVELOPE_KEYS {
        m.shift_remove(*k); // preserve order of remaining keys
    }
    Value::Object(m)
}

#[test]
fn gap_branch_parity() {
    let calls = run("tests/fixtures/gaps.log");

    // (endpoint, handler body) captured from the reference Python client.
    let expected: Vec<(&str, Value)> = vec![
        (
            "api/client/add_mtga_account",
            json!({"screen_name": "Tester", "full_screen_name": null}),
        ),
        (
            "api/client/add_pack",
            json!({
                "payload": {"DraftStatus": "PickNext", "EventName": "QuickDraft_Set", "PackNumber": 1, "PickNumber": 1, "DraftPack": ["100", "200", "300"]},
                "event_name": "QuickDraft_Set", "pack_number": 1, "pick_number": 1, "card_ids": [100, 200, 300]
            }),
        ),
        (
            "api/client/add_pick",
            json!({"event_name": "QuickDraft_Set", "pack_number": 1, "pick_number": 1, "card_id": 100, "card_ids": [100, 200]}),
        ),
        (
            "api/client/add_human_draft_pack",
            json!({
                "payload": {"DraftId": "draft-1", "EventId": "PremierDraft_Set", "PackNumber": 2, "PickNumber": 3, "CardsInPack": [11, 22, 33], "PickGrpId": "22", "AutoPick": false, "TimeRemainingOnPick": 42.5},
                "draft_id": "draft-1", "event_name": "PremierDraft_Set", "pack_number": 2, "pick_number": 3, "card_ids": [11, 22, 33], "method": "LogBusiness"
            }),
        ),
        (
            "api/client/add_human_draft_pick",
            json!({
                "payload": {"DraftId": "draft-1", "EventId": "PremierDraft_Set", "PackNumber": 2, "PickNumber": 3, "CardsInPack": [11, 22, 33], "PickGrpId": "22", "AutoPick": false, "TimeRemainingOnPick": 42.5},
                "draft_id": "draft-1", "event_name": "PremierDraft_Set", "pack_number": 2, "pick_number": 3, "card_id": 22, "auto_pick": false, "time_remaining": 42.5
            }),
        ),
        (
            "api/client/mark_event_ended",
            json!({"event_name": "PremierDraft_Set"}),
        ),
        (
            "api/client/update_event_course",
            json!({
                "payload": {"InternalEventName": "PremierDraft_Set", "DraftId": "draft-1", "CourseId": "course-1", "CardPool": [1, 2, 3]},
                "event_name": "PremierDraft_Set", "draft_id": "draft-1", "course_id": "course-1", "card_pool": [1, 2, 3]
            }),
        ),
        (
            "api/client/update_inventory",
            json!({"inventory": {"Gems": 100, "Gold": 5000, "Boosters": [{"CollationId": 1, "Count": 3}], "WildCardCommons": 2}}),
        ),
        (
            "api/client/update_card_collection",
            json!({"card_counts": {"70000": 4, "70001": 2}}),
        ),
        (
            "api/client/record_event_join",
            json!({"payload": {"Course": {"InternalEventName": "PremierDraft_Set", "CourseId": "course-1", "CardPool": [1, 2, 3]}}}),
        ),
        (
            "api/client/add_deck",
            json!({
                "payload": {"EventName": "PremierDraft_Set", "Deck": {"MainDeck": [{"cardId": 101, "quantity": 2}, {"cardId": 102, "quantity": 1}], "Sideboard": [{"cardId": 103, "quantity": 1}], "Companions": [{"cardId": 104}]}},
                "event_name": "PremierDraft_Set", "maindeck_card_ids": [101, 101, 102], "sideboard_card_ids": [103], "companion": 104, "is_during_match": false
            }),
        ),
        (
            "api/client/update_ongoing_events",
            json!({"courses": [{"InternalEventName": "PremierDraft_Set", "CourseId": "course-1"}, {"InternalEventName": "QuickDraft_Set", "CourseId": "course-2"}]}),
        ),
        (
            "api/client/add_rank",
            json!({
                "rank_data": {"playerId": "ACC123", "limitedClass": "Gold", "limitedLevel": 3, "limitedStep": 1, "constructedClass": "Platinum", "constructedLevel": 2, "constructedStep": 0, "limitedSeasonOrdinal": 42, "constructedSeasonOrdinal": 42},
                "limited_rank": null, "constructed_rank": null
            }),
        ),
        (
            "api/client/update_player_progress",
            json!({"progress": {"NodeStates": {"RewardTierUpgrade": {"Tier": 2}}, "CurrentLevel": 10}}),
        ),
    ];

    assert_eq!(calls.len(), expected.len(), "submission count");

    for (i, (call, (endpoint, body))) in calls.iter().zip(&expected).enumerate() {
        assert_eq!(&call.endpoint, endpoint, "endpoint at [{i}]");

        // Handler body - compared via Python-JSON serialization (key order + values).
        assert_eq!(
            to_python_json_string(&handler_body(&call.payload)),
            to_python_json_string(body),
            "payload body at [{i}] ({endpoint})",
        );

        // Envelope: present, in order, with the expected fixed fields.
        let obj = call.payload.as_object().unwrap();
        let head: Vec<&str> = obj
            .keys()
            .take(ENVELOPE_KEYS.len())
            .map(|s| s.as_str())
            .collect();
        assert_eq!(head, ENVELOPE_KEYS, "envelope key order at [{i}]");
        assert_eq!(obj["token"], json!(TOKEN));
        assert_eq!(obj["client_version"], json!("0.1.44.p"));
        assert_eq!(obj["player_id"], json!("ACC123"), "player_id at [{i}]");
        assert_eq!(obj["event_time"], Value::Null);
        // `time` / `utc_time` are ISO strings (exact values were proven against the Python client;
        // here we keep the assertion timezone-independent and portable).
        assert!(obj["time"].as_str().unwrap().contains('T'), "time at [{i}]");
        assert!(
            obj["utc_time"].as_str().unwrap().contains('T'),
            "utc_time at [{i}]"
        );
    }

    // None of these endpoints are gzipped (only add_game is).
    assert!(calls.iter().all(|c| !c.use_gzip));
}
