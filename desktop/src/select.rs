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
	/// collapse-on-release (Q50). Both panes ask this same question, at the press and again
	/// at the release, which is why it has a name instead of being the same two conditions
	/// written four times.
	pub fn defers(self, already_selected: bool) -> bool {
		already_selected && self == Click::Replace
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
