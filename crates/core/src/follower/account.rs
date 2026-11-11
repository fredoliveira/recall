// SPDX-License-Identifier: MIT

//! Account and player-level handlers: login/logout/reconnect, screen names, rank,
//! collection, inventory, mastery progress.

use super::*;

impl<S: Service> Follower<S> {
    pub(super) fn maybe_handle_account_info(&mut self, line: &str) {
        if let Some(c) = re_account_info().captures(line) {
            let screen_name = c[1].to_string();
            self.user.id = Some(c[2].to_string());
            self.update_screen_name(&screen_name);
            return;
        }
        if let Some(c) = re_match_account_info().captures(line) {
            self.user.id = c
                .get(2)
                .or_else(|| c.get(3))
                .map(|m| m.as_str().to_string());
            return;
        }
        if let Some(c) = re_login().captures(line) {
            self.user.full_screen_name = Some(c[1].to_string());
        }
    }

    pub(super) fn update_screen_name(&mut self, screen_name: &str) {
        if self.user.screen_name.as_deref() == Some(screen_name) {
            return;
        }
        self.user.screen_name = Some(screen_name.to_string());
        log::info!(target: CHATTER, "Updating user info");
        let event = GameEvent::ScreenNameUpdated {
            screen_name: self.user.screen_name.clone(),
            full_screen_name: self.user.full_screen_name.clone(),
            ctx: self.context(),
        };
        self.emit(&event);
    }

    pub(super) fn handle_login(&mut self, obj: &Value) {
        self.clear_game_data(false);
        if let Some(po) = obj.get("params").and_then(|p| p.get("payloadObject")) {
            if let Some(pid) = po.get("playerId").and_then(|v| v.as_str()) {
                self.user.id = Some(pid.to_string());
            }
            if let Some(name) = po.get("screenName").and_then(|v| v.as_str()) {
                self.update_screen_name(name);
            }
        }
    }

    pub(super) fn handle_self_rank_info(&mut self, obj: &Value) {
        self.user.rank = Some(obj.clone());
        if let Some(pid) = obj.get("playerId").and_then(|v| v.as_str()) {
            self.user.id = Some(pid.to_string());
        }
        log::info!(target: CHATTER, "Parsed rank info");
        let event = GameEvent::RankUpdated {
            rank_data: obj.clone(),
            ctx: self.context(),
        };
        self.emit(&event);
    }

    pub(super) fn handle_collection(&mut self, obj: &Value) {
        if self.user.id.is_none() {
            log::info!("Skipping collection submission because player id is still unknown");
            return;
        }
        log::info!(target: CHATTER, "Collection submission");
        let event = GameEvent::CollectionUpdated {
            card_counts: obj.clone(),
            ctx: self.context(),
        };
        self.emit(&event);
    }

    pub(super) fn handle_inventory(&mut self, obj: &Value) {
        log::info!(target: CHATTER, "Submitting inventory");
        let event = GameEvent::InventoryUpdated {
            inventory: obj.clone(),
            ctx: self.context(),
        };
        self.emit(&event);
    }

    pub(super) fn handle_player_progress(&mut self, obj: &Value) {
        log::info!(target: CHATTER, "Submitting mastery progress");
        let event = GameEvent::MasteryProgress {
            progress: obj.clone(),
            ctx: self.context(),
        };
        self.emit(&event);
    }

    pub(super) fn reset_current_user(&mut self) {
        log::info!("User logged out from MTGA");
        if self.user.id.is_some() {
            self.disconnected = self.user.clone();
        }
        self.user = UserInfo::default();
    }

    pub(super) fn handle_reconnect_result(&mut self) {
        log::info!("Reconnected - restoring prior user info");
        self.user = self.disconnected.clone();
    }
}
