//! The one file clecta writes: `clecta-data/settings.json` (PLAN §11).
//!
//! The rule that shapes this module is that **a settings file must never be able to stop
//! the app from starting**. Absent, empty, truncated, wrong types, hand-edited nonsense —
//! every one of them reads as defaults and a line on stderr. So neither `load` nor `save`
//! returns a `Result`: there is nothing the caller could usefully do with one.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mixer::Curve;
use crate::paths;

const FILE: &str = "settings.json";

/// A window smaller than this cannot show the two players and the mixer side by side, and
/// one larger than any real display is a lost window. Both are reachable by editing the
/// file, so both are clamped on the way in.
const MIN_WINDOW: f32 = 480.0;
const MAX_WINDOW: f32 = 16_000.0;

/// What survives a restart. Deliberately small: the mixer's settings, where the browser
/// was, and how big the window was.
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
}

impl Default for Settings {
	fn default() -> Self {
		Self {
			curve: Curve::default(),
			faders: [1.0, 1.0],
			crossfader: 0.5,
			folder: None,
			window: (1180.0, 760.0),
		}
	}
}

impl Settings {
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
		// A folder that has been deleted, renamed or unmounted since the last run: fall
		// back to the home folder rather than opening on an error message.
		if !self.folder.as_deref().is_some_and(Path::is_dir) {
			self.folder = None;
		}

		self
	}
}

#[cfg(test)]
mod tests {
	use super::*;

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
		}
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
	}

	#[test]
	fn out_of_range_values_fall_back_one_field_at_a_time() {
		// Arrange: a hand-edited file, with the crossfader still perfectly good.
		let text = r#"{"faders": [1.5, -0.2], "crossfader": 0.4, "window": [10.0, 1e9]}"#;

		// Act
		let settings = Settings::from_json(text);

		// Assert: the impossible values are replaced, the possible one survives — the
		// whole file is not thrown away over one bad number.
		let default = Settings::default();
		assert_eq!(settings.faders, default.faders, "both out of range");
		assert_eq!(settings.crossfader, 0.4, "in range, so kept");
		assert_eq!(settings.window, default.window, "too small and too large");
	}

	#[test]
	fn a_folder_that_no_longer_exists_is_forgotten() {
		// Arrange: the drive was unplugged, or the folder was renamed.
		let text = r#"{"folder": "/nowhere/at/all"}"#;

		// Act / Assert: `None` means "open on home", which always exists.
		assert_eq!(Settings::from_json(text).folder, None);
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
