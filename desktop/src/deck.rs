//! One player's model (PLAN §7): which of the two it is, what is loaded, and where the
//! transport sits.
//!
//! Deliberately free of rodio and iced. The transport is a pure `transition` function,
//! which is what lets every edge of the state machine be checked with no audio device in
//! the room (PLAN §12) — and what stops "what should Pause do to a stopped player?" from
//! being answered differently in three places.
//!
//! The drop policy lives here too, at the bottom: `drop_outcome` decides what a dropped
//! file does and `idle_target` decides which player gets it, and PLAN §10 wants them
//! side by side because between them they are the whole of what a drop means.
//!
//! The type is `Deck`, not `Player`, because rodio's playback handle is already called
//! `Player`. The *user's* word for the two halves is still "Player 1" / "Player 2", which
//! is what `DeckId::label` returns (PLAN §5).

use std::path::PathBuf;
use std::time::Duration;

/// Which of the two players.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeckId {
	One,
	Two,
}

impl DeckId {
	/// Both of them, in the order they are drawn. Saves writing the pair out at every
	/// call site that has to touch each player in turn.
	pub const ALL: [DeckId; 2] = [DeckId::One, DeckId::Two];

	/// Index into a two-element array of anything per-deck.
	pub fn index(self) -> usize {
		match self {
			DeckId::One => 0,
			DeckId::Two => 1,
		}
	}

	/// What the user calls this player.
	pub fn label(self) -> &'static str {
		match self {
			DeckId::One => "Player 1",
			DeckId::Two => "Player 2",
		}
	}
}

/// The transport state machine of PLAN §7, with `Empty` for "nothing loaded yet".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transport {
	#[default]
	Empty,
	Stopped,
	Playing,
	Paused,
}

impl Transport {
	/// Whether a track is loaded at all. Every transport button is dead without one.
	pub fn has_track(self) -> bool {
		self != Transport::Empty
	}
}

/// Everything that can move the transport. `Ended` is not a button: it is the polled
/// `Player::empty()` going true, which is the only end-of-track signal rodio gives
/// (PLAN §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
	Loaded,
	Play,
	Pause,
	Stop,
	Ended,
}

/// The whole state machine, in one place and with no `self`.
///
/// Two rules make the table read the way it does: a successful load always lands on
/// `Stopped` (loading is not playing, PLAN §7), and an empty player ignores every
/// transport event, because there is nothing to play.
pub fn transition(state: Transport, event: Event) -> Transport {
	match (state, event) {
		(_, Event::Loaded) => Transport::Stopped,
		(Transport::Empty, _) => Transport::Empty,
		(_, Event::Play) => Transport::Playing,
		(_, Event::Stop) => Transport::Stopped,
		(Transport::Playing, Event::Pause) => Transport::Paused,
		(Transport::Playing, Event::Ended) => Transport::Stopped,
		// Pausing a stopped player, or a track ending on a player that is not playing:
		// both are events arriving for a state that has already moved past them.
		(_, Event::Pause | Event::Ended) => state,
	}
}

/// What is loaded in a player.
#[derive(Debug, Clone)]
pub struct Track {
	pub path: PathBuf,
	/// The file name, kept as its own field because the view needs it every frame and
	/// `Path::file_name` returns an `OsStr` that has to be re-converted each time.
	pub name: String,
	/// `None` when the decoder cannot work one out — a real case for a stream, and the
	/// reason the time readout has to handle a missing total (PLAN §7).
	pub duration: Option<Duration>,
}

/// One player, as the app models it.
#[derive(Debug)]
pub struct Deck {
	pub transport: Transport,
	pub track: Option<Track>,
	/// Last polled playhead. The decoder's position, which leads the speaker by the
	/// device buffer (PLAN §7) — fine for a readout.
	pub position: Duration,
	/// This player's volume fader, 0..=1. Not a gain: `mixer::gains` tapers it and folds
	/// the crossfader in (PLAN §8).
	pub fader: f32,
}

impl Default for Deck {
	fn default() -> Self {
		Self {
			transport: Transport::Empty,
			track: None,
			position: Duration::ZERO,
			// Wide open, not zero. A player that starts silent reads as broken.
			fader: 1.0,
		}
	}
}

impl Deck {
	/// The track name, or the placeholder the empty player shows.
	pub fn title(&self) -> &str {
		match &self.track {
			Some(track) => &track.name,
			None => "── no track ──",
		}
	}

	pub fn is_playing(&self) -> bool {
		self.transport == Transport::Playing
	}
}

/// Which player a file lands on when the gesture carries no aim — an OS drop (PLAN §10),
/// and a double-click in the files pane, which has the same "I did not say where"
/// quality.
///
/// Derived from state, so there is no armed flag to hold and nothing to get out of sync
/// with what the user sees. Rule 2 is the one that matters: it never cuts off audible
/// playback while an idle player exists.
///
/// `ponytail:` both playing means Player 1 loses its track. Arbitrary, but the hover ring
/// says which before the release, so it is shown rather than surprising. Any other
/// tie-break is one line and one test away.
pub fn idle_target(deck1: &Deck, deck2: &Deck) -> DeckId {
	if !deck1.transport.has_track() {
		DeckId::One
	} else if !deck2.transport.has_track() {
		DeckId::Two
	} else if !deck1.is_playing() {
		DeckId::One
	} else if !deck2.is_playing() {
		DeckId::Two
	} else {
		DeckId::One
	}
}

/// What one dropped path does.
///
/// `Decline` rather than a silent no-op, because a gesture that appears to do nothing is
/// indistinguishable from a broken app (PLAN §10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropOutcome {
	Load(PathBuf),
	Decline(String),
}

/// Decide what a dropped path does, before any player is touched.
///
/// `first` separates the first file of a drop from the rest of it: a multi-file drop
/// arrives as one event per file, and with two players and no queue the first one wins
/// and the others are declined out loud (PLAN §10).
///
/// Not free of the filesystem — `is_dir` is the only way to tell a folder from a file
/// with no extension — but free of `self`, which is what makes both gestures share one
/// decision and lets it be tested with no window (PLAN §12).
pub fn drop_outcome(path: PathBuf, first: bool) -> DropOutcome {
	let name = crate::fsio::name_of(&path);

	if !first {
		DropOutcome::Decline(format!("one file at a time — {name} was ignored"))
	} else if path.is_dir() {
		DropOutcome::Decline(format!("{name} is a folder"))
	} else if !crate::browser::kind_of(&path).is_media() {
		// The extension only. Whatever the decoder says is the real answer, and `load`
		// is where that arrives (PLAN §3).
		DropOutcome::Decline(format!("{name} is not a media file"))
	} else {
		DropOutcome::Load(path)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A deck in a given transport state, with a track whenever the state implies one.
	fn deck(transport: Transport) -> Deck {
		Deck {
			transport,
			track: transport.has_track().then(|| Track {
				path: PathBuf::from("/music/track.mp3"),
				name: "track.mp3".to_string(),
				duration: None,
			}),
			..Default::default()
		}
	}

	#[test]
	fn loading_always_lands_on_stopped() {
		// Arrange / Act / Assert: from every state, including over a playing track —
		// load replaces, and the replacement is paused at 0 (PLAN §7).
		for state in [
			Transport::Empty,
			Transport::Stopped,
			Transport::Playing,
			Transport::Paused,
		] {
			assert_eq!(
				transition(state, Event::Loaded),
				Transport::Stopped,
				"loading over {state:?}"
			);
		}
	}

	#[test]
	fn an_empty_player_ignores_every_transport_event() {
		// Arrange / Act / Assert: the buttons are drawn disabled, but the state machine
		// must not depend on the view having remembered to do that.
		for event in [Event::Play, Event::Pause, Event::Stop, Event::Ended] {
			assert_eq!(
				transition(Transport::Empty, event),
				Transport::Empty,
				"{event:?} on an empty player"
			);
		}
	}

	#[test]
	fn play_pause_and_stop_walk_the_three_states() {
		// Arrange / Act / Assert: the loop in PLAN §7's diagram, edge by edge.
		assert_eq!(
			transition(Transport::Stopped, Event::Play),
			Transport::Playing
		);
		assert_eq!(
			transition(Transport::Playing, Event::Pause),
			Transport::Paused
		);
		assert_eq!(
			transition(Transport::Paused, Event::Play),
			Transport::Playing
		);
		assert_eq!(
			transition(Transport::Playing, Event::Stop),
			Transport::Stopped
		);
		assert_eq!(
			transition(Transport::Paused, Event::Stop),
			Transport::Stopped
		);
	}

	#[test]
	fn an_event_for_a_state_already_moved_past_changes_nothing() {
		// Arrange / Act / Assert: a tick can report `empty()` on a player the user has
		// just stopped, and Pause can arrive for a player that is not playing. Neither
		// is an error; both are no-ops.
		assert_eq!(
			transition(Transport::Stopped, Event::Ended),
			Transport::Stopped
		);
		assert_eq!(
			transition(Transport::Paused, Event::Ended),
			Transport::Paused
		);
		assert_eq!(
			transition(Transport::Stopped, Event::Pause),
			Transport::Stopped
		);
		assert_eq!(
			transition(Transport::Paused, Event::Pause),
			Transport::Paused
		);
	}

	#[test]
	fn an_unaimed_load_prefers_an_empty_player_then_an_idle_one() {
		// Arrange / Act / Assert: the table from PLAN §12, case for case.
		let empty = || deck(Transport::Empty);
		let stopped = || deck(Transport::Stopped);
		let playing = || deck(Transport::Playing);
		let paused = || deck(Transport::Paused);

		assert_eq!(idle_target(&empty(), &empty()), DeckId::One, "both empty");
		assert_eq!(
			idle_target(&stopped(), &empty()),
			DeckId::Two,
			"one loaded, two empty"
		);
		assert_eq!(
			idle_target(&empty(), &stopped()),
			DeckId::One,
			"one empty, two loaded"
		);
		assert_eq!(
			idle_target(&playing(), &paused()),
			DeckId::Two,
			"never interrupt the playing one"
		);
		assert_eq!(
			idle_target(&paused(), &playing()),
			DeckId::One,
			"never interrupt the playing one, mirrored"
		);
		assert_eq!(
			idle_target(&playing(), &playing()),
			DeckId::One,
			"both playing — arbitrary but total"
		);
	}

	#[test]
	fn an_unaimed_load_never_names_a_playing_player_while_an_idle_one_exists() {
		// Arrange: every pairing of the four states.
		let states = [
			Transport::Empty,
			Transport::Stopped,
			Transport::Playing,
			Transport::Paused,
		];

		for first in states {
			for second in states {
				let (deck1, deck2) = (deck(first), deck(second));

				// Act
				let target = idle_target(&deck1, &deck2);
				let chosen = match target {
					DeckId::One => &deck1,
					DeckId::Two => &deck2,
				};

				// Assert: the invariant, not the table — this is what rule 2 is for.
				if chosen.is_playing() {
					assert!(
						deck1.is_playing() && deck2.is_playing(),
						"{first:?} + {second:?} chose a playing player with an idle one available"
					);
				}
			}
		}
	}

	#[test]
	fn a_dropped_media_file_loads() {
		// Arrange / Act / Assert: the ordinary case. The path need not exist — only a
		// folder check touches the disk.
		let path = PathBuf::from("/music/track.mp3");
		assert_eq!(drop_outcome(path.clone(), true), DropOutcome::Load(path));
	}

	#[test]
	fn a_dropped_folder_is_declined_by_name() {
		// Arrange: a directory that certainly exists, so no fixture has to be created —
		// the extension test alone could not tell this from a file (PLAN §10).
		let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

		// Act
		let outcome = drop_outcome(path, true);

		// Assert
		let DropOutcome::Decline(reason) = outcome else {
			panic!("a folder must be declined");
		};
		assert!(reason.contains("folder"), "{reason:?}");
		assert!(
			reason.contains("desktop"),
			"names the thing dropped: {reason:?}"
		);
	}

	#[test]
	fn a_dropped_non_media_file_is_declined() {
		// Arrange / Act / Assert: declined here rather than after a decoder error, so the
		// notice says something the user can act on.
		let outcome = drop_outcome(PathBuf::from("/music/sleeve.jpg"), true);
		let DropOutcome::Decline(reason) = outcome else {
			panic!("a non-media file must be declined");
		};
		assert!(reason.contains("not a media file"), "{reason:?}");
	}

	#[test]
	fn only_the_first_file_of_a_multi_file_drop_is_taken() {
		// Arrange / Act: a perfectly good second media file, arriving as the second event
		// of one drop.
		let outcome = drop_outcome(PathBuf::from("/music/second.flac"), false);

		// Assert: declined, and said out loud — the rest of the drop vanishing in silence
		// is the outcome PLAN §10 rules out.
		let DropOutcome::Decline(reason) = outcome else {
			panic!("the rest of a multi-file drop must be declined");
		};
		assert!(reason.contains("one file at a time"), "{reason:?}");
		assert!(reason.contains("second.flac"), "{reason:?}");
	}
}
