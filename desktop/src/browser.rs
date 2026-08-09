//! The files pane's model (PLAN §9): one directory's worth of entries, what kind of file
//! each one is, and which of them are shown.
//!
//! Pure: no `std::fs` here, that is `fsio`'s job. What lives here is the part with rules
//! worth testing — the extension table, the sort, the hidden filter.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::select::{self, Click};

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
	///
	/// A set rather than one path (PLAN §9a). Unordered on purpose — the *order* of a
	/// selection is the order of the rows, which `selection` reads back off the listing, so
	/// this cannot disagree with what is on screen after a re-sort.
	selected: HashSet<PathBuf>,
	/// Where a Shift-click measures from: the row the last plain or command click landed on.
	/// `None` when nothing has been clicked yet, which makes a Shift-click a plain one.
	anchor: Option<PathBuf>,
	/// Which rows the cache already answers for in full, and how long the music in each runs
	/// (PLAN §11c, §14c).
	///
	/// A *copy* of what the store said, not the store itself: `view` runs every frame and the
	/// cache is a file on disk. It is replaced wholesale each time a listing is asked about, and
	/// added to one path at a time as scans land — which is why it can only ever be optimistic
	/// between two listings, never stale in the other direction.
	///
	/// One map rather than a set and a map beside it: the mark and the playing time are the same
	/// fact read from the same two tables, and two containers would be two chances to hold a
	/// tick for a row with no time or the other way about.
	prepared: HashMap<PathBuf, Option<Duration>>,
	/// Off by default — a *local* media browser is not usually opened to find `.config`
	/// (PLAN §9).
	pub show_hidden: bool,
	pub error: Option<String>,
	/// How far the pane is scrolled, in pixels, as the `scrollable` last reported it.
	///
	/// Not a preference and not persisted: it is here because the *view* needs it to decide
	/// which rows are worth building at all (PLAN §9). Reset when a new folder is chosen,
	/// and deliberately **not** reset by a refresh — re-reading a folder should leave you
	/// where you were reading.
	pub scroll: f32,
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

		// Drop whatever the new listing does not contain, so "load the selection" can never
		// point at something that is no longer there. The rows that *are* still there keep
		// their highlight, which is what makes a refresh mid-selection survivable.
		self.selected
			.retain(|path| entries.iter().any(|entry| &entry.path == path));
		if self
			.anchor
			.as_ref()
			.is_some_and(|path| !self.selected.contains(path))
		{
			self.anchor = None;
		}

		// Same rule as the selection, for the same reason: a mark that outlived its file would
		// be an answer about a path the pane is no longer showing. The rows that survive keep
		// their mark, so a refresh does not blink the whole column off while the store is
		// asked again.
		self.prepared.retain(|path, _| {
			entries
				.iter()
				.any(|entry| &entry.path == path && entry.kind.is_media())
		});

		self.folder = Some(folder);
		self.entries = entries;
		self.error = None;
	}

	/// The whole listing, filter or no filter — what the cache is asked about.
	///
	/// `visible` would have been wrong here: the hidden toggle costs no filesystem work by
	/// design (PLAN §9), so a listing has to be asked about in full or revealing a dotfile
	/// would show an unmarked row that is in fact prepared.
	pub fn entries(&self) -> &[Entry] {
		&self.entries
	}

	/// Whether the store already holds everything a load of this file would need (PLAN §11c).
	pub fn is_prepared(&self, path: &Path) -> bool {
		self.prepared.contains_key(path)
	}

	/// How long this file's music runs, if anything has worked it out (PLAN §14c).
	///
	/// Two layers, and both are worth asking: the outer `None` is *nobody has scanned this*, and
	/// `Some(None)` is *scanned, and there is no music in it* — a file of silence, which is an
	/// answer rather than a gap. The column says the two differently for the same reason the
	/// queues do.
	pub fn music(&self, path: &Path) -> Option<Option<Duration>> {
		self.prepared.get(path).copied()
	}

	/// What the store said about this listing, replacing whatever was believed before.
	pub fn marked_prepared(&mut self, prepared: HashMap<PathBuf, Option<Duration>>) {
		self.prepared = prepared;
	}

	/// One more file worked out, while the pane is showing it.
	pub fn mark_prepared(&mut self, path: &Path, music: Option<Duration>) {
		if self.entries.iter().any(|entry| entry.path == path) {
			self.prepared.insert(path.to_path_buf(), music);
		}
	}

	/// Nothing is prepared any more — what emptying the store means on screen.
	pub fn forget_prepared(&mut self) {
		self.prepared.clear();
	}

	/// Record a listing that failed. The previous contents stay on screen — cmote's
	/// "never flash empty" rule (PLAN §4).
	pub fn fail(&mut self, error: String) {
		self.error = Some(error);
	}

	/// Everything selected, **top to bottom** (PLAN §9a).
	///
	/// Read off the listing rather than out of the set, which is what makes the order the
	/// order on screen: a natural-numeric sort, and whatever the hidden filter is showing. A
	/// selected row that has been hidden since is therefore not in it — every action works on
	/// what the user can see.
	pub fn selection(&self) -> impl Iterator<Item = &Entry> {
		self.visible()
			.filter(|entry| self.selected.contains(&entry.path))
	}

	/// The selected media files, in the same order — what every action on the pane operates
	/// on. Non-media rows are listed so the user can see the folder is the right one
	/// (PLAN §9) and are simply not part of any of them.
	pub fn selected_media(&self) -> Vec<PathBuf> {
		self.selection()
			.filter(|entry| entry.kind.is_media())
			.map(|entry| entry.path.clone())
			.collect()
	}

	/// Whether anything a player or a queue could take is selected — what every button on the
	/// pane is enabled by.
	pub fn has_media_selection(&self) -> bool {
		self.selection().any(|entry| entry.kind.is_media())
	}

	pub fn is_selected(&self, path: &Path) -> bool {
		self.selected.contains(path)
	}

	/// Apply a press on a row (PLAN §9a).
	///
	/// The anchor moves for a plain or command click and **stays put for a range**, which is
	/// what lets a Shift-click be adjusted: clicking further down again re-measures from the
	/// same start rather than from wherever the last one ended.
	///
	/// A range replaces the selection rather than adding to it. That is the behaviour of the
	/// pane this one is modelled on, and it is the one that can be corrected by a second
	/// Shift-click; a range that accumulated would need a plain click to undo, which is the
	/// gesture people use to *stop* selecting.
	pub fn click(&mut self, path: &Path, kind: Click) {
		match kind {
			Click::Replace => {
				self.selected.clear();
				self.selected.insert(path.to_path_buf());
				self.anchor = Some(path.to_path_buf());
			}
			Click::Toggle => {
				if !self.selected.remove(path) {
					self.selected.insert(path.to_path_buf());
				}
				self.anchor = Some(path.to_path_buf());
			}
			// A range needs two rows and a *position* for each, so it is the one case that has
			// to walk the listing. Without an anchor there is nothing to measure from, and the
			// press falls back to being a plain one.
			Click::Range => {
				let rows: Vec<&Entry> = self.visible().collect();
				let position = |wanted: &Path| rows.iter().position(|entry| entry.path == wanted);

				let Some((anchor, clicked)) = self
					.anchor
					.as_deref()
					.and_then(position)
					.zip(position(path))
				else {
					return self.click(path, Click::Replace);
				};

				self.selected = select::between(anchor, clicked)
					.filter_map(|index| rows.get(index).map(|entry| entry.path.clone()))
					.collect();
			}
		}
	}

	/// Select every row the pane is showing — so the hidden filter decides what "all" means,
	/// which is the same rule `selection` follows.
	pub fn select_all(&mut self) {
		let shown: Vec<PathBuf> = self.visible().map(|entry| entry.path.clone()).collect();
		self.anchor = shown.first().cloned();
		self.selected = shown.into_iter().collect();
	}

	pub fn clear_selection(&mut self) {
		self.selected.clear();
		self.anchor = None;
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

	/// A pane showing five tracks and a text file, which is the fixture every selection test
	/// below works on.
	fn pane() -> Browser {
		let mut browser = Browser::default();
		browser.show(
			PathBuf::from("/music"),
			vec![
				entry("1.mp3"),
				entry("2.mp3"),
				entry("3.mp3"),
				entry("4.mp3"),
				entry("notes.txt"),
			],
		);
		browser
	}

	/// What is selected, by name and in the order the pane hands them out.
	fn names(browser: &Browser) -> Vec<String> {
		browser
			.selection()
			.map(|entry| entry.name.clone())
			.collect()
	}

	fn row(name: &str) -> PathBuf {
		PathBuf::from("/music").join(name)
	}

	#[test]
	fn a_selection_the_new_listing_lost_is_forgotten() {
		// Arrange: two rows selected, and the folder is refreshed with one of them gone.
		let mut browser = pane();
		browser.click(&row("1.mp3"), Click::Replace);
		browser.click(&row("2.mp3"), Click::Toggle);

		// Act
		browser.show(PathBuf::from("/music"), vec![entry("1.mp3")]);

		// Assert: the row that survived keeps its highlight and the one that did not is
		// forgotten — otherwise "load the selection" points at a file that is not there.
		assert_eq!(names(&browser), ["1.mp3"]);
	}

	#[test]
	fn a_plain_click_replaces_and_a_command_click_adds_or_takes_away() {
		// Arrange
		let mut browser = pane();

		// Act / Assert: the plain press is the whole of the old behaviour, kept.
		browser.click(&row("2.mp3"), Click::Replace);
		assert_eq!(names(&browser), ["2.mp3"]);

		browser.click(&row("4.mp3"), Click::Replace);
		assert_eq!(names(&browser), ["4.mp3"], "the other one goes");

		// Command-clicking adds, and adds *in row order* however it was clicked — the order
		// is the pane's, not the order the rows were picked, because that is the order the
		// actions run in.
		browser.click(&row("1.mp3"), Click::Toggle);
		assert_eq!(names(&browser), ["1.mp3", "4.mp3"], "top to bottom");

		// And clicking a selected row again takes it out rather than leaving it stuck.
		browser.click(&row("4.mp3"), Click::Toggle);
		assert_eq!(names(&browser), ["1.mp3"]);
	}

	#[test]
	fn a_shift_click_takes_everything_between_and_can_be_adjusted() {
		// Arrange: an anchor in the middle.
		let mut browser = pane();
		browser.click(&row("2.mp3"), Click::Replace);

		// Act / Assert: downwards, then upwards, then further — each measured from the same
		// anchor, which is what lets a range be corrected without starting again.
		browser.click(&row("4.mp3"), Click::Range);
		assert_eq!(names(&browser), ["2.mp3", "3.mp3", "4.mp3"]);

		browser.click(&row("3.mp3"), Click::Range);
		assert_eq!(names(&browser), ["2.mp3", "3.mp3"], "pulled back in");

		browser.click(&row("1.mp3"), Click::Range);
		assert_eq!(names(&browser), ["1.mp3", "2.mp3"], "the other way");

		// A range with nothing to measure from is a plain click rather than nothing at all.
		let mut fresh = pane();
		fresh.click(&row("3.mp3"), Click::Range);
		assert_eq!(names(&fresh), ["3.mp3"], "no anchor yet");
	}

	#[test]
	fn selecting_everything_means_everything_on_screen() {
		// Arrange: a hidden file, which is not on screen.
		let mut browser = Browser::default();
		browser.show(
			PathBuf::from("/music"),
			vec![entry("a.mp3"), entry(".secret.mp3")],
		);

		// Act / Assert: what is shown, which is what the filter says — an action reaching a
		// row nobody can see is the thing this rule exists to prevent.
		browser.select_all();
		assert_eq!(names(&browser), ["a.mp3"]);

		browser.show_hidden = true;
		browser.select_all();
		assert_eq!(names(&browser), [".secret.mp3", "a.mp3"]);

		browser.clear_selection();
		assert!(names(&browser).is_empty());
	}

	#[test]
	fn only_media_is_ever_handed_to_a_player() {
		// Arrange: everything selected, text file included.
		let mut browser = pane();
		browser.select_all();

		// Act / Assert: the `.txt` is listed and selectable — it is how you tell you are in
		// the right folder (PLAN §9) — and it is not part of any action.
		assert_eq!(names(&browser).len(), 5, "all five rows");
		let media: Vec<PathBuf> = browser.selected_media();
		assert_eq!(media.len(), 4, "four tracks");
		assert!(!media.contains(&row("notes.txt")));
		assert!(browser.has_media_selection());

		// A selection of nothing but a text file enables nothing.
		browser.click(&row("notes.txt"), Click::Replace);
		assert!(!browser.has_media_selection());
		assert!(browser.selected_media().is_empty());
	}

	#[test]
	fn a_mark_survives_a_refresh_but_not_the_loss_of_its_file() {
		// Arrange: the store's answer about the listing on screen. One of them was scanned and
		// found to hold no music at all, which is an answer and not a gap.
		let mut browser = pane();
		browser.marked_prepared(HashMap::from([
			(row("1.mp3"), Some(Duration::from_secs(215))),
			(row("2.mp3"), None),
		]));
		assert!(browser.is_prepared(&row("1.mp3")));
		assert_eq!(
			browser.music(&row("1.mp3")),
			Some(Some(Duration::from_secs(215)))
		);
		assert_eq!(browser.music(&row("2.mp3")), Some(None), "scanned, silent");
		assert_eq!(browser.music(&row("3.mp3")), None, "never scanned");

		// Act: the folder is read again with one of them gone.
		browser.show(PathBuf::from("/music"), vec![entry("1.mp3")]);

		// Assert: the row that is still there keeps its mark, so a refresh does not blink the
		// whole column off while the store is asked again — and the row that went takes its
		// mark with it, or a folder of new files would inherit the last one's answers.
		assert!(browser.is_prepared(&row("1.mp3")), "still listed");
		assert!(!browser.is_prepared(&row("2.mp3")), "gone with its file");

		// Act / Assert: a file worked out while the pane is showing it gets marked, and one the
		// pane has never heard of is not — a scan of a folder the user has navigated away from
		// must not mark rows by name in whatever folder they are looking at now.
		browser.mark_prepared(&row("1.mp3"), Some(Duration::from_secs(180)));
		browser.mark_prepared(&PathBuf::from("/elsewhere/1.mp3"), None);
		assert!(browser.is_prepared(&row("1.mp3")));
		assert_eq!(
			browser.music(&row("1.mp3")),
			Some(Some(Duration::from_secs(180))),
			"the fresh scan's answer, not the old one"
		);
		assert!(!browser.is_prepared(Path::new("/elsewhere/1.mp3")));

		// And emptying the store empties the column.
		browser.forget_prepared();
		assert!(!browser.is_prepared(&row("1.mp3")));
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
