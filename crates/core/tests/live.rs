// SPDX-License-Identifier: MIT

//! Live views (`recall_core::live`): a view is told of every event, including the ones a
//! muted replay withholds from the services, and of the game in progress - and attaching
//! one changes nothing about what the services are sent.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use recall_core::checkpoint::Checkpoints;
use recall_core::follower::Follower;
use recall_core::live::{GameView, Live, View};
use recall_core::services::seventeenlands::{
    RecordingSubmitter, SeventeenLands, to_python_json_string,
};

const TOKEN: &str = "00000000-0000-4000-8000-000000000000";
const GAME: &str = "tests/fixtures/game.log";

#[derive(Debug, Clone, PartialEq)]
enum Told {
    /// An event's name, and whether it came from the muted replay.
    Event(&'static str, bool),
    Game(Option<GameView>),
}

/// A view that writes down what it is told.
#[derive(Clone, Default)]
struct Recorder(Rc<RefCell<Vec<Told>>>);

impl View for Recorder {
    fn handle(&mut self, update: Live<'_>) {
        self.0.borrow_mut().push(match update {
            Live::Event { event, replayed } => Told::Event(event.name(), replayed),
            Live::Game(game) => Told::Game(game.cloned()),
        });
    }
}

/// One run over `log` with a view attached. Returns what the view was told and what the
/// 17Lands service submitted.
fn run(log: &Path, checkpoints: Option<&mut Checkpoints>) -> (Vec<Told>, Vec<String>) {
    let recorder = Recorder::default();
    let mut f = Follower::with_sink(SeventeenLands::new(TOKEN.into(), RecordingSubmitter::new()))
        .with_view(recorder.clone());
    let never = Arc::new(AtomicBool::new(false));
    f.parse_log_resumable(&log.to_string_lossy(), false, &never, checkpoints);
    let submitted = f
        .sink
        .wire
        .calls
        .iter()
        .map(|c| format!("{} {}", c.endpoint, to_python_json_string(&c.payload)))
        .collect();
    (recorder.0.take(), submitted)
}

fn games(told: &[Told]) -> Vec<&Option<GameView>> {
    told.iter()
        .filter_map(|t| match t {
            Told::Game(game) => Some(game),
            Told::Event(..) => None,
        })
        .collect()
}

#[test]
fn a_view_changes_nothing_the_services_are_sent() {
    let mut plain =
        Follower::with_sink(SeventeenLands::new(TOKEN.into(), RecordingSubmitter::new()));
    plain.parse_log(GAME, false);
    let expected: Vec<String> = plain
        .sink
        .wire
        .calls
        .iter()
        .map(|c| format!("{} {}", c.endpoint, to_python_json_string(&c.payload)))
        .collect();

    let (_, submitted) = run(Path::new(GAME), None);
    assert!(!expected.is_empty());
    assert_eq!(submitted, expected);
}

#[test]
fn the_game_in_progress_adds_up() {
    let (told, _) = run(Path::new(GAME), None);
    let games = games(&told);

    let first = games[0].as_ref().expect("a game starts");
    assert_eq!(first.deck.len(), 60);
    assert_eq!(first.sideboard.len(), 15);

    // Whenever the library's size is known, it is the deck minus the cards seen elsewhere.
    let mut known = 0;
    for game in games.iter().filter_map(|g| g.as_ref()) {
        let Some(library) = game.library else {
            continue;
        };
        let left: usize = game.remaining().iter().map(|c| c.left).sum();
        assert_eq!(library, left, "turn {}", game.turn);
        known += 1;
    }
    assert!(known > 20, "only {known} updates knew the library");

    // The opening seven came out of the library, and the game ends.
    assert!(games.iter().any(|g| {
        g.as_ref()
            .is_some_and(|g| g.hand.len() == 7 && g.library == Some(53))
    }));
    assert_eq!(games.last(), Some(&&None));
    assert_eq!(told.last(), Some(&Told::Event("game_completed", false)));
}

#[test]
fn a_replay_tells_the_view_what_it_withholds_from_the_services() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("Player.log");
    std::fs::copy(GAME, &log).unwrap();
    let checkpoints = || Checkpoints::load_from(Some(tmp.path().join("state.json")), "test".into());

    let (first, submitted) = run(&log, Some(&mut checkpoints()));
    assert!(!submitted.is_empty());
    assert!(first.iter().all(|t| !matches!(t, Told::Event(_, true))));

    // Nothing new in the log: the services get nothing, the view gets it all again.
    let (second, submitted) = run(&log, Some(&mut checkpoints()));
    assert!(submitted.is_empty());
    let unflagged = |told: &[Told]| -> Vec<Told> {
        told.iter()
            .map(|t| match t {
                Told::Event(name, _) => Told::Event(name, false),
                game => game.clone(),
            })
            .collect()
    };
    assert_eq!(unflagged(&second), unflagged(&first));
    assert!(second.iter().all(|t| !matches!(t, Told::Event(_, false))));
}
