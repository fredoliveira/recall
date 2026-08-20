// SPDX-License-Identifier: MIT

//! The game-state machine: GRE message handling, board/hand/draw tracking, game and
//! match results, and assembling the `GameCompleted` record.

use super::*;

impl<S: Service> Follower<S> {
    pub(super) fn add_to_game_history(&mut self, message_blob: &Value, event_time: &Option<Value>) {
        // WIRE: Python `None if timestamp is None else timestamp.isoformat()`. After the
        // variable-reuse quirk `timestamp` is `blob.get("EventTime")`, which is absent in
        // real logs (→ null). If it were ever present (never observed) Python would raise
        // on `.isoformat()` of a non-datetime; we carry the value through instead.
        let ts = match event_time {
            None => Value::Null,
            Some(v) => v.clone(),
        };
        let mut entry = Map::new();
        entry.insert("_timestamp".into(), ts);
        if let Some(map) = message_blob.as_object() {
            for (k, v) in map {
                entry.insert(k.clone(), v.clone());
            }
        }
        self.game.history_events.push(Value::Object(entry));
    }

    pub(super) fn handle_gre_to_client_message(
        &mut self,
        message_blob: &Value,
        event_time: &Option<Value>,
    ) {
        let msg_type = message_blob
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Add to history before processing - we may submit the game right away.
        let is_game_state = matches!(
            msg_type,
            "GREMessageType_QueuedGameStateMessage" | "GREMessageType_GameStateMessage"
        );
        let is_ui_chat = msg_type == "GREMessageType_UIMessage"
            && message_blob
                .get("uiMessage")
                .map(|u| u.get("onChat").is_some())
                .unwrap_or(false);
        if is_game_state || is_ui_chat {
            self.add_to_game_history(message_blob, event_time);
        }

        match msg_type {
            "GREMessageType_ConnectResp" => self.handle_gre_connect_response(message_blob),
            "GREMessageType_EdictalMessage" => {
                self.handle_gre_edictal_message(message_blob, event_time)
            }
            "GREMessageType_GameStateMessage" => self.handle_game_state_message(message_blob),
            _ => {}
        }
    }

    pub(super) fn handle_game_state_message(&mut self, message_blob: &Value) {
        if let Some(seat) = message_blob
            .get("systemSeatIds")
            .and_then(|s| s.as_array())
            .and_then(|a| a.first())
            .and_then(to_i64)
        {
            self.match_state.seat_id = Some(seat);
        }

        let gsm = message_blob
            .get("gameStateMessage")
            .unwrap_or(&EMPTY_OBJECT);

        if let Some(game_info) = gsm.get("gameInfo") {
            // On a new matchID, switch and clear the event id.
            if let Some(match_id) = game_info.get("matchID")
                && Some(match_id) != self.match_state.match_id.as_ref()
            {
                self.match_state.match_id = Some(match_id.clone());
                self.match_state.event_id = None;
            }
        }

        let turn_info = gsm.get("turnInfo").unwrap_or(&EMPTY_OBJECT);
        let players: &[Value] = gsm
            .get("players")
            .and_then(|p| p.as_array())
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        // turn_count = turnInfo.turnNumber if truthy, else max(turn_count, sum of players' turnNumber).
        match turn_info.get("turnNumber").and_then(to_i64) {
            Some(tn) if tn != 0 => self.game.turn_count = tn,
            _ => {
                let turns_sum: i64 = players
                    .iter()
                    .map(|p| p.get("turnNumber").and_then(to_i64).unwrap_or(0))
                    .sum();
                self.game.turn_count = self.game.turn_count.max(turns_sum);
            }
        }

        // gameObjects → objects_by_owner (Card / SplitCard only).
        if let Some(objs) = gsm.get("gameObjects").and_then(|o| o.as_array()) {
            for game_object in objs {
                let t = game_object
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if t != "GameObjectType_Card" && t != "GameObjectType_SplitCard" {
                    continue;
                }
                let (owner, instance_id, card_id) = match (
                    game_object.get("ownerSeatId").and_then(to_i64),
                    game_object.get("instanceId").and_then(to_i64),
                    game_object.get("overlayGrpId"),
                ) {
                    (Some(o), Some(i), Some(c)) => (o, i, c.clone()),
                    _ => continue,
                };
                self.game
                    .objects_by_owner
                    .entry(owner)
                    .or_default()
                    .insert(instance_id.to_string(), card_id);
            }
        }

        // zones → cards_in_hand + drawn_cards_by_instance_id (ZoneType_Hand).
        if let Some(zones) = gsm.get("zones").and_then(|z| z.as_array()) {
            for zone in zones {
                if zone.get("type").and_then(|v| v.as_str()) != Some("ZoneType_Hand") {
                    continue;
                }
                let Some(owner) = zone.get("ownerSeatId").and_then(to_i64) else {
                    continue;
                };
                let hand_ids: Vec<i64> = zone
                    .get("objectInstanceIds")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(to_i64).collect())
                    .unwrap_or_default();

                // Disjoint field borrows: read objects while writing hands/draws.
                let player_objects = &*self.game.objects_by_owner.entry(owner).or_default();

                // cards_in_hand: skip falsy (0) instance ids; missing card -> null entry.
                let hand: Vec<Value> = hand_ids
                    .iter()
                    .filter(|&&iid| iid != 0)
                    .map(|iid| {
                        player_objects
                            .get(&iid.to_string())
                            .cloned()
                            .unwrap_or(Value::Null)
                    })
                    .collect();
                self.cards_in_hand.insert(owner, hand);

                // drawn_cards_by_instance_id: record found cards.
                let drawn = self
                    .game
                    .drawn_cards_by_instance_id
                    .entry(owner)
                    .or_default();
                for iid in &hand_ids {
                    if let Some(card_id) = player_objects.get(&iid.to_string()) {
                        drawn.insert(iid.to_string(), card_id.clone());
                    }
                }
            }
        }

        // Mulligan / opening-hand bookkeeping.
        let mut deciding: Vec<(i64, i64)> = Vec::new();
        for p in players {
            if p.get("pendingMessageType").and_then(|v| v.as_str())
                == Some("ClientMessageType_MulliganResp")
            {
                let seat = p.get("systemSeatNumber").and_then(to_i64);
                let mull = p.get("mulliganCount").and_then(to_i64).unwrap_or(0);
                if let Some(seat) = seat
                    && !deciding.contains(&(seat, mull))
                {
                    deciding.push((seat, mull));
                }
            }
        }
        for (player_id, mulligan_count) in deciding {
            if self.game.starting_team_id.is_none() {
                self.game.starting_team_id = turn_info.get("activePlayer").and_then(to_i64);
            }
            *self
                .game
                .opening_hand_count_by_seat
                .entry(player_id)
                .or_insert(0) += 1;

            let hands = self.game.drawn_hands.entry(player_id).or_default();
            if mulligan_count == hands.len() as i64 {
                let hand = self
                    .cards_in_hand
                    .get(&player_id)
                    .cloned()
                    .unwrap_or_default();
                hands.push(hand);
            }
        }

        // Capture the opening hand at (Phase_Beginning, Step_Upkeep, turn 1).
        let at_opening = turn_info.get("phase").and_then(|v| v.as_str()) == Some("Phase_Beginning")
            && turn_info.get("step").and_then(|v| v.as_str()) == Some("Step_Upkeep")
            && turn_info.get("turnNumber").and_then(to_i64) == Some(1);
        if self.game.opening_hand.is_empty() && at_opening {
            let snapshot: Vec<(i64, Vec<Value>)> = self
                .cards_in_hand
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            for (owner, hand) in snapshot {
                self.game.opening_hand.insert(owner, hand);
            }
        }

        self.maybe_handle_game_over_stage(gsm);
    }

    pub(super) fn handle_gre_connect_response(&mut self, blob: &Value) {
        let mut deck_info = blob
            .get("connectResp")
            .and_then(|c| c.get("deckMessage"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        self.split_deck_info(&mut deck_info);
    }

    /// `.pop()`-then-store deck handling shared by connect-resp and submit-deck-resp:
    /// remove `deckCards`/`sideboardCards`, keep the rest as additional info.
    pub(super) fn split_deck_info(&mut self, deck_info: &mut Value) {
        if let Some(map) = deck_info.as_object_mut() {
            // WIRE: shift_remove (not remove/swap_remove) so the *remaining* keys kept as
            // additional_deck_info preserve their order, matching Python's `dict.pop`.
            self.game.maindeck = Some(map.shift_remove("deckCards").unwrap_or_else(|| json!([])));
            self.game.sideboard = Some(
                map.shift_remove("sideboardCards")
                    .unwrap_or_else(|| json!([])),
            );
            self.game.additional_deck_info = Some(Value::Object(map.clone()));
        } else {
            self.game.maindeck = Some(json!([]));
            self.game.sideboard = Some(json!([]));
            self.game.additional_deck_info = Some(deck_info.clone());
        }
    }

    pub(super) fn handle_client_to_gre_message(
        &mut self,
        payload: &Value,
        event_time: &Option<Value>,
    ) {
        let ptype = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if ptype == "ClientMessageType_SelectNResp" {
            self.add_to_game_history(payload, event_time);
        }

        if ptype == "ClientMessageType_SubmitDeckResp" {
            self.clear_game_data(true);
            let mut deck_info = payload
                .get("submitDeckResp")
                .and_then(|s| s.get("deck"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            self.split_deck_info(&mut deck_info);
        }
    }

    pub(super) fn handle_client_to_gre_ui_message(
        &mut self,
        payload: &Value,
        event_time: &Option<Value>,
    ) {
        if payload
            .get("uiMessage")
            .map(|u| u.get("onChat").is_some())
            .unwrap_or(false)
        {
            self.add_to_game_history(payload, event_time);
        }
    }

    pub(super) fn handle_gre_edictal_message(
        &mut self,
        payload: &Value,
        event_time: &Option<Value>,
    ) {
        let edict = payload
            .get("edictalMessage")
            .and_then(|e| e.get("edictMessage"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        self.handle_client_to_gre_message(&edict, event_time);
    }

    pub(super) fn handle_log_business_game_end(&mut self, payload: &Value) {
        if self.game.starting_team_id.is_none() {
            self.game.starting_team_id = payload.get("StartingTeamId").and_then(to_i64);
        }
        if self.enqueue_game_data() {
            let won = self.match_state.seat_id == payload.get("WinningTeamId").and_then(to_i64);
            self.game.pending_result = Some(GameResult {
                game_end_payload: Some(payload.clone()),
                game_number: opt_val(&payload.get("GameNumber").cloned()),
                won,
                win_type: payload.get("WinningType").cloned(),
                game_end_reason: payload.get("WinningReason").cloned(),
            });
            log::info!("Added pending game result via LogBusinessEvents");
        }
    }

    pub(super) fn maybe_handle_game_over_stage(&mut self, gsm: &Value) {
        let game_info = gsm.get("gameInfo").unwrap_or(&EMPTY_OBJECT);
        if game_info.get("stage").and_then(|v| v.as_str()) != Some("GameStage_GameOver") {
            return;
        }
        if let Some(results) = game_info.get("results").and_then(|r| r.as_array())
            && !results.is_empty()
            && self.enqueue_game_data()
        {
            self.enqueue_game_results(results, None);
        }
    }

    pub(super) fn handle_match_state_changed(&mut self, blob: &Value) {
        let game_room_info = blob
            .get("matchGameRoomStateChangedEvent")
            .and_then(|e| e.get("gameRoomInfo"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let game_room_config = game_room_info
            .get("gameRoomConfig")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let mut updated_match_id = game_room_config.get("matchId").cloned();
        let mut updated_event_id = game_room_config.get("eventId").cloned();

        if let Some(players) = game_room_config
            .get("reservedPlayers")
            .and_then(|p| p.as_array())
        {
            let mut oppo_player_id = String::new();
            for player in players {
                if let (Some(seat), Some(name)) = (
                    player.get("systemSeatId").and_then(to_i64),
                    player.get("playerName").and_then(|v| v.as_str()),
                ) {
                    self.match_state
                        .screen_names
                        .insert(seat, name.split('#').next().unwrap_or("").to_string());
                }
                let user_id = player.get("userId").and_then(|v| v.as_str());
                if user_id.is_some() && user_id.map(|s| s.to_string()) == self.user.id {
                    if let Some(name) = player.get("playerName").and_then(|v| v.as_str()) {
                        self.update_screen_name(name);
                    }
                    if let Some(eid) = player.get("eventId") {
                        updated_event_id = Some(eid.clone());
                    }
                } else if let Some(uid) = user_id {
                    oppo_player_id = uid.to_string();
                }
            }

            if !oppo_player_id.is_empty()
                && let Some(metadata) = game_room_config.get("clientMetadata")
            {
                self.cur_opponent_level = Some(get_rank_string(
                    metadata.get(format!("{oppo_player_id}_RankClass")),
                    metadata.get(format!("{oppo_player_id}_RankTier")),
                    metadata.get(format!("{oppo_player_id}_LeaderboardPercentile")),
                    metadata.get(format!("{oppo_player_id}_LeaderboardPlacement")),
                    None,
                ));
                self.cur_opponent_match_id = game_room_config.get("matchId").cloned();
                log::info!("Parsed opponent rank info");
            }
        }

        let match_truthy = updated_match_id.as_ref().is_some_and(is_truthy);
        let event_truthy = updated_event_id.as_ref().is_some_and(is_truthy);
        if match_truthy && event_truthy {
            self.match_state.match_id = updated_match_id.take();
            self.match_state.event_id = updated_event_id.take();
        }

        if let Some(sm) = game_room_config.get("serviceMetadata") {
            self.game.service_metadata = Some(sm.clone());
        }
        if let Some(cm) = game_room_config.get("clientMetadata") {
            self.game.client_metadata = Some(cm.clone());
        }

        if let Some(final_result) = game_room_info.get("finalMatchResult") {
            let results = final_result
                .get("resultList")
                .and_then(|r| r.as_array())
                .cloned()
                .unwrap_or_default();
            if !results.is_empty() && self.enqueue_game_data() {
                self.enqueue_game_results(&results, Some(blob));
            }
            self.clear_match_data(true);
        }
    }

    /// WIRE: upstream's threshold - fewer than 6 history events or no draws means the game
    /// is never submitted at all.
    pub(super) fn has_pending_game_data(&self) -> bool {
        !self.game.drawn_cards_by_instance_id.is_empty() && self.game.history_events.len() > 5
    }

    pub(super) fn enqueue_game_results(&mut self, results: &[Value], match_obj: Option<&Value>) {
        let game_results: Vec<&Value> = results
            .iter()
            .filter(|r| r.get("scope").and_then(|v| v.as_str()) == Some("MatchScope_Game"))
            .collect();
        if let Some(this) = game_results.last() {
            let won = self.match_state.seat_id == this.get("winningTeamId").and_then(to_i64);
            self.game.pending_result = Some(GameResult {
                game_end_payload: None,
                game_number: Value::from(1.max(game_results.len() as i64)),
                won,
                win_type: this.get("result").cloned(),
                game_end_reason: this.get("reason").cloned(),
            });
            log::info!("Added pending game result");
        }

        if let Some(match_result) = results
            .iter()
            .find(|r| r.get("scope").and_then(|v| v.as_str()) == Some("MatchScope_Match"))
        {
            let won_match =
                self.match_state.seat_id == match_result.get("winningTeamId").and_then(to_i64);
            self.game.pending_match_result = Some(MatchResult {
                won_match,
                match_result_type: match_result.get("result").cloned(),
                match_end_reason: match_result.get("reason").cloned(),
                match_result_payload: match_obj.cloned(),
            });
            log::info!("Added pending match result");
        }
    }

    pub(super) fn enqueue_game_data(&mut self) -> bool {
        if !self.has_pending_game_data() {
            return false;
        }

        let opponent_id = if self.match_state.seat_id == Some(1) {
            2
        } else {
            1
        };
        let opponent_card_ids: Vec<Value> = self
            .game
            .objects_by_owner
            .get(&opponent_id)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();

        if self.match_state.match_id != self.cur_opponent_match_id {
            self.cur_opponent_level = None;
        }

        let seat = self.match_state.seat_id;
        let seat_hand = |map: &HashMap<i64, Vec<Value>>| -> Vec<Value> {
            seat.and_then(|s| map.get(&s)).cloned().unwrap_or_default()
        };
        let opening_hand = seat_hand(&self.game.opening_hand);
        let drawn_hands: Vec<Vec<Value>> = seat
            .and_then(|s| self.game.drawn_hands.get(&s))
            .cloned()
            .unwrap_or_default();
        let mulligans: Vec<Vec<Value>> = if drawn_hands.is_empty() {
            Vec::new()
        } else {
            drawn_hands[..drawn_hands.len() - 1].to_vec()
        };
        let drawn_cards: Vec<Value> = seat
            .and_then(|s| self.game.drawn_cards_by_instance_id.get(&s))
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        // WIRE: defaultdict(int) semantics - a seat never seen counts 0, so these can be -1.
        let mulligan_count = seat
            .and_then(|s| self.game.opening_hand_count_by_seat.get(&s))
            .copied()
            .unwrap_or(0)
            - 1;
        let opponent_mulligan_count = self
            .game
            .opening_hand_count_by_seat
            .get(&opponent_id)
            .copied()
            .unwrap_or(0)
            - 1;

        let on_play = self.match_state.seat_id == self.game.starting_team_id;

        log::info!("Completed game");
        log::info!(
            "Adding game history ({} events)",
            self.game.history_events.len()
        );
        // copy.deepcopy(game): everything is cloned out of the live state, so later
        // mutation can't leak into the pending record.
        self.pending_game = Some(GameData {
            event_name: self.match_state.event_id.clone(),
            match_id: self.match_state.match_id.clone(),
            on_play,
            opening_hand,
            mulligans,
            drawn_hands,
            drawn_cards,
            mulligan_count,
            opponent_mulligan_count,
            turns: self.game.turn_count,
            opponent_card_ids,
            rank_data: self.user.rank.clone(),
            opponent_rank: self.cur_opponent_level.clone(),
            maindeck_card_ids: self.game.maindeck.clone(),
            sideboard_card_ids: self.game.sideboard.clone(),
            additional_deck_info: self.game.additional_deck_info.clone(),
            service_metadata: self.game.service_metadata.clone(),
            client_metadata: self.game.client_metadata.clone(),
            history: GameHistory {
                seat_id: self.match_state.seat_id,
                opponent_seat_id: opponent_id,
                screen_name: seat
                    .and_then(|s| self.match_state.screen_names.get(&s))
                    .cloned()
                    .unwrap_or_default(),
                opponent_screen_name: self
                    .match_state
                    .screen_names
                    .get(&opponent_id)
                    .cloned()
                    .unwrap_or_default(),
                events: self.game.history_events.clone(),
            },
        });
        true
    }

    pub(super) fn maybe_submit_pending_game(&mut self) {
        if self.pending_game.is_none() || self.game.pending_result.is_none() {
            return;
        }
        let game = self.pending_game.take().expect("checked above");
        let result = self.game.pending_result.clone().expect("checked above");
        log::info!("Submitting queued game result");
        let event = GameEvent::GameCompleted {
            game: Box::new(game),
            result,
            match_result: self.game.pending_match_result.clone(),
            ctx: self.context(),
        };
        self.sink.handle(&event);
        self.clear_game_data(true);
    }
}

/// WIRE: Python truthiness for the `if updated_match_id and updated_event_id` guard: non-null,
/// non-empty-string, non-zero.
fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
