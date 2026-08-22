//! One player's model (PLAN §7): which of the two it is, what is loaded, and where the
//! transport sits.
//!
//! Free of rodio and iced. The transport is a pure `transition` function, which is what
//! stops "what should Pause do to a stopped player?" from being answered differently in
//! three places — and the methods that *pair* that decision with the audio take the engine as
//! an `Option`, so passing `None` leaves the state machine and nothing else. Every edge is
//! therefore still checkable with no audio device in the room (PLAN §12, Q48).
//!
//! It names `audio::Engine`, which names rodio; it does not name rodio. The seam is that the
//! only module that knows what a `rodio::Player` is remains `audio`.
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

use crate::audio::Engine;

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
/// (PLAN §4). Neither is `Seeked` — it is a click on the waveform or one of the two jump
/// buttons above it (PLAN §14b, §14c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
	Loaded,
	Play,
	Pause,
	Stop,
	Ended,
	Seeked,
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
		// A seek leaves the transport alone (Q14) — except from `Stopped`, which in this app
		// means *at the top of the track*: it is what Stop rewinds to and what every load
		// lands on, so a player labelled "stopped" sitting at 1:30 is the label lying about
		// where Play would start. The edge lived in `app.rs` until Q48 and is here now,
		// because a transport edge that the state machine has never heard of is a transport
		// edge nothing can check.
		(Transport::Stopped, Event::Seeked) => Transport::Paused,
		// Pausing a stopped player, a track ending on a player that is not playing, a seek on
		// one that is already going: all events arriving for a state that has moved past them.
		(_, Event::Pause | Event::Ended | Event::Seeked) => state,
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
	/// The loaded track's amplitude scan, drawn by `ui::waveform` (PLAN §14a).
	///
	/// Empty until it lands: the scan decodes the whole file on a thread of its own and a
	/// long track takes a good fraction of a second. Empty also covers a scan that failed,
	/// which is a case the notice line has already explained — a player with no waveform
	/// still plays.
	pub peaks: Vec<f32>,
	/// A scan is running for this player right now, which is what makes the strip animate.
	///
	/// Not derivable from `peaks` being empty: so is an empty player, and so is one whose
	/// scan failed, and neither of those should be shown as working.
	pub scanning: bool,
}

impl Default for Deck {
	fn default() -> Self {
		Self {
			transport: Transport::Empty,
			track: None,
			position: Duration::ZERO,
			// Wide open, not zero. A player that starts silent reads as broken.
			fader: 1.0,
			peaks: Vec::new(),
			scanning: false,
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

	/// Move the transport, and make the audio agree (Q48).
	///
	/// The pure decision and the effect it implies, paired **here** and nowhere else. They used
	/// to be re-paired by hand in four places in `update`, which is how the seek grew an edge
	/// the state machine had never heard of.
	///
	/// Returns the line for the status bar when the device refused, and `None` otherwise. A
	/// returned error does **not** stop the transport moving: a stream that will not rewind is
	/// still a player the user pressed Stop on, and leaving the button lit would be a second
	/// wrong answer on top of the device's.
	///
	/// `engine` is an `Option` because the app runs without an audio device (PLAN §11) — which
	/// is also what makes every edge of this checkable with nothing plugged in: pass `None` and
	/// what is left is the state machine.
	pub fn moved(&mut self, id: DeckId, event: Event, engine: &Option<Engine>) -> Option<String> {
		let next = transition(self.transport, event);
		if next == self.transport && self.transport == Transport::Empty {
			return None;
		}

		let outcome = match (engine, event) {
			(Some(engine), Event::Play) => {
				engine.play(id);
				Ok(())
			}
			(Some(engine), Event::Pause) => {
				engine.pause(id);
				Ok(())
			}
			// `ponytail:` a stream that cannot seek fails to rewind. PLAN §7's fallback is to
			// re-open and re-append the file; for now the transport still stops and the notice
			// says the position stayed put.
			(Some(engine), Event::Stop) => engine.stop(id),
			_ => Ok(()),
		};

		self.transport = next;
		// Both ends of a track land at the top of it. `Stop` is the button and `Ended` is the
		// file running out, and they were zeroing the playhead in three separate places before
		// this — with `Ended` doing it in two of them and not in the third.
		if matches!(event, Event::Stop | Event::Ended) {
			self.position = Duration::ZERO;
		}

		outcome
			.err()
			.map(|error| format!("{}: {error:#}", id.label()))
	}

	/// Move the playhead, which is what both gestures come down to: the strip works in
	/// fractions of a width, the two buttons above it work in seconds (PLAN §14b, §14c).
	///
	/// The position is set here rather than left to the tick, which runs only while something
	/// plays: a paused player would otherwise keep drawing its old playhead until it was
	/// started again, and clicking a strip that visibly does nothing is worse than not clicking.
	///
	/// A device that refuses leaves the playhead where it was — the model must not claim a move
	/// the audio did not make.
	pub fn seek(&mut self, id: DeckId, to: Duration, engine: &Option<Engine>) -> Option<String> {
		// Nothing to seek in, and nothing that would show the result. Unreachable from the two
		// gestures today — the strip needs a length to work a fraction out of and the jump
		// buttons are dead — but a playhead moving inside an empty player is the kind of thing
		// only a guard here can promise, since both those reasons live in other files.
		if !self.transport.has_track() {
			return None;
		}

		if let Some(engine) = engine
			&& let Err(error) = engine.seek(id, to)
		{
			// `ponytail:` a stream that cannot seek keeps its old position, the same failure
			// Stop already has. PLAN §7's fallback — re-open and re-append — fixes both at once.
			return Some(format!("{}: {error:#}", id.label()));
		}

		self.position = to;
		self.transport = transition(self.transport, Event::Seeked);
		None
	}

	/// The file ran out (PLAN §7): put it back in the player, and stop at the top of it.
	///
	/// `empty()` going true means rodio has *consumed* the source and dropped it, so there is
	/// nothing left in the player to start. The app is about to show that track stopped at 0:00
	/// with a live Play button, and that button would be a lie — `play()` on an empty player is
	/// silence.
	///
	/// `ponytail:` this re-opens the file even when a handover replaces it a moment later, which
	/// costs one header parse and one `clear()` per track. Cheap, and it is the one place where
	/// the model and rodio can disagree — worth splitting only if a handover ever feels like it
	/// hitches.
	pub fn ended(&mut self, id: DeckId, engine: &Option<Engine>) -> Option<String> {
		let reload = match (engine, &self.track) {
			(Some(engine), Some(track)) => engine
				.load(id, &track.path)
				.err()
				.map(|error| format!("{}: {error:#}", id.label())),
			_ => None,
		};

		// The reload's failure is reported over the move's, because the move cannot fail: what
		// went wrong is the file, and that is the more useful of the two things to say.
		reload.or(self.moved(id, Event::Ended, engine))
	}

	/// Take the track out of the player: back to `Empty`, as if nothing had ever been
	/// loaded — except the fader, which is a mixer setting rather than track state, and
	/// survives the way it survives a load.
	///
	/// `scanning` goes too, deliberately: the `Scanned` guard in `update` drops a result
	/// for a track that is no longer loaded, so a scan still running for the unloaded
	/// track would otherwise leave the strip animating forever.
	///
	/// No return, unlike `moved`: dropping a source is the one thing a device cannot
	/// refuse.
	pub fn unloaded(&mut self, id: DeckId, engine: &Option<Engine>) {
		if let Some(engine) = engine {
			engine.clear(id);
		}
		self.transport = Transport::Empty;
		self.track = None;
		self.position = Duration::ZERO;
		self.peaks = Vec::new();
		self.scanning = false;
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

	/// No audio device in the room, which is what the `Option` is for — and a real state the
	/// app runs in when an interface is unplugged (PLAN §11).
	const SILENT: Option<Engine> = None;

	#[test]
	fn unloading_empties_everything_but_the_fader() {
		// Arrange: a playing player mid-track, mid-scan, fader ridden down — and no device
		// (Q48), so what is checked is the model.
		let mut player = deck(Transport::Playing);
		player.position = Duration::from_secs(90);
		player.peaks = vec![0.5];
		player.scanning = true;
		player.fader = 0.3;

		// Act
		player.unloaded(DeckId::One, &SILENT);

		// Assert: the empty state, except the fader — a mixer setting, not track state.
		// `scanning` must go with the track: the `Scanned` guard drops a result for a track
		// no longer loaded, so nothing else would ever stop the animation.
		assert_eq!(player.transport, Transport::Empty);
		assert!(player.track.is_none());
		assert_eq!(player.position, Duration::ZERO);
		assert!(player.peaks.is_empty());
		assert!(!player.scanning);
		assert_eq!(
			player.fader, 0.3,
			"the fader is the mixer's, not the track's"
		);
	}

	#[test]
	fn a_seek_leaves_the_transport_alone_unless_it_was_at_the_top() {
		// Arrange / Act / Assert: Q14's rule — moving the playhead is not a transport gesture,
		// so a playing player carries on and a paused one stays paused.
		assert_eq!(
			transition(Transport::Playing, Event::Seeked),
			Transport::Playing
		);
		assert_eq!(
			transition(Transport::Paused, Event::Seeked),
			Transport::Paused
		);

		// Except from Stopped, which in this app means *at the top of the track*: a player
		// labelled "stopped" sitting at 1:30 is the label lying about where Play would start.
		assert_eq!(
			transition(Transport::Stopped, Event::Seeked),
			Transport::Paused
		);
	}

	#[test]
	fn moving_the_transport_moves_the_model_with_no_device_at_all() {
		// Arrange: a playing player and nothing plugged in (Q48).
		let mut player = deck(Transport::Playing);
		player.position = Duration::from_secs(90);

		// Act / Assert: Pause is the state and nothing else — the playhead stays where the
		// listener last heard it, which is what makes Play a resume.
		assert_eq!(player.moved(DeckId::One, Event::Pause, &SILENT), None);
		assert_eq!(player.transport, Transport::Paused);
		assert_eq!(player.position, Duration::from_secs(90), "paused in place");

		// Stop is a rewind, and it says so in the model rather than waiting for the tick —
		// which does not run while nothing is playing.
		assert_eq!(player.moved(DeckId::One, Event::Stop, &SILENT), None);
		assert_eq!(player.transport, Transport::Stopped);
		assert_eq!(player.position, Duration::ZERO);
	}

	#[test]
	fn both_ends_of_a_track_land_at_the_top_of_it() {
		// Arrange: a playing track about to run out.
		let mut player = deck(Transport::Playing);
		player.position = Duration::from_secs(215);

		// Act / Assert: the file running out and the Stop button agree about where the player
		// ends up. They were zeroing the playhead in three separate places before Q48, and one
		// of the three did not.
		assert_eq!(player.ended(DeckId::One, &SILENT), None);
		assert_eq!(player.transport, Transport::Stopped);
		assert_eq!(player.position, Duration::ZERO);
	}

	#[test]
	fn a_seek_on_a_stopped_player_moves_the_playhead_and_the_label_together() {
		// Arrange: a stopped player, which means one sitting at the top of its track.
		let mut player = deck(Transport::Stopped);

		// Act
		assert_eq!(
			player.seek(DeckId::One, Duration::from_secs(90), &SILENT),
			None
		);

		// Assert: both, or the readout and the label disagree about the same player — the
		// playhead is set here rather than left to the tick, which does not run for a player
		// that is not playing.
		assert_eq!(player.position, Duration::from_secs(90));
		assert_eq!(player.transport, Transport::Paused);
	}

	#[test]
	fn an_empty_player_cannot_be_moved_by_anything() {
		// Arrange: no track, which is every button drawn dead — but the model must not depend
		// on the view having remembered that.
		let mut player = deck(Transport::Empty);

		// Act / Assert
		for event in [Event::Play, Event::Pause, Event::Stop, Event::Ended] {
			assert_eq!(player.moved(DeckId::One, event, &SILENT), None);
			assert_eq!(player.transport, Transport::Empty, "{event:?}");
		}

		// A seek is the one that could have slipped through, because its own rule is about
		// `Stopped` and an empty player is not that — and it moves the playhead before it looks
		// at the transport at all, so an empty player would have shown 0:09 on its readout.
		assert_eq!(
			player.seek(DeckId::Two, Duration::from_secs(9), &SILENT),
			None
		);
		assert_eq!(player.transport, Transport::Empty);
		assert_eq!(player.position, Duration::ZERO, "nowhere to seek to");
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
		for event in [
			Event::Play,
			Event::Pause,
			Event::Stop,
			Event::Ended,
			Event::Seeked,
		] {
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
