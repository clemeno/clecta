//! What a click on a row means (PLAN §9a) — the one rule the files pane and the three
//! queues share.
//!
//! They store their selections differently and have to: the pane is keyed by **path**,
//! because a refresh renumbers its rows underneath it, and a queue is keyed by **index**,
//! because a queue may hold the same track twice and a path names no row there. What is the
//! same in both is the gesture, and it is three lines of rule that are wrong in a way nobody
//! notices until a set is half selected.
//!
//! Pure, and free of iced for the usual reason (PLAN §12): the modifiers arrive as two
//! `bool`s so the whole thing can be checked with no window and no keyboard.

/// What a press on a row does to the selection around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
	/// Everything else goes; this row is the selection, and the anchor.
	Replace,
	/// This row joins or leaves the selection, and becomes the anchor either way.
	Toggle,
	/// Everything from the anchor to this row, inclusive.
	Range,
}

impl Click {
	/// Which of the three a press is, from the modifiers held while it happened.
	///
	/// **Shift wins over the command key**, which matters only for the two held together and
	/// is the answer every file manager gives: extending a selection is the more specific
	/// request, and a range that silently toggled instead would be a hard gesture to undo.
	///
	/// `command` is Cmd on macOS and Ctrl everywhere else — `Modifiers::command()` already
	/// says which, so this takes the answer rather than the platform.
	pub fn of(command: bool, shift: bool) -> Self {
		if shift {
			Click::Range
		} else if command {
			Click::Toggle
		} else {
			Click::Replace
		}
	}

	/// Whether a press leaves the selection alone until its release.
	///
	/// A plain press on a row that is already selected must not collapse the selection — it
	/// is arming a drag that may want to carry all of it (PLAN §9a). If nothing else claims
	/// the click, the release finishes the job the press deferred, which is
	/// collapse-on-release (Q50). Both panes ask this same question at their press, which is
	/// why it has a name instead of being the same two conditions written twice.
	pub fn defers(self, already_selected: bool) -> bool {
		already_selected && self == Click::Replace
	}
}

/// The two-stage life of the click a deferring press still owes (PLAN §9a, Q50, Q58).
///
/// **Armed** from the press to its release, **pending** from the release to the timer that
/// finally fires the plain click — and every way a newer gesture can claim the click in
/// between is a method here, rather than a pair of field assignments nine `update` arms had
/// to keep in the right order. Generic over the payload, because the two panes remember a
/// pressed row differently (a path, or a queue and an index) for the reasons their
/// selections already differ.
#[derive(Debug)]
pub struct Collapse<T> {
	/// What a deferring press remembered, press → release.
	armed: Option<T>,
	/// What the release left to do, waiting out the double-click window (Q50).
	pending: Option<T>,
}

/// Written out rather than derived: `#[derive(Default)]` would demand `T: Default` for two
/// fields that start as `None` whatever `T` is.
impl<T> Default for Collapse<T> {
	fn default() -> Self {
		Self {
			armed: None,
			pending: None,
		}
	}
}

impl<T> Collapse<T> {
	/// A press deferred (`defers`): remember what it will owe its release — and abandon
	/// whatever an older click still had pending, because a press is a new gesture. Done here,
	/// synchronously, rather than left to the raw listener's press-anywhere message, which can
	/// trail the widget's own by a frame while the timer does not wait (Q53).
	pub fn arm(&mut self, pressed: T) {
		self.pending = None;
		self.armed = Some(pressed);
	}

	/// A press that acted immediately: nothing is owed to its release, and an older pending
	/// click is abandoned for the same reason `arm` abandons one.
	pub fn disarm(&mut self) {
		self.armed = None;
		self.pending = None;
	}

	/// Any press, any button, from the raw listener (Q53). Pending only, deliberately: the
	/// armed stage belongs to the very press being processed, and this message can be handled
	/// on either side of the row's own — clearing both here would race it.
	pub fn pressed_anywhere(&mut self) {
		self.pending = None;
	}

	/// The release: what the press deferred starts waiting out the double-click window (Q50).
	pub fn release(&mut self) {
		self.pending = self.armed.take();
	}

	/// Something claimed the click — a double click's load, ⌘A, Escape, a drop that landed.
	/// Both stages go: a deferral still held by a pressed button would be promoted by its
	/// release and fire anyway (Q50).
	pub fn cancel(&mut self) {
		self.armed = None;
		self.pending = None;
	}

	/// The timer fired: the click to finish, if nothing claimed it first — and taking it is
	/// what ends the timer's subscription.
	pub fn due(&mut self) -> Option<T> {
		self.pending.take()
	}

	/// Whether the timer needs to exist at all.
	pub fn waiting(&self) -> bool {
		self.pending.is_some()
	}
}

/// The rows between two, in order, whichever way round they were clicked.
///
/// Inclusive at both ends: a Shift-click means "and this one too", so the row under the
/// pointer is part of what it selects — and so is the anchor, which is already selected and
/// must not be dropped by a range that starts past it.
pub fn between(anchor: usize, clicked: usize) -> std::ops::RangeInclusive<usize> {
	anchor.min(clicked)..=anchor.max(clicked)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_modifiers_decide_which_of_the_three_a_press_is() {
		// Arrange / Act / Assert: the plain press is the one that must stay simple — every
		// click in the app that is not about selection at all goes through it.
		assert_eq!(Click::of(false, false), Click::Replace);
		assert_eq!(Click::of(true, false), Click::Toggle, "command");
		assert_eq!(Click::of(false, true), Click::Range, "shift");

		// Both held: a range, not a toggle. The pair is easy to hit on the way to a
		// Shift-click, and the wrong answer there scatters a selection rather than extending
		// it — which takes a lot of clicks to put back.
		assert_eq!(Click::of(true, true), Click::Range, "shift wins");
	}

	#[test]
	fn only_a_plain_press_on_a_selected_row_defers() {
		// Arrange / Act / Assert: deferring exists for the drag's sake, so it is exactly the
		// press that would otherwise destroy what the drag is about to carry — a plain press
		// on a selected row — and nothing else (PLAN §9a, Q50).
		assert!(Click::Replace.defers(true));
		assert!(
			!Click::Replace.defers(false),
			"an unselected row is a plain click"
		);
		assert!(!Click::Toggle.defers(true), "a toggle acts on the press");
		assert!(!Click::Range.defers(true), "a range acts on the press");
		assert!(!Click::Toggle.defers(false));
		assert!(!Click::Range.defers(false));
	}

	#[test]
	fn a_deferred_press_becomes_a_click_only_when_the_timer_finds_it_still_pending() {
		// Arrange / Act: the ordinary life of a slow click — press, release, timer.
		let mut collapse = Collapse::default();
		collapse.arm("row");
		assert!(!collapse.waiting(), "nothing pending until the release");
		collapse.release();
		assert!(
			collapse.waiting(),
			"the release starts the double-click window"
		);

		// Assert: the timer spends it exactly once.
		assert_eq!(collapse.due(), Some("row"));
		assert_eq!(collapse.due(), None, "spent");
		assert!(!collapse.waiting());
	}

	#[test]
	fn a_press_anywhere_abandons_the_pending_stage_and_only_that_stage() {
		// Arrange: Q53's ordering hazard, as an assertion. The raw listener's message and the
		// row's own can be handled in either order, so the anywhere-press must not touch the
		// armed stage — it belongs to the very press being processed.
		let mut collapse = Collapse::default();
		collapse.arm("row");
		collapse.pressed_anywhere();
		collapse.release();
		assert_eq!(collapse.due(), Some("row"), "the armed stage survives");

		// Act / Assert: after the release, it is exactly what kills a stale click — a press
		// held on a transport button across the timer's tick (Q53).
		collapse.arm("row");
		collapse.release();
		collapse.pressed_anywhere();
		assert_eq!(collapse.due(), None, "abandoned before the timer fired");
	}

	#[test]
	fn a_claimed_click_owes_nothing_at_either_stage() {
		// Arrange / Act / Assert: a double click's load, ⌘A, Escape, a landed drop — anything
		// that acts on the selection cancels both stages, because a deferral still held by a
		// pressed button would be promoted by its release and fire anyway (Q50).
		let mut collapse = Collapse::default();
		collapse.arm("row");
		collapse.cancel();
		collapse.release();
		assert_eq!(collapse.due(), None, "cancelled while armed");

		collapse.arm("row");
		collapse.release();
		collapse.cancel();
		assert_eq!(collapse.due(), None, "cancelled while pending");
	}

	#[test]
	fn a_new_press_abandons_an_older_clicks_deferral_by_itself() {
		// Arrange: a press that acts immediately (an unselected row) abandons what an older
		// click still owed — without waiting for the raw listener's message, which can trail
		// the widget's own by a frame (Q53).
		let mut collapse = Collapse::default();
		collapse.arm("older");
		collapse.release();
		collapse.disarm();
		assert_eq!(collapse.due(), None, "the older click is abandoned");

		// Act / Assert: and a press that defers abandons it the same way, synchronously.
		collapse.arm("older");
		collapse.release();
		collapse.arm("newer");
		assert!(
			!collapse.waiting(),
			"the older pending went with the new press"
		);
		collapse.release();
		assert_eq!(collapse.due(), Some("newer"));
	}

	#[test]
	fn a_range_is_inclusive_and_does_not_care_which_way_it_was_dragged() {
		// Arrange / Act / Assert: clicking upwards selects exactly what clicking downwards
		// does, and both ends are in it — the anchor is already selected and a range that
		// dropped it would deselect the row the user started from.
		assert_eq!(between(2, 5).collect::<Vec<_>>(), [2, 3, 4, 5]);
		assert_eq!(between(5, 2).collect::<Vec<_>>(), [2, 3, 4, 5], "upwards");
		assert_eq!(between(4, 4).collect::<Vec<_>>(), [4], "itself");
	}
}
