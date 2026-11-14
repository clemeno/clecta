//! The three queues as one thing (PLAN §7a, Q47): the queues, where each is scrolled to, and
//! which files are out being measured.
//!
//! `queue.rs` is one queue — its rows, its selection, what every edit does to that selection.
//! This is the *set* of three, and the difference is not bookkeeping: half the rules in §7a are
//! about the set rather than about a queue. A track is a duplicate if any of the three holds it.
//! A file is measured once however many rows hold it. The track after this one comes from a
//! player's own cue *or* the shared queue. None of those can be asked of a `Queue`, which is
//! why `queue.rs` had grown three free functions over `&[Queue; 3]` — a type with no name,
//! doing a job with no home.
//!
//! It owns three fields that used to sit on `Clecta` beside each other. That is the tell: an
//! array of three queues, an array of three scroll offsets and a set of paths in flight are one
//! thing described three times, and `update` was indexing all of them by `QueueId::index()` on
//! every queue arm.
//!
//! Pure, like the module under it: no iced, no filesystem. The app still owns the `Task`s, the
//! dirty flag and the drag; what moved here is everything that is only about the queues.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cache::Ready;
use crate::deck::DeckId;
use crate::queue::{Item, Queue};
use crate::waveform::Trim;

/// What the duplicate warning answered (PLAN §9a).
///
/// Three, because a batch is what made two too few: a set of twenty where one is already
/// queued has an answer that is neither "all of them" nor "none of them", and it is the one
/// most people mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
	/// Queue everything, repeats included.
	All,
	/// Queue only the tracks that are not already somewhere.
	Fresh,
	/// Queue nothing — the action did not happen.
	Nothing,
}

impl Admission {
	/// Whether the track at this position in the batch survives the answer.
	///
	/// A position rather than a path, so a caller can filter rows and tracks together with the
	/// same test — which is what the `←` / `→` buttons need, since what leaves one queue has to
	/// be exactly what arrives in the other.
	fn keeps(self, index: usize, duplicates: &[(usize, QueueId)]) -> bool {
		match self {
			Admission::All => true,
			Admission::Fresh => !duplicates.iter().any(|(duplicate, _)| *duplicate == index),
			Admission::Nothing => false,
		}
	}

	/// The batch that survives the answer, in its own order.
	///
	/// One place for the enumerate–filter–map all three doors were writing out by hand — and
	/// generic for the sake of the `←` / `→` buttons, which zip rows and tracks and filter the
	/// pairs, so what leaves one queue is exactly what arrives in the other.
	fn admitted<T>(self, batch: Vec<T>, duplicates: &[(usize, QueueId)]) -> Vec<T> {
		batch
			.into_iter()
			.enumerate()
			.filter(|(position, _)| self.keeps(*position, duplicates))
			.map(|(_, kept)| kept)
			.collect()
	}
}

/// What a handover does, worked out before anything is loaded (PLAN §7a, §7b): the track, where
/// it starts, and whether it plays. The app carries it out — the load is IO, and the seek and
/// the Play only happen if the file actually arrived.
#[derive(Debug, PartialEq)]
pub struct NextTrack {
	pub path: PathBuf,
	/// `None` leaves the playhead where the load put it, at the top of the file.
	pub start: Option<Duration>,
	/// Lands on `Stopped` otherwise, like every other load (PLAN §7): on a mixer an unrequested
	/// fade-in is a mistake that cannot be taken back, which is why this is a switch and not
	/// the default.
	pub auto_play: bool,
}

/// Which of the three queues. The player-owned ones are `Cue`, the shared one is `Shared`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueueId {
	/// What one player plays next, and no other player's business.
	Cue(DeckId),
	/// The shared pool: whichever player finishes first takes from it (PLAN §7a).
	Shared,
}

impl QueueId {
	/// All three, left to right as they are drawn — which is also the order the `←` and `→`
	/// buttons step through, so `neighbour` and the layout cannot disagree.
	pub const ALL: [QueueId; 3] = [
		QueueId::Cue(DeckId::One),
		QueueId::Shared,
		QueueId::Cue(DeckId::Two),
	];

	/// Index into a three-element array of anything per-queue.
	///
	/// Still public, because the *view* needs it — the three `scrollable` ids are a
	/// `[&'static str; 3]` and iced wants a `&'static str`. Nothing in `update` uses it any
	/// more: that is what `Queues` is for.
	pub fn index(self) -> usize {
		match self {
			QueueId::Cue(DeckId::One) => 0,
			QueueId::Shared => 1,
			QueueId::Cue(DeckId::Two) => 2,
		}
	}

	/// What the user calls this queue.
	pub fn label(self) -> &'static str {
		match self {
			QueueId::Cue(DeckId::One) => "Cue 1",
			QueueId::Shared => "Next up",
			QueueId::Cue(DeckId::Two) => "Cue 2",
		}
	}

	/// The queue one step in this direction, or `None` at either end.
	///
	/// Neighbours only: `Cue 1` and `Cue 2` are not adjacent, so the arrows cannot throw a
	/// track across the shared queue without it stopping there. That is the point of the
	/// middle queue being in the middle.
	pub fn neighbour(self, right: bool) -> Option<QueueId> {
		let step = if right { 1 } else { -1 };
		let index = self.index() as isize + step;

		QueueId::ALL.get(usize::try_from(index).ok()?).copied()
	}
}

/// The three queues, and the two things that are true of them together rather than of any one.
#[derive(Debug, Default)]
pub struct Queues {
	queues: [Queue; 3],
	/// How far down each panel is scrolled (PLAN §9).
	///
	/// Here rather than on `Queue` because it is not a fact about the queue — the same queue
	/// drawn twice would be scrolled to two places — and here rather than on `Clecta` because a
	/// second array indexed by the same `QueueId` is the shape this module exists to absorb. The
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
	/// The three queues as they were left, in draw order.
	pub fn restored(queues: [Queue; 3]) -> Self {
		Self {
			queues,
			..Self::default()
		}
	}

	/// One queue, to read.
	pub fn get(&self, id: QueueId) -> &Queue {
		&self.queues[id.index()]
	}

	/// One queue, to edit. The only door: every queue edit in the app goes through here, so
	/// `QueueId::index` is this module's business and nobody else's.
	pub fn get_mut(&mut self, id: QueueId) -> &mut Queue {
		&mut self.queues[id.index()]
	}

	/// All three with their names, in draw order.
	pub fn all(&self) -> impl Iterator<Item = (QueueId, &Queue)> {
		QueueId::ALL.into_iter().map(|id| (id, self.get(id)))
	}

	/// How far down a panel is scrolled, and where it has been scrolled to.
	pub fn scroll(&self, id: QueueId) -> f32 {
		self.scroll[id.index()]
	}

	pub fn scrolled(&mut self, id: QueueId, offset: f32) {
		self.scroll[id.index()] = offset;
	}

	/// Where this track is already queued, if it is, so the app can ask before queueing it
	/// twice (PLAN §7a).
	///
	/// **All three queues, not just the one being added to.** The mistake worth catching is a
	/// track that plays twice in an evening, and Cue 1 and Cue 2 each holding it does that
	/// exactly as surely as one queue holding it twice — the duplicate is in the *set*, not in a
	/// queue. Which is the sentence that says this belongs to `Queues` and not to `Queue`.
	///
	/// `moving` is the rows that are on their way out of a queue, and it is the reason this takes
	/// a parameter at all: dragging rows from one queue to another, or sending them with `←` /
	/// `→`, finds them in their old home and would warn about tracks colliding with themselves.
	///
	/// Searched in draw order, so the queue named is the leftmost one — an arbitrary rule, but a
	/// stable one, and the message names a queue rather than counting them.
	pub fn already_queued(&self, path: &Path, moving: &[(QueueId, usize)]) -> Option<QueueId> {
		self.all()
			.find(|(id, queue)| {
				queue
					.items()
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
	/// to do with this answer. A batch is checked one track at a time and against the *queues*,
	/// not against itself — a selection cannot contain the same file twice, because a pane's
	/// rows are a folder's files.
	pub fn duplicates(&self, items: &[Item], moving: &[(QueueId, usize)]) -> Vec<(usize, QueueId)> {
		items
			.iter()
			.enumerate()
			.filter_map(|(index, item)| {
				self.already_queued(&item.path, moving)
					.map(|id| (index, id))
			})
			.collect()
	}

	/// Whether the track playing on this player should give way now rather than at the end of
	/// its file (PLAN §7b), given where it is and its music's edges.
	///
	/// Asked of the queue that would *supply* the next track, which is the same rule the two
	/// switches follow (PLAN §7a): the queue waiting behind a player is what says how it wants
	/// to take over. And it is asked at all only when there is something to take over — the
	/// last track of the evening plays its run-out, because cutting it short would leave a
	/// player stopped early for nothing.
	pub fn cuts_early(&self, id: DeckId, position: Duration, trim: Option<Trim>) -> bool {
		self.next_source(id)
			.is_some_and(|source| self.get(source).handover.hands_over_early(position, trim))
	}

	/// Take the next track for a player that has just finished, and say how it is to start
	/// (PLAN §7a, §7b): `None`, taking nothing, when no queue offers one — which is also where
	/// switching every queue off leaves the player simply stopped.
	///
	/// Taken rather than marked played: a queue is what is still to come, so the row leaving the
	/// queue as it reaches the player is what makes the queue mean that. And all three answers
	/// come from the **source queue**, not the player: the queue that gave the track is the queue
	/// that says whether it plays and where from, so Cue 1 can run the evening by itself while
	/// the shared pool stays a shelf someone takes from by hand. `trim` is what is known about
	/// a file's music, for where a queue that skips the blanks starts it.
	pub fn hand_over(
		&mut self,
		id: DeckId,
		trim: impl FnOnce(&Path) -> Option<Trim>,
	) -> Option<NextTrack> {
		let source = self.next_source(id)?;
		let queue = self.get_mut(source);
		let item = queue.take_next()?;
		Some(NextTrack {
			start: queue.handover.starts_at(trim(&item.path)),
			auto_play: queue.auto_play,
			path: item.path,
		})
	}

	/// Ask about a batch's repeats, if it has any (PLAN §9a): `All` without a word when none of
	/// them is queued anywhere, which is the ordinary case and costs a scan of three short
	/// queues. Otherwise `ask` decides — the app's dialog, because queueing a track twice is
	/// sometimes exactly what someone means and nothing in the queues can tell.
	fn admission(
		&self,
		items: &[Item],
		moving: &[(QueueId, usize)],
		ask: impl FnOnce(&[Item], &[(usize, QueueId)]) -> Admission,
	) -> (Admission, Vec<(usize, QueueId)>) {
		let duplicates = self.duplicates(items, moving);
		let admission = if duplicates.is_empty() {
			Admission::All
		} else {
			ask(items, &duplicates)
		};
		(admission, duplicates)
	}

	/// Queue a batch at the top or the end of one queue, past the duplicate warning (PLAN §7a,
	/// §9a). Whether anything was queued.
	///
	/// In the batch's own order either way — the pane's, top to bottom — so a batch plays in the
	/// order it was looked at.
	pub fn add(
		&mut self,
		id: QueueId,
		items: Vec<Item>,
		prepend: bool,
		ask: impl FnOnce(&[Item], &[(usize, QueueId)]) -> Admission,
	) -> bool {
		let (admission, duplicates) = self.admission(&items, &[], ask);
		let items = admission.admitted(items, &duplicates);
		if items.is_empty() {
			return false;
		}

		let queue = self.get_mut(id);
		let at = if prepend { 0 } else { queue.items().len() };
		queue.insert_many(at, items);
		true
	}

	/// Send a queue's selected rows to the end of its neighbour — the `←` / `→` buttons (PLAN
	/// §7a). Whether anything moved.
	///
	/// Both halves or neither: tracks taken out of one queue and not put into another are
	/// tracks the user just lost. Which is also why the rows are *looked at* before they are
	/// taken — a warning the user cancels must leave them where they were, and putting them
	/// back afterwards would have to rebuild the selection too. The warning can leave *some* of
	/// them standing, so the rows and the tracks are filtered together: what leaves the queue is
	/// exactly what arrives in the other.
	pub fn shift(
		&mut self,
		id: QueueId,
		right: bool,
		ask: impl FnOnce(&[Item], &[(usize, QueueId)]) -> Admission,
	) -> bool {
		let Some(target) = id.neighbour(right) else {
			return false;
		};

		let rows: Vec<usize> = self.get(id).selection().collect();
		let items = self.get(id).selected_items();
		let moving: Vec<(QueueId, usize)> = rows.iter().map(|&index| (id, index)).collect();

		let (admission, duplicates) = self.admission(&items, &moving, ask);
		let (rows, items): (Vec<usize>, Vec<Item>) = admission
			.admitted(rows.into_iter().zip(items).collect(), &duplicates)
			.into_iter()
			.unzip();
		if items.is_empty() {
			return false;
		}

		self.get_mut(id).take_rows(&rows);
		let at = self.get(target).items().len();
		self.get_mut(target).insert_many(at, items);
		true
	}

	/// Put a drag's tracks into a queue **above** row `at` (PLAN §7a, §10): `from` is the queue
	/// and the rows they are being dragged out of, or `None` from the files pane. Whether
	/// anything changed.
	///
	/// Three cases, and the first is the one worth separating: rows dragged **within their own
	/// queue** are a reorder, not a remove followed by an insert. Doing it as two steps would
	/// need the caret index adjusting for the holes the rows left behind, which is what
	/// `relocate` exists to get right. A reorder is also the one drop that cannot produce a
	/// duplicate — the rows are already in this queue and are only moving inside it — so it
	/// never asks.
	pub fn dropped(
		&mut self,
		into: QueueId,
		at: usize,
		items: Vec<Item>,
		from: Option<(QueueId, Vec<usize>)>,
		ask: impl FnOnce(&[Item], &[(usize, QueueId)]) -> Admission,
	) -> bool {
		if let Some((source, rows)) = &from
			&& *source == into
		{
			return self.get_mut(into).relocate(rows, at);
		}

		// The rows on their way out, so a track dragged from one queue to another does not warn
		// about colliding with itself.
		let moving: Vec<(QueueId, usize)> = match &from {
			Some((source, rows)) => rows.iter().map(|&row| (*source, row)).collect(),
			None => Vec::new(),
		};

		let (admission, duplicates) = self.admission(&items, &moving, ask);
		let items = admission.admitted(items, &duplicates);
		if items.is_empty() {
			return false;
		}

		// Across two queues: taken out of one and put into the other, in that order. `at` is
		// the caret in the *destination*, which the removal cannot have moved. From the files
		// pane there is nothing to take out — a drop from there is a copy, so the folder keeps
		// its files.
		if let Some((source, rows)) = from {
			// Only what the warning left standing, by position, so what leaves is what lands.
			let rows = admission.admitted(rows, &duplicates);
			self.get_mut(source).take_rows(&rows);
		}

		self.get_mut(into).insert_many(at, items);
		true
	}

	/// Where the track after this one comes from, when a player's track ends (PLAN §7a).
	///
	/// **Own cue first, the shared queue second.** A track deliberately cued to Player 1 outranks
	/// the pool, which is what makes the pool "whatever is free" rather than a third queue with
	/// rules of its own. `None` when neither has anything to offer, and the player simply stops.
	///
	/// A queue with `auto_load` off has nothing to offer however full it is: it is a shelf the
	/// user takes from by hand. It is *skipped*, not blocking — a cue switched off still lets
	/// the shared queue feed the player, because the switch belongs to one queue and says nothing
	/// about the other.
	pub fn next_source(&self, id: DeckId) -> Option<QueueId> {
		let cue = QueueId::Cue(id);
		if self.get(cue).auto_load && !self.get(cue).is_empty() {
			Some(cue)
		} else if self.get(QueueId::Shared).auto_load && !self.get(QueueId::Shared).is_empty() {
			Some(QueueId::Shared)
		} else {
			None
		}
	}

	/// Which tracks still need their length looked up — and count them as out from now on
	/// (PLAN §7a).
	///
	/// `take_` rather than `to_`, because it is not a view of the queues: the same call twice in a
	/// row gives the batch and then nothing, which is the whole point of it.
	///
	/// Asking and recording are one operation on purpose: they were two lines beside each other
	/// in `update`, and the day they stop being beside each other is the day a file goes out
	/// twice. The set is subtracted here rather than by the caller for the same reason.
	///
	/// Deduplicated across the three queues as well, because the same track may sit in two of
	/// them and one answer settles both rows. Returned in draw order, which is arbitrary but
	/// stable.
	pub fn take_unmeasured(&mut self) -> Vec<PathBuf> {
		let mut wanted: Vec<PathBuf> = Vec::new();

		for path in self.queues.iter().flat_map(Queue::unmeasured) {
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
	/// By path rather than by index, because the queues can be edited while the measuring runs
	/// and an index would name a different track by the time the answer came back. A queue may
	/// hold the same track twice, so one answer settles both rows.
	pub fn measured(&mut self, path: &Path, length: Option<Duration>, ready: Option<Ready>) {
		self.measuring.remove(path);
		for queue in &mut self.queues {
			queue.measured(path, length, ready);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::queue::Handover;
	use crate::select::Click;

	fn queue(names: &[&str]) -> Queue {
		Queue::from_paths(
			names
				.iter()
				.map(|name| PathBuf::from("/m").join(name))
				.collect(),
		)
	}

	fn three(one: &[&str], shared: &[&str], two: &[&str]) -> Queues {
		Queues::restored([queue(one), queue(shared), queue(two)])
	}

	fn path(name: &str) -> PathBuf {
		PathBuf::from("/m").join(name)
	}

	#[test]
	fn the_arrows_only_reach_a_neighbour() {
		// Arrange / Act / Assert: the middle queue is the only way across, so a track cannot
		// jump from one player's cue to the other's in one press.
		let (one, two) = (QueueId::Cue(DeckId::One), QueueId::Cue(DeckId::Two));

		assert_eq!(one.neighbour(true), Some(QueueId::Shared));
		assert_eq!(one.neighbour(false), None, "nothing left of Cue 1");
		assert_eq!(QueueId::Shared.neighbour(true), Some(two));
		assert_eq!(QueueId::Shared.neighbour(false), Some(one));
		assert_eq!(two.neighbour(true), None, "nothing right of Cue 2");
	}

	#[test]
	fn a_player_takes_from_its_own_cue_before_the_shared_queue() {
		// Arrange: both queues have something to offer.
		let both = three(&["a.mp3"], &["b.mp3"], &[]);

		// Act / Assert: the cue wins, which is what makes the shared queue a pool rather than
		// a third queue with rules of its own.
		assert_eq!(
			both.next_source(DeckId::One),
			Some(QueueId::Cue(DeckId::One))
		);

		// An empty cue falls through to the shared queue; both empty is a player that stops.
		let shared = three(&[], &["b.mp3"], &[]);
		assert_eq!(shared.next_source(DeckId::One), Some(QueueId::Shared));
		assert_eq!(three(&[], &[], &[]).next_source(DeckId::One), None);

		// And the other player's cue is not this player's business.
		assert_eq!(three(&[], &[], &["c.mp3"]).next_source(DeckId::One), None);
	}

	#[test]
	fn a_queue_that_hands_nothing_over_is_skipped_rather_than_blocking() {
		// Arrange: a full cue with Auto-load off, and a shared queue with it on (PLAN §7a).
		let mut queues = three(&["a.mp3"], &["b.mp3"], &[]);
		queues.get_mut(QueueId::Cue(DeckId::One)).auto_load = false;

		// Act / Assert: the cue is passed over rather than treated as an empty queue that ends
		// the handover — a cue switched off still lets the shared queue feed that player.
		assert_eq!(queues.next_source(DeckId::One), Some(QueueId::Shared));

		// Both off is the player stopping with full queues in front of it, which is what the
		// switches are for.
		queues.get_mut(QueueId::Shared).auto_load = false;
		assert_eq!(queues.next_source(DeckId::One), None);
	}

	#[test]
	fn a_track_is_found_wherever_it_is_already_queued() {
		// Arrange: one track in the shared queue, one in the far cue, one nowhere.
		let queues = three(&[], &["a.mp3"], &["b.mp3"]);

		// Act / Assert: all three queues are searched, not just the one being added to — the
		// duplicate is in the set (PLAN §7a).
		assert_eq!(
			queues.already_queued(&path("a.mp3"), &[]),
			Some(QueueId::Shared)
		);
		assert_eq!(
			queues.already_queued(&path("b.mp3"), &[]),
			Some(QueueId::Cue(DeckId::Two))
		);
		assert_eq!(queues.already_queued(&path("c.mp3"), &[]), None);
	}

	#[test]
	fn a_row_on_its_way_out_of_a_queue_is_not_its_own_duplicate() {
		// Arrange: the same track in two queues, and the shared queue's copy is being moved.
		let queues = three(&[], &["a.mp3"], &["a.mp3"]);
		let moving = [(QueueId::Shared, 0)];

		// Act / Assert: it must not warn about colliding with itself, or every cross-queue move
		// would ask a question with one honest answer...
		assert_eq!(
			queues.already_queued(&path("a.mp3"), &moving),
			Some(QueueId::Cue(DeckId::Two)),
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
			vec![
				(1, QueueId::Cue(DeckId::One)),
				(3, QueueId::Cue(DeckId::Two))
			]
		);
	}

	#[test]
	fn nothing_is_sent_to_be_measured_twice() {
		// Arrange: the same track in two queues, plus one of its own, and nothing measured.
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
		// Arrange: the same track queued twice in one queue and once in another.
		let mut queues = three(&["a.mp3", "a.mp3"], &["a.mp3"], &[]);

		// Act
		queues.measured(&path("a.mp3"), Some(Duration::from_secs(215)), None);

		// Assert: by path, not by index — the queues can be edited while a job runs, and one
		// file's length is one file's length however many rows are showing it.
		for (_, queue) in queues.all() {
			for item in queue.items() {
				assert_eq!(item.duration, Some(Some(Duration::from_secs(215))));
			}
		}
	}

	#[test]
	fn a_panel_remembers_where_it_was_scrolled_to_on_its_own() {
		// Arrange / Act: three panels, one offset each.
		let mut queues = three(&[], &[], &[]);
		queues.scrolled(QueueId::Shared, 120.0);

		// Assert: three queues means three offsets, or scrolling one would scroll all of them.
		assert_eq!(queues.scroll(QueueId::Shared), 120.0);
		assert_eq!(queues.scroll(QueueId::Cue(DeckId::One)), 0.0);
		assert_eq!(queues.scroll(QueueId::Cue(DeckId::Two)), 0.0);
	}

	const ONE: QueueId = QueueId::Cue(DeckId::One);

	/// Each queue's tracks by file name, left to right — what the user would see.
	fn names(queues: &Queues) -> [Vec<String>; 3] {
		QueueId::ALL.map(|id| {
			queues
				.get(id)
				.items()
				.iter()
				.map(|item| item.name.clone())
				.collect()
		})
	}

	/// Tracks as the files pane hands them over.
	fn items(names: &[&str]) -> Vec<Item> {
		names.iter().map(|name| Item::new(path(name))).collect()
	}

	/// A warning that must not be raised.
	fn never(_: &[Item], _: &[(usize, QueueId)]) -> Admission {
		panic!("asked about a batch with nothing to ask about")
	}

	#[test]
	fn the_warnings_answer_filters_a_batch_by_position() {
		// Arrange: a batch of four, of which the second and the fourth are already queued.
		let duplicates = vec![(1, QueueId::Shared), (3, QueueId::Shared)];
		let batch = || vec!["a", "b", "c", "d"];

		// Act / Assert: Yes queues repeats too, No takes only the fresh ones, Cancel nothing.
		assert_eq!(
			Admission::All.admitted(batch(), &duplicates),
			["a", "b", "c", "d"]
		);
		assert_eq!(Admission::Fresh.admitted(batch(), &duplicates), ["a", "c"]);
		assert!(Admission::Nothing.admitted(batch(), &duplicates).is_empty());

		// The `←` / `→` buttons zip rows and tracks and filter the pairs, so what leaves one
		// queue is exactly what arrives in the other.
		let paired = vec![(4, "a"), (7, "b")];
		assert_eq!(
			Admission::Fresh.admitted(paired, &[(1, QueueId::Shared)]),
			[(4, "a")]
		);
	}

	#[test]
	fn a_batch_with_no_repeats_is_queued_without_a_word() {
		// Arrange
		let mut queues = three(&["a.mp3"], &[], &[]);

		// Act: at the top, then at the end.
		let first = queues.add(ONE, items(&["b.mp3"]), true, never);
		let second = queues.add(ONE, items(&["c.mp3"]), false, never);

		// Assert
		assert!(first && second);
		assert_eq!(names(&queues)[0], ["b.mp3", "a.mp3", "c.mp3"]);
	}

	#[test]
	fn cancel_leaves_every_queue_as_it_was_whichever_way_in() {
		// Arrange: `a` queued in Cue 1, `b` selected in the shared queue and also in Cue 2.
		let mut queues = three(&["a.mp3"], &["b.mp3"], &["b.mp3"]);
		queues.get_mut(QueueId::Shared).click(0, Click::Replace);
		let before = names(&queues);
		let cancel = |_: &[Item], _: &[(usize, QueueId)]| Admission::Nothing;

		// Act: the three edits that ask, each answered Cancel.
		let added = queues.add(QueueId::Shared, items(&["a.mp3"]), false, cancel);
		let shifted = queues.shift(QueueId::Shared, true, cancel);
		let dropped = queues.dropped(
			ONE,
			0,
			items(&["b.mp3"]),
			Some((QueueId::Shared, vec![0])),
			cancel,
		);

		// Assert: nothing happened, and each says so.
		assert_eq!((added, shifted, dropped), (false, false, false));
		assert_eq!(names(&queues), before);
	}

	#[test]
	fn no_moves_exactly_the_rows_it_admits_and_leaves_the_repeats_behind() {
		// Arrange: two selected shared rows going right to Cue 2, which already holds `b`.
		let mut queues = three(&[], &["a.mp3", "b.mp3"], &["b.mp3"]);
		queues.get_mut(QueueId::Shared).click(0, Click::Toggle);
		queues.get_mut(QueueId::Shared).click(1, Click::Toggle);
		let mut asked = Vec::new();

		// Act
		let moved = queues.shift(QueueId::Shared, true, |batch, duplicates| {
			asked = duplicates.to_vec();
			assert_eq!(batch.len(), 2, "asked about the whole batch");
			Admission::Fresh
		});

		// Assert: `a` left and arrived; `b` stayed where it was, both halves of it.
		assert!(moved);
		assert_eq!(asked, [(1, QueueId::Cue(DeckId::Two))]);
		assert_eq!(
			names(&queues),
			[
				vec![],
				vec!["b.mp3".to_string()],
				vec!["b.mp3".to_string(), "a.mp3".to_string()]
			]
		);
	}

	#[test]
	fn a_drag_across_queues_takes_only_what_the_warning_lets_land() {
		// Arrange: `a` and `b` dragged out of the shared queue into Cue 1, which holds `b`.
		let mut queues = three(&["b.mp3"], &["a.mp3", "b.mp3"], &[]);

		// Act
		let changed = queues.dropped(
			ONE,
			1,
			items(&["a.mp3", "b.mp3"]),
			Some((QueueId::Shared, vec![0, 1])),
			|_, _| Admission::Fresh,
		);

		// Assert: `a` moved under the caret; `b` stayed in the shared queue.
		assert!(changed);
		assert_eq!(names(&queues)[0], ["b.mp3", "a.mp3"]);
		assert_eq!(names(&queues)[1], ["b.mp3"]);
	}

	#[test]
	fn a_reorder_never_asks_and_a_copy_from_the_pane_takes_nothing_out() {
		// Arrange
		let mut queues = three(&["a.mp3", "b.mp3"], &[], &[]);

		// Act: `b` above `a`, inside Cue 1; then `c` from the files pane into the shared queue.
		let reordered = queues.dropped(ONE, 0, items(&["b.mp3"]), Some((ONE, vec![1])), never);
		let copied = queues.dropped(QueueId::Shared, 0, items(&["c.mp3"]), None, never);

		// Assert
		assert!(reordered && copied);
		assert_eq!(names(&queues)[0], ["b.mp3", "a.mp3"]);
		assert_eq!(names(&queues)[1], ["c.mp3"]);
	}

	/// Music from 2 s to 3:00 in every file — what a scan of each would have found.
	fn edges(_: &Path) -> Option<Trim> {
		Some(TRIM)
	}

	const TRIM: Trim = Trim {
		start: Duration::from_secs(2),
		end: Duration::from_secs(180),
	};

	/// Past the music's end: the run-out is playing.
	const RUN_OUT: Duration = Duration::from_secs(181);

	#[test]
	fn the_last_track_of_the_evening_plays_its_run_out() {
		// Arrange: Cue 1 skips the blanks but has nothing left to give.
		let mut queues = three(&[], &[], &[]);
		queues.get_mut(ONE).handover = Handover::Trimmed;

		// Act / Assert: nothing would take over, so nothing is cut.
		assert!(!queues.cuts_early(DeckId::One, RUN_OUT, Some(TRIM)));

		// The moment there is a next track, the run-out goes.
		queues.add(ONE, items(&["b.mp3"]), false, never);
		assert!(queues.cuts_early(DeckId::One, RUN_OUT, Some(TRIM)));
		assert!(
			!queues.cuts_early(DeckId::One, RUN_OUT, None),
			"a track nobody scanned plays whole"
		);
	}

	#[test]
	fn the_queue_that_would_supply_the_track_decides_when_it_takes_over() {
		// Arrange: Cue 1 skips the blanks but is switched off; the shared queue plays whole.
		let mut queues = three(&["a.mp3"], &["b.mp3"], &[]);
		queues.get_mut(ONE).handover = Handover::Trimmed;
		queues.get_mut(ONE).auto_load = false;

		// Act / Assert: the shared queue is the supplier, and it waits for the end of the file.
		assert!(!queues.cuts_early(DeckId::One, RUN_OUT, Some(TRIM)));

		// Its own setting changed, it cuts.
		queues.get_mut(QueueId::Shared).handover = Handover::Trimmed;
		assert!(queues.cuts_early(DeckId::One, RUN_OUT, Some(TRIM)));
	}

	#[test]
	fn a_handover_takes_the_top_track_and_starts_it_as_its_own_queue_says() {
		// Arrange: Cue 1 plays whole and waits; the shared queue skips the blanks and plays.
		let mut queues = three(&["a.mp3", "b.mp3"], &["c.mp3"], &[]);
		let shared = queues.get_mut(QueueId::Shared);
		shared.handover = Handover::Trimmed;
		shared.auto_play = true;

		// Act: Player 1 runs out twice with its cue, then a third time.
		let first = queues.hand_over(DeckId::One, edges);
		let second = queues.hand_over(DeckId::One, edges);
		let third = queues.hand_over(DeckId::One, edges);

		// Assert: the cue's tracks at the top and stopped, then the shared queue's at the music.
		let next = |name: &str, start, auto_play| NextTrack {
			path: path(name),
			start,
			auto_play,
		};
		assert_eq!(first, Some(next("a.mp3", None, false)));
		assert_eq!(second, Some(next("b.mp3", None, false)));
		assert_eq!(third, Some(next("c.mp3", Some(TRIM.start), true)));
		assert_eq!(
			names(&queues),
			[Vec::<String>::new(), vec![], vec![]],
			"each taken"
		);
	}

	#[test]
	fn nothing_to_hand_over_takes_nothing() {
		// Arrange: a full cue with Auto-load off, and nothing shared.
		let mut queues = three(&["a.mp3"], &[], &[]);
		queues.get_mut(ONE).auto_load = false;

		// Act
		let next = queues.hand_over(DeckId::One, edges);

		// Assert: the player simply stops, and the shelf keeps its track.
		assert_eq!(next, None);
		assert_eq!(names(&queues)[0], ["a.mp3"]);
	}
}
