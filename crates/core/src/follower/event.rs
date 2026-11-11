// SPDX-License-Identifier: MIT

//! Event-lifecycle handlers: joining pods/events, deck submission, ongoing courses,
//! prize claims (event end), completed-draft card pools.

use super::*;

impl<S: Service> Follower<S> {
    pub(super) fn handle_joined_pod(&mut self, obj: &Value) {
        self.clear_game_data(true);
        if let Some(name) = obj.get("EventName") {
            self.cur_draft_event = Some(name.clone());
            log::info!("Joined draft pod: {name}");
        }
    }

    pub(super) fn handle_joined_event_response(&mut self, obj: &Value) {
        self.clear_game_data(true);
        let event = GameEvent::EventJoined {
            raw: obj.clone(),
            ctx: self.context(),
        };
        self.emit(&event);
        log::info!(target: CHATTER, "Joined event successfully");
    }

    pub(super) fn handle_deck_submission(&mut self, obj: &Value) {
        self.clear_game_data(true);
        let Some(decks) = obj.get("Deck") else { return };

        // Entries without a `cardId` are dropped; a missing/invalid quantity counts as 0.
        let entries = |key: &str| -> Vec<DeckEntry> {
            decks
                .get(key)
                .and_then(|d| d.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|d| {
                            d.get("cardId").map(|card_id| DeckEntry {
                                card_id: card_id.clone(),
                                quantity: d.get("quantity").and_then(to_i64).unwrap_or(0),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let companion = decks
            .get("Companions")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.get("cardId"))
            .cloned();

        log::info!("Deck submission (Event_SetDeck)");
        let event = GameEvent::DeckSubmitted(DeckSubmission {
            event_name: obj.get("EventName").cloned(),
            maindeck: entries("MainDeck"),
            sideboard: entries("Sideboard"),
            companion,
            raw: obj.clone(),
            ctx: self.context(),
        });
        self.emit(&event);
    }

    pub(super) fn handle_ongoing_events(&mut self, obj: &Value) {
        log::info!(target: CHATTER, "Updated ongoing events");
        let event = GameEvent::OngoingEvents {
            courses: obj.get("Courses").cloned(),
            ctx: self.context(),
        };
        self.emit(&event);
    }

    pub(super) fn handle_claim_prize(&mut self, obj: &Value) {
        log::info!("Event ended");
        let event = GameEvent::EventEnded {
            event_name: obj.get("EventName").cloned(),
            ctx: self.context(),
        };
        self.emit(&event);
    }

    pub(super) fn handle_event_course(&mut self, obj: &Value) {
        log::info!("Event course");
        let event = GameEvent::EventCourse {
            event_name: obj.get("InternalEventName").cloned(),
            draft_id: obj.get("DraftId").cloned(),
            course_id: obj.get("CourseId").cloned(),
            card_pool: obj.get("CardPool").cloned(),
            raw: obj.clone(),
            ctx: self.context(),
        };
        self.emit(&event);
    }
}
