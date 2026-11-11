// SPDX-License-Identifier: MIT

//! Blob parsing and the dispatch table (port of `__handle_blob` and the if/elif chain
//! in `__handle_complete_log_entry`).
//!
//! WIRE: the dispatch chain is first-match-wins - its **order** decides which handler
//! consumes an entry and therefore what gets emitted. Reorder only with upstream.

use super::*;

impl<S: Service> Follower<S> {
    /// Port of `__handle_blob`: locate JSON, `raw_decode`, extract payload, dispatch.
    pub(super) fn handle_blob(&mut self, full_log: &str) {
        let Some(m) = re_json_start().find(full_log) else {
            return;
        };
        let start = m.start();

        // raw_decode: one JSON value from `start`, ignoring trailing text.
        let mut stream =
            serde_json::Deserializer::from_str(&full_log[start..]).into_iter::<Value>();
        let json_obj = match stream.next() {
            Some(Ok(v)) => v,
            _ => {
                // The entry itself stays out of the log: it is raw Arena output and can
                // carry account details.
                log::debug!(
                    "JSON decode error at {} ({} bytes)",
                    isoformat(&self.cur_log_time),
                    full_log.len()
                );
                return;
            }
        };

        let json_obj = extract_payload(json_obj);
        if !json_obj.is_object() {
            return;
        }

        // utc_time / event_time (each wrapped in try/ignore in Python).
        if let Some(t) = maybe_get_utc_timestamp(&json_obj) {
            self.last_utc_time = t;
        }
        // WIRE: upstream variable-reuse quirk (mtga_follower.py:496-505): the local `maybe_time` is reused -
        // after updating `last_utc_time` it is reassigned to `blob.get("EventTime")` before
        // being passed to the game-history handlers. So the history `_timestamp` is driven
        // by EventTime (absent in real logs → always null), NOT the utc timestamp. Mirror
        // that exactly.
        let event_time = json_obj.get("EventTime").cloned();
        if let Some(et) = &event_time {
            self.last_event_time = Some(et.clone());
        }

        self.dispatch(full_log, &json_obj, event_time);
    }

    /// The §6 dispatch table - first match wins, order is significant. `event_time` is the
    /// reused `maybe_time` (= `blob.get("EventTime")`) passed to game-history handlers.
    pub(super) fn dispatch(&mut self, full_log: &str, obj: &Value, event_time: Option<Value>) {
        // WIRE: first-match-wins - the order of this chain decides which handler consumes
        // an entry, hence what is emitted. Reorder only in lockstep with upstream.
        let has = |k: &str| obj.get(k).is_some();
        let client_connected = Value::String("Client.Connected".into());
        let c2gre = Value::String("ClientToMatchServiceMessageType_ClientToGREMessage".into());
        let c2greui = Value::String("ClientToMatchServiceMessageType_ClientToGREUIMessage".into());

        if json_value_matches(&client_connected, &["params", "messageName"], obj) {
            self.handle_login(obj);
        } else if contains_log_key("Event_Join", full_log) && has("EventName") {
            self.handle_joined_pod(obj);
        } else if contains_log_key("Event_Join", full_log) && has("Course") {
            self.handle_joined_event_response(obj);
        } else if has("DraftStatus") {
            self.handle_bot_draft_pack(obj);
        } else if contains_log_key("BotDraft_DraftPick", full_log) && has("PickInfo") {
            self.handle_bot_draft_pick(obj.get("PickInfo").unwrap_or(&Value::Null));
        } else if contains_log_key("LogBusinessEvents", full_log) && has("PickGrpId") {
            self.handle_human_draft_combined(obj);
        } else if contains_log_key("LogBusinessEvents", full_log) && has("WinningType") {
            self.handle_log_business_game_end(obj);
        } else if full_log.contains("Draft.Notify ") && !has("method") {
            self.handle_human_draft_pack(obj);
        } else if contains_log_key("EventPlayerDraftMakePick", full_log) && has("GrpIds") {
            self.handle_player_draft_pick(obj);
        } else if contains_log_key("Event_SetDeck", full_log) && has("EventName") {
            self.handle_deck_submission(obj);
        } else if contains_log_key("Event_GetCourses", full_log) && has("Courses") {
            self.handle_ongoing_events(obj);
        } else if contains_log_key("Event_ClaimPrize", full_log) && has("EventName") {
            self.handle_claim_prize(obj);
        } else if contains_log_key("Draft_CompleteDraft", full_log) && has("DraftId") {
            self.handle_event_course(obj);
        } else if has("authenticateResponse") {
            if let Some(name) = obj
                .get("authenticateResponse")
                .and_then(|a| a.get("screenName"))
                .and_then(|n| n.as_str())
            {
                self.update_screen_name(name);
            }
        } else if has("matchGameRoomStateChangedEvent") {
            self.handle_match_state_changed(obj);
        } else if obj
            .get("greToClientEvent")
            .map(|g| g.get("greToClientMessages").is_some())
            .unwrap_or(false)
        {
            if let Some(messages) = obj["greToClientEvent"]["greToClientMessages"].as_array() {
                for message in messages {
                    self.handle_gre_to_client_message(message, &event_time);
                }
            }
        } else if json_value_matches(&c2gre, &["clientToMatchServiceMessageType"], obj) {
            self.handle_client_to_gre_message(
                obj.get("payload").unwrap_or(&EMPTY_OBJECT),
                &event_time,
            );
        } else if json_value_matches(&c2greui, &["clientToMatchServiceMessageType"], obj) {
            self.handle_client_to_gre_ui_message(
                obj.get("payload").unwrap_or(&EMPTY_OBJECT),
                &event_time,
            );
        } else if contains_log_key("Rank_GetCombinedRankInfo", full_log)
            && has("limitedSeasonOrdinal")
        {
            self.handle_self_rank_info(obj);
        } else if full_log.contains(" PlayerInventory.GetPlayerCardsV3 ") && !has("method") {
            self.handle_collection(obj);
        } else if has("DTO_InventoryInfo") {
            self.handle_inventory(obj.get("DTO_InventoryInfo").unwrap_or(&Value::Null));
        } else if obj
            .get("NodeStates")
            .map(|n| n.get("RewardTierUpgrade").is_some())
            .unwrap_or(false)
        {
            self.handle_player_progress(obj);
        } else if full_log.contains("FrontDoorConnection.Close ") {
            self.reset_current_user();
        } else if full_log.contains("Reconnect result : Connected") {
            self.handle_reconnect_result();
        }
    }
}
