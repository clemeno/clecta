//! The three queues as one thing (PLAN §7a, Q47): the lists, where each is scrolled to, and
//! which files are out being measured.
//!
//! `playlist.rs` is one list — its rows, its selection, what every edit does to that selection.
//! This is the *set* of three, and the difference is not bookkeeping: half the rules in §7a are
//! about the set rather than about a list. A track is a duplicate if any of the three holds it.
//! A file is measured once however many rows hold it. The track after this one comes from a
//! player's own cue *or* the shared list. None of those can be asked of a `Playlist`, which is
//! why `playlist.rs` had grown three free functions over `&[Playlist; 3]` — a type with no name,
//! doing a job with no home.
//!
//! It owns three fields that used to sit on `Clecta` beside each other. That is the tell: an
//! array of three lists, an array of three scroll offsets and a set of paths in flight are one
//! thing described three times, and `update` was indexing all of them by `ListId::index()` on
//! every queue arm.
//!
//! Pure, like the module under it: no iced, no filesystem. The app still owns the `Task`s, the
//! dirty flag and the drag; what moved here is everything that is only about the lists.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cache::Ready;
use crate::deck::DeckId;
use crate::playlist::{Item, Playlist};

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
	///
	/// Still public, because the *view* needs it — the three `scrollable` ids are a
	/// `[&'static str; 3]` and iced wants a `&'static str`. Nothing in `update` uses it any
	/// more: that is what `Queues` is for.
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

/// The three lists, and the two things that are true of them together rather than of any one.
#[derive(Debug, Default)]
pub struct Queues {
	lists: [Playlist; 3],
	/// How far down each panel is scrolled (PLAN §9).
	///
	/// Here rather than on `Playlist` because it is not a fact about the list — the same list
	/// drawn twice would be scrolled to two places — and here rather than on `Clecta` because a
	/// second array indexed by the same `ListId` is the shape this module exists to absorb. The
	/// files pane keeps its own offset on its own model for exactly the same reason.
	scroll: [f32; 3],
	/// Which files are out being measured this moment.
	///
	/// A row counts as unmeasured until its answer *lands*, which is long after the job that
	/// will produce it started — so without this, two quick edits would send the same file to
	/// be opened and parsed twice, and twenty would send it twenty times.
	measuring: HashSet<PathBuf>,
}

impl Queues {
	/// The three lists as they were left, in draw order.
	pub fn restored(lists: [Playlist; 3]) -> Self {
		Self {
			lists,
			..Self::default()
		}
	}

	/// One list, to read.
	pub fn get(&self, id: ListId) -> &Playlist {
		&self.lists[id.index()]
	}

	/// One list, to edit. The only door: every queue edit in the app goes through here, so
	/// `ListId::index` is this module's business and nobody else's.
	pub fn get_mut(&mut self, id: ListId) -> &mut Playlist {
		&mut self.lists[id.index()]
	}

	/// All three with their names, in draw order.
	pub fn all(&self) -> impl Iterator<Item = (ListId, &Playlist)> {
		ListId::ALL.into_iter().map(|id| (id, self.get(id)))
	}

	/// How far down a panel is scrolled, and where it has been scrolled to.
	pub fn scroll(&self, id: ListId) -> f32 {
		self.scroll[id.index()]
	}

	pub fn scrolled(&mut self, id: ListId, offset: f32) {
		self.scroll[id.index()] = offset;
	}

	/// Where this track is already queued, if it is, so the app can ask before queueing it
	/// twice (PLAN §7a).
	///
	/// **All three lists, not just the one being added to.** The mistake worth catching is a
	/// track that plays twice in an evening, and Cue 1 and Cue 2 each holding it does that
	/// exactly as surely as one list holding it twice — the duplicate is in the *set*, not in a
	/// list. Which is the sentence that says this belongs to `Queues` and not to `Playlist`.
	///
	/// `moving` is the rows that are on their way out of a list, and it is the reason this takes
	/// a parameter at all: dragging rows from one list to another, or sending them with `←` /
	/// `→`, finds them in their old home and would warn about tracks colliding with themselves.
	///
	/// Searched in draw order, so the list named is the leftmost one — an arbitrary rule, but a
	/// stable one, and the message names a list rather than counting them.
	pub fn already_queued(&self, path: &Path, moving: &[(ListId, usize)]) -> Option<ListId> {
		self.all()
			.find(|(id, list)| {
				list.items()
					.iter()
					.enumerate()
					.any(|(index, item)| item.path == path && !moving.contains(&(*id, index)))
			})
			.map(|(id, _)| id)
	}

	/// Which of a batch of tracks are already queued, and where — as positions into `items`, so
	/// a caller can filter rows and tracks together (PLAN §7a, §9a).
	///
	/// The whole of what the duplicate warning is about, and pure: the dialog only decides what
	/// to do with this answer. A batch is checked one track at a time and against the *lists*,
	/// not against itself — a selection cannot contain the same file twice, because a pane's
	/// rows are a folder's files.
	pub fn duplicates(&self, items: &[Item], moving: &[(ListId, usize)]) -> Vec<(usize, ListId)> {
		items
			.iter()
			.enumerate()
			.filter_map(|(index, item)| {
				self.already_queued(&item.path, moving)
					.map(|id| (index, id))
			})
			.collect()
	}

	/// Where the track after this one comes from, when a player's track ends (PLAN §7a).
	///
	/// **Own cue first, the shared list second.** A track deliberately cued to Player 1 outranks
	/// the pool, which is what makes the pool "whatever is free" rather than a third queue with
	/// rules of its own. `None` when neither has anything to offer, and the player simply stops.
	///
	/// A list with `auto_load` off has nothing to offer however full it is: it is a shelf the
	/// user takes from by hand. It is *skipped*, not blocking — a cue switched off still lets
	/// the shared list feed the player, because the switch belongs to one list and says nothing
	/// about the other.
	pub fn next_source(&self, id: DeckId) -> Option<ListId> {
		let cue = ListId::Cue(id);
		if self.get(cue).auto_load && !self.get(cue).is_empty() {
			Some(cue)
		} else if self.get(ListId::Common).auto_load && !self.get(ListId::Common).is_empty() {
			Some(ListId::Common)
		} else {
			None
		}
	}

	/// Which tracks still need their length looked up — and count them as out from now on
	/// (PLAN §7a).
	///
	/// `take_` rather than `to_`, because it is not a view of the lists: the same call twice in a
	/// row gives the batch and then nothing, which is the whole point of it.
	///
	/// Asking and recording are one operation on purpose: they were two lines beside each other
	/// in `update`, and the day they stop being beside each other is the day a file goes out
	/// twice. The set is subtracted here rather than by the caller for the same reason.
	///
	/// Deduplicated across the three lists as well, because the same track may sit in two of
	/// them and one answer settles both rows. Returned in draw order, which is arbitrary but
	/// stable.
	pub fn take_unmeasured(&mut self) -> Vec<PathBuf> {
		let mut wanted: Vec<PathBuf> = Vec::new();

		for path in self.lists.iter().flat_map(Playlist::unmeasured) {
			// A `Vec` for the batch, against the `HashSet` for what is in flight: this one is
			// built once per edit and is tens of entries, and the linear scan keeps the order.
			if !self.measuring.contains(path) && !wanted.iter().any(|seen| seen == path) {
				wanted.push(path.to_path_buf());
			}
		}

		self.measuring.extend(wanted.iter().cloned());
		wanted
	}

	/// Record what a job worked out, against every row holding this path, and stop counting the
	/// file as out (PLAN §7a, §14c, §14d).
	///
	/// By path rather than by index, because the lists can be edited while the measuring runs
	/// and an index would name a different track by the time the answer came back. A queue may
	/// hold the same track twice, so one answer settles both rows.
	pub fn measured(&mut self, path: &Path, length: Option<Duration>, ready: Option<Ready>) {
		self.measuring.remove(path);
		for list in &mut self.lists {
			list.measured(path, length, ready);
		}
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

	fn three(one: &[&str], common: &[&str], two: &[&str]) -> Queues {
		Queues::restored([list(one), list(common), list(two)])
	}

	fn path(name: &str) -> PathBuf {
		PathBuf::from("/m").join(name)
	}

	#[test]
	fn the_arrows_only_reach_a_neighbour() {
		// Arrange / Act / Assert: the middle list is the only way across, so a track cannot
		// jump from one player's cue to the other's in one press.
		let (one, two) = (ListId::Cue(DeckId::One), ListId::Cue(DeckId::Two));

		assert_eq!(one.neighbour(true), Some(ListId::Common));
		assert_eq!(one.neighbour(false), None, "nothing left of Cue 1");
		assert_eq!(ListId::Common.neighbour(true), Some(two));
		assert_eq!(ListId::Common.neighbour(false), Some(one));
		assert_eq!(two.neighbour(true), None, "nothing right of Cue 2");
	}

	#[test]
	fn a_player_takes_from_its_own_cue_before_the_shared_list() {
		// Arrange: both lists have something to offer.
		let both = three(&["a.mp3"], &["b.mp3"], &[]);

		// Act / Assert: the cue wins, which is what makes the shared list a pool rather than
		// a third queue with rules of its own.
		assert_eq!(
			both.next_source(DeckId::One),
			Some(ListId::Cue(DeckId::One))
		);

		// An empty cue falls through to the shared list; both empty is a player that stops.
		let shared = three(&[], &["b.mp3"], &[]);
		assert_eq!(shared.next_source(DeckId::One), Some(ListId::Common));
		assert_eq!(three(&[], &[], &[]).next_source(DeckId::One), None);

		// And the other player's cue is not this player's business.
		assert_eq!(three(&[], &[], &["c.mp3"]).next_source(DeckId::One), None);
	}

	#[test]
	fn a_list_that_hands_nothing_over_is_skipped_rather_than_blocking() {
		// Arrange: a full cue with Auto-load off, and a shared list with it on (PLAN §7a).
		let mut queues = three(&["a.mp3"], &["b.mp3"], &[]);
		queues.get_mut(ListId::Cue(DeckId::One)).auto_load = false;

		// Act / Assert: the cue is passed over rather than treated as an empty list that ends
		// the handover — a cue switched off still lets the shared list feed that player.
		assert_eq!(queues.next_source(DeckId::One), Some(ListId::Common));

		// Both off is the player stopping with full lists in front of it, which is what the
		// switches are for.
		queues.get_mut(ListId::Common).auto_load = false;
		assert_eq!(queues.next_source(DeckId::One), None);
	}

	#[test]
	fn a_track_is_found_wherever_it_is_already_queued() {
		// Arrange: one track in the shared list, one in the far cue, one nowhere.
		let queues = three(&[], &["a.mp3"], &["b.mp3"]);

		// Act / Assert: all three lists are searched, not just the one being added to — the
		// duplicate is in the set (PLAN §7a).
		assert_eq!(
			queues.already_queued(&path("a.mp3"), &[]),
			Some(ListId::Common)
		);
		assert_eq!(
			queues.already_queued(&path("b.mp3"), &[]),
			Some(ListId::Cue(DeckId::Two))
		);
		assert_eq!(queues.already_queued(&path("c.mp3"), &[]), None);
	}

	#[test]
	fn a_row_on_its_way_out_of_a_list_is_not_its_own_duplicate() {
		// Arrange: the same track in two lists, and the shared list's copy is being moved.
		let queues = three(&[], &["a.mp3"], &["a.mp3"]);
		let moving = [(ListId::Common, 0)];

		// Act / Assert: it must not warn about colliding with itself, or every cross-list move
		// would ask a question with one honest answer...
		assert_eq!(
			queues.already_queued(&path("a.mp3"), &moving),
			Some(ListId::Cue(DeckId::Two)),
			"the other copy is still a duplicate"
		);

		// ...and the exception is the *row*, not the track: with the far copy gone too there is
		// nothing left to collide with.
		let only = three(&[], &["a.mp3"], &[]);
		assert_eq!(only.already_queued(&path("a.mp3"), &moving), None);
	}

	#[test]
	fn a_batch_says_which_of_its_rows_are_already_queued() {
		// Arrange: four tracks going in, two of which are somewhere already.
		let queues = three(&["b.mp3"], &[], &["d.mp3"]);
		let batch: Vec<Item> = ["a.mp3", "b.mp3", "c.mp3", "d.mp3"]
			.iter()
			.map(|name| Item::new(path(name)))
			.collect();

		// Act / Assert: positions into the batch, so the caller can filter rows and tracks by
		// the same answer (PLAN §9a).
		assert_eq!(
			queues.duplicates(&batch, &[]),
			vec![(1, ListId::Cue(DeckId::One)), (3, ListId::Cue(DeckId::Two))]
		);
	}

	#[test]
	fn nothing_is_sent_to_be_measured_twice() {
		// Arrange: the same track in two lists, plus one of its own, and nothing measured.
		let mut queues = three(&["a.mp3", "b.mp3"], &["a.mp3"], &[]);

		// Act / Assert: one entry per *file*, not per row — one answer settles every row
		// holding it, so asking twice would be opening the file twice for one number.
		assert_eq!(queues.take_unmeasured(), vec![path("a.mp3"), path("b.mp3")]);

		// And asking again while those are still out gives nothing: the recording is part of
		// the asking, which is what stops twenty quick edits sending one file twenty times.
		assert!(queues.take_unmeasured().is_empty(), "still in flight");
	}

	#[test]
	fn a_measured_track_is_never_asked_about_again() {
		// Arrange: two rows out being measured, one of which answers with *nothing* — a stream,
		// or a file that would not open.
		let mut queues = three(&["a.mp3", "b.mp3"], &[], &[]);
		let _ = queues.take_unmeasured();

		// Act: both answer, which also takes them out of flight.
		queues.measured(&path("a.mp3"), Some(Duration::from_secs(10)), None);
		queues.measured(&path("b.mp3"), None, None);

		// Assert: neither is asked about again, which is what stops the app re-opening an
		// unreadable file on every edit for the rest of the run.
		assert!(queues.take_unmeasured().is_empty());
	}

	#[test]
	fn one_answer_settles_every_row_holding_that_track() {
		// Arrange: the same track queued twice in one list and once in another.
		let mut queues = three(&["a.mp3", "a.mp3"], &["a.mp3"], &[]);

		// Act
		queues.measured(&path("a.mp3"), Some(Duration::from_secs(215)), None);

		// Assert: by path, not by index — the lists can be edited while a job runs, and one
		// file's length is one file's length however many rows are showing it.
		for (_, list) in queues.all() {
			for item in list.items() {
				assert_eq!(item.duration, Some(Some(Duration::from_secs(215))));
			}
		}
	}

	#[test]
	fn a_panel_remembers_where_it_was_scrolled_to_on_its_own() {
		// Arrange / Act: three panels, one offset each.
		let mut queues = three(&[], &[], &[]);
		queues.scrolled(ListId::Common, 120.0);

		// Assert: three lists means three offsets, or scrolling one would scroll all of them.
		assert_eq!(queues.scroll(ListId::Common), 120.0);
		assert_eq!(queues.scroll(ListId::Cue(DeckId::One)), 0.0);
		assert_eq!(queues.scroll(ListId::Cue(DeckId::Two)), 0.0);
	}
}
