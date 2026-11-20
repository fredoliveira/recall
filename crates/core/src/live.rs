// SPDX-License-Identifier: MIT

//! Live views: consumers that show what is happening now (an overlay, a status display)
//! rather than deliver it somewhere.
//!
//! A [`Service`](crate::events::Service) and a [`View`] want different things from the same
//! log. A service must get each event once, so the replay after a restart is muted for it
//! (see [`crate::checkpoint`]). A view has no memory to protect: it wants the state the log
//! describes, and that includes everything the replay rebuilds - the pack that was on
//! screen when the app quit, the game that is half played. So a view is handed every event,
//! muted or not, with a flag saying which.
//!
//! A view also gets what no service is sent: the game in progress, as a [`GameView`]
//! rebuilt after each GRE game-state message. Services only ever see a game once it is
//! over ([`GameEvent::GameCompleted`]).
//!
//! Not part of the wire contract. Nothing here reaches a service, and a follower without a
//! view does none of this work.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;

use crate::events::GameEvent;

/// A consumer of the live stream. Attach one with
/// [`Follower::with_view`](crate::follower::Follower::with_view).
pub trait View {
    fn handle(&mut self, update: Live<'_>);
}

/// One thing a [`View`] is told.
pub enum Live<'a> {
    /// An event, exactly as the services get it. `replayed` marks one from the muted replay
    /// after a restart: it happened before this run, and the services are not sent it again.
    Event {
        event: &'a GameEvent,
        replayed: bool,
    },
    /// The game in progress as it now stands, or `None` once there is none.
    Game(Option<&'a GameView>),
}

/// The game in progress, from the seat of the player whose log this is.
///
/// Cards are Arena card ids (`grpId`), one entry per copy. A card is identified the way the
/// game record identifies a drawn card: its `overlayGrpId`, for objects of type `Card` or
/// `SplitCard`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameView {
    /// The player's seat, once a game-state message has said which it is.
    pub seat: Option<i64>,
    /// The deck as submitted for this game.
    pub deck: Vec<i64>,
    pub sideboard: Vec<i64>,
    /// How many cards are in the player's library, once known.
    pub library: Option<usize>,
    /// The player's cards in hand.
    pub hand: Vec<i64>,
    /// The player's own cards known to be outside the library: in hand, on the battlefield
    /// or the stack, in the graveyard or in exile. Includes cards that never were in the
    /// deck (conjured, fetched from the sideboard).
    pub seen: Vec<i64>,
    /// The opponent's cards the player has been shown, in the same zones.
    pub opponent_seen: Vec<i64>,
    pub turn: i64,
}

/// One distinct card of a deck: how many copies it started with and how many are unseen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Remaining {
    pub card_id: i64,
    pub total: usize,
    pub left: usize,
}

impl GameView {
    /// The deck card by card, in deck order, with how many copies of each are still unseen:
    /// the deck minus [`seen`](Self::seen). That is what is left in the library, give or take
    /// what the log cannot show (a card of the player's exiled face down, say), so the
    /// `left` counts can add up to more than [`library`](Self::library).
    pub fn remaining(&self) -> Vec<Remaining> {
        let mut cards: Vec<Remaining> = Vec::new();
        for &card_id in &self.deck {
            match cards.iter_mut().find(|c| c.card_id == card_id) {
                Some(card) => card.total += 1,
                None => cards.push(Remaining {
                    card_id,
                    total: 1,
                    left: 0,
                }),
            }
        }
        let mut seen: HashMap<i64, usize> = HashMap::new();
        for card_id in &self.seen {
            *seen.entry(*card_id).or_default() += 1;
        }
        for card in &mut cards {
            card.left = card
                .total
                .saturating_sub(seen.get(&card.card_id).copied().unwrap_or(0));
        }
        cards
    }
}

#[derive(Debug, Clone, Copy)]
struct Object {
    card_id: Option<i64>,
    owner: Option<i64>,
}

#[derive(Debug, Default)]
struct Zone {
    kind: String,
    owner: Option<i64>,
    ids: Vec<i64>,
}

/// Rebuilds the [`GameView`] from GRE game-state messages: one full snapshot, then diffs.
///
/// State-based on purpose. Where a card is comes from the zones' own contents, never from
/// following it from zone to zone, so a mulligan, a shuffle or a card going back on top of
/// the library needs no special case: the card's object is deleted, its id leaves every
/// zone but the library, and it stops counting as seen.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    view: GameView,
    /// A game is under way.
    active: bool,
    /// What was last handed to the view, to stay quiet when a message changes nothing.
    published: Option<GameView>,
    objects: HashMap<i64, Object>,
    /// Ordered by zone id, so the card lists come out in the same order on every run.
    zones: BTreeMap<i64, Zone>,
}

fn int(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.parse().ok())
}

fn ints(v: Option<&Value>) -> Vec<i64> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(int).collect())
        .unwrap_or_default()
}

impl Tracker {
    /// A deck was submitted: a game is starting with it.
    pub(crate) fn start(&mut self, deck: Option<&Value>, sideboard: Option<&Value>) {
        *self = Tracker {
            view: GameView {
                deck: ints(deck),
                sideboard: ints(sideboard),
                ..GameView::default()
            },
            active: true,
            // Kept, so the view hears of the new game even if it looks like the last one.
            published: self.published.take(),
            ..Tracker::default()
        };
    }

    /// The game is over, or the log moved on without saying so.
    pub(crate) fn end(&mut self) {
        self.active = false;
        self.objects.clear();
        self.zones.clear();
    }

    /// Apply one `gameStateMessage`. `seat` is the seat the enclosing message was sent to.
    pub(crate) fn apply(&mut self, seat: Option<i64>, gsm: &Value) {
        let stage = gsm
            .get("gameInfo")
            .and_then(|info| info.get("stage"))
            .and_then(Value::as_str);
        let full = gsm.get("type").and_then(Value::as_str) == Some("GameStateType_Full");
        if stage == Some("GameStage_GameOver") {
            self.end();
            return;
        }
        if !self.active {
            // The next game of a match starts with a full snapshot and, when the deck was
            // not changed between games, nothing else.
            if !(full && stage == Some("GameStage_Start")) {
                return;
            }
            let (deck, sideboard) = (
                std::mem::take(&mut self.view.deck),
                std::mem::take(&mut self.view.sideboard),
            );
            self.view = GameView {
                deck,
                sideboard,
                ..GameView::default()
            };
            self.active = true;
        }
        if full {
            self.objects.clear();
            self.zones.clear();
        }
        if seat.is_some() {
            self.view.seat = seat;
        }
        if let Some(turn) = gsm
            .get("turnInfo")
            .and_then(|t| t.get("turnNumber"))
            .and_then(int)
        {
            self.view.turn = turn;
        }

        for object in gsm
            .get("gameObjects")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = object.get("instanceId").and_then(int) else {
                continue;
            };
            let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
            let is_card = matches!(kind, "GameObjectType_Card" | "GameObjectType_SplitCard");
            let card_id = object
                .get("overlayGrpId")
                .or_else(|| object.get("grpId"))
                .and_then(int)
                .filter(|&id| is_card && id != 0);
            self.objects.insert(
                id,
                Object {
                    card_id,
                    owner: object.get("ownerSeatId").and_then(int),
                },
            );
        }
        // A zone in a message carries its whole contents; one without `objectInstanceIds`
        // is now empty (the log leaves empty lists out).
        for zone in gsm
            .get("zones")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = zone.get("zoneId").and_then(int) else {
                continue;
            };
            let entry = self.zones.entry(id).or_default();
            if let Some(kind) = zone.get("type").and_then(Value::as_str) {
                entry.kind = kind.to_string();
            }
            if let Some(owner) = zone.get("ownerSeatId").and_then(int) {
                entry.owner = Some(owner);
            }
            entry.ids = ints(zone.get("objectInstanceIds"));
        }
        // A deleted object is not always followed by its zone's new contents.
        let deleted = ints(gsm.get("diffDeletedInstanceIds"));
        if !deleted.is_empty() {
            for id in &deleted {
                self.objects.remove(id);
            }
            for zone in self.zones.values_mut() {
                zone.ids.retain(|id| !deleted.contains(id));
            }
        }

        self.rebuild();
    }

    /// Recompute the card lists from the zones.
    fn rebuild(&mut self) {
        let seat = self.view.seat;
        let (mut hand, mut seen, mut opponent_seen) = (Vec::new(), Vec::new(), Vec::new());
        let mut library = None;
        for zone in self.zones.values() {
            let mine = zone.owner.is_some() && zone.owner == seat;
            match zone.kind.as_str() {
                "ZoneType_Library" if mine => library = Some(zone.ids.len()),
                "ZoneType_Hand"
                | "ZoneType_Battlefield"
                | "ZoneType_Stack"
                | "ZoneType_Graveyard"
                | "ZoneType_Exile" => {
                    for object in zone.ids.iter().filter_map(|id| self.objects.get(id)) {
                        let Some(card_id) = object.card_id else {
                            continue;
                        };
                        if object.owner.is_some() && object.owner == seat {
                            seen.push(card_id);
                            if zone.kind == "ZoneType_Hand" {
                                hand.push(card_id);
                            }
                        } else {
                            opponent_seen.push(card_id);
                        }
                    }
                }
                _ => {}
            }
        }
        self.view.library = library;
        self.view.hand = hand;
        self.view.seen = seen;
        self.view.opponent_seen = opponent_seen;
    }

    /// The view to hand on, if it differs from the last one handed on: `Some(None)` when
    /// the game ended since.
    pub(crate) fn changed(&mut self) -> Option<Option<&GameView>> {
        let current = self.active.then_some(&self.view);
        if current == self.published.as_ref() {
            return None;
        }
        self.published = current.cloned();
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn card(id: i64, card_id: i64, owner: i64) -> Value {
        json!({"instanceId": id, "grpId": card_id, "overlayGrpId": card_id, "type": "GameObjectType_Card", "ownerSeatId": owner})
    }

    fn started() -> Tracker {
        let mut tracker = Tracker::default();
        tracker.start(Some(&json!([10, 10, 11, 12])), Some(&json!([20])));
        tracker.apply(
            Some(1),
            &json!({
                "type": "GameStateType_Full",
                "gameInfo": {"stage": "GameStage_Start"},
                "zones": [
                    {"zoneId": 1, "type": "ZoneType_Hand", "ownerSeatId": 1},
                    {"zoneId": 2, "type": "ZoneType_Library", "ownerSeatId": 1, "objectInstanceIds": [100, 101, 102, 103]},
                    {"zoneId": 3, "type": "ZoneType_Battlefield"},
                    {"zoneId": 4, "type": "ZoneType_Hand", "ownerSeatId": 2},
                    {"zoneId": 5, "type": "ZoneType_Library", "ownerSeatId": 2, "objectInstanceIds": [200, 201]},
                ],
            }),
        );
        tracker
    }

    fn left(view: &GameView) -> Vec<(i64, usize, usize)> {
        view.remaining()
            .iter()
            .map(|c| (c.card_id, c.left, c.total))
            .collect()
    }

    #[test]
    fn a_new_game_has_the_whole_deck_left() {
        let tracker = started();
        assert_eq!(tracker.view.library, Some(4));
        assert_eq!(left(&tracker.view), [(10, 2, 2), (11, 1, 1), (12, 1, 1)]);
    }

    #[test]
    fn a_drawn_card_leaves_the_library() {
        let mut tracker = started();
        tracker.apply(
            Some(1),
            &json!({
                "type": "GameStateType_Diff",
                "turnInfo": {"turnNumber": 1},
                "gameObjects": [card(150, 10, 1)],
                "zones": [
                    {"zoneId": 1, "type": "ZoneType_Hand", "ownerSeatId": 1, "objectInstanceIds": [150]},
                    {"zoneId": 2, "type": "ZoneType_Library", "ownerSeatId": 1, "objectInstanceIds": [101, 102, 103]},
                ],
                "diffDeletedInstanceIds": [100],
            }),
        );
        assert_eq!(tracker.view.library, Some(3));
        assert_eq!(tracker.view.hand, [10]);
        assert_eq!(tracker.view.turn, 1);
        assert_eq!(left(&tracker.view), [(10, 1, 2), (11, 1, 1), (12, 1, 1)]);
    }

    #[test]
    fn a_card_shuffled_back_counts_as_left_again() {
        let mut tracker = started();
        tracker.apply(
            Some(1),
            &json!({
                "gameObjects": [card(150, 11, 1)],
                "zones": [{"zoneId": 1, "ownerSeatId": 1, "objectInstanceIds": [150]}],
            }),
        );
        assert_eq!(left(&tracker.view)[1], (11, 0, 1));
        // Back into the library: the object goes, and the hand is sent without its list.
        tracker.apply(Some(1), &json!({"diffDeletedInstanceIds": [150]}));
        assert_eq!(tracker.view.hand, Vec::<i64>::new());
        assert_eq!(left(&tracker.view)[1], (11, 1, 1));
    }

    #[test]
    fn the_opponents_cards_and_tokens_are_not_the_players() {
        let mut tracker = started();
        tracker.apply(
            Some(1),
            &json!({
                "gameObjects": [
                    card(300, 12, 2),
                    {"instanceId": 301, "grpId": 10, "type": "GameObjectType_Token", "ownerSeatId": 1},
                ],
                "zones": [{"zoneId": 3, "objectInstanceIds": [300, 301]}],
            }),
        );
        assert_eq!(tracker.view.seen, Vec::<i64>::new());
        assert_eq!(tracker.view.opponent_seen, [12]);
        assert_eq!(left(&tracker.view), [(10, 2, 2), (11, 1, 1), (12, 1, 1)]);
    }

    #[test]
    fn the_view_is_handed_on_only_when_it_changes() {
        let mut tracker = started();
        assert!(matches!(tracker.changed(), Some(Some(_))));
        assert!(tracker.changed().is_none());
        tracker.apply(
            Some(1),
            &json!({"gameInfo": {"stage": "GameStage_GameOver"}}),
        );
        assert!(matches!(tracker.changed(), Some(None)));
        assert!(tracker.changed().is_none());
        // Messages after the end (the match result) start nothing.
        tracker.apply(
            Some(1),
            &json!({"zones": [{"zoneId": 1, "ownerSeatId": 1}]}),
        );
        assert!(tracker.changed().is_none());
    }

    #[test]
    fn the_next_game_of_a_match_reuses_the_deck() {
        let mut tracker = started();
        tracker.apply(
            Some(1),
            &json!({"gameInfo": {"stage": "GameStage_GameOver"}}),
        );
        tracker.apply(
            Some(1),
            &json!({
                "type": "GameStateType_Full",
                "gameInfo": {"stage": "GameStage_Start"},
                "zones": [{"zoneId": 2, "type": "ZoneType_Library", "ownerSeatId": 1, "objectInstanceIds": [1, 2, 3, 4]}],
            }),
        );
        assert_eq!(tracker.view.deck, [10, 10, 11, 12]);
        assert_eq!(tracker.view.library, Some(4));
        assert!(matches!(tracker.changed(), Some(Some(_))));
    }
}
