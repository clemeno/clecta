//! What clecta remembers about *itself*: `clecta-data/settings.json` (PLAN §11).
//!
//! One of the two files it writes, and the one that is the source of truth. The other is the
//! cache beside it (`cache.rs`, PLAN §11a), which holds what has been worked out about other
//! people's files and can be deleted at any time without losing anything but time.
//!
//! The rule that shapes this module is that **a settings file must never be able to stop
//! the app from starting**. Absent, empty, truncated, wrong types, hand-edited nonsense —
//! every one of them reads as defaults and a line on stderr. So neither `load` nor `save`
//! returns a `Result`: there is nothing the caller could usefully do with one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app::MIN_PANE;
use crate::deck::DeckId;
use crate::mixer::Curve;
use crate::paths;
use crate::queue::{Handover, Queue};
use crate::queues::{QueueId, Queues};

const FILE: &str = "settings.json";

/// A window smaller than this cannot show the two players and the mixer side by side, and
/// one larger than any real display is a lost window. Both are reachable by editing the
/// file, so both are clamped on the way in.
///
/// The ceiling is not cosmetic: it is a hard renderer limit, found by asking for a
/// 15000×15000 window and watching wgpu panic during `Surface::configure` — *before* the
/// first frame, so the app died at launch with a file it was supposed to survive. wgpu
/// guarantees a maximum texture dimension of only 8192, and a surface is measured in
/// **physical** pixels, so a 2× display doubles whatever is asked for here. 4096 leaves
/// the margin, and is already larger than any real display measured in logical points.
const MIN_WINDOW: f32 = 480.0;
const MAX_WINDOW: f32 = 4096.0;

/// What survives a restart. Deliberately small: the mixer's settings, where the browser
/// was, how big the window was, and how the window's height is shared out.
///
/// `#[serde(default)]` fills in anything a older or hand-edited file is missing, so
/// adding a field later cannot invalidate an existing file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
	pub curve: Curve,
	/// Both volume faders, in player order.
	pub faders: [f32; 2],
	pub crossfader: f32,
	/// The folder the files pane was showing. `None` on a first run.
	pub folder: Option<PathBuf>,
	/// Window size as `(width, height)`. Not the position: a window restored onto a
	/// monitor that is no longer attached is worse than a centred one.
	pub window: (f32, f32),
	/// How tall the players and the mixer are, in pixels. A height and not a fraction,
	/// because that panel's rows are fixed-size and a taller window should give the extra
	/// space to the browser (PLAN §6).
	pub decks_height: f32,
	/// The two per-player queues, in player order, and the shared one (PLAN §7a).
	///
	/// The only *unbounded* thing this file holds, which is the reason to say out loud that
	/// it is worth it: a cue built over an evening and lost to a quit is worse than no
	/// cue. Stored as plain paths — a queue is an ordered list of files and nothing
	/// else, so there is no state here that could go stale except the files themselves.
	pub cues: [Vec<PathBuf>; 2],
	/// The key stays `common` — it is older than the word. Renaming it would read every
	/// existing file's shared queue as absent, and `#[serde(default)]` would empty it.
	#[serde(rename = "common")]
	pub shared: Vec<PathBuf>,
	/// Whether each queue hands its top track to a player that has run out, and whether that
	/// track then starts by itself (PLAN §7a).
	///
	/// Indexed by `QueueId::index` — Cue 1, Next up, Cue 2 — which is deliberately **not** the
	/// order `cues` above is in: that pair is per player and has no slot for the shared queue.
	/// Three of each, in the order the queues are drawn, so a hand-edited file reads left to
	/// right.
	///
	/// Nothing sanitizes them, because a `bool` has no wrong value and serde has already
	/// rejected anything that is not one. A file predating them reads as "load, do not play",
	/// which is what the app did before the switches existed.
	pub auto_load: [bool; 3],
	pub auto_play: [bool; 3],
	/// When each queue's track takes over from the one playing (PLAN §7b). Same order and same
	/// reasoning as the two switches above, and nothing sanitizes it for the same reason: serde
	/// has already rejected anything that is not one of the two variants, and a file that
	/// predates the field reads as `Whole` — what the app did before there was a choice.
	/// The key stays `transition` for the same reason `shared` keeps `common`: the file is
	/// older than the word, and a renamed key would silently reset every queue to `Whole`.
	#[serde(rename = "transition")]
	pub handover: [Handover; 3],
	/// Tempos corrected by hand, by file (PLAN §14d).
	///
	/// **Here and not in the cache**, which is the whole decision: a detected tempo is worked out
	/// from the file and costs a decode to get back, so it belongs beside the waveforms where
	/// **Clear cache** can take it. A corrected one cannot be worked out from anything — it is a
	/// person's answer about a track — so losing it loses something, and the file that must never
	/// lose anything is this one. It is also what makes the two clearable apart: emptying this map
	/// puts every row back to what the detector said, and no waveform is touched.
	///
	/// A `BTreeMap` so the file reads in path order and a save does not shuffle the lines a hand
	/// editor is looking at.
	///
	/// `ponytail:` unbounded, like the queues above — one line per corrected track, in the file
	/// that is rewritten whole on a throttle. A library with thousands of corrections in it wants
	/// a file of its own; a set's worth does not.
	pub tempos: BTreeMap<PathBuf, f32>,
}

impl Default for Settings {
	fn default() -> Self {
		Self {
			curve: Curve::default(),
			faders: [1.0, 1.0],
			crossfader: 0.5,
			folder: None,
			window: (1180.0, 760.0),
			// Enough for the two panels' four rows, the mixer's controls, and a queue under
			// each of them deep enough to be worth looking at (PLAN §7a) — while leaving the
			// rest of a default window to the browser.
			decks_height: 480.0,
			cues: [Vec::new(), Vec::new()],
			shared: Vec::new(),
			auto_load: [true; 3],
			auto_play: [false; 3],
			handover: [Handover::Whole; 3],
			tempos: BTreeMap::new(),
		}
	}
}

impl Settings {
	/// The three queues this file restores, in draw order — the one place its two orderings
	/// meet. `cues` is per player and has no slot for the shared queue, while the three switch
	/// arrays are per drawn queue (PLAN §7a); zipping them used to be `boot`'s job, done index
	/// by index, and a swapped index there was silent — nothing checked that Cue 2's paths got
	/// Cue 2's switches.
	pub fn queues(&self) -> [Queue; 3] {
		let mut queues = [
			Queue::from_paths(self.cues[0].clone()),
			Queue::from_paths(self.shared.clone()),
			Queue::from_paths(self.cues[1].clone()),
		];
		for (index, queue) in queues.iter_mut().enumerate() {
			queue.auto_load = self.auto_load[index];
			queue.auto_play = self.auto_play[index];
			queue.handover = self.handover[index];
		}
		queues
	}

	/// The same three queues written back into the file's two orderings — the inverse of
	/// `queues`, kept beside it and beside the serde pins so the three cannot drift apart.
	pub fn record_queues(&mut self, queues: &Queues) {
		self.cues = [
			queues.get(QueueId::Cue(DeckId::One)).paths(),
			queues.get(QueueId::Cue(DeckId::Two)).paths(),
		];
		self.shared = queues.get(QueueId::Shared).paths();
		self.auto_load = QueueId::ALL.map(|id| queues.get(id).auto_load);
		self.auto_play = QueueId::ALL.map(|id| queues.get(id).auto_play);
		self.handover = QueueId::ALL.map(|id| queues.get(id).handover);
	}

	/// Read the settings file, or return defaults.
	pub fn load() -> Self {
		let path = paths::data_dir().join(FILE);

		match std::fs::read_to_string(&path) {
			Ok(text) => Self::from_json(&text),
			// A missing file is a first run, not a problem worth mentioning.
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
			Err(error) => {
				eprintln!("clecta: cannot read {}: {error}", path.display());
				Self::default()
			}
		}
	}

	/// Write the settings file, reporting a failure to stderr and carrying on.
	///
	/// `ponytail:` a plain write, not write-to-temp-then-rename. Losing the fader
	/// positions to a crash during the write is not worth an atomic-replace dance; the
	/// next run reads defaults, which is where the app started anyway.
	pub fn save(&self) {
		let dir = paths::data_dir();
		let path = dir.join(FILE);

		let text = match serde_json::to_string_pretty(self) {
			Ok(text) => text,
			Err(error) => {
				eprintln!("clecta: cannot encode settings: {error}");
				return;
			}
		};

		if let Err(error) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text))
		{
			eprintln!("clecta: cannot write {}: {error}", path.display());
		}
	}

	/// Parse, keeping the parts that make sense. Split out from `load` so the whole rule
	/// is testable without a filesystem.
	fn from_json(text: &str) -> Self {
		match serde_json::from_str::<Self>(text) {
			Ok(settings) => settings.sanitized(),
			Err(error) => {
				eprintln!("clecta: ignoring an unreadable settings file: {error}");
				Self::default()
			}
		}
	}

	/// Replace any value the UI could not have produced with its default.
	///
	/// The file is plain text a user can edit, so this is a trust boundary rather than
	/// mere deserialization. `NaN` fails every comparison below, which is the answer we
	/// want: it is replaced too.
	fn sanitized(mut self) -> Self {
		let default = Self::default();

		for (fader, fallback) in self.faders.iter_mut().zip(default.faders) {
			if !(0.0..=1.0).contains(fader) {
				*fader = fallback;
			}
		}
		if !(0.0..=1.0).contains(&self.crossfader) {
			self.crossfader = default.crossfader;
		}
		if !(MIN_WINDOW..=MAX_WINDOW).contains(&self.window.0) {
			self.window.0 = default.window.0;
		}
		if !(MIN_WINDOW..=MAX_WINDOW).contains(&self.window.1) {
			self.window.1 = default.window.1;
		}
		// Smaller than a pane is allowed to be, or taller than any window: the app compacts
		// the panel to whatever the window can spare anyway, so this only has to reject what
		// the splitter could never have produced.
		if !(MIN_PANE..=MAX_WINDOW).contains(&self.decks_height) {
			self.decks_height = default.decks_height;
		}
		// A folder that has been deleted, renamed or unmounted since the last run: fall
		// back to the home folder rather than opening on an error message.
		if !self.folder.as_deref().is_some_and(Path::is_dir) {
			self.folder = None;
		}

		// A queued track that has been deleted, renamed or unmounted since the last run.
		// Dropped rather than kept, because a queue is a promise about what plays next and a
		// row that cannot play is a promise the app would break at the worst moment — when a
		// track ends and the next one is due. The rest of the queue survives, which is the
		// same "one bad value does not take the good ones with it" rule as the faders above.
		for queue in self
			.cues
			.iter_mut()
			.chain(std::iter::once(&mut self.shared))
		{
			queue.retain(|path| path.is_file() && crate::browser::kind_of(path).is_media());
		}

		// A correction is only about a file that is still there, and it has to be a number the
		// column could print: this map is the one part of the file somebody might well edit by
		// hand, so a `0`, a negative or a `NaN` is dropped rather than drawn.
		self.tempos.retain(|path, tempo| {
			tempo.is_finite()
				&& *tempo > 0.0
				&& path.is_file()
				&& crate::browser::kind_of(path).is_media()
		});

		self
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A real file with a media extension. `sanitized` drops a queued path that is not one,
	/// so a fixture built from imaginary paths would fail its own round trip.
	fn queued(name: &str) -> PathBuf {
		let path = std::env::temp_dir().join(name);
		std::fs::write(&path, b"x").expect("the temp file for a queue fixture");
		path
	}

	/// Settings with nothing left at its default, so a round trip that drops a field is
	/// visible.
	fn edited() -> Settings {
		Settings {
			curve: Curve::Linear,
			faders: [0.25, 0.75],
			crossfader: 0.3,
			// An existing folder, because `sanitized` drops one that is not there.
			folder: Some(std::env::temp_dir()),
			window: (900.0, 600.0),
			decks_height: 260.0,
			cues: [
				vec![queued("clecta-cue-one.mp3")],
				vec![queued("clecta-cue-two.flac")],
			],
			shared: vec![queued("clecta-common.wav")],
			auto_load: [false, true, false],
			auto_play: [true, false, true],
			handover: [Handover::Trimmed, Handover::Whole, Handover::Trimmed],
			tempos: BTreeMap::from([(queued("clecta-corrected.mp3"), 128.5)]),
		}
	}

	#[test]
	fn the_files_two_orderings_zip_back_into_the_queues_they_came_from() {
		// Arrange: three queues that are all different — distinct paths and a distinct switch
		// pattern each, so a swapped index anywhere shows up as a mismatch.
		let mut one = Queue::from_paths(vec![PathBuf::from("/m/one.mp3")]);
		one.auto_load = false;
		one.handover = Handover::Trimmed;
		let mut shared = Queue::from_paths(vec![PathBuf::from("/m/shared.mp3")]);
		shared.auto_play = true;
		let two = Queue::from_paths(vec![PathBuf::from("/m/two.mp3")]);
		let queues = Queues::restored([one.clone(), shared.clone(), two.clone()]);

		// Act: into the file's two orderings and back out.
		let mut settings = Settings::default();
		settings.record_queues(&queues);
		let restored = settings.queues();

		// Assert: each queue keeps its own paths *and* its own switches. The paths are stored
		// per player and the switches per drawn queue, and a swapped index between those two
		// orderings was silent until this test.
		assert_eq!(restored, [one, shared, two]);
	}

	#[test]
	fn a_round_trip_changes_nothing() {
		// Arrange
		let settings = edited();

		// Act
		let text = serde_json::to_string(&settings).expect("settings encode");
		let restored = Settings::from_json(&text);

		// Assert
		assert_eq!(restored, settings);
		// The disk promise behind the `shared` field's serde rename: the file keeps the key
		// it has always had, so an existing shared queue survives the word changing.
		assert!(
			text.contains("\"common\""),
			"the on-disk key is still `common`"
		);
		assert!(
			!text.contains("\"shared\""),
			"the new word never reaches the file"
		);
		assert!(
			text.contains("\"transition\"") && !text.contains("\"handover\""),
			"the handover setting keeps its old key too"
		);
	}

	#[test]
	fn every_kind_of_broken_file_reads_as_defaults() {
		// Arrange: the four ways a settings file goes wrong in practice.
		let broken = [
			("", "empty"),
			("{\"curve\": \"Linear\"", "truncated"),
			("{\"faders\": \"loud\"}", "wrong type"),
			("[1, 2, 3]", "not an object"),
		];

		// Act / Assert
		for (text, what) in broken {
			assert_eq!(Settings::from_json(text), Settings::default(), "{what}");
		}
	}

	#[test]
	fn a_missing_field_keeps_its_default_instead_of_failing() {
		// Arrange: what an older file looks like once a field is added.
		let text = "{\"crossfader\": 0.25}";

		// Act
		let settings = Settings::from_json(text);

		// Assert: the one value present is kept, the rest default — adding a field must
		// not invalidate a file someone already has.
		assert_eq!(settings.crossfader, 0.25);
		assert_eq!(settings.faders, Settings::default().faders);

		// The queue switches are the newest fields, and this is the case that matters for
		// them: every file written before they existed is missing them, and must go on
		// behaving exactly as it did — handing tracks over, never starting them (PLAN §7a).
		assert_eq!(settings.auto_load, [true; 3], "a file that predates them");
		assert_eq!(settings.auto_play, [false; 3]);
		assert_eq!(
			settings.handover,
			[Handover::Whole; 3],
			"and files play whole until someone says otherwise"
		);

		// And the newest field of all: a file written before anyone could correct a tempo has
		// corrected none, which is what every row showing the detector's answer means (PLAN §14d).
		assert!(settings.tempos.is_empty(), "nothing corrected yet");
	}

	#[test]
	fn a_corrected_tempo_survives_only_if_it_could_be_drawn() {
		// Arrange: the one part of this file somebody really might edit by hand, with each of
		// the ways a number in it can be useless — and a real correction beside them (PLAN §14d).
		let real = queued("clecta-tempo-kept.mp3");
		let sleeve = queued("clecta-tempo-sleeve.jpg");
		let settings = Settings {
			tempos: BTreeMap::from([
				(real.clone(), 128.5),
				(PathBuf::from("/nowhere/gone.mp3"), 120.0),
				(sleeve.clone(), 90.0),
				(queued("clecta-tempo-zero.mp3"), 0.0),
				(queued("clecta-tempo-negative.mp3"), -128.0),
				(queued("clecta-tempo-nan.mp3"), f32::NAN),
			]),
			..Settings::default()
		};

		// Act
		let tempos = settings.sanitized().tempos;

		// Assert: one bad entry does not take the good one with it, which is the same rule the
		// faders and the queues follow.
		assert_eq!(tempos, BTreeMap::from([(real, 128.5)]));
		assert!(!tempos.contains_key(&sleeve), "not a media file");
	}

	#[test]
	fn out_of_range_values_fall_back_one_field_at_a_time() {
		// Arrange: a hand-edited file, with the crossfader still perfectly good. The window
		// height is the value that really crashed wgpu at launch, not a made-up huge
		// number — this is the regression, pinned.
		let text = r#"{"faders": [1.5, -0.2], "crossfader": 0.4, "window": [10.0, 15000.0],
			"decks_height": 12.0}"#;

		// Act
		let settings = Settings::from_json(text);

		// Assert: the impossible values are replaced, the possible one survives — the
		// whole file is not thrown away over one bad number.
		let default = Settings::default();
		assert_eq!(settings.faders, default.faders, "both out of range");
		assert_eq!(settings.crossfader, 0.4, "in range, so kept");
		assert_eq!(settings.window, default.window, "too small and too large");
		assert_eq!(
			settings.decks_height, default.decks_height,
			"smaller than a pane may be"
		);
	}

	#[test]
	fn a_folder_that_no_longer_exists_is_forgotten() {
		// Arrange: the drive was unplugged, or the folder was renamed.
		let text = r#"{"folder": "/nowhere/at/all"}"#;

		// Act / Assert: `None` means "open on home", which always exists.
		assert_eq!(Settings::from_json(text).folder, None);
	}

	#[test]
	fn a_queued_track_that_no_longer_exists_is_dropped_without_the_rest() {
		// Arrange: a queue holding one real media file, one that was deleted since the last
		// run, and one that is not media at all — a settings file someone hand-edited.
		let real = queued("clecta-queue-survivor.mp3");
		let sleeve = queued("clecta-queue-sleeve.jpg");
		let settings = Settings {
			shared: vec![
				PathBuf::from("/nowhere/gone.mp3"),
				real.clone(),
				sleeve.clone(),
			],
			..Settings::default()
		};

		// Act
		let shared = settings.sanitized().shared;

		// Assert: a queue is a promise about what plays next, and the worst moment to
		// discover a broken one is when a track ends and the next is due. The good row
		// survives — one bad path does not empty the queue.
		assert_eq!(shared, vec![real], "kept the one that can still play");
		assert!(!shared.contains(&sleeve), "a .jpg is not a queueable track");
	}

	#[test]
	fn a_not_a_number_fader_is_replaced() {
		// Arrange: JSON has no NaN literal, so it can only arrive by arithmetic — but a
		// comparison against NaN is false, and this is the one place that matters.
		let settings = Settings {
			faders: [f32::NAN, 0.5],
			..Settings::default()
		};

		// Act / Assert
		assert_eq!(settings.sanitized().faders, [1.0, 0.5]);
	}
}
