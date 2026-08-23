//! Which list the keyboard is talking to (PLAN §9a).
//!
//! ⌘A and Escape used to act on the files pane and nothing else, and §9a said why: the app had
//! no focus model, so "select all" in a window with four selectable lists would have to guess
//! which one was meant. This is the smallest model that stops the guessing — **the list you
//! last pressed a row in** — and it needs no new gesture, no Tab order and no focus discipline
//! to learn, because the press that moves it is a press the user was making anyway.
//!
//! One rule, stated here: a key acts on the focused list and leaves every other list's
//! selection exactly as it was. Selecting a set in one queue, going to look at another and
//! coming back should find the set still there, and Escape must never be the key that quietly
//! emptied three lists at once.
//!
//! Takes the panes as arguments rather than owning them, the same way `known.rs` does and for
//! the same reason (Q59): the app stays the owner of its panes, and this stays a rule that can
//! be checked with no window.

use crate::browser::Browser;
use crate::queues::{QueueId, Queues};

/// The list a key press means, and the pane it starts on.
///
/// `Files` is the default because it is where the app opens, and because it is the list with
/// thousands of rows in it — the one where selecting them all by hand is not an option, which
/// is the argument that got ⌘A its first list in the first place (PLAN §9a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
	#[default]
	Files,
	Queue(QueueId),
}

impl Focus {
	/// Select every row of the focused list (PLAN §9a).
	///
	/// "Every row" means something slightly different in each, and deliberately so: the pane
	/// selects what it is *showing*, so the hidden filter decides what all means, while a queue
	/// has no filter and every row is every row.
	pub fn select_all(self, browser: &mut Browser, queues: &mut Queues) {
		match self {
			Focus::Files => browser.select_all(),
			Focus::Queue(id) => queues.get_mut(id).select_all(),
		}
	}

	/// Clear the focused list's selection, and only that one's.
	pub fn clear(self, browser: &mut Browser, queues: &mut Queues) {
		match self {
			Focus::Files => browser.clear_selection(),
			Focus::Queue(id) => queues.get_mut(id).clear_selection(),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::browser::Entry;
	use crate::deck::DeckId;
	use crate::queue::Queue;
	use std::path::PathBuf;

	/// A pane of two files, and three queues holding two rows each — enough that "all" is a
	/// different set in every one of the four, so a key acting on the wrong one shows up.
	fn lists() -> (Browser, Queues) {
		let mut browser = Browser::default();
		browser.show(
			PathBuf::from("/m"),
			vec![
				Entry::new(PathBuf::from("/m/a.mp3"), 9, None),
				Entry::new(PathBuf::from("/m/b.mp3"), 9, None),
			],
		);

		let rows = || Queue::from_paths(vec![PathBuf::from("/m/a.mp3"), PathBuf::from("/m/b.mp3")]);
		(browser, Queues::restored([rows(), rows(), rows()]))
	}

	/// How many rows each of the four lists has selected, in one tuple: the pane, then the
	/// three queues in draw order.
	fn counts(browser: &Browser, queues: &Queues) -> (usize, [usize; 3]) {
		(
			browser.selection().count(),
			QueueId::ALL.map(|id| queues.get(id).selection().count()),
		)
	}

	#[test]
	fn the_pane_is_what_the_keys_mean_until_a_queue_row_is_pressed() {
		// Arrange / Act / Assert: the default is the files pane, which is where ⌘A was before
		// there was anything to choose between (PLAN §9a).
		let (mut browser, mut queues) = lists();
		Focus::default().select_all(&mut browser, &mut queues);
		assert_eq!(counts(&browser, &queues), (2, [0, 0, 0]));
	}

	#[test]
	fn select_all_fills_the_focused_list_and_no_other() {
		// Arrange: the shared queue is the one that was last pressed in.
		let (mut browser, mut queues) = lists();
		let focus = Focus::Queue(QueueId::Shared);

		// Act
		focus.select_all(&mut browser, &mut queues);

		// Assert: that queue holds both its rows, and the pane and the two cues are untouched —
		// a set built in one list must survive going to look at another.
		assert_eq!(counts(&browser, &queues), (0, [0, 2, 0]));
	}

	#[test]
	fn clearing_empties_the_focused_list_and_leaves_the_rest_selected() {
		// Arrange: something selected everywhere, the way a session in progress looks.
		let (mut browser, mut queues) = lists();
		browser.select_all();
		for id in QueueId::ALL {
			queues.get_mut(id).select_all();
		}
		assert_eq!(counts(&browser, &queues), (2, [2, 2, 2]));

		// Act: Escape, with Cue 1 focused.
		Focus::Queue(QueueId::Cue(DeckId::One)).clear(&mut browser, &mut queues);

		// Assert: one list emptied. Escape is "never mind" about the thing being worked on, and
		// a key that quietly emptied all four would be a key nothing could undo.
		assert_eq!(counts(&browser, &queues), (2, [0, 2, 2]));

		// And the pane's own turn, which is the case that used to be the only one.
		Focus::Files.clear(&mut browser, &mut queues);
		assert_eq!(counts(&browser, &queues), (0, [0, 2, 2]));
	}
}
