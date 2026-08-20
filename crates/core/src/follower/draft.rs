// SPDX-License-Identifier: MIT

//! Draft handlers: bot drafts (`DraftStatus`/`BotDraft_DraftPick`) and human drafts
//! (`LogBusinessEvents`, `Draft.Notify`, `EventPlayerDraftMakePick`).

use super::*;

impl<S: Service> Follower<S> {
    pub(super) fn handle_bot_draft_pack(&mut self, obj: &Value) {
        if obj.get("DraftStatus").and_then(|v| v.as_str()) != Some("PickNext") {
            return;
        }
        self.clear_game_data(true);
        self.cur_draft_event = obj.get("EventName").cloned();
        log::info!("Draft pack");
        let event = GameEvent::DraftPack(DraftPack {
            source: DraftSource::BotDraft,
            draft_id: None,
            event_name: obj.get("EventName").cloned(),
            pack_number: obj.get("PackNumber").and_then(to_i64),
            pick_number: obj.get("PickNumber").and_then(to_i64),
            card_ids: Value::Array(obj.get("DraftPack").map(int_array).unwrap_or_default()),
            raw: obj.clone(),
            ctx: self.context(),
        });
        self.sink.handle(&event);
    }

    pub(super) fn handle_bot_draft_pick(&mut self, obj: &Value) {
        self.clear_game_data(true);
        self.cur_draft_event = obj.get("EventName").cloned();
        log::info!("Draft pick");
        let event = GameEvent::DraftPick(DraftPick {
            source: DraftSource::BotDraft,
            draft_id: None,
            event_name: obj.get("EventName").cloned(),
            pack_number: obj.get("PackNumber").and_then(to_i64),
            pick_number: obj.get("PickNumber").and_then(to_i64),
            card_id: obj.get("CardId").and_then(to_i64),
            card_ids: obj.get("CardIds").map(int_array).map(Value::Array),
            auto_pick: None,
            time_remaining: None,
            raw: None,
            ctx: self.context(),
        });
        self.sink.handle(&event);
    }

    pub(super) fn handle_human_draft_combined(&mut self, obj: &Value) {
        self.clear_game_data(true);
        self.cur_draft_event = obj.get("EventId").cloned();

        log::info!("Human draft pack (combined)");
        let pack = GameEvent::DraftPack(DraftPack {
            source: DraftSource::LogBusiness,
            draft_id: obj.get("DraftId").cloned(),
            event_name: obj.get("EventId").cloned(),
            pack_number: obj.get("PackNumber").and_then(to_i64),
            pick_number: obj.get("PickNumber").and_then(to_i64),
            card_ids: obj.get("CardsInPack").cloned().unwrap_or(Value::Null),
            raw: obj.clone(),
            ctx: self.context(),
        });
        self.sink.handle(&pack);

        log::info!("Human draft pick (combined)");
        let pick = GameEvent::DraftPick(DraftPick {
            source: DraftSource::LogBusiness,
            draft_id: obj.get("DraftId").cloned(),
            event_name: obj.get("EventId").cloned(),
            pack_number: obj.get("PackNumber").and_then(to_i64),
            pick_number: obj.get("PickNumber").and_then(to_i64),
            // int(PickGrpId), or None on failure.
            card_id: obj.get("PickGrpId").and_then(to_i64),
            card_ids: None,
            auto_pick: obj.get("AutoPick").cloned(),
            time_remaining: obj.get("TimeRemainingOnPick").cloned(),
            raw: Some(obj.clone()),
            ctx: self.context(),
        });
        self.sink.handle(&pick);
    }

    pub(super) fn handle_human_draft_pack(&mut self, obj: &Value) {
        self.clear_game_data(true);

        // `PackCards` is a comma-separated string of card ids.
        let card_ids: Vec<Value> = obj
            .get("PackCards")
            .and_then(|v| v.as_str())
            .map(|s| {
                s.split(',')
                    .filter_map(|x| x.trim().parse::<i64>().ok())
                    .map(Value::from)
                    .collect()
            })
            .unwrap_or_default();

        log::info!("Human draft pack (Draft.Notify)");
        let event = GameEvent::DraftPack(DraftPack {
            source: DraftSource::DraftNotify,
            draft_id: obj.get("draftId").cloned(),
            event_name: self.cur_draft_event.clone(),
            pack_number: obj.get("SelfPack").and_then(to_i64),
            pick_number: obj.get("SelfPick").and_then(to_i64),
            card_ids: Value::Array(card_ids),
            raw: obj.clone(),
            ctx: self.context(),
        });
        self.sink.handle(&event);
    }

    pub(super) fn handle_player_draft_pick(&mut self, obj: &Value) {
        self.clear_game_data(true);
        log::info!("Human draft pick (EventPlayerDraftMakePick)");
        let event = GameEvent::DraftPick(DraftPick {
            source: DraftSource::PlayerDraftMakePick,
            draft_id: obj.get("DraftId").cloned(),
            event_name: self.cur_draft_event.clone(),
            pack_number: obj.get("Pack").and_then(to_i64),
            pick_number: obj.get("Pick").and_then(to_i64),
            card_id: None,
            card_ids: obj.get("GrpIds").cloned(),
            auto_pick: None,
            time_remaining: None,
            raw: Some(obj.clone()),
            ctx: self.context(),
        });
        self.sink.handle(&event);
    }
}
