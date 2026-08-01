//! The files pane's model (PLAN §9): one directory's worth of entries, what kind of file
//! each one is, and which of them are shown.
//!
//! Pure: no `std::fs` here, that is `fsio`'s job. What lives here is the part with rules
//! worth testing — the extension table, the sort, the hidden filter.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What a file is, as far as the browser is concerned.
///
/// Everything is listed and only media is loadable, because hiding the rest would hide
/// the `.cue` and the artwork that tell the user they are in the right folder (PLAN §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
	Audio,
	Video,
	Other,
}

impl Kind {
	/// The leading glyph in a row. A media browser is scanned by name, so the marker has
	/// to be readable at a glance and take almost no width.
	pub fn glyph(self) -> &'static str {
		match self {
			Kind::Audio => "♪",
			Kind::Video => "▶",
			Kind::Other => " ",
		}
	}

	/// Whether a player will accept this. Only a guess from the extension — the real
	/// answer is whatever the decoder says (PLAN §3).
	pub fn is_media(self) -> bool {
		self != Kind::Other
	}
}

/// Containers symphonia decodes out of the box, plus `mkv`/`webm` from the
/// `symphonia-mkv` feature (PLAN §3).
const AUDIO_EXTENSIONS: [&str; 5] = ["mp3", "flac", "wav", "ogg", "m4a"];

/// Video containers whose *audio track* v1 plays. The picture is deferred (PLAN §14).
///
/// `ponytail:` no `.mov`. It is ISO-BMFF-adjacent and might well work, but PLAN §3 says
/// do not promise it before it is verified, and offering a row that fails to load is a
/// worse answer than not offering it. One string here once someone tests it.
const VIDEO_EXTENSIONS: [&str; 4] = ["mp4", "m4v", "mkv", "webm"];

/// Classify a path by its extension, case-insensitively.
pub fn kind_of(path: &Path) -> Kind {
	let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
		return Kind::Other;
	};
	let extension = extension.to_ascii_lowercase();

	if AUDIO_EXTENSIONS.contains(&extension.as_str()) {
		Kind::Audio
	} else if VIDEO_EXTENSIONS.contains(&extension.as_str()) {
		Kind::Video
	} else {
		Kind::Other
	}
}

/// One row of the files pane.
#[derive(Debug, Clone)]
pub struct Entry {
	pub path: PathBuf,
	pub name: String,
	pub size: u64,
	/// `None` when the filesystem will not say — rare, but a network mount can refuse.
	pub modified: Option<SystemTime>,
	pub kind: Kind,
	/// A dotfile. Kept on the entry rather than recomputed, because the filter runs on
	/// every frame and the name is already here.
	pub hidden: bool,
}

impl Entry {
	/// Build an entry from the pieces `fsio` reads off the filesystem.
	pub fn new(path: PathBuf, size: u64, modified: Option<SystemTime>) -> Self {
		let name = path
			.file_name()
			.map(|name| name.to_string_lossy().into_owned())
			.unwrap_or_default();

		Self {
			hidden: name.starts_with('.'),
			kind: kind_of(&path),
			name,
			path,
			size,
			modified,
		}
	}
}

/// The files pane.
#[derive(Debug, Default)]
pub struct Browser {
	/// The folder being shown. `None` before the first listing lands.
	pub folder: Option<PathBuf>,
	/// Sorted and unfiltered — `visible` applies the hidden filter, so toggling it costs
	/// no filesystem work (PLAN §9).
	entries: Vec<Entry>,
	/// Selected by path, not index: a refresh can renumber the rows underneath it.
	pub selected: Option<PathBuf>,
	/// Off by default — a *local* media browser is not usually opened to find `.config`
	/// (PLAN §9).
	pub show_hidden: bool,
	pub error: Option<String>,
}

impl Browser {
	/// The rows to draw, in order.
	pub fn visible(&self) -> impl Iterator<Item = &Entry> {
		self.entries
			.iter()
			.filter(|entry| self.show_hidden || !entry.hidden)
	}

	/// Accept a fresh listing. Sorting happens here so the view never has to.
	pub fn show(&mut self, folder: PathBuf, mut entries: Vec<Entry>) {
		sort(&mut entries);

		// Drop a selection the new listing does not contain, so "load the selected file"
		// can never point at something that is no longer there.
		if let Some(selected) = &self.selected
			&& !entries.iter().any(|entry| &entry.path == selected)
		{
			self.selected = None;
		}

		self.folder = Some(folder);
		self.entries = entries;
		self.error = None;
	}

	/// Record a listing that failed. The previous contents stay on screen — cmote's
	/// "never flash empty" rule (PLAN §4).
	pub fn fail(&mut self, error: String) {
		self.error = Some(error);
	}

	/// The entry the user has selected, if it is currently visible.
	pub fn selection(&self) -> Option<&Entry> {
		let selected = self.selected.as_ref()?;
		self.visible().find(|entry| &entry.path == selected)
	}
}

/// Sort a listing: by name, case-insensitively, digits compared as numbers so `track2`
/// precedes `track10` (PLAN §9).
pub fn sort(entries: &mut [Entry]) {
	entries.sort_by(|a, b| natural_cmp(&a.name, &b.name));
}

/// Compare two names the way a person reads them: case-insensitively, and with runs of
/// digits compared by value rather than character by character.
///
/// `ponytail:` ASCII digits and ASCII case only. Enough for filenames; a locale-aware
/// collation would need a crate, and getting `ä` to sort next to `a` is not what this
/// project is for.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
	let mut left = a.chars().peekable();
	let mut right = b.chars().peekable();

	loop {
		match (left.peek().copied(), right.peek().copied()) {
			(None, None) => return Ordering::Equal,
			(None, Some(_)) => return Ordering::Less,
			(Some(_), None) => return Ordering::Greater,
			(Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
				// A digit run on both sides: compare the numbers, not the text, so the
				// shorter run does not automatically win.
				let x = take_number(&mut left);
				let y = take_number(&mut right);
				match x.cmp(&y) {
					Ordering::Equal => continue,
					other => return other,
				}
			}
			(Some(x), Some(y)) => {
				let (x, y) = (x.to_ascii_lowercase(), y.to_ascii_lowercase());
				match x.cmp(&y) {
					Ordering::Equal => {
						let _ = left.next();
						let _ = right.next();
					}
					other => return other,
				}
			}
		}
	}
}

/// Consume the leading run of digits and return its value.
///
/// Saturating, so a filename padded with sixty zeroes compares as "very large" instead of
/// wrapping. Nobody sorts by such a name deliberately; nobody should get a wrong answer
/// for it either.
fn take_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> u128 {
	let mut value: u128 = 0;
	while let Some(digit) = chars.peek().and_then(|c| c.to_digit(10)) {
		value = value.saturating_mul(10).saturating_add(u128::from(digit));
		let _ = chars.next();
	}
	value
}

#[cfg(test)]
mod tests {
	use super::*;

	fn entry(name: &str) -> Entry {
		Entry::new(PathBuf::from("/music").join(name), 0, None)
	}

	#[test]
	fn the_extension_decides_the_kind_whatever_its_case() {
		// Arrange / Act / Assert: one from each row of PLAN §3's format table.
		assert_eq!(kind_of(Path::new("/m/a.flac")), Kind::Audio);
		assert_eq!(kind_of(Path::new("/m/a.MP3")), Kind::Audio, "upper case");
		assert_eq!(
			kind_of(Path::new("/m/a.mkv")),
			Kind::Video,
			"needs the mkv feature"
		);
		assert_eq!(kind_of(Path::new("/m/a.mp4")), Kind::Video);
		assert_eq!(kind_of(Path::new("/m/notes.txt")), Kind::Other);
		assert_eq!(kind_of(Path::new("/m/README")), Kind::Other, "no extension");
		assert_eq!(kind_of(Path::new("/m/a.avi")), Kind::Other, "out of scope");
		assert_eq!(
			kind_of(Path::new("/m/a.mov")),
			Kind::Other,
			"unverified, so not offered"
		);
	}

	#[test]
	fn only_media_is_loadable() {
		// Arrange / Act / Assert
		assert!(Kind::Audio.is_media());
		assert!(Kind::Video.is_media());
		assert!(!Kind::Other.is_media());
	}

	#[test]
	fn numbers_in_names_sort_by_value_not_by_digit() {
		// Arrange: the case that makes a plain string sort look broken to a user.
		let mut entries = vec![
			entry("track10.mp3"),
			entry("track2.mp3"),
			entry("track1.mp3"),
		];

		// Act
		sort(&mut entries);

		// Assert
		let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
		assert_eq!(names, ["track1.mp3", "track2.mp3", "track10.mp3"]);
	}

	#[test]
	fn sorting_ignores_case_but_still_orders_everything() {
		// Arrange
		let mut entries = vec![entry("Beta.mp3"), entry("alpha.mp3"), entry("Gamma.mp3")];

		// Act
		sort(&mut entries);

		// Assert: alphabetical as read, not ASCII order — which would put every capital
		// first and scatter a folder of mixed-case names.
		let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
		assert_eq!(names, ["alpha.mp3", "Beta.mp3", "Gamma.mp3"]);
	}

	#[test]
	fn a_leading_zero_does_not_change_a_numbers_value() {
		// Arrange / Act / Assert: `07` and `7` are the same track number, so neither
		// ordering is wrong — but they must not be scattered apart by the padding.
		assert_eq!(natural_cmp("track07", "track7"), Ordering::Equal);
		assert_eq!(natural_cmp("track007", "track8"), Ordering::Less);
	}

	#[test]
	fn hidden_entries_are_filtered_not_dropped() {
		// Arrange: the distinction that lets the toggle work with no filesystem hit.
		let mut browser = Browser::default();
		browser.show(
			PathBuf::from("/music"),
			vec![entry("a.mp3"), entry(".hidden.mp3")],
		);

		// Act / Assert: off by default...
		assert_eq!(browser.visible().count(), 1, "hidden by default");

		// ...and the entry is still there to show when asked.
		browser.show_hidden = true;
		assert_eq!(browser.visible().count(), 2, "shown on request");
	}

	#[test]
	fn a_selection_the_new_listing_lost_is_forgotten() {
		// Arrange: the folder is refreshed and the selected file is gone from it.
		let mut browser = Browser::default();
		browser.show(
			PathBuf::from("/music"),
			vec![entry("a.mp3"), entry("b.mp3")],
		);
		browser.selected = Some(PathBuf::from("/music/b.mp3"));

		// Act
		browser.show(PathBuf::from("/music"), vec![entry("a.mp3")]);

		// Assert: otherwise "load the selection" points at a file that is not there.
		assert_eq!(browser.selected, None);
	}

	#[test]
	fn a_failed_listing_keeps_the_rows_that_are_already_shown() {
		// Arrange
		let mut browser = Browser::default();
		browser.show(PathBuf::from("/music"), vec![entry("a.mp3")]);

		// Act
		browser.fail("permission denied".to_string());

		// Assert: never flash empty (PLAN §4).
		assert_eq!(browser.visible().count(), 1);
		assert!(browser.error.is_some());
	}
}
