//! The three queues (PLAN §7a): one in front of each player, and one shared between them.
//!
//! Pure, like `deck.rs` and for the same reason — every rule here is an edit to a list, and
//! an edit to a list is exactly the kind of thing that is wrong by one and looks right. So
//! the whole module is `Vec` arithmetic with no iced and no filesystem, and the interesting
//! part is not the moving but **what happens to the selection when the list moves under
//! it**: a row that stays highlighted while a different track slides beneath it is worse
//! than no highlight at all.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::deck::DeckId;
use crate::select::{self, Click};
use crate::waveform::Trim;

/// Which of the three lists. The player-owned ones are `Cue`, the shared one is `Common`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListId {
	/// What one player plays next, and no other player's business.
	Cue(DeckId),
	/// The shared pool: whichever player finishes first takes from it (PLAN §7a).
	Common,
}

impl ListId {
	/// All three, left to right as they are drawn — which is also the order the `←` and `→`
	/// buttons step through, so `neighbour` and the layout cannot disagree.
	pub const ALL: [ListId; 3] = [
		ListId::Cue(DeckId::One),
		ListId::Common,
		ListId::Cue(DeckId::Two),
	];

	/// Index into a three-element array of anything per-list.
	pub fn index(self) -> usize {
		match self {
			ListId::Cue(DeckId::One) => 0,
			ListId::Common => 1,
			ListId::Cue(DeckId::Two) => 2,
		}
	}

	/// What the user calls this list.
	pub fn label(self) -> &'static str {
		match self {
			ListId::Cue(DeckId::One) => "Cue 1",
			ListId::Common => "Next up",
			ListId::Cue(DeckId::Two) => "Cue 2",
		}
	}

	/// The list one step in this direction, or `None` at either end.
	///
	/// Neighbours only: `Cue 1` and `Cue 2` are not adjacent, so the arrows cannot throw a
	/// track across the shared list without it stopping there. That is the point of the
	/// middle list being in the middle.
	pub fn neighbour(self, right: bool) -> Option<ListId> {
		let step = if right { 1 } else { -1 };
		let index = self.index() as isize + step;

		ListId::ALL.get(usize::try_from(index).ok()?).copied()
	}
}

/// When one track gives way to the next (PLAN §7b).
///
/// Two positions, and the difference between them is entirely about the silence a file
/// carries at either end: the encoder's padding, the engineer's run-out, the two seconds of
/// room tone somebody left on the master. `Whole` waits for the file; `Trimmed` waits for the
/// *music*, and starts the next one where its own music starts.
///
/// `Serialize`/`Deserialize` because it is persisted per list (PLAN §11), and `Display`
/// because it is drawn in a `pick_list` — the same pair `mixer::Curve` needs for the same
/// reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Transition {
	/// Hand over when the file runs out, wherever the music stopped. The app's behaviour
	/// before this setting existed, and the default for the same reason every load lands on
	/// `Stopped`: cutting a track short is not something to start doing unasked.
	#[default]
	Whole,
	/// Hand over when the *music* stops, and start the next track where its music starts.
	/// Silent about a track whose edges have never been scanned, which simply plays whole
	/// (PLAN §14c).
	Trimmed,
}

impl Transition {
	pub const ALL: [Transition; 2] = [Transition::Whole, Transition::Trimmed];
}

impl std::fmt::Display for Transition {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		formatter.write_str(match self {
			Transition::Whole => "Whole track",
			Transition::Trimmed => "Skip blanks",
		})
	}
}

/// One queued track: a path, the name the view draws every frame, and how long it is.
///
/// The name is cached for the same reason `deck::Track` caches it — `Path::file_name`
/// returns an `OsStr` that would be re-converted on every row of every frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
	pub path: PathBuf,
	pub name: String,
	/// Two questions in one field, and both have to be asked: `None` means *not measured
	/// yet*, and `Some(None)` means measured and the decoder could give no length — a
	/// stream, or a file that no longer opens (PLAN §7a). Collapsing them into one `None`
	/// would make the app re-open an unreadable file for ever.
	pub duration: Option<Option<Duration>>,
	/// How long the music runs, with the leader and the run-out taken off (PLAN §14c) — the
	/// number a set is actually built from, when anything has worked it out.
	///
	/// One layer where `duration` has two, and deliberately: a length is a header parse that
	/// every queue edit pays for, so *not measured* and *no length* are worth telling apart. The
	/// music's edges cost a full decode and are only ever **read** here (`cached_facts`), so
	/// `None` means nothing more than nobody has scanned this file yet — the same rule
	/// `App::remember` follows, and for the same reason.
	pub music: Option<Duration>,
}

impl Item {
	pub fn new(path: PathBuf) -> Self {
		let name = crate::fsio::name_of(&path);
		Self {
			path,
			name,
			duration: None,
			music: None,
		}
	}
}

/// One list, and which of its rows is selected.
///
/// Selected by **index**, not by path, which is the opposite of the files pane (`browser.rs`)
/// and deliberately so: a queue may hold the same track twice — playing something twice in a
/// set is a thing people do — so a path does not name a row. The price is that every edit has
/// to carry the selection with it, which is what most of this module is.
#[derive(Debug, Clone, PartialEq)]
pub struct Playlist {
	items: Vec<Item>,
	/// Which rows are selected (PLAN §9a). A `BTreeSet` because every action on them runs
	/// **top to bottom**, and a sorted set is that order without anything having to remember
	/// it — which matters most in the two places the order is load-bearing: the tracks handed
	/// to a player, and a block of rows moved by a drag.
	selected: BTreeSet<usize>,
	/// Where a Shift-click measures from. Kept as an index like the selection, and dropped
	/// whenever the row it named stops being selected.
	anchor: Option<usize>,
	/// Whether this list hands its top track to a player that has just run out (PLAN §7a),
	/// and whether that track then starts by itself.
	///
	/// Two switches rather than one three-way setting, because they answer two questions and
	/// the middle position is the useful one: a list that loads without playing is the app's
	/// own default, and it is neither of the extremes a single toggle would offer.
	///
	/// Per *list* rather than per player, which is what makes them worth having at all — Cue 1
	/// can run the evening by itself while the shared pool sits there as a manual shelf, and
	/// that is one setting each rather than a mode the whole app is in.
	pub auto_load: bool,
	pub auto_play: bool,
	/// When the handover happens, and where the track it hands over starts (PLAN §7b).
	///
	/// The third setting on the same list and read at the same moment as the other two, which
	/// is what keeps it one question rather than two: *this* list decides when its own track
	/// takes over, so Cue 1 can run an evening back to back while **Next up** stays a shelf
	/// that plays whatever it is handed, whole.
	pub transition: Transition,
}

impl Default for Playlist {
	fn default() -> Self {
		Self {
			items: Vec::new(),
			selected: BTreeSet::new(),
			anchor: None,
			// On: a queue is a list of what plays next, and one that had to be switched on
			// before it did anything would be a list that quietly did nothing.
			auto_load: true,
			// Off, for the same reason every load lands on `Stopped` (PLAN §7): on a mixer,
			// audio nobody asked for is a mistake that cannot be taken back. Someone who wants
			// the evening to run itself says so once, per list.
			auto_play: false,
			transition: Transition::default(),
		}
	}
}

impl Playlist {
	/// Build from stored paths (PLAN §11). Nothing is selected on a fresh start, and the two
	/// switches are the caller's business — `app` sets them from the settings file.
	pub fn from_paths(paths: Vec<PathBuf>) -> Self {
		Self {
			items: paths.into_iter().map(Item::new).collect(),
			..Self::default()
		}
	}

	/// The paths, for the settings file.
	pub fn paths(&self) -> Vec<PathBuf> {
		self.items.iter().map(|item| item.path.clone()).collect()
	}

	pub fn items(&self) -> &[Item] {
		&self.items
	}

	pub fn is_empty(&self) -> bool {
		self.items.is_empty()
	}

	/// The selected rows, top to bottom (PLAN §9a).
	///
	/// Double-ended because two callers need it backwards: taking rows out and swapping them
	/// downwards both have to start at the end, or an index is stale by the time its turn
	/// comes.
	pub fn selection(&self) -> impl DoubleEndedIterator<Item = usize> + '_ {
		self.selected.iter().copied()
	}

	pub fn is_selected(&self, index: usize) -> bool {
		self.selected.contains(&index)
	}

	pub fn has_selection(&self) -> bool {
		!self.selected.is_empty()
	}

	/// The topmost and bottommost selected rows, which is what the `▲` and `▼` buttons are
	/// enabled by: a block already touching the top of the list cannot go up.
	pub fn first_selected(&self) -> Option<usize> {
		self.selected.first().copied()
	}

	pub fn last_selected(&self) -> Option<usize> {
		self.selected.last().copied()
	}

	/// The selected rows themselves, in order, for a caller that has to look at the tracks
	/// before deciding whether to move them.
	pub fn selected_items(&self) -> Vec<Item> {
		self.selection()
			.filter_map(|index| self.items.get(index).cloned())
			.collect()
	}

	/// How long everything in the list is, and whether that is the whole truth.
	///
	/// `false` means at least one row has no length — still being measured, or a file the
	/// decoder could not answer for. The footer says so with a `+` rather than rounding the
	/// missing rows to zero: a running time that silently leaves tracks out is worse than one
	/// that admits it is still counting, because the number exists to be planned against.
	pub fn total(&self) -> (Duration, bool) {
		self.items
			.iter()
			.fold((Duration::ZERO, true), |(sum, whole), item| {
				match item.duration {
					Some(Some(length)) => (sum + length, whole),
					_ => (sum, false),
				}
			})
	}

	/// The tracks whose length nobody has looked up yet.
	pub fn unmeasured(&self) -> impl Iterator<Item = &Path> {
		self.items
			.iter()
			.filter(|item| item.duration.is_none())
			.map(|item| item.path.as_path())
	}

	/// Record what a job worked out against every row holding this path.
	///
	/// By path rather than by index, because the lists can be edited while the measuring runs
	/// and an index would name a different track by the time the answer came back. A queue may
	/// hold the same track twice, so one answer settles both rows.
	///
	/// The two halves are applied under **different rules**, which is why they are not one `if`.
	/// A length is settled once and kept, `None` included, or an unreadable file would be
	/// re-opened on every edit for the rest of the run. The music's edges are only ever read
	/// from the store, so a job that has none is a job that did not look deep enough — and a row
	/// measured long ago still has to learn its playing time the moment a folder scan works it
	/// out. Filtering both on `duration.is_none()` would have meant exactly that row never
	/// learning it.
	pub fn measured(&mut self, path: &Path, length: Option<Duration>, music: Option<Duration>) {
		for item in self.items.iter_mut().filter(|item| item.path == path) {
			if item.duration.is_none() {
				item.duration = Some(length);
			}
			if music.is_some() {
				item.music = music;
			}
		}
	}

	/// Apply a press on a row (PLAN §9a) — the same three-way rule the files pane follows, on
	/// indices rather than paths.
	pub fn click(&mut self, index: usize, kind: Click) {
		if index >= self.items.len() {
			return;
		}

		match kind {
			Click::Replace => {
				self.selected.clear();
				self.selected.insert(index);
				self.anchor = Some(index);
			}
			Click::Toggle => {
				if !self.selected.remove(&index) {
					self.selected.insert(index);
				}
				self.anchor = Some(index);
			}
			// The anchor stays put, so a range can be adjusted by Shift-clicking again — and
			// without one there is nothing to measure from, so the press is a plain one.
			Click::Range => match self.anchor {
				Some(anchor) if anchor < self.items.len() => {
					self.selected = select::between(anchor, index).collect();
				}
				_ => self.click(index, Click::Replace),
			},
		}
	}

	/// Re-number the selection after the list has moved under it.
	///
	/// One place for the arithmetic every edit needs, because it is the same arithmetic every
	/// time and it is wrong by one in a way that looks right: `moved` maps an old index to
	/// where that row is now, or to `None` for a row that has gone. A selection is a set of
	/// *tracks* the user pointed at, so a track that moves takes its highlight with it and a
	/// track that leaves takes it away.
	fn resettle(&mut self, moved: impl Fn(usize) -> Option<usize>) {
		// Taken rather than borrowed, so the new set is built from the old one without the
		// assignment fighting the read.
		let selected = std::mem::take(&mut self.selected);
		self.selected = selected.into_iter().filter_map(&moved).collect();
		self.anchor = self
			.anchor
			.and_then(&moved)
			.filter(|_| self.has_selection());
	}

	/// Add tracks at a position, clamped to the end, keeping their order — a drag of a whole
	/// selection, the overflow of a batch handed to a player, or one track on its own
	/// (PLAN §9a).
	///
	/// One entry point rather than the `append` / `prepend` / `insert` trio it replaced: every
	/// caller now arrives with a batch, and the difference between the three was only ever
	/// which index they passed. The selection follows its rows down.
	pub fn insert_many(&mut self, index: usize, items: Vec<Item>) {
		let index = index.min(self.items.len());
		let count = items.len();

		for (offset, item) in items.into_iter().enumerate() {
			self.items.insert(index + offset, item);
		}
		self.resettle(|selected| {
			Some(if selected >= index {
				selected + count
			} else {
				selected
			})
		});
	}

	/// Take a row out. Every other row keeps its highlight, shifted to wherever it now is.
	pub fn remove(&mut self, index: usize) -> Option<Item> {
		if index >= self.items.len() {
			return None;
		}
		let item = self.items.remove(index);

		self.resettle(|selected| match selected.cmp(&index) {
			std::cmp::Ordering::Less => Some(selected),
			// The row that went takes its own highlight with it. Where the *selection* lands
			// afterwards is `remove_selected`'s business, not this one's — the handover also
			// comes through here, and a track ending must not move a highlight the user put
			// somewhere else.
			std::cmp::Ordering::Equal => None,
			std::cmp::Ordering::Greater => Some(selected - 1),
		});

		Some(item)
	}

	/// Take the row the player would play next, which is always the top one.
	pub fn take_next(&mut self) -> Option<Item> {
		self.remove(0)
	}

	/// Take every selected row out, top to bottom, for `✕`, the `←` / `→` buttons and a drag
	/// that leaves the list.
	///
	/// The selection then lands on the row that *slid up into* the topmost hole — the next
	/// track — rather than jumping to the top or vanishing, so pressing `✕` three times
	/// removes three consecutive rows. It is the single-row rule unchanged; a block of rows
	/// leaves one hole as far as the eye is concerned.
	pub fn take_selected(&mut self) -> Vec<Item> {
		let Some(top) = self.first_selected() else {
			return Vec::new();
		};

		let rows: Vec<usize> = self.selection().collect();
		let taken = self.take_rows(&rows);

		self.selected.clear();
		self.anchor = None;
		if !self.items.is_empty() {
			let landed = top.min(self.items.len() - 1);
			self.selected.insert(landed);
			self.anchor = Some(landed);
		}

		taken
	}

	/// Take exactly these rows out, top to bottom.
	///
	/// Separate from `take_selected` because a warning can leave *some* of a selection behind
	/// (PLAN §7a): what is taken is then a subset of what is highlighted, and the rows that
	/// stay keep their highlight where `remove` leaves it.
	pub fn take_rows(&mut self, rows: &[usize]) -> Vec<Item> {
		// Sorted and deduplicated rather than trusted: the walk below goes high to low so that
		// every index is still valid when its turn comes, and one repeated index would take a
		// row nobody asked for.
		let mut rows = rows.to_vec();
		rows.sort_unstable();
		rows.dedup();

		let mut taken: Vec<Item> = rows
			.iter()
			.rev()
			.filter_map(|&index| self.remove(index))
			.collect();
		taken.reverse();
		taken
	}

	/// Move every selected row one place up or down, carrying the selection with them — the
	/// selection names *tracks*, so tracks that move take their highlight along.
	///
	/// `false` when the block cannot go that way rather than a silent no-op, so the caller can
	/// leave the button disabled instead of offering a press that does nothing. A selection
	/// touching the end it is moving towards blocks the **whole** move, scattered rows
	/// included: moving some of what was asked for and not the rest is worse than moving none,
	/// because it is not undone by pressing the other button.
	pub fn shift_selected(&mut self, up: bool) -> bool {
		let (Some(first), Some(last)) = (self.first_selected(), self.last_selected()) else {
			return false;
		};
		if (up && first == 0) || (!up && last + 1 >= self.items.len()) {
			return false;
		}

		// Away from the edge being moved towards, so a row never swaps into one that has not
		// moved yet: upwards from the top, downwards from the bottom.
		let order: Vec<usize> = if up {
			self.selection().collect()
		} else {
			self.selection().rev().collect()
		};
		for index in order {
			let other = if up { index - 1 } else { index + 1 };
			self.items.swap(index, other);
		}

		self.resettle(|selected| Some(if up { selected - 1 } else { selected + 1 }));
		true
	}

	/// Move rows to sit **above row `to`**, which is how a drag reads: the caret sits
	/// *between* rows, and `to` names the row it is above — `len` meaning past the last one.
	///
	/// The whole reason this is a function and not two lines at the call site is the
	/// off-by-one in the middle: once the rows are lifted out, everything below them has
	/// already shifted up, so a caret that was *below* them lands as many places earlier as
	/// there were rows above it. Dropping a block back where it already is returns `false`
	/// rather than performing a move that changes nothing.
	///
	/// The moved rows end up selected, which is not an extra rule — the press that started the
	/// drag selected them, and they should still be the selection when they land.
	pub fn relocate(&mut self, from: &[usize], to: usize) -> bool {
		if from.is_empty()
			|| to > self.items.len()
			|| from.iter().any(|&index| index >= self.items.len())
		{
			return false;
		}

		// Where the block lands once the rows that were above the caret are out of the way.
		let target = to - from.iter().filter(|&&index| index < to).count();
		if from.iter().copied().eq(target..target + from.len()) {
			return false;
		}

		// High to low on the way out, so every index is still valid when its turn comes.
		let mut lifted: Vec<Item> = from
			.iter()
			.rev()
			.map(|&index| self.items.remove(index))
			.collect();
		lifted.reverse();

		for (offset, item) in lifted.into_iter().enumerate() {
			self.items.insert(target + offset, item);
		}

		self.selected = (target..target + from.len()).collect();
		self.anchor = Some(target);
		true
	}
}

/// Which tracks still need their length looked up, given what is already being looked up
/// (PLAN §7a).
///
/// `in_flight` is the whole point. A row counts as unmeasured until its answer *lands*, and
/// the answer lands long after the job that will produce it started — so without this, two
/// edits in quick succession would send the same file to be opened and parsed twice, and
/// twenty would send it twenty times. Subtracting what is already on its way makes each file
/// asked about exactly once.
///
/// Deduplicated across the three lists as well, because the same track may sit in two of them
/// and one answer settles both rows. Returned in draw order, which is arbitrary but stable.
pub fn to_measure(queues: &[Playlist; 3], in_flight: &HashSet<PathBuf>) -> Vec<PathBuf> {
	let mut wanted: Vec<PathBuf> = Vec::new();

	for path in queues.iter().flat_map(Playlist::unmeasured) {
		// A `Vec` for the batch, against the `HashSet` for what is in flight: this one is
		// built once per edit and is tens of entries, and the linear scan keeps the order.
		if !in_flight.contains(path) && !wanted.iter().any(|seen| seen == path) {
			wanted.push(path.to_path_buf());
		}
	}

	wanted
}

/// Where this track is already queued, if it is, so the app can ask before queueing it twice
/// (PLAN §7a).
///
/// **All three lists, not just the one being added to.** The mistake worth catching is a track
/// that plays twice in an evening, and Cue 1 and Cue 2 each holding it does that exactly as
/// surely as one list holding it twice — the duplicate is in the *set*, not in a list.
///
/// `moving` is the rows that are on their way out of a list, and it is the reason this takes a
/// parameter at all: dragging rows from one list to another, or sending them with `←` / `→`,
/// finds them in their old home and would warn about tracks colliding with themselves.
///
/// Searched in draw order, so the list named is the leftmost one — an arbitrary rule, but a
/// stable one, and the message names a list rather than counting them.
pub fn already_queued(
	queues: &[Playlist; 3],
	path: &Path,
	moving: &[(ListId, usize)],
) -> Option<ListId> {
	ListId::ALL.into_iter().find(|list| {
		queues[list.index()]
			.items
			.iter()
			.enumerate()
			.any(|(index, item)| item.path == path && !moving.contains(&(*list, index)))
	})
}

/// Which of a batch of tracks are already queued, and where — as positions into `items`, so a
/// caller can filter rows and tracks together (PLAN §7a, §9a).
///
/// The whole of what the duplicate warning is about, and pure: the dialog only decides what to
/// do with this answer. A batch is checked one track at a time and against the *lists*, not
/// against itself — a selection cannot contain the same file twice, because a pane's rows are
/// a folder's files.
pub fn duplicates(
	queues: &[Playlist; 3],
	items: &[Item],
	moving: &[(ListId, usize)],
) -> Vec<(usize, ListId)> {
	items
		.iter()
		.enumerate()
		.filter_map(|(index, item)| {
			already_queued(queues, &item.path, moving).map(|list| (index, list))
		})
		.collect()
}

/// Where the track after this one comes from, when a player's track ends (PLAN §7a).
///
/// **Own cue first, the shared list second.** A track deliberately cued to Player 1 outranks
/// the pool, which is what makes the pool "whatever is free" rather than a third queue with
/// rules of its own. `None` when neither has anything to offer, and the player simply stops.
///
/// A list with `auto_load` off has nothing to offer however full it is: it is a shelf the user
/// takes from by hand. It is *skipped*, not blocking — a cue switched off still lets the shared
/// list feed the player, because the switch belongs to one list and says nothing about the
/// other.
pub fn next_source(id: DeckId, cue: &Playlist, common: &Playlist) -> Option<ListId> {
	if cue.auto_load && !cue.is_empty() {
		Some(ListId::Cue(id))
	} else if common.auto_load && !common.is_empty() {
		Some(ListId::Common)
	} else {
		None
	}
}

/// Whether the track playing now should give way already (PLAN §7b).
///
/// Three things have to be true at once, and each of them is a reason not to cut: the list
/// that would supply the next track has to be set to skip the blanks, this track's edges have
/// to have been scanned, and the playhead has to have reached the second one. A track nobody
/// has scanned plays whole — silently, because the alternative is a notice line every four
/// minutes about a setting the user already knows they turned on.
///
/// The caller has already asked `next_source` whether there *is* a next track. That is the
/// fourth condition and it is deliberately not here: cutting the run-out off the last track of
/// the evening would leave a player stopped early for no benefit at all.
pub fn hands_over_early(transition: Transition, position: Duration, trim: Option<Trim>) -> bool {
	transition == Transition::Trimmed && trim.is_some_and(|trim| position >= trim.end)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn list(names: &[&str]) -> Playlist {
		Playlist::from_paths(
			names
				.iter()
				.map(|name| PathBuf::from("/m").join(name))
				.collect(),
		)
	}

	fn names(list: &Playlist) -> Vec<&str> {
		list.items().iter().map(|item| item.name.as_str()).collect()
	}

	#[test]
	fn the_arrows_only_reach_a_neighbour() {
		// Arrange / Act / Assert: the middle list is the only way across, so a track cannot
		// jump from one player's cue to the other's in one press.
		let (one, two) = (ListId::Cue(DeckId::One), ListId::Cue(DeckId::Two));

		assert_eq!(one.neighbour(true), Some(ListId::Common));
		assert_eq!(ListId::Common.neighbour(true), Some(two));
		assert_eq!(two.neighbour(false), Some(ListId::Common));
		assert_eq!(ListId::Common.neighbour(false), Some(one));

		// And the ends are ends — this is what disables the buttons.
		assert_eq!(one.neighbour(false), None, "left of the first");
		assert_eq!(two.neighbour(true), None, "right of the last");
	}

	#[test]
	fn every_list_has_its_own_slot() {
		// Arrange / Act / Assert: `index` is what makes `[Playlist; 3]` legal, so a
		// collision would silently merge two lists into one.
		let indices: Vec<usize> = ListId::ALL.iter().map(|id| id.index()).collect();
		assert_eq!(indices, vec![0, 1, 2]);
	}

	/// Put tracks into a list at a position, the way every caller does.
	fn add(list: &mut Playlist, index: usize, names: &[&str]) {
		let items = names
			.iter()
			.map(|name| Item::new(PathBuf::from("/m").join(name)))
			.collect();
		list.insert_many(index, items);
	}

	/// The selected rows, as indices.
	fn chosen(list: &Playlist) -> Vec<usize> {
		list.selection().collect()
	}

	#[test]
	fn adding_puts_tracks_where_they_were_asked_to_go() {
		// Arrange
		let mut list = list(&["c.mp3", "d.mp3"]);

		// Act: at the end, then at the top — and a whole batch at once, which is what a
		// selection dropped in is (PLAN §9a).
		add(&mut list, 2, &["e.mp3"]);
		add(&mut list, 0, &["a.mp3", "b.mp3"]);

		// Assert: the batch keeps its own order, top to bottom, rather than arriving reversed
		// — which is what inserting each one at the same index would do.
		assert_eq!(names(&list), ["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"]);
	}

	#[test]
	fn an_insert_above_the_selection_carries_it_down() {
		// Arrange: the third row selected.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3"]);
		list.click(2, Click::Replace);

		// Act: something arrives at the top.
		add(&mut list, 0, &["new.mp3"]);

		// Assert: the highlight is still on `c.mp3`, which is now row 3. A selection that
		// stayed on index 2 would be highlighting `b.mp3` — the same row, a different track.
		assert_eq!(chosen(&list), [3]);
		assert_eq!(names(&list)[3], "c.mp3");

		// An insert *below* it moves nothing.
		add(&mut list, 4, &["last.mp3"]);
		assert_eq!(chosen(&list), [3]);

		// And a batch above it carries the highlight by as many rows as arrived.
		add(&mut list, 0, &["one.mp3", "two.mp3"]);
		assert_eq!(chosen(&list), [5]);
		assert_eq!(names(&list)[5], "c.mp3");
	}

	#[test]
	fn removing_the_selected_rows_selects_what_slid_into_their_place() {
		// Arrange: two rows selected, with more below them.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);
		list.click(1, Click::Replace);
		list.click(2, Click::Range);

		// Act
		let removed = list.take_selected();

		// Assert: they come back in the order they were in, and the highlight lands on the
		// next track — so pressing `✕` repeatedly removes consecutive rows rather than
		// requiring a re-aim after each one.
		let taken: Vec<&str> = removed.iter().map(|item| item.name.as_str()).collect();
		assert_eq!(taken, ["b.mp3", "c.mp3"]);
		assert_eq!(chosen(&list), [1]);
		assert_eq!(names(&list)[1], "d.mp3");
	}

	#[test]
	fn removing_the_last_row_falls_back_to_the_new_last_row() {
		// Arrange: the bottom row selected, with nothing below to slide up.
		let mut list = list(&["a.mp3", "b.mp3"]);
		list.click(1, Click::Replace);

		// Act / Assert
		list.take_selected();
		assert_eq!(chosen(&list), [0], "the row above takes the highlight");

		list.take_selected();
		assert!(chosen(&list).is_empty(), "an empty list selects nothing");
		assert!(list.is_empty());
	}

	#[test]
	fn removing_a_row_above_the_selection_keeps_the_same_track_selected() {
		// Arrange
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3"]);
		list.click(2, Click::Replace);

		// Act: a row above the selection goes — a handover taking the top track, which must
		// not move a highlight the user put somewhere else.
		list.remove(0);

		// Assert: still `c.mp3`, one row higher.
		assert_eq!(chosen(&list), [1]);
		assert_eq!(names(&list)[1], "c.mp3");
	}

	#[test]
	fn a_press_selects_one_row_or_several_depending_on_what_is_held() {
		// Arrange
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);

		// Act / Assert: the same three-way rule the files pane follows (PLAN §9a), on indices.
		list.click(1, Click::Replace);
		assert_eq!(chosen(&list), [1]);

		list.click(3, Click::Toggle);
		assert_eq!(chosen(&list), [1, 3], "in row order, not click order");

		list.click(1, Click::Toggle);
		assert_eq!(chosen(&list), [3], "clicked again, and gone");

		// A range measures from the anchor, which the toggle above moved to row 1.
		list.click(1, Click::Replace);
		list.click(3, Click::Range);
		assert_eq!(chosen(&list), [1, 2, 3]);

		// A row that is not there selects nothing rather than a phantom index — the guard the
		// rest of this module assumes.
		let mut short = list_of_one();
		short.click(5, Click::Replace);
		assert!(chosen(&short).is_empty());
	}

	fn list_of_one() -> Playlist {
		list(&["a.mp3"])
	}

	#[test]
	fn taking_the_next_track_takes_the_top_one() {
		// Arrange
		let mut list = list(&["a.mp3", "b.mp3"]);

		// Act / Assert
		assert_eq!(list.take_next().map(|item| item.name), Some("a.mp3".into()));
		assert_eq!(names(&list), ["b.mp3"]);
		assert_eq!(list.take_next().map(|item| item.name), Some("b.mp3".into()));
		assert_eq!(list.take_next(), None, "an empty queue stops the player");
	}

	#[test]
	fn shifting_rows_takes_their_highlight_with_them() {
		// Arrange
		let mut single = list(&["a.mp3", "b.mp3", "c.mp3"]);
		single.click(2, Click::Replace);

		// Act
		assert!(single.shift_selected(true));

		// Assert: the track moved and the highlight went with it. This is the one a
		// swap-only implementation gets wrong.
		assert_eq!(names(&single), ["a.mp3", "c.mp3", "b.mp3"]);
		assert_eq!(chosen(&single), [1]);

		// And a block of two moves as a block, keeping its own order — swapping them one at a
		// time in the wrong direction would reverse the pair as it went.
		let mut block = list(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);
		block.click(0, Click::Replace);
		block.click(1, Click::Range);
		assert!(block.shift_selected(false));
		assert_eq!(names(&block), ["c.mp3", "a.mp3", "b.mp3", "d.mp3"]);
		assert_eq!(chosen(&block), [1, 2]);
	}

	#[test]
	fn a_scattered_selection_still_moves_together() {
		// Arrange: rows 0 and 2 of four, which is what a pair of command-clicks makes.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);
		list.click(0, Click::Replace);
		list.click(2, Click::Toggle);

		// Act
		assert!(list.shift_selected(false));

		// Assert: each moved down one, past the row that was below it.
		assert_eq!(names(&list), ["b.mp3", "a.mp3", "d.mp3", "c.mp3"]);
		assert_eq!(chosen(&list), [1, 3]);
	}

	#[test]
	fn a_selection_cannot_be_shifted_off_either_end() {
		// Arrange / Act / Assert: `false` rather than a no-op, so the button is drawn dead
		// instead of offering a press that does nothing.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3"]);
		assert!(!list.shift_selected(true), "nothing selected");

		list.click(0, Click::Replace);
		assert!(!list.shift_selected(true), "up from the top");

		list.click(2, Click::Replace);
		assert!(!list.shift_selected(false), "down from the bottom");

		// The whole move is blocked, not part of it: a selection touching the top cannot go up
		// even though the rows below it could. Moving some of what was asked for and not the
		// rest is not undone by pressing the other button.
		list.click(0, Click::Replace);
		list.click(2, Click::Toggle);
		assert!(!list.shift_selected(true), "one of them is at the top");
		assert_eq!(names(&list), ["a.mp3", "b.mp3", "c.mp3"], "nothing moved");
	}

	#[test]
	fn a_drag_downwards_lands_where_the_caret_was() {
		// Arrange: the caret between `c` and `d` is index 3, and the row being dragged is
		// above it — so lifting the row out shifts the caret up with everything else. This is
		// the case that is wrong by one in every implementation that forgets it.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);

		// Act: `a` to just above `d`.
		assert!(list.relocate(&[0], 3));

		// Assert: `a` sits between `c` and `d`, not after `d`.
		assert_eq!(names(&list), ["b.mp3", "c.mp3", "a.mp3", "d.mp3"]);
		assert_eq!(chosen(&list), [2], "the dragged row stays selected");
	}

	#[test]
	fn a_drag_upwards_needs_no_adjustment() {
		// Arrange / Act: the mirror of the case above — the caret is above the row, so
		// nothing has shifted under it.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);
		assert!(list.relocate(&[3], 1));

		// Assert
		assert_eq!(names(&list), ["a.mp3", "d.mp3", "b.mp3", "c.mp3"]);
		assert_eq!(chosen(&list), [1]);
	}

	#[test]
	fn a_drag_to_the_very_end_puts_the_row_last() {
		// Arrange / Act: `len` is the caret past the last row, which is the one index that
		// does not name a row at all.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3"]);
		assert!(list.relocate(&[0], 3));

		// Assert
		assert_eq!(names(&list), ["b.mp3", "c.mp3", "a.mp3"]);
		assert_eq!(chosen(&list), [2]);
	}

	#[test]
	fn a_drag_that_lands_where_it_started_changes_nothing() {
		// Arrange: the two carets that touch a row — its own top edge, and its bottom one.
		// Both mean "leave it alone", and both are easy to reach with a twitchy hand.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3"]);

		// Act / Assert
		assert!(!list.relocate(&[1], 1), "the caret above itself");
		assert!(!list.relocate(&[1], 2), "the caret below itself");
		assert!(!list.relocate(&[9], 0), "a row that is not there");
		assert!(!list.relocate(&[0], 9), "a caret past the end of the list");
		assert!(!list.relocate(&[], 0), "nothing being dragged");
		assert_eq!(names(&list), ["a.mp3", "b.mp3", "c.mp3"], "nothing moved");

		// And the same for a *block*: the carets either side of it, and every caret inside it,
		// leave it exactly where it is (PLAN §9a).
		assert!(!list.relocate(&[0, 1], 0), "above the block");
		assert!(!list.relocate(&[0, 1], 2), "below the block");
		assert_eq!(names(&list), ["a.mp3", "b.mp3", "c.mp3"], "still nothing");
	}

	#[test]
	fn a_block_of_rows_lands_where_the_caret_was_and_keeps_its_order() {
		// Arrange: two rows either side of a third, which is the case that shows whether the
		// block is moved as a block or one row at a time.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"]);

		// Act: `a` and `c` to just above `e`. Two rows lie above that caret, so the block
		// lands two places earlier than the caret's index says.
		assert!(list.relocate(&[0, 2], 4));

		// Assert: in their own order, together, and selected where they landed.
		assert_eq!(names(&list), ["b.mp3", "d.mp3", "a.mp3", "c.mp3", "e.mp3"]);
		assert_eq!(chosen(&list), [2, 3]);
	}

	#[test]
	fn every_drag_within_a_list_keeps_every_track() {
		// Arrange: exhaustive, because a reorder that loses or duplicates a row is the one
		// failure a queue cannot survive — and there are only 25 cases.
		let original = ["a.mp3", "b.mp3", "c.mp3", "d.mp3"];

		for from in 0..original.len() {
			for to in 0..=original.len() {
				// Act
				let mut list = list(&original);
				list.relocate(&[from], to);

				// Assert: same set, same length, whatever the move did.
				let mut got = names(&list);
				got.sort_unstable();
				assert_eq!(got, original, "relocate({from}, {to}) changed the contents");
			}
		}

		// And every *pair* of rows to every caret, which is the same failure with a second way
		// to reach it: 30 more cases, each also checking that both rows end up selected.
		for first in 0..original.len() {
			for second in first + 1..original.len() {
				for to in 0..=original.len() {
					let mut list = list(&original);
					let moved = list.relocate(&[first, second], to);

					let mut got = names(&list);
					got.sort_unstable();
					assert_eq!(
						got, original,
						"relocate([{first}, {second}], {to}) lost a row"
					);

					// A move that happened leaves both rows highlighted where they landed; one
					// that did not leaves the selection exactly as it was, which here is
					// nothing at all.
					assert_eq!(
						chosen(&list).len(),
						usize::from(moved) * 2,
						"relocate([{first}, {second}], {to}) lost a highlight"
					);
				}
			}
		}
	}

	#[test]
	fn taking_some_of_a_selection_leaves_the_rest_highlighted() {
		// Arrange: three rows selected, of which the duplicate warning will let two through
		// (PLAN §7a) — so what leaves the list is a subset of what is highlighted.
		let mut queue = list(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);
		queue.click(0, Click::Replace);
		queue.click(2, Click::Range);

		// Act: rows 0 and 2 go, row 1 stays behind.
		let taken = queue.take_rows(&[0, 2]);

		// Assert: the tracks come back in their own order, and the row that stayed keeps its
		// highlight where the removals left it.
		let taken: Vec<&str> = taken.iter().map(|item| item.name.as_str()).collect();
		assert_eq!(taken, ["a.mp3", "c.mp3"]);
		assert_eq!(names(&queue), ["b.mp3", "d.mp3"]);
		assert_eq!(chosen(&queue), [0], "b.mp3, still selected and now first");

		// An index handed in twice takes one row, not two — the walk goes high to low and a
		// repeat would take whatever slid into the place.
		let mut twice = list(&["a.mp3", "b.mp3"]);
		assert_eq!(twice.take_rows(&[0, 0]).len(), 1);
		assert_eq!(names(&twice), ["b.mp3"]);
	}

	#[test]
	fn a_running_time_admits_what_it_has_not_counted() {
		// Arrange: three tracks, none of them measured yet.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3"]);
		assert_eq!(
			list.total(),
			(Duration::ZERO, false),
			"nothing measured is not a list of length zero"
		);

		// Act: two answers arrive, and one of them is "this file has no length".
		list.measured(
			&PathBuf::from("/m/a.mp3"),
			Some(Duration::from_secs(90)),
			None,
		);
		list.measured(&PathBuf::from("/m/b.mp3"), None, None);

		// Assert: the known lengths add up, and the total still says it is not the whole
		// story — which is what the `+` in the footer is.
		assert_eq!(list.total(), (Duration::from_secs(90), false));

		// Act / Assert: everything has now been *asked*, and the answer is still not whole —
		// a row nobody can measure keeps the `+` for ever, which is the honest outcome. The
		// two states differ in what the app does next, not in what the footer says: one is
		// waiting for a thread and the other has stopped asking.
		list.measured(
			&PathBuf::from("/m/c.mp3"),
			Some(Duration::from_secs(30)),
			None,
		);
		assert_eq!(list.total(), (Duration::from_secs(120), false));
		assert_eq!(list.unmeasured().count(), 0, "nothing left to ask about");

		// A list with no unknowns in it is exact, and so is an empty one.
		list.remove(1);
		assert_eq!(list.total(), (Duration::from_secs(120), true));
		assert_eq!(Playlist::default().total(), (Duration::ZERO, true));
	}

	#[test]
	fn one_answer_settles_every_row_holding_that_track() {
		// Arrange: the same track queued twice, which is the reason `measured` works by path
		// and the reason a row is selected by index.
		let mut list = list(&["a.mp3", "b.mp3", "a.mp3"]);

		// Act
		list.measured(
			&PathBuf::from("/m/a.mp3"),
			Some(Duration::from_secs(60)),
			None,
		);

		// Assert: both copies, and nothing else.
		assert_eq!(
			list.items()[0].duration,
			Some(Some(Duration::from_secs(60)))
		);
		assert_eq!(
			list.items()[2].duration,
			Some(Some(Duration::from_secs(60)))
		);
		assert_eq!(list.items()[1].duration, None, "a different track");

		// And an answer that failed is remembered as an answer, or the app would re-open an
		// unreadable file every time anything else was added.
		list.measured(&PathBuf::from("/m/b.mp3"), None, None);
		assert_eq!(list.items()[1].duration, Some(None));
		assert_eq!(
			list.unmeasured().count(),
			0,
			"nothing is still waiting to be measured"
		);
	}

	#[test]
	fn a_playing_time_arrives_after_the_length_and_never_unlearns_itself() {
		// Arrange: a row measured the moment it was queued, which is every row — a length is a
		// header parse and the queues pay for it on every edit.
		let mut list = list(&["a.mp3"]);
		list.measured(
			&PathBuf::from("/m/a.mp3"),
			Some(Duration::from_secs(215)),
			None,
		);
		assert_eq!(list.items()[0].music, None, "nothing has scanned it");

		// Act: the folder scan reaches it, long after. The length is settled and stays settled;
		// the playing time has to land anyway, or a row measured once could never learn it.
		list.measured(
			&PathBuf::from("/m/a.mp3"),
			Some(Duration::from_secs(1)),
			Some(Duration::from_secs(198)),
		);
		assert_eq!(
			list.items()[0].duration,
			Some(Some(Duration::from_secs(215))),
			"the first answer stands"
		);
		assert_eq!(list.items()[0].music, Some(Duration::from_secs(198)));

		// Act / Assert: and a later queue edit, which only ever *reads* the store, must not take
		// it away again — the same rule `App::remember` follows.
		list.measured(&PathBuf::from("/m/a.mp3"), None, None);
		assert_eq!(
			list.items()[0].music,
			Some(Duration::from_secs(198)),
			"a job that did not look deep enough says nothing, not nothing-there"
		);
	}

	#[test]
	fn nothing_is_sent_to_be_measured_twice() {
		// Arrange: the same track in two lists, plus one of its own, and nothing measured.
		let queues = [
			list(&["a.mp3", "b.mp3"]),
			list(&["a.mp3"]),
			Playlist::default(),
		];
		let path = |name: &str| PathBuf::from("/m").join(name);

		// Act / Assert: one entry per *file*, not per row — one answer settles every row
		// holding it, so asking twice would be opening the file twice for one number.
		assert_eq!(
			to_measure(&queues, &HashSet::new()),
			vec![path("a.mp3"), path("b.mp3")]
		);

		// Act / Assert: and nothing already on its way. This is the whole reason the set
		// exists: a row stays unmeasured until its answer lands, so a second edit arriving
		// mid-flight would otherwise send the same file off to be parsed all over again.
		let in_flight = HashSet::from([path("a.mp3")]);
		assert_eq!(to_measure(&queues, &in_flight), vec![path("b.mp3")]);

		let both = HashSet::from([path("a.mp3"), path("b.mp3")]);
		assert!(
			to_measure(&queues, &both).is_empty(),
			"everything is already being measured"
		);
	}

	#[test]
	fn a_measured_track_is_never_asked_about_again() {
		// Arrange: one row answered, one answered with *nothing* — a stream, or a file that
		// would not open.
		let mut queue = list(&["a.mp3", "b.mp3"]);
		queue.measured(
			&PathBuf::from("/m/a.mp3"),
			Some(Duration::from_secs(10)),
			None,
		);
		queue.measured(&PathBuf::from("/m/b.mp3"), None, None);
		let queues = [queue, Playlist::default(), Playlist::default()];

		// Act / Assert: neither is asked about again, which is what stops the app re-opening
		// an unreadable file on every edit for the rest of the run.
		assert!(to_measure(&queues, &HashSet::new()).is_empty());
	}

	#[test]
	fn a_track_is_found_wherever_it_is_already_queued() {
		// Arrange: one track in Cue 2 and nowhere else, in the app's own `[Playlist; 3]`
		// order — Cue 1, Next up, Cue 2.
		let queues = [
			list(&["a.mp3"]),
			Playlist::default(),
			list(&["b.mp3", "c.mp3"]),
		];
		let path = |name: &str| PathBuf::from("/m").join(name);

		// Act / Assert: found in the list it is actually in, not merely in the one being
		// added to — a track in both cues plays twice, which is the mistake worth catching.
		assert_eq!(
			already_queued(&queues, &path("c.mp3"), &[]),
			Some(ListId::Cue(DeckId::Two))
		);
		assert_eq!(
			already_queued(&queues, &path("a.mp3"), &[]),
			Some(ListId::Cue(DeckId::One))
		);
		assert_eq!(
			already_queued(&queues, &path("new.mp3"), &[]),
			None,
			"a track nothing holds"
		);

		// And a whole batch at once, which is what the warning actually asks about: the
		// positions of the ones already queued, so a caller can filter rows and tracks
		// together (PLAN §9a).
		let batch = [
			Item::new(path("new.mp3")),
			Item::new(path("c.mp3")),
			Item::new(path("also-new.mp3")),
			Item::new(path("a.mp3")),
		];
		assert_eq!(
			duplicates(&queues, &batch, &[]),
			vec![(1, ListId::Cue(DeckId::Two)), (3, ListId::Cue(DeckId::One))]
		);
	}

	#[test]
	fn a_row_on_its_way_out_is_not_its_own_duplicate() {
		// Arrange: the row being dragged from Cue 1 into Next up. Without the exception it
		// would find itself in Cue 1 and warn about colliding with itself, which would make
		// every single cross-list move ask a question with one honest answer.
		let queues = [list(&["a.mp3", "b.mp3"]), Playlist::default(), list(&[])];
		let moving = [(ListId::Cue(DeckId::One), 1)];

		// Act / Assert
		assert_eq!(
			already_queued(&queues, &PathBuf::from("/m/b.mp3"), &moving),
			None
		);

		// The exception is the *row*, not the track: a second copy elsewhere still counts.
		let queues = [
			list(&["a.mp3", "b.mp3"]),
			list(&["b.mp3"]),
			Playlist::default(),
		];
		assert_eq!(
			already_queued(&queues, &PathBuf::from("/m/b.mp3"), &moving),
			Some(ListId::Common)
		);
	}

	#[test]
	fn the_next_track_comes_from_the_players_own_cue_first() {
		// Arrange: both lists have something.
		let cue = list(&["mine.mp3"]);
		let common = list(&["shared.mp3"]);
		let empty = Playlist::default();

		// Act / Assert: the deliberate cue outranks the pool.
		assert_eq!(
			next_source(DeckId::One, &cue, &common),
			Some(ListId::Cue(DeckId::One))
		);

		// The pool is the fallback, not the first choice…
		assert_eq!(
			next_source(DeckId::Two, &empty, &common),
			Some(ListId::Common)
		);

		// …and two empty lists mean the player just stops.
		assert_eq!(next_source(DeckId::One, &empty, &empty), None);
	}

	#[test]
	fn a_list_with_auto_load_off_is_skipped_rather_than_blocking() {
		// Arrange: both lists have a track, and the player's own cue is switched off.
		let off = |names: &[&str]| Playlist {
			auto_load: false,
			..list(names)
		};
		let cue = off(&["mine.mp3"]);
		let common = list(&["shared.mp3"]);

		// Act / Assert: the cue is passed over — not treated as an empty list that stops the
		// handover, which is the difference the word *skipped* is doing. The switch belongs to
		// one list and says nothing about the other.
		assert_eq!(
			next_source(DeckId::One, &cue, &common),
			Some(ListId::Common)
		);

		// The shared list switched off too: nothing is offered, and the player stops with two
		// full lists in front of it — which is exactly what switching both off asks for.
		let common = off(&["shared.mp3"]);
		assert_eq!(next_source(DeckId::One, &cue, &common), None);

		// And a fresh list hands over: the default is what the app did before there were
		// switches at all.
		assert!(Playlist::default().auto_load, "on unless it is turned off");
		assert!(!Playlist::default().auto_play, "loading is not playing");
	}

	#[test]
	fn a_track_gives_way_early_only_when_all_three_things_are_true() {
		// Arrange: a four-minute file whose music stops twelve seconds before it does.
		let trim = Some(Trim {
			start: Duration::from_secs(2),
			end: Duration::from_secs(228),
		});
		let early =
			|transition, seconds| hands_over_early(transition, Duration::from_secs(seconds), trim);

		// Act / Assert: the run-out is what gets skipped, and only for a list that asked.
		assert!(!early(Transition::Trimmed, 227), "still playing music");
		assert!(early(Transition::Trimmed, 228), "the music has stopped");
		assert!(early(Transition::Trimmed, 239), "seeked into the run-out");
		assert!(
			!early(Transition::Whole, 239),
			"this list plays files whole"
		);

		// A track nobody has scanned plays whole, whatever the setting says: there is no
		// second edge to reach, so there is nothing to cut to (PLAN §14c).
		assert!(
			!hands_over_early(Transition::Trimmed, Duration::from_secs(600), None),
			"never scanned"
		);

		// And a fresh list plays whole, which is what the app did before the setting existed.
		assert_eq!(Playlist::default().transition, Transition::Whole);
	}
}
