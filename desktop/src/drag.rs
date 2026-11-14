//! A drag, start to release (PLAN §10, §7a): what is being carried, where a release would land,
//! the one indicator that says so, and the queue edge a drag is scrolling.
//!
//! The rules were four fields of the app and eleven arms of `update`, and they were the half of
//! the app that had always broken here — two indicators lit at once, a leave wiping a newer
//! enter, a queue scrolling on after the release. They live behind one value now, the way the
//! collapse does since Q58, so each of them is a test rather than a thing to try by hand. What a
//! landing *does* — load a player, move rows, cancel the collapse — stays with the app, which
//! owns the players and the queues; this module only says where it landed.

use crate::deck::DeckId;
use crate::queue::Item;
use crate::queues::QueueId;

/// Where a release would put what is being dragged (PLAN §10, §7a).
///
/// One type for both kinds of destination, so the hover is one field and the rules about what
/// beats what live in one `match` rather than in two flags that can both be set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
	/// Load it into this player, now.
	Player(DeckId),
	/// Put it in this queue, **above** this row. `index == len` is past the last row, which is
	/// the one index that names no row — the caret sits between rows, not on them.
	Row(QueueId, usize),
}

/// The panel a pointer can leave, which is coarser than what it can land on: a queue has as
/// many targets as it has rows, but leaving it is one event.
///
/// Leaving is separate from entering because the two arrive in an order nothing guarantees.
/// A leave that only clears *its own* zone cannot wipe an enter that has already landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
	Player(DeckId),
	Queue(QueueId),
}

impl DropTarget {
	/// The panel this target sits in, which is what a pointer leaves.
	fn zone(self) -> Zone {
		match self {
			DropTarget::Player(id) => Zone::Player(id),
			DropTarget::Row(queue, _) => Zone::Queue(queue),
		}
	}
}

/// What an in-app drag is carrying, and where it came from.
///
/// A whole selection rather than one row (PLAN §9a), in the order the rows are on screen — so a
/// drop puts them down in the order they were looked at. `from` is one queue and the rows inside
/// it, because a drag starts with a press and a press is in one panel — and it is the whole
/// difference between a copy and a move: a drag out of the files pane leaves the file where it
/// is, and a drag out of a queue takes the rows with it.
#[derive(Debug, Clone)]
pub struct Carried {
	pub items: Vec<Item>,
	pub from: Option<(QueueId, Vec<usize>)>,
}

/// Everything a drag in flight has to remember, in-app and from outside alike.
#[derive(Debug, Default)]
pub struct Drag {
	/// What an in-app drag is carrying. Armed by a press on a media row and taken by the
	/// release, so a plain click is a drag that landed on nothing.
	carried: Option<Carried>,
	/// Where the pointer is, of the places a release could land. Only ever reported while a
	/// drag is armed, because that is the only time the panels are drop targets at all.
	hover: Option<DropTarget>,
	/// Whether files from outside are hovering the window right now.
	files: bool,
	/// The queue whose edge a drag is resting on, and whether it is the top one. `None` at
	/// rest, which is what gates the timer that does the scrolling.
	edge: Option<(QueueId, bool)>,
}

impl Drag {
	/// Start carrying a selection, or nothing when the press selected nothing that can be
	/// carried — a press on a folder, say, still arms nothing.
	pub fn arm(&mut self, items: Vec<Item>, from: Option<(QueueId, Vec<usize>)>) {
		self.carried = (!items.is_empty()).then_some(Carried { items, from });
	}

	/// Stop carrying without landing: the rows have already gone somewhere else, as when the
	/// press that armed this was the first half of a double click that played them.
	pub fn disarm(&mut self) {
		self.carried = None;
	}

	/// Whether an in-app drag is in flight, which is what makes the panels drop targets.
	pub fn is_armed(&self) -> bool {
		self.carried.is_some()
	}

	/// The pointer has entered a place a release could land.
	pub fn over(&mut self, target: DropTarget) {
		self.hover = Some(target);
	}

	/// The pointer has left a panel. Compared rather than merely cleared: every panel sees the
	/// same cursor move, and the one being left is not always the one reported first.
	pub fn out(&mut self, zone: Zone) {
		if self.hover.is_some_and(|target| target.zone() == zone) {
			self.hover = None;
		}
	}

	/// The pointer has entered or left one of a queue's two edges. The same shape as `out`,
	/// for the same reason: a leave clears only the edge it is actually about.
	pub fn edge(&mut self, queue: QueueId, up: bool, entering: bool) {
		if entering {
			self.edge = Some((queue, up));
		} else if self.edge == Some((queue, up)) {
			self.edge = None;
		}
	}

	/// The queue a drag is scrolling, and whether upwards.
	pub fn scrolling(&self) -> Option<(QueueId, bool)> {
		self.edge
	}

	/// Files from outside have started or stopped hovering the window.
	pub fn files_hovered(&mut self, hovering: bool) {
		self.files = hovering;
	}

	/// One file of an OS drop has arrived: whether it is the *first* of its burst, which is
	/// the one that counts (PLAN §10). Taking the flag is what makes every later file of the
	/// same drop answer `false`.
	///
	/// `ponytail:` this leans on the hover arriving before the drop, which is the only burst
	/// boundary the event stream has. True on macOS and Windows (checked in winit's
	/// `performDragOperation:` and `IDropTarget::Drop`, neither of which cancels the hover
	/// first). A platform that skipped the hover would decline the drop with a notice, not
	/// swallow it.
	pub fn files_dropped(&mut self) -> bool {
		std::mem::take(&mut self.files)
	}

	/// The player a release would land on right now, given the one an unaimed load would pick.
	/// Exactly one or none, or the ring would promise aiming the app cannot do.
	pub fn ring(&self, idle: DeckId) -> Option<DeckId> {
		if self.files {
			// An OS drag carries no position, so the target is derived — and shown before the
			// release rather than discovered after it (PLAN §10).
			Some(idle)
		} else if self.carried.is_some() {
			// An in-app drag is truly aimed: the pointer is ours the whole way. A drag headed
			// for a *queue* lights no player — the caret there is showing where it goes, and
			// two indicators at once would each be half a lie.
			match self.hover {
				Some(DropTarget::Player(id)) => Some(id),
				_ => None,
			}
		} else {
			None
		}
	}

	/// Where a release would put the carried rows in this queue, if that is where they are
	/// headed. `None` at rest and for the queues the pointer is not over, so exactly one caret
	/// is ever drawn — the same "one indicator, and it tells the truth" rule as the ring.
	pub fn caret(&self, queue: QueueId) -> Option<usize> {
		match self.hover {
			Some(DropTarget::Row(id, index)) if id == queue && self.carried.is_some() => {
				Some(index)
			}
			_ => None,
		}
	}

	/// The button has come up: where what was carried lands, or `None` for a release that
	/// carried nothing or landed on nothing — both a plain click.
	///
	/// Everything is cleared, whatever the release was over, so nothing is left armed behind a
	/// drag the user thought better of. The edge too, rather than left to a leave: the edges
	/// only exist while a drag is in flight, so releasing *on* one destroys the widget that would
	/// have reported the pointer leaving it, and the queue would scroll for ever (PLAN §7a).
	pub fn release(&mut self) -> Option<(DropTarget, Carried)> {
		self.edge = None;
		let target = self.hover.take();
		let carried = self.carried.take();
		target.zip(carried)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::path::PathBuf;

	fn rows() -> Vec<Item> {
		vec![Item::new(PathBuf::from("/m/a.mp3"))]
	}

	/// A drag armed from the files pane, which is enough for every rule here: where it came
	/// from only matters to what the app does with the landing.
	fn armed() -> Drag {
		let mut drag = Drag::default();
		drag.arm(rows(), None);
		drag
	}

	const CUE: QueueId = QueueId::Cue(DeckId::One);

	#[test]
	fn a_press_on_nothing_carriable_arms_nothing() {
		// Arrange
		let mut drag = Drag::default();

		// Act
		drag.arm(Vec::new(), None);

		// Assert
		assert!(!drag.is_armed());
	}

	#[test]
	fn exactly_one_indicator_shows_where_a_release_would_land() {
		// Arrange
		let mut drag = armed();

		// Act / Assert: over a row, the caret in that queue and no ring.
		drag.over(DropTarget::Row(CUE, 2));
		assert_eq!(drag.caret(CUE), Some(2));
		assert_eq!(drag.caret(QueueId::Shared), None);
		assert_eq!(drag.ring(DeckId::Two), None);

		// Act / Assert: over a player, its ring and no caret anywhere.
		drag.over(DropTarget::Player(DeckId::One));
		assert_eq!(drag.ring(DeckId::Two), Some(DeckId::One));
		assert_eq!(drag.caret(CUE), None);
	}

	#[test]
	fn a_leave_reported_late_does_not_wipe_the_newer_enter() {
		// Arrange: the pointer goes from the cue to the player, and the player's enter is
		// reported before the cue's leave.
		let mut drag = armed();
		drag.over(DropTarget::Row(CUE, 0));
		drag.over(DropTarget::Player(DeckId::One));

		// Act
		drag.out(Zone::Queue(CUE));

		// Assert
		assert_eq!(drag.ring(DeckId::Two), Some(DeckId::One));
	}

	#[test]
	fn a_leave_of_the_panel_hovered_clears_it() {
		// Arrange
		let mut drag = armed();
		drag.over(DropTarget::Row(CUE, 0));

		// Act
		drag.out(Zone::Queue(CUE));

		// Assert
		assert_eq!(drag.caret(CUE), None);
		assert!(drag.release().is_none(), "a release now lands on nothing");
	}

	#[test]
	fn files_from_outside_ring_the_idle_player_with_nothing_armed() {
		// Arrange
		let mut drag = Drag::default();

		// Act
		drag.files_hovered(true);

		// Assert
		assert_eq!(drag.ring(DeckId::Two), Some(DeckId::Two));
	}

	#[test]
	fn nothing_is_ringed_or_careted_at_rest() {
		// Arrange: a hover with nothing armed, which the panels never report, but a stale one
		// must still draw nothing.
		let mut drag = Drag::default();

		// Act
		drag.over(DropTarget::Row(CUE, 0));

		// Assert
		assert_eq!(drag.caret(CUE), None);
		assert_eq!(drag.ring(DeckId::One), None);
	}

	#[test]
	fn only_the_first_file_of_an_os_drop_counts() {
		// Arrange
		let mut drag = Drag::default();
		drag.files_hovered(true);

		// Act
		let first = drag.files_dropped();
		let second = drag.files_dropped();

		// Assert
		assert!(first);
		assert!(!second);
		assert_eq!(drag.ring(DeckId::One), None, "the ring goes with the drop");
	}

	#[test]
	fn a_release_lands_what_was_carried_and_clears_everything() {
		// Arrange: carried over a row, resting on that queue's bottom edge.
		let mut drag = Drag::default();
		drag.arm(rows(), Some((QueueId::Shared, vec![0])));
		drag.over(DropTarget::Row(CUE, 3));
		drag.edge(CUE, false, true);

		// Act
		let landing = drag.release();

		// Assert
		let (target, carried) = landing.expect("a landing");
		assert_eq!(target, DropTarget::Row(CUE, 3));
		assert_eq!(carried.from, Some((QueueId::Shared, vec![0])));
		assert!(!drag.is_armed());
		assert_eq!(
			drag.scrolling(),
			None,
			"released on an edge, the scrolling stops"
		);
		assert_eq!(drag.caret(CUE), None);
	}

	#[test]
	fn a_release_over_nothing_is_a_plain_click_and_disarms() {
		// Arrange
		let mut drag = armed();

		// Act
		let landing = drag.release();

		// Assert
		assert!(landing.is_none());
		assert!(!drag.is_armed());
	}

	#[test]
	fn leaving_one_edge_does_not_stop_the_scroll_at_another() {
		// Arrange: from the cue's bottom edge onto the shared queue's top edge, the enter
		// reported first.
		let mut drag = armed();
		drag.edge(CUE, false, true);
		drag.edge(QueueId::Shared, true, true);

		// Act
		drag.edge(CUE, false, false);

		// Assert
		assert_eq!(drag.scrolling(), Some((QueueId::Shared, true)));
	}
}
