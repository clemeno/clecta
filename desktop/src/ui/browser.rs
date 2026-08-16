//! The files pane (PLAN §9): a header, then one row per file.
//!
//! Rows are built by hand — `scrollable(column(rows))` — rather than with
//! `widget::table`, because `table` has no row element to carry a click or a selected
//! background. The layout spike is what settled that; PLAN §9 records why.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::widget::{Space, button, checkbox, column, container, mouse_area, row, scrollable, text};
use iced::{Element, Fill, Left, Right, Theme};

use crate::app::Message;
use crate::browser::{Browser, Entry};
use crate::deck::DeckId;
use crate::ui;

/// Fixed column widths, in pixels. The name column takes what is left.
const MARK_WIDTH: f32 = 14.0;
const GLYPH_WIDTH: f32 = 18.0;
const TEMPO_WIDTH: f32 = 46.0;
const MUSIC_WIDTH: f32 = 48.0;
const SIZE_WIDTH: f32 = 80.0;
const DATE_WIDTH: f32 = 92.0;

/// A file the store already holds a full scan of (PLAN §11c). Leading the row rather than
/// trailing it so the marks line up in a column at the pane's edge, which is what makes a
/// prepared folder readable without reading any of it.
///
/// A character, like the `♪` beside it, and it leans on the same font fallback that one
/// already does on both targets.
const PREPARED: &str = "✓";

/// What the leading column is saying about a row (PLAN §11c).
///
/// One column and two states, because they are the two ends of the same sentence — this file
/// is being worked out, this file has been. A named pair rather than the glyph itself, so the
/// green that means *ready* is chosen by asking which state this is and not by comparing two
/// strings that happen to differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
	/// A thread has this file open right now.
	Working,
	/// The store holds every fact about it.
	Prepared,
}

/// Which of the two a row is in, if either.
///
/// **Working wins.** A file being re-scanned is already prepared — that is what re-preparing
/// means — and the row that says so is the one lying about what is happening to it.
fn mark_of(working: bool, prepared: bool) -> Option<Mark> {
	match (working, prepared) {
		(true, _) => Some(Mark::Working),
		(false, true) => Some(Mark::Prepared),
		(false, false) => None,
	}
}

/// What a load button promises, with this many rows highlighted (PLAN §9a).
///
/// The count only appears once there is more than one, because "→ Player 1" with five rows
/// selected is a different promise from the same button with one — and a `(1)` on every
/// ordinary single click would be noise on the commonest case there is.
fn load_label(id: DeckId, selected: usize) -> String {
	match selected {
		0 | 1 => format!("→ {}", id.label()),
		count => format!("→ {} ({count})", id.label()),
	}
}

/// Horizontal padding inside a row. Small: the pane is scanned, so more rows on screen
/// beats a roomier row. There is no vertical padding any more — the row is a fixed
/// `ROW_HEIGHT` with its contents centred, which is what makes the height a number rather
/// than a consequence.
const ROW_PADDING: [u16; 2] = [0, 6];

/// One row's height in pixels, **pinned** rather than left to the text inside it.
///
/// Virtualization has to know where a row *would* be without laying it out, and iced offers
/// no way to ask a widget how tall it turned out. Choosing the height instead of measuring
/// it makes `visible_rows` exact by construction — the same trick the players' panel uses
/// for its own height (PLAN §6): a number the layout is *told* is never a number the layout
/// has to be asked for.
const ROW_HEIGHT: f32 = 24.0;

/// The `scrollable`'s name, so choosing a folder can send it back to the top.
const FILES_SCROLL: &str = "files";

/// The pane's scrollable, addressable from `update`.
pub fn scroll_id() -> iced::advanced::widget::Id {
	iced::advanced::widget::Id::new(FILES_SCROLL)
}

/// `scanning` is how far a folder scan has got, as `(done, total)`, and `None` when none is
/// running (PLAN §11b). `working` is the handful of files a thread is decoding this moment and
/// `sweep` is how far round the animation is, which together are the spinner (PLAN §11c).
pub fn view<'a>(
	browser: &'a Browser,
	scanning: Option<(usize, usize)>,
	working: &[&Path],
	sweep: f32,
	tempos: &BTreeMap<PathBuf, f32>,
) -> Element<'a, Message> {
	// Counted rather than stored, because `visible` is the hidden filter and the count has
	// to be of what is *shown*. One pass over a few thousand `bool`s, against the thousands
	// of widgets this is here to not build.
	let total = browser.visible().count();
	let range = ui::visible_rows(browser.scroll, total, ROW_HEIGHT, ui::ROWS_BUILT);

	let rows = browser
		.visible()
		.skip(range.start)
		.take(range.len())
		// The mark is worked out here, per built row, rather than stored per entry: `working`
		// is at most six paths and only the rows on screen are ever asked about.
		.map(|entry| {
			let mark = mark_of(
				working.contains(&entry.path.as_path()),
				browser.is_prepared(&entry.path),
			);
			let ready = browser.ready(&entry.path);
			file_row(
				entry,
				browser.is_selected(&entry.path),
				mark,
				sweep,
				// The correction, where there is one, is applied here rather than kept in the
				// pane's map (PLAN §14d) — so the map stays a report of what the store said.
				ui::edited_tempo(tempos, &entry.path, ready.map(|ready| ready.tempo)),
				ready.map(|ready| ready.music()),
			)
		});

	// The rows above and below, as their height and nothing else.
	let above = Space::new().height(range.start as f32 * ROW_HEIGHT);
	let below = Space::new().height((total - range.end) as f32 * ROW_HEIGHT);

	column![
		header(browser, scanning, !tempos.is_empty()),
		scrollable(column![above, column(rows), below])
			.spacing(ui::SCROLLBAR_GAP)
			.id(scroll_id())
			.on_scroll(Message::Scrolled)
			.height(Fill),
	]
	.spacing(6)
	.padding(8)
	.into()
}

/// Where we are, and everything that can be done to the listing as a whole.
fn header(
	browser: &Browser,
	scanning: Option<(usize, usize)>,
	edited: bool,
) -> Element<'_, Message> {
	let folder = match &browser.folder {
		Some(folder) => folder.display().to_string(),
		None => "no folder".to_string(),
	};

	// Loading needs a media file selected: the buttons say what they will do, and go
	// dead rather than complaining afterwards. The *count* is on the button once there is more
	// than one, because "→ Player 1" with five rows highlighted is a different promise from
	// the same button with one (PLAN §9a).
	let selected = browser.selected_media().len();

	let load_into = move |id: DeckId| {
		button(text(load_label(id, selected)).size(12))
			.padding([3, 8])
			.on_press_maybe((selected > 0).then_some(Message::LoadSelected(id)))
	};

	column![
		row![
			text(format!("FILES — {folder}")).size(12).width(Fill),
			button(text("Open folder…").size(12))
				.padding([3, 8])
				.on_press(Message::OpenFolderPressed),
			button(text("Refresh").size(12))
				.padding([3, 8])
				.on_press(Message::RefreshPressed),
		]
		.spacing(6),
		row![
			checkbox(browser.show_hidden)
				.label(".* hidden")
				.text_size(12)
				.size(14)
				.on_toggle(Message::HiddenToggled),
			Space::new().width(Fill),
			load_into(DeckId::One),
			load_into(DeckId::Two),
		]
		.spacing(6)
		.align_y(iced::Center),
		preparation(browser, scanning, edited),
	]
	.spacing(6)
	.into()
}

/// Working the folder out ahead of time, and throwing all of it away again (PLAN §11b, §11a).
///
/// A row of its own under the two that are about *this listing*, because these two are about
/// the tree under it and about the cache — the same folder, a different scope. While a scan
/// runs the row becomes the count and a way to stop, since starting a second one over the
/// first is the only thing the buttons could otherwise do.
fn preparation(
	browser: &Browser,
	scanning: Option<(usize, usize)>,
	edited: bool,
) -> Element<'_, Message> {
	if let Some((done, total)) = scanning {
		return row![
			text(format!("preparing {done} of {total} files…"))
				.size(12)
				.width(Fill),
			button(text("Stop").size(12))
				.padding([3, 8])
				.on_press(Message::ScanFolderCancelled),
		]
		.spacing(6)
		.align_y(iced::Center)
		.into();
	}

	row![
		Space::new().width(Fill),
		button(text("Prepare selected").size(12))
			.padding([3, 8])
			.on_press_maybe(
				browser
					.has_media_selection()
					.then_some(Message::ScanSelectedPressed)
			),
		button(text("Prepare folder").size(12))
			.padding([3, 8])
			// Dead until there is a folder to walk, like every other control that needs one.
			.on_press_maybe(browser.folder.as_ref().map(|_| Message::ScanFolderPressed)),
		button(text("Clear cache").size(12))
			.padding([3, 8])
			.on_press(Message::ClearCachePressed),
		// Beside it, because they are the same shape of button and deliberately not the same
		// thing: that one throws away work, this one throws away *decisions* (PLAN §14d).
		button(text("Clear BPM edits").size(12))
			.padding([3, 8])
			.on_press_maybe(edited.then_some(Message::ClearTempoEditsPressed)),
	]
	.spacing(6)
	.align_y(iced::Center)
	.into()
}

/// One file. Single click selects, double click loads it into whichever player is idle —
/// the same rule an unaimed OS drop uses (PLAN §10).
///
/// `mark` is the leading column: a turning glyph while a thread is on this file, a `✓` once the
/// store holds a full scan of it, and nothing at all otherwise (PLAN §11c). One column for both,
/// because they are the two ends of the same sentence — this file is being worked out, this file
/// has been — and a row is only ever in one of those states.
///
/// `ready` is everything the same scan worked out — the tempo and how long the music runs — which
/// is why both arrive with the `✓` rather than behind it (PLAN §14c, §14d). Before the size,
/// because they are the questions actually being asked of a folder of tracks: the size is what a
/// file *is*, and these two are what it is *for*.
fn file_row<'a>(
	entry: &'a Entry,
	selected: bool,
	mark: Option<Mark>,
	sweep: f32,
	tempo: Option<Option<f32>>,
	music: Option<Option<Duration>>,
) -> Element<'a, Message> {
	// Green for a file that is ready, the same green the waveform draws the music's edges in;
	// the spinner keeps the row's own colour, since it is saying "wait", not "good".
	let prepared = mark == Some(Mark::Prepared);
	let mark = text(match mark {
		Some(Mark::Working) => ui::spinner(sweep),
		Some(Mark::Prepared) => PREPARED,
		None => "",
	})
	.size(12)
	.width(MARK_WIDTH)
	.style(move |theme: &Theme| iced::widget::text::Style {
		color: prepared.then(|| theme.extended_palette().success.base.color),
	});

	let cells = row![
		mark,
		text(entry.kind.glyph()).size(13).width(GLYPH_WIDTH),
		text(&entry.name).size(13).width(Fill),
		text(ui::format_tempo(tempo))
			.size(12)
			.width(TEMPO_WIDTH)
			.align_x(Right),
		// Only the whole answer is drawn here: the pane has never had a length column, so one
		// that showed a file's raw length would be a second new column rather than this one.
		text(ui::format_lengths(None, music))
			.size(12)
			.width(MUSIC_WIDTH)
			.align_x(Right),
		text(ui::format_size(entry.size))
			.size(12)
			.width(SIZE_WIDTH)
			.align_x(Right),
		text(ui::format_date(entry.modified))
			.size(12)
			.width(DATE_WIDTH)
			.align_x(Left),
	]
	.spacing(8);

	let body = container(cells)
		.padding(ROW_PADDING)
		.width(Fill)
		// Fixed, and the reason `visible_rows` can be arithmetic instead of a measurement.
		.height(ROW_HEIGHT)
		.align_y(iced::Center)
		.style(move |theme: &Theme| ui::row_style(theme, selected));

	let area = mouse_area(body).on_press(Message::RowSelected(entry.path.clone()));

	// Only media loads, and only media has a menu. A double click on a `.txt` selects it and
	// does nothing else, which is the honest outcome — the row is listed so the user can see the
	// folder is the right one (PLAN §9).
	if entry.kind.is_media() {
		area.on_double_click(Message::LoadUnaimed(entry.path.clone()))
			.on_right_press(Message::RowMenuOpened(entry.path.clone()))
			.into()
	} else {
		area.into()
	}
}

// `visible_rows` and `row_style` moved to `ui/mod.rs`, which all three lists share. What is left
// here that is not a widget is the two decisions below.
#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_file_being_worked_out_says_so_even_though_it_is_already_ready() {
		// Arrange / Act / Assert: the ordinary three, in the order the pane fills up.
		assert_eq!(mark_of(false, false), None, "never scanned");
		assert_eq!(mark_of(true, false), Some(Mark::Working), "first scan");
		assert_eq!(mark_of(false, true), Some(Mark::Prepared), "done");

		// And the one that decides the precedence: **Prepare folder** over a folder already
		// prepared re-reads every file, and a row showing a settled `✓` through all of it is
		// the row lying about what is happening to it.
		assert_eq!(mark_of(true, true), Some(Mark::Working), "being re-scanned");
	}

	#[test]
	fn a_load_button_counts_the_rows_only_once_there_is_more_than_one() {
		// Arrange / Act / Assert: no count on the commonest case there is, and no count at all
		// on a dead button — which reads as the plain promise it will make once it wakes up.
		assert_eq!(load_label(DeckId::One, 0), "→ Player 1");
		assert_eq!(load_label(DeckId::One, 1), "→ Player 1");
		assert_eq!(load_label(DeckId::Two, 5), "→ Player 2 (5)");
	}
}
