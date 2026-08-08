//! The three queues (PLAN §7a): one in front of each player, and one shared between them.
//!
//! Pure, like `deck.rs` and for the same reason — every rule here is an edit to a list, and
//! an edit to a list is exactly the kind of thing that is wrong by one and looks right. So
//! the whole module is `Vec` arithmetic with no iced and no filesystem, and the interesting
//! part is not the moving but **what happens to the selection when the list moves under
//! it**: a row that stays highlighted while a different track slides beneath it is worse
//! than no highlight at all.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::deck::DeckId;

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
}

impl Item {
	pub fn new(path: PathBuf) -> Self {
		let name = crate::fsio::name_of(&path);
		Self {
			path,
			name,
			duration: None,
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
	selected: Option<usize>,
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
}

impl Default for Playlist {
	fn default() -> Self {
		Self {
			items: Vec::new(),
			selected: None,
			// On: a queue is a list of what plays next, and one that had to be switched on
			// before it did anything would be a list that quietly did nothing.
			auto_load: true,
			// Off, for the same reason every load lands on `Stopped` (PLAN §7): on a mixer,
			// audio nobody asked for is a mistake that cannot be taken back. Someone who wants
			// the evening to run itself says so once, per list.
			auto_play: false,
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

	pub fn selected(&self) -> Option<usize> {
		self.selected
	}

	/// The selected row itself, for a caller that has to look at a track before deciding
	/// whether to move it.
	pub fn selected_item(&self) -> Option<&Item> {
		self.items.get(self.selected?)
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

	/// Record a length against every row holding this path that is still waiting for one.
	///
	/// By path rather than by index, because the lists can be edited while the measuring runs
	/// and an index would name a different track by the time the answer came back. A queue may
	/// hold the same track twice, so one answer settles both rows.
	pub fn measured(&mut self, path: &Path, length: Option<Duration>) {
		for item in self
			.items
			.iter_mut()
			.filter(|item| item.duration.is_none() && item.path == path)
		{
			item.duration = Some(length);
		}
	}

	/// Select a row, or clear the selection if the index is not one.
	pub fn select(&mut self, index: usize) {
		self.selected = (index < self.items.len()).then_some(index);
	}

	/// Add to the end. The selection does not move: rows above it are untouched.
	pub fn append(&mut self, item: Item) {
		self.items.push(item);
	}

	/// Add to the front — "play this next" without a reorder.
	pub fn prepend(&mut self, item: Item) {
		self.insert(0, item);
	}

	/// Add at a position, clamped to the end. The selection follows its row down.
	pub fn insert(&mut self, index: usize, item: Item) {
		let index = index.min(self.items.len());
		self.items.insert(index, item);

		if let Some(selected) = self.selected.as_mut()
			&& *selected >= index
		{
			*selected += 1;
		}
	}

	/// Take a row out, and leave the selection somewhere sensible.
	///
	/// Removing the selected row leaves the selection on the row that *slid up into its
	/// place* — the next track — rather than jumping to the top or vanishing, so pressing
	/// remove three times removes three consecutive rows.
	pub fn remove(&mut self, index: usize) -> Option<Item> {
		if index >= self.items.len() {
			return None;
		}
		let item = self.items.remove(index);

		self.selected = match self.selected {
			// Above the hole: unmoved.
			Some(selected) if selected < index => Some(selected),
			// Below it: shifted up with everything else.
			Some(selected) if selected > index => Some(selected - 1),
			// It *was* the hole. Keep the index if a row slid into it, else the new last row,
			// else there is nothing left to select. Written out rather than with a `?`,
			// which would return from `remove` itself and throw the removed item away.
			Some(_) if self.items.is_empty() => None,
			Some(_) => Some(index.min(self.items.len() - 1)),
			None => None,
		};

		Some(item)
	}

	/// Take the row the player would play next, which is always the top one.
	pub fn take_next(&mut self) -> Option<Item> {
		self.remove(0)
	}

	/// Take the selected row out, for the `←` / `→` buttons and a drag that leaves the list.
	pub fn take_selected(&mut self) -> Option<Item> {
		self.remove(self.selected?)
	}

	/// Move a row one place up or down, carrying the selection with it — the selection names
	/// a *track*, so a track that moves must take its highlight along.
	///
	/// `false` at either end rather than a silent no-op, so the caller can leave the button
	/// disabled instead of offering a press that does nothing.
	pub fn shift(&mut self, index: usize, up: bool) -> bool {
		let Some(other) = (if up {
			index.checked_sub(1)
		} else {
			(index + 1 < self.items.len()).then_some(index + 1)
		}) else {
			return false;
		};
		if index >= self.items.len() {
			return false;
		}

		self.items.swap(index, other);
		if self.selected == Some(index) {
			self.selected = Some(other);
		} else if self.selected == Some(other) {
			self.selected = Some(index);
		}

		true
	}

	/// Move a row to sit **above row `to`**, which is how a drag reads: the caret sits
	/// *between* rows, and `to` names the row it is above — `len` meaning past the last one.
	///
	/// The whole reason this is a function and not two lines at the call site is the
	/// off-by-one in the middle: once the row is lifted out, everything below it has already
	/// shifted up, so a caret that was *below* the row lands one place earlier than its index
	/// said. Dropping a row just above or just below itself is where it already is, and
	/// returns `false` rather than performing a move that changes nothing.
	///
	/// The moved row ends up selected, which is not an extra rule — the press that started
	/// the drag selected it, and it should still be the selected one when it lands.
	pub fn relocate(&mut self, from: usize, to: usize) -> bool {
		if from >= self.items.len() || to > self.items.len() || to == from || to == from + 1 {
			return false;
		}

		let item = self.items.remove(from);
		let to = if to > from { to - 1 } else { to };
		self.items.insert(to, item);
		self.selected = Some(to);

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
/// `moving` is the row that is on its way out of a list, and it is the reason this takes a
/// parameter at all: dragging a row from one list to another, or sending it with `←` / `→`,
/// finds the row in its old home and would warn about the track colliding with itself.
///
/// Searched in draw order, so the list named is the leftmost one — an arbitrary rule, but a
/// stable one, and the message names a list rather than counting them.
pub fn already_queued(
	queues: &[Playlist; 3],
	path: &Path,
	moving: Option<(ListId, usize)>,
) -> Option<ListId> {
	ListId::ALL.into_iter().find(|list| {
		queues[list.index()]
			.items
			.iter()
			.enumerate()
			.any(|(index, item)| item.path == path && moving != Some((*list, index)))
	})
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

	#[test]
	fn adding_puts_a_track_where_it_was_asked_to_go() {
		// Arrange
		let mut list = list(&["b.mp3", "c.mp3"]);

		// Act
		list.append(Item::new(PathBuf::from("/m/d.mp3")));
		list.prepend(Item::new(PathBuf::from("/m/a.mp3")));

		// Assert
		assert_eq!(names(&list), ["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);
	}

	#[test]
	fn an_insert_above_the_selection_carries_it_down() {
		// Arrange: the third row selected.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3"]);
		list.select(2);

		// Act: something arrives at the top.
		list.prepend(Item::new(PathBuf::from("/m/new.mp3")));

		// Assert: the highlight is still on `c.mp3`, which is now row 3. A selection that
		// stayed on index 2 would be highlighting `b.mp3` — the same row, a different track.
		assert_eq!(list.selected(), Some(3));
		assert_eq!(names(&list)[3], "c.mp3");

		// An insert *below* it moves nothing.
		list.insert(4, Item::new(PathBuf::from("/m/last.mp3")));
		assert_eq!(list.selected(), Some(3));
	}

	#[test]
	fn removing_the_selected_row_selects_what_slid_into_its_place() {
		// Arrange
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3"]);
		list.select(1);

		// Act
		let removed = list.remove(1);

		// Assert: the next track, so pressing remove repeatedly removes consecutive rows
		// rather than requiring a re-aim after each one.
		assert_eq!(removed.map(|item| item.name), Some("b.mp3".to_string()));
		assert_eq!(list.selected(), Some(1));
		assert_eq!(names(&list)[1], "c.mp3");
	}

	#[test]
	fn removing_the_last_row_falls_back_to_the_new_last_row() {
		// Arrange: the bottom row selected, with nothing below to slide up.
		let mut list = list(&["a.mp3", "b.mp3"]);
		list.select(1);

		// Act / Assert
		list.remove(1);
		assert_eq!(
			list.selected(),
			Some(0),
			"the row above takes the highlight"
		);

		list.remove(0);
		assert_eq!(list.selected(), None, "an empty list has nothing to select");
		assert!(list.is_empty());
	}

	#[test]
	fn removing_a_row_above_the_selection_keeps_the_same_track_selected() {
		// Arrange
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3"]);
		list.select(2);

		// Act: a row above the selection goes.
		list.remove(0);

		// Assert: still `c.mp3`, one row higher.
		assert_eq!(list.selected(), Some(1));
		assert_eq!(names(&list)[1], "c.mp3");
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
	fn shifting_a_row_takes_its_highlight_with_it() {
		// Arrange
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3"]);
		list.select(2);

		// Act
		assert!(list.shift(2, true));

		// Assert: the track moved and the highlight went with it. This is the one a
		// swap-only implementation gets wrong.
		assert_eq!(names(&list), ["a.mp3", "c.mp3", "b.mp3"]);
		assert_eq!(list.selected(), Some(1));
	}

	#[test]
	fn shifting_the_row_a_selection_swaps_with_moves_the_selection_too() {
		// Arrange: move the row *above* the selected one down onto it.
		let mut list = list(&["a.mp3", "b.mp3"]);
		list.select(1);

		// Act
		assert!(list.shift(0, false));

		// Assert: `b.mp3` is now row 0 and is still the selected track.
		assert_eq!(names(&list), ["b.mp3", "a.mp3"]);
		assert_eq!(list.selected(), Some(0));
	}

	#[test]
	fn a_row_cannot_be_shifted_off_either_end() {
		// Arrange / Act / Assert: `false` rather than a no-op, so the button is drawn dead
		// instead of offering a press that does nothing.
		let mut list = list(&["a.mp3", "b.mp3"]);

		assert!(!list.shift(0, true), "up from the top");
		assert!(!list.shift(1, false), "down from the bottom");
		assert!(!list.shift(9, true), "a row that is not there");
		assert_eq!(names(&list), ["a.mp3", "b.mp3"], "nothing moved");
	}

	#[test]
	fn selecting_a_row_that_is_not_there_selects_nothing() {
		// Arrange / Act / Assert: the guard that keeps every `selected` index valid, which
		// the rest of this module assumes.
		let mut list = list(&["a.mp3"]);
		list.select(5);
		assert_eq!(list.selected(), None);
	}

	#[test]
	fn a_drag_downwards_lands_where_the_caret_was() {
		// Arrange: the caret between `c` and `d` is index 3, and the row being dragged is
		// above it — so lifting the row out shifts the caret up with everything else. This is
		// the case that is wrong by one in every implementation that forgets it.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);

		// Act: `a` to just above `d`.
		assert!(list.relocate(0, 3));

		// Assert: `a` sits between `c` and `d`, not after `d`.
		assert_eq!(names(&list), ["b.mp3", "c.mp3", "a.mp3", "d.mp3"]);
		assert_eq!(
			list.selected(),
			Some(2),
			"the dragged row stays the selected one"
		);
	}

	#[test]
	fn a_drag_upwards_needs_no_adjustment() {
		// Arrange / Act: the mirror of the case above — the caret is above the row, so
		// nothing has shifted under it.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);
		assert!(list.relocate(3, 1));

		// Assert
		assert_eq!(names(&list), ["a.mp3", "d.mp3", "b.mp3", "c.mp3"]);
		assert_eq!(list.selected(), Some(1));
	}

	#[test]
	fn a_drag_to_the_very_end_puts_the_row_last() {
		// Arrange / Act: `len` is the caret past the last row, which is the one index that
		// does not name a row at all.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3"]);
		assert!(list.relocate(0, 3));

		// Assert
		assert_eq!(names(&list), ["b.mp3", "c.mp3", "a.mp3"]);
		assert_eq!(list.selected(), Some(2));
	}

	#[test]
	fn a_drag_that_lands_where_it_started_changes_nothing() {
		// Arrange: the two carets that touch a row — its own top edge, and its bottom one.
		// Both mean "leave it alone", and both are easy to reach with a twitchy hand.
		let mut list = list(&["a.mp3", "b.mp3", "c.mp3"]);

		// Act / Assert
		assert!(!list.relocate(1, 1), "the caret above itself");
		assert!(!list.relocate(1, 2), "the caret below itself");
		assert!(!list.relocate(9, 0), "a row that is not there");
		assert!(!list.relocate(0, 9), "a caret past the end of the list");
		assert_eq!(names(&list), ["a.mp3", "b.mp3", "c.mp3"], "nothing moved");
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
				list.relocate(from, to);

				// Assert: same set, same length, whatever the move did.
				let mut got = names(&list);
				got.sort_unstable();
				assert_eq!(got, original, "relocate({from}, {to}) changed the contents");
			}
		}
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
		list.measured(&PathBuf::from("/m/a.mp3"), Some(Duration::from_secs(90)));
		list.measured(&PathBuf::from("/m/b.mp3"), None);

		// Assert: the known lengths add up, and the total still says it is not the whole
		// story — which is what the `+` in the footer is.
		assert_eq!(list.total(), (Duration::from_secs(90), false));

		// Act / Assert: everything has now been *asked*, and the answer is still not whole —
		// a row nobody can measure keeps the `+` for ever, which is the honest outcome. The
		// two states differ in what the app does next, not in what the footer says: one is
		// waiting for a thread and the other has stopped asking.
		list.measured(&PathBuf::from("/m/c.mp3"), Some(Duration::from_secs(30)));
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
		list.measured(&PathBuf::from("/m/a.mp3"), Some(Duration::from_secs(60)));

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
		list.measured(&PathBuf::from("/m/b.mp3"), None);
		assert_eq!(list.items()[1].duration, Some(None));
		assert_eq!(
			list.unmeasured().count(),
			0,
			"nothing is still waiting to be measured"
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
		queue.measured(&PathBuf::from("/m/a.mp3"), Some(Duration::from_secs(10)));
		queue.measured(&PathBuf::from("/m/b.mp3"), None);
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
			already_queued(&queues, &path("c.mp3"), None),
			Some(ListId::Cue(DeckId::Two))
		);
		assert_eq!(
			already_queued(&queues, &path("a.mp3"), None),
			Some(ListId::Cue(DeckId::One))
		);
		assert_eq!(
			already_queued(&queues, &path("new.mp3"), None),
			None,
			"a track nothing holds"
		);
	}

	#[test]
	fn a_row_on_its_way_out_is_not_its_own_duplicate() {
		// Arrange: the row being dragged from Cue 1 into Next up. Without the exception it
		// would find itself in Cue 1 and warn about colliding with itself, which would make
		// every single cross-list move ask a question with one honest answer.
		let queues = [list(&["a.mp3", "b.mp3"]), Playlist::default(), list(&[])];
		let moving = Some((ListId::Cue(DeckId::One), 1));

		// Act / Assert
		assert_eq!(
			already_queued(&queues, &PathBuf::from("/m/b.mp3"), moving),
			None
		);

		// The exception is the *row*, not the track: a second copy elsewhere still counts.
		let queues = [
			list(&["a.mp3", "b.mp3"]),
			list(&["b.mp3"]),
			Playlist::default(),
		];
		assert_eq!(
			already_queued(&queues, &PathBuf::from("/m/b.mp3"), moving),
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
}
