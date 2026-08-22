//! The three queues (PLAN §7a): one in front of each player, and one shared between them.
//!
//! Pure, like `deck.rs` and for the same reason — every rule here is an edit to a queue, and
//! an edit to a queue is exactly the kind of thing that is wrong by one and looks right. So
//! the whole module is `Vec` arithmetic with no iced and no filesystem, and the interesting
//! part is not the moving but **what happens to the selection when the queue moves under
//! it**: a row that stays highlighted while a different track slides beneath it is worse
//! than no highlight at all.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cache::Ready;
use crate::select::{self, Click};
use crate::waveform::Trim;

/// When one track gives way to the next (PLAN §7b).
///
/// Two positions, and the difference between them is entirely about the silence a file
/// carries at either end: the encoder's padding, the engineer's run-out, the two seconds of
/// room tone somebody left on the master. `Whole` waits for the file; `Trimmed` waits for the
/// *music*, and starts the next one where its own music starts.
///
/// `Serialize`/`Deserialize` because it is persisted per queue (PLAN §11), and `Display`
/// because it is drawn in a `pick_list` — the same pair `mixer::Curve` needs for the same
/// reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Handover {
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

impl Handover {
	pub const ALL: [Handover; 2] = [Handover::Whole, Handover::Trimmed];

	/// Whether the track playing now should give way already (PLAN §7b).
	///
	/// Three things have to be true at once, and each of them is a reason not to cut: the queue
	/// that would supply the next track has to be set to skip the blanks, this track's edges have
	/// to have been scanned, and the playhead has to have reached the second one. A track nobody
	/// has scanned plays whole — silently, because the alternative is a notice line every four
	/// minutes about a setting the user already knows they turned on.
	///
	/// The caller has already asked `next_source` whether there *is* a next track. That is the
	/// fourth condition and it is deliberately not here: cutting the run-out off the last track of
	/// the evening would leave a player stopped early for no benefit at all.
	pub fn hands_over_early(self, position: Duration, trim: Option<Trim>) -> bool {
		self == Handover::Trimmed && trim.is_some_and(|trim| position >= trim.end)
	}

	/// Where the track this queue hands over starts (PLAN §7b) — the same setting answering its
	/// other half: a queue that skips the blanks at the end of one track skips them at the start
	/// of the next.
	///
	/// `None` means "leave the playhead where the load put it", which is the top of the file —
	/// the answer for a queue that plays files whole, and also for a track nobody has scanned,
	/// which plays from the top whatever the setting says (PLAN §14c).
	pub fn starts_at(self, trim: Option<Trim>) -> Option<Duration> {
		match self {
			Handover::Whole => None,
			Handover::Trimmed => trim.map(|trim| trim.start),
		}
	}
}

impl std::fmt::Display for Handover {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		formatter.write_str(match self {
			Handover::Whole => "Whole track",
			Handover::Trimmed => "Skip blanks",
		})
	}
}

/// One queued track: a path, the name the view draws every frame, and how long it is.
///
/// The name is cached for the same reason `deck::Track` caches it — `Path::file_name`
/// returns an `OsStr` that would be re-converted on every row of every frame.
///
/// `PartialEq` and no longer `Eq`, because a tempo is an `f32` and `f32` has a value that is not
/// equal to itself. Nothing here needs the stronger promise — the comparisons are `assert_eq!` in
/// tests and "is this row the one that was dragged" — and the alternative was storing hundredths
/// of a beat as an integer to keep a trait nobody asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
	pub path: PathBuf,
	pub name: String,
	/// Two questions in one field, and both have to be asked: `None` means *not measured
	/// yet*, and `Some(None)` means measured and the decoder could give no length — a
	/// stream, or a file that no longer opens (PLAN §7a). Collapsing them into one `None`
	/// would make the app re-open an unreadable file for ever.
	pub duration: Option<Option<Duration>>,
	/// What a full scan of this file found — the music's edges and the tempo (PLAN §14c, §14d) —
	/// when anything has scanned it.
	///
	/// One field and one layer, where `duration` has two. Both of those are deliberate. **One
	/// field** because the two facts inside it come out of one decode and are stored under one
	/// rule (Q44), so a row with a playing time and no tempo is not a state anything can produce
	/// — and this is the same value the files pane holds per row, so both panes now answer the
	/// question with the same expression. **One layer** because a length is a header parse that
	/// every queue edit pays for, so *not measured* and *no length* are worth telling apart,
	/// while a scan is only ever **read** here (`cached_facts`) — so `None` means nothing more
	/// than nobody has scanned this file yet.
	pub ready: Option<Ready>,
}

impl Item {
	pub fn new(path: PathBuf) -> Self {
		let name = crate::fsio::name_of(&path);
		Self {
			path,
			name,
			duration: None,
			ready: None,
		}
	}
}

/// One queue, and which of its rows is selected.
///
/// Selected by **index**, not by path, which is the opposite of the files pane (`browser.rs`)
/// and deliberately so: a queue may hold the same track twice — playing something twice in a
/// set is a thing people do — so a path does not name a row. The price is that every edit has
/// to carry the selection with it, which is what most of this module is.
#[derive(Debug, Clone, PartialEq)]
pub struct Queue {
	items: Vec<Item>,
	/// Which rows are selected (PLAN §9a). A `BTreeSet` because every action on them runs
	/// **top to bottom**, and a sorted set is that order without anything having to remember
	/// it — which matters most in the two places the order is load-bearing: the tracks handed
	/// to a player, and a block of rows moved by a drag.
	selected: BTreeSet<usize>,
	/// Where a Shift-click measures from. Kept as an index like the selection, and dropped
	/// whenever the row it named stops being selected.
	anchor: Option<usize>,
	/// Whether this queue hands its top track to a player that has just run out (PLAN §7a),
	/// and whether that track then starts by itself.
	///
	/// Two switches rather than one three-way setting, because they answer two questions and
	/// the middle position is the useful one: a queue that loads without playing is the app's
	/// own default, and it is neither of the extremes a single toggle would offer.
	///
	/// Per *queue* rather than per player, which is what makes them worth having at all — Cue 1
	/// can run the evening by itself while the shared pool sits there as a manual shelf, and
	/// that is one setting each rather than a mode the whole app is in.
	pub auto_load: bool,
	pub auto_play: bool,
	/// When the handover happens, and where the track it hands over starts (PLAN §7b).
	///
	/// The third setting on the same queue and read at the same moment as the other two, which
	/// is what keeps it one question rather than two: *this* queue decides when its own track
	/// takes over, so Cue 1 can run an evening back to back while **Next up** stays a shelf
	/// that plays whatever it is handed, whole.
	pub handover: Handover,
}

impl Default for Queue {
	fn default() -> Self {
		Self {
			items: Vec::new(),
			selected: BTreeSet::new(),
			anchor: None,
			// On: a queue is a queue of what plays next, and one that had to be switched on
			// before it did anything would be a queue that quietly did nothing.
			auto_load: true,
			// Off, for the same reason every load lands on `Stopped` (PLAN §7): on a mixer,
			// audio nobody asked for is a mistake that cannot be taken back. Someone who wants
			// the evening to run itself says so once, per queue.
			auto_play: false,
			handover: Handover::default(),
		}
	}
}

impl Queue {
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

	/// Whether this index still names this exact track — the question a delayed click has to
	/// ask, because a handover can take the top row while a timer runs, and an index alone
	/// cannot see that: after the shift it stays in range and names the row *below* the one
	/// it meant (Q50).
	pub fn holds(&self, index: usize, path: &Path) -> bool {
		self.items.get(index).is_some_and(|item| item.path == path)
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
	/// enabled by: a block already touching the top of the queue cannot go up.
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

	/// How long everything in the queue is, and whether that is the whole truth.
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
	/// By path rather than by index, because the queues can be edited while the measuring runs
	/// and an index would name a different track by the time the answer came back. A queue may
	/// hold the same track twice, so one answer settles both rows.
	///
	/// The two parts are applied under **two different rules**, which is why they are not one
	/// `if`. A length is settled once and kept, `None` included, or an unreadable file would be
	/// re-opened on every edit for the rest of the run. A scan is only ever read from the store,
	/// so a job that has none is a job that did not look deep enough — and a row measured long
	/// ago still has to learn it the moment a folder scan works it out. Filtering it on
	/// `duration.is_none()` would have meant exactly that row never learning it at all.
	pub fn measured(&mut self, path: &Path, length: Option<Duration>, ready: Option<Ready>) {
		for item in self.items.iter_mut().filter(|item| item.path == path) {
			if item.duration.is_none() {
				item.duration = Some(length);
			}
			if ready.is_some() {
				item.ready = ready;
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

	/// Re-number the selection after the queue has moved under it.
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
	/// that leaves the queue.
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

#[cfg(test)]
mod tests {
	use super::*;

	fn filled(names: &[&str]) -> Queue {
		Queue::from_paths(
			names
				.iter()
				.map(|name| PathBuf::from("/m").join(name))
				.collect(),
		)
	}

	#[test]
	fn an_index_only_holds_the_track_that_is_still_under_it() {
		// Arrange / Act / Assert: the answer is about the exact track, so a queue that
		// shifted under a stale index reads as "gone" rather than as the row that slid up
		// into its place — which is what keeps a delayed click off the wrong row (Q50).
		let queue = filled(&["a", "b", "c"]);
		assert!(queue.holds(1, &PathBuf::from("/m").join("b")));
		assert!(
			!queue.holds(1, &PathBuf::from("/m").join("a")),
			"the row above, as if the top had been handed over"
		);
		assert!(
			!queue.holds(9, &PathBuf::from("/m").join("a")),
			"out of range"
		);
	}

	fn names(queue: &Queue) -> Vec<&str> {
		queue
			.items()
			.iter()
			.map(|item| item.name.as_str())
			.collect()
	}

	/// Put tracks into a queue at a position, the way every caller does.
	fn add(queue: &mut Queue, index: usize, names: &[&str]) {
		let items = names
			.iter()
			.map(|name| Item::new(PathBuf::from("/m").join(name)))
			.collect();
		queue.insert_many(index, items);
	}

	/// The selected rows, as indices.
	fn chosen(queue: &Queue) -> Vec<usize> {
		queue.selection().collect()
	}

	#[test]
	fn adding_puts_tracks_where_they_were_asked_to_go() {
		// Arrange
		let mut queue = filled(&["c.mp3", "d.mp3"]);

		// Act: at the end, then at the top — and a whole batch at once, which is what a
		// selection dropped in is (PLAN §9a).
		add(&mut queue, 2, &["e.mp3"]);
		add(&mut queue, 0, &["a.mp3", "b.mp3"]);

		// Assert: the batch keeps its own order, top to bottom, rather than arriving reversed
		// — which is what inserting each one at the same index would do.
		assert_eq!(names(&queue), ["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"]);
	}

	#[test]
	fn an_insert_above_the_selection_carries_it_down() {
		// Arrange: the third row selected.
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3"]);
		queue.click(2, Click::Replace);

		// Act: something arrives at the top.
		add(&mut queue, 0, &["new.mp3"]);

		// Assert: the highlight is still on `c.mp3`, which is now row 3. A selection that
		// stayed on index 2 would be highlighting `b.mp3` — the same row, a different track.
		assert_eq!(chosen(&queue), [3]);
		assert_eq!(names(&queue)[3], "c.mp3");

		// An insert *below* it moves nothing.
		add(&mut queue, 4, &["last.mp3"]);
		assert_eq!(chosen(&queue), [3]);

		// And a batch above it carries the highlight by as many rows as arrived.
		add(&mut queue, 0, &["one.mp3", "two.mp3"]);
		assert_eq!(chosen(&queue), [5]);
		assert_eq!(names(&queue)[5], "c.mp3");
	}

	#[test]
	fn removing_the_selected_rows_selects_what_slid_into_their_place() {
		// Arrange: two rows selected, with more below them.
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);
		queue.click(1, Click::Replace);
		queue.click(2, Click::Range);

		// Act
		let removed = queue.take_selected();

		// Assert: they come back in the order they were in, and the highlight lands on the
		// next track — so pressing `✕` repeatedly removes consecutive rows rather than
		// requiring a re-aim after each one.
		let taken: Vec<&str> = removed.iter().map(|item| item.name.as_str()).collect();
		assert_eq!(taken, ["b.mp3", "c.mp3"]);
		assert_eq!(chosen(&queue), [1]);
		assert_eq!(names(&queue)[1], "d.mp3");
	}

	#[test]
	fn removing_the_last_row_falls_back_to_the_new_last_row() {
		// Arrange: the bottom row selected, with nothing below to slide up.
		let mut queue = filled(&["a.mp3", "b.mp3"]);
		queue.click(1, Click::Replace);

		// Act / Assert
		queue.take_selected();
		assert_eq!(chosen(&queue), [0], "the row above takes the highlight");

		queue.take_selected();
		assert!(chosen(&queue).is_empty(), "an empty queue selects nothing");
		assert!(queue.is_empty());
	}

	#[test]
	fn removing_a_row_above_the_selection_keeps_the_same_track_selected() {
		// Arrange
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3"]);
		queue.click(2, Click::Replace);

		// Act: a row above the selection goes — a handover taking the top track, which must
		// not move a highlight the user put somewhere else.
		queue.remove(0);

		// Assert: still `c.mp3`, one row higher.
		assert_eq!(chosen(&queue), [1]);
		assert_eq!(names(&queue)[1], "c.mp3");
	}

	#[test]
	fn a_press_selects_one_row_or_several_depending_on_what_is_held() {
		// Arrange
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);

		// Act / Assert: the same three-way rule the files pane follows (PLAN §9a), on indices.
		queue.click(1, Click::Replace);
		assert_eq!(chosen(&queue), [1]);

		queue.click(3, Click::Toggle);
		assert_eq!(chosen(&queue), [1, 3], "in row order, not click order");

		queue.click(1, Click::Toggle);
		assert_eq!(chosen(&queue), [3], "clicked again, and gone");

		// A range measures from the anchor, which the toggle above moved to row 1.
		queue.click(1, Click::Replace);
		queue.click(3, Click::Range);
		assert_eq!(chosen(&queue), [1, 2, 3]);

		// A row that is not there selects nothing rather than a phantom index — the guard the
		// rest of this module assumes.
		let mut short = queue_of_one();
		short.click(5, Click::Replace);
		assert!(chosen(&short).is_empty());
	}

	fn queue_of_one() -> Queue {
		filled(&["a.mp3"])
	}

	#[test]
	fn taking_the_next_track_takes_the_top_one() {
		// Arrange
		let mut queue = filled(&["a.mp3", "b.mp3"]);

		// Act / Assert
		assert_eq!(
			queue.take_next().map(|item| item.name),
			Some("a.mp3".into())
		);
		assert_eq!(names(&queue), ["b.mp3"]);
		assert_eq!(
			queue.take_next().map(|item| item.name),
			Some("b.mp3".into())
		);
		assert_eq!(queue.take_next(), None, "an empty queue stops the player");
	}

	#[test]
	fn shifting_rows_takes_their_highlight_with_them() {
		// Arrange
		let mut single = filled(&["a.mp3", "b.mp3", "c.mp3"]);
		single.click(2, Click::Replace);

		// Act
		assert!(single.shift_selected(true));

		// Assert: the track moved and the highlight went with it. This is the one a
		// swap-only implementation gets wrong.
		assert_eq!(names(&single), ["a.mp3", "c.mp3", "b.mp3"]);
		assert_eq!(chosen(&single), [1]);

		// And a block of two moves as a block, keeping its own order — swapping them one at a
		// time in the wrong direction would reverse the pair as it went.
		let mut block = filled(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);
		block.click(0, Click::Replace);
		block.click(1, Click::Range);
		assert!(block.shift_selected(false));
		assert_eq!(names(&block), ["c.mp3", "a.mp3", "b.mp3", "d.mp3"]);
		assert_eq!(chosen(&block), [1, 2]);
	}

	#[test]
	fn a_scattered_selection_still_moves_together() {
		// Arrange: rows 0 and 2 of four, which is what a pair of command-clicks makes.
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);
		queue.click(0, Click::Replace);
		queue.click(2, Click::Toggle);

		// Act
		assert!(queue.shift_selected(false));

		// Assert: each moved down one, past the row that was below it.
		assert_eq!(names(&queue), ["b.mp3", "a.mp3", "d.mp3", "c.mp3"]);
		assert_eq!(chosen(&queue), [1, 3]);
	}

	#[test]
	fn a_selection_cannot_be_shifted_off_either_end() {
		// Arrange / Act / Assert: `false` rather than a no-op, so the button is drawn dead
		// instead of offering a press that does nothing.
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3"]);
		assert!(!queue.shift_selected(true), "nothing selected");

		queue.click(0, Click::Replace);
		assert!(!queue.shift_selected(true), "up from the top");

		queue.click(2, Click::Replace);
		assert!(!queue.shift_selected(false), "down from the bottom");

		// The whole move is blocked, not part of it: a selection touching the top cannot go up
		// even though the rows below it could. Moving some of what was asked for and not the
		// rest is not undone by pressing the other button.
		queue.click(0, Click::Replace);
		queue.click(2, Click::Toggle);
		assert!(!queue.shift_selected(true), "one of them is at the top");
		assert_eq!(names(&queue), ["a.mp3", "b.mp3", "c.mp3"], "nothing moved");
	}

	#[test]
	fn a_drag_downwards_lands_where_the_caret_was() {
		// Arrange: the caret between `c` and `d` is index 3, and the row being dragged is
		// above it — so lifting the row out shifts the caret up with everything else. This is
		// the case that is wrong by one in every implementation that forgets it.
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);

		// Act: `a` to just above `d`.
		assert!(queue.relocate(&[0], 3));

		// Assert: `a` sits between `c` and `d`, not after `d`.
		assert_eq!(names(&queue), ["b.mp3", "c.mp3", "a.mp3", "d.mp3"]);
		assert_eq!(chosen(&queue), [2], "the dragged row stays selected");
	}

	#[test]
	fn a_drag_upwards_needs_no_adjustment() {
		// Arrange / Act: the mirror of the case above — the caret is above the row, so
		// nothing has shifted under it.
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);
		assert!(queue.relocate(&[3], 1));

		// Assert
		assert_eq!(names(&queue), ["a.mp3", "d.mp3", "b.mp3", "c.mp3"]);
		assert_eq!(chosen(&queue), [1]);
	}

	#[test]
	fn a_drag_to_the_very_end_puts_the_row_last() {
		// Arrange / Act: `len` is the caret past the last row, which is the one index that
		// does not name a row at all.
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3"]);
		assert!(queue.relocate(&[0], 3));

		// Assert
		assert_eq!(names(&queue), ["b.mp3", "c.mp3", "a.mp3"]);
		assert_eq!(chosen(&queue), [2]);
	}

	#[test]
	fn a_drag_that_lands_where_it_started_changes_nothing() {
		// Arrange: the two carets that touch a row — its own top edge, and its bottom one.
		// Both mean "leave it alone", and both are easy to reach with a twitchy hand.
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3"]);

		// Act / Assert
		assert!(!queue.relocate(&[1], 1), "the caret above itself");
		assert!(!queue.relocate(&[1], 2), "the caret below itself");
		assert!(!queue.relocate(&[9], 0), "a row that is not there");
		assert!(
			!queue.relocate(&[0], 9),
			"a caret past the end of the queue"
		);
		assert!(!queue.relocate(&[], 0), "nothing being dragged");
		assert_eq!(names(&queue), ["a.mp3", "b.mp3", "c.mp3"], "nothing moved");

		// And the same for a *block*: the carets either side of it, and every caret inside it,
		// leave it exactly where it is (PLAN §9a).
		assert!(!queue.relocate(&[0, 1], 0), "above the block");
		assert!(!queue.relocate(&[0, 1], 2), "below the block");
		assert_eq!(names(&queue), ["a.mp3", "b.mp3", "c.mp3"], "still nothing");
	}

	#[test]
	fn a_block_of_rows_lands_where_the_caret_was_and_keeps_its_order() {
		// Arrange: two rows either side of a third, which is the case that shows whether the
		// block is moved as a block or one row at a time.
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"]);

		// Act: `a` and `c` to just above `e`. Two rows lie above that caret, so the block
		// lands two places earlier than the caret's index says.
		assert!(queue.relocate(&[0, 2], 4));

		// Assert: in their own order, together, and selected where they landed.
		assert_eq!(names(&queue), ["b.mp3", "d.mp3", "a.mp3", "c.mp3", "e.mp3"]);
		assert_eq!(chosen(&queue), [2, 3]);
	}

	#[test]
	fn every_drag_within_a_list_keeps_every_track() {
		// Arrange: exhaustive, because a reorder that loses or duplicates a row is the one
		// failure a queue cannot survive — and there are only 25 cases.
		let original = ["a.mp3", "b.mp3", "c.mp3", "d.mp3"];

		for from in 0..original.len() {
			for to in 0..=original.len() {
				// Act
				let mut queue = filled(&original);
				queue.relocate(&[from], to);

				// Assert: same set, same length, whatever the move did.
				let mut got = names(&queue);
				got.sort_unstable();
				assert_eq!(got, original, "relocate({from}, {to}) changed the contents");
			}
		}

		// And every *pair* of rows to every caret, which is the same failure with a second way
		// to reach it: 30 more cases, each also checking that both rows end up selected.
		for first in 0..original.len() {
			for second in first + 1..original.len() {
				for to in 0..=original.len() {
					let mut queue = filled(&original);
					let moved = queue.relocate(&[first, second], to);

					let mut got = names(&queue);
					got.sort_unstable();
					assert_eq!(
						got, original,
						"relocate([{first}, {second}], {to}) lost a row"
					);

					// A move that happened leaves both rows highlighted where they landed; one
					// that did not leaves the selection exactly as it was, which here is
					// nothing at all.
					assert_eq!(
						chosen(&queue).len(),
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
		// (PLAN §7a) — so what leaves the queue is a subset of what is highlighted.
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3", "d.mp3"]);
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
		let mut twice = filled(&["a.mp3", "b.mp3"]);
		assert_eq!(twice.take_rows(&[0, 0]).len(), 1);
		assert_eq!(names(&twice), ["b.mp3"]);
	}

	#[test]
	fn a_running_time_admits_what_it_has_not_counted() {
		// Arrange: three tracks, none of them measured yet.
		let mut queue = filled(&["a.mp3", "b.mp3", "c.mp3"]);
		assert_eq!(
			queue.total(),
			(Duration::ZERO, false),
			"nothing measured is not a queue of length zero"
		);

		// Act: two answers arrive, and one of them is "this file has no length".
		queue.measured(
			&PathBuf::from("/m/a.mp3"),
			Some(Duration::from_secs(90)),
			None,
		);
		queue.measured(&PathBuf::from("/m/b.mp3"), None, None);

		// Assert: the known lengths add up, and the total still says it is not the whole
		// story — which is what the `+` in the footer is.
		assert_eq!(queue.total(), (Duration::from_secs(90), false));

		// Act / Assert: everything has now been *asked*, and the answer is still not whole —
		// a row nobody can measure keeps the `+` for ever, which is the honest outcome. The
		// two states differ in what the app does next, not in what the footer says: one is
		// waiting for a thread and the other has stopped asking.
		queue.measured(
			&PathBuf::from("/m/c.mp3"),
			Some(Duration::from_secs(30)),
			None,
		);
		assert_eq!(queue.total(), (Duration::from_secs(120), false));
		assert_eq!(queue.unmeasured().count(), 0, "nothing left to ask about");

		// A queue with no unknowns in it is exact, and so is an empty one.
		queue.remove(1);
		assert_eq!(queue.total(), (Duration::from_secs(120), true));
		assert_eq!(Queue::default().total(), (Duration::ZERO, true));
	}

	#[test]
	fn one_answer_settles_every_row_holding_that_track() {
		// Arrange: the same track queued twice, which is the reason `measured` works by path
		// and the reason a row is selected by index.
		let mut queue = filled(&["a.mp3", "b.mp3", "a.mp3"]);

		// Act
		queue.measured(
			&PathBuf::from("/m/a.mp3"),
			Some(Duration::from_secs(60)),
			None,
		);

		// Assert: both copies, and nothing else.
		assert_eq!(
			queue.items()[0].duration,
			Some(Some(Duration::from_secs(60)))
		);
		assert_eq!(
			queue.items()[2].duration,
			Some(Some(Duration::from_secs(60)))
		);
		assert_eq!(queue.items()[1].duration, None, "a different track");

		// And an answer that failed is remembered as an answer, or the app would re-open an
		// unreadable file every time anything else was added.
		queue.measured(&PathBuf::from("/m/b.mp3"), None, None);
		assert_eq!(queue.items()[1].duration, Some(None));
		assert_eq!(
			queue.unmeasured().count(),
			0,
			"nothing is still waiting to be measured"
		);
	}

	#[test]
	fn a_playing_time_and_a_tempo_arrive_after_the_length_and_never_unlearn_themselves() {
		// Arrange: a row measured the moment it was queued, which is every row — a length is a
		// header parse and the queues pay for it on every edit.
		let mut queue = filled(&["a.mp3"]);
		queue.measured(
			&PathBuf::from("/m/a.mp3"),
			Some(Duration::from_secs(215)),
			None,
		);
		assert_eq!(queue.items()[0].ready, None, "nothing has scanned it");

		// Act: the folder scan reaches it, long after. The length is settled and stays settled;
		// the scan has to land anyway, or a row measured once could never learn it at all.
		let scanned = Ready {
			trim: Some(Trim {
				start: Duration::from_secs(2),
				end: Duration::from_secs(200),
			}),
			tempo: Some(128.5),
		};
		queue.measured(
			&PathBuf::from("/m/a.mp3"),
			Some(Duration::from_secs(1)),
			Some(scanned),
		);
		assert_eq!(
			queue.items()[0].duration,
			Some(Some(Duration::from_secs(215))),
			"the first answer stands"
		);
		assert_eq!(queue.items()[0].ready, Some(scanned));
		assert_eq!(
			queue.items()[0].ready.and_then(Ready::music),
			Some(Duration::from_secs(198))
		);

		// Act / Assert: and a later queue edit, which only ever *reads* the store, must not take
		// it away again — the same rule `App::remember` follows.
		queue.measured(&PathBuf::from("/m/a.mp3"), None, None);
		assert_eq!(
			queue.items()[0].ready,
			Some(scanned),
			"a job that did not look deep enough says nothing, not nothing-there"
		);
	}

	#[test]
	fn a_fresh_list_hands_over_and_does_not_start_anything() {
		// Arrange / Act / Assert: the defaults are what the app did before there were switches
		// at all, which is the only defensible thing for a switch to default to. What happens
		// when one of them is *off* is a question about the set of three, and is asked there.
		assert!(Queue::default().auto_load, "on unless it is turned off");
		assert!(!Queue::default().auto_play, "loading is not playing");
	}

	#[test]
	fn a_track_gives_way_early_only_when_all_three_things_are_true() {
		// Arrange: a four-minute file whose music stops twelve seconds before it does.
		let trim = Some(Trim {
			start: Duration::from_secs(2),
			end: Duration::from_secs(228),
		});
		let early = |handover: Handover, seconds| {
			handover.hands_over_early(Duration::from_secs(seconds), trim)
		};

		// Act / Assert: the run-out is what gets skipped, and only for a queue that asked.
		assert!(!early(Handover::Trimmed, 227), "still playing music");
		assert!(early(Handover::Trimmed, 228), "the music has stopped");
		assert!(early(Handover::Trimmed, 239), "seeked into the run-out");
		assert!(!early(Handover::Whole, 239), "this queue plays files whole");

		// A track nobody has scanned plays whole, whatever the setting says: there is no
		// second edge to reach, so there is nothing to cut to (PLAN §14c).
		assert!(
			!Handover::Trimmed.hands_over_early(Duration::from_secs(600), None),
			"never scanned"
		);

		// And a fresh queue plays whole, which is what the app did before the setting existed.
		assert_eq!(Queue::default().handover, Handover::Whole);
	}

	#[test]
	fn where_the_next_track_starts_is_the_same_setting_answering_again() {
		// Arrange: a track whose music starts two seconds in.
		let trim = Some(Trim {
			start: Duration::from_secs(2),
			end: Duration::from_secs(200),
		});

		// Act / Assert: a queue that skips the blanks starts the next track where its music
		// starts; one that plays files whole leaves the playhead at the top — and so does a
		// track nobody has scanned, whatever the setting says, because there is no edge to
		// start at (PLAN §14c).
		assert_eq!(
			Handover::Trimmed.starts_at(trim),
			Some(Duration::from_secs(2))
		);
		assert_eq!(Handover::Whole.starts_at(trim), None, "files play whole");
		assert_eq!(Handover::Trimmed.starts_at(None), None, "never scanned");
	}
}
