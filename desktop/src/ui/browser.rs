//! The files pane (PLAN §9): a header, then one row per file.
//!
//! Rows are built by hand — `scrollable(column(rows))` — rather than with
//! `widget::table`, because `table` has no row element to carry a click or a selected
//! background. The layout spike is what settled that; PLAN §9 records why.

use std::ops::Range;

use iced::widget::{Space, button, checkbox, column, container, mouse_area, row, scrollable, text};
use iced::{Element, Fill, Left, Right, Theme};

use crate::app::Message;
use crate::browser::{Browser, Entry};
use crate::deck::DeckId;
use crate::ui;

/// Fixed column widths, in pixels. The name column takes what is left.
const GLYPH_WIDTH: f32 = 18.0;
const SIZE_WIDTH: f32 = 80.0;
const DATE_WIDTH: f32 = 92.0;

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

/// How many rows are built per frame, however long the folder is.
///
/// A window is clamped to 4096 points tall (`settings.rs`), so a pane can never show more
/// than 171 rows of `ROW_HEIGHT`; 200 covers that with room to spare for a scroll offset
/// that is a frame stale. A fixed count rather than the pane's measured height is what
/// keeps this to *one* number of state — and the margin is nearly free, since the whole
/// point is that 200 rows cost the same whether the folder holds 300 or 30 000.
const ROWS_BUILT: usize = 200;

/// The `scrollable`'s name, so choosing a folder can send it back to the top.
const FILES_SCROLL: &str = "files";

/// The pane's scrollable, addressable from `update`.
pub fn scroll_id() -> iced::advanced::widget::Id {
	iced::advanced::widget::Id::new(FILES_SCROLL)
}

/// Which rows are worth building, for a pane scrolled this far down a list this long
/// (PLAN §9).
///
/// The rest of the list is two blank blocks of exactly the right height, so the scrollbar is
/// the size it would have been and every row is where it would have been.
fn visible_rows(scroll: f32, total: usize) -> Range<usize> {
	// The last window that still fills the pane. Clamping to it means a scroll offset left
	// over from a longer listing shows the *end* of the new one rather than a blank pane,
	// which is exactly what the `scrollable` itself does when its content shrinks.
	let last = total.saturating_sub(ROWS_BUILT);

	// `as usize` saturates rather than wrapping: a negative offset lands on 0, and so does a
	// `NaN`. Worth relying on here, because this number indexes a list.
	let start = ((scroll / ROW_HEIGHT) as usize).min(last);

	start..(start + ROWS_BUILT).min(total)
}

pub fn view(browser: &Browser) -> Element<'_, Message> {
	// Counted rather than stored, because `visible` is the hidden filter and the count has
	// to be of what is *shown*. One pass over a few thousand `bool`s, against the thousands
	// of widgets this is here to not build.
	let total = browser.visible().count();
	let range = visible_rows(browser.scroll, total);

	let rows = browser
		.visible()
		.skip(range.start)
		.take(range.len())
		.map(|entry| {
			let selected = browser.selected.as_deref() == Some(entry.path.as_path());
			file_row(entry, selected)
		});

	// The rows above and below, as their height and nothing else.
	let above = Space::new().height(range.start as f32 * ROW_HEIGHT);
	let below = Space::new().height((total - range.end) as f32 * ROW_HEIGHT);

	column![
		header(browser),
		scrollable(column![above, column(rows), below])
			.id(scroll_id())
			.on_scroll(Message::Scrolled)
			.height(Fill),
	]
	.spacing(6)
	.padding(8)
	.into()
}

/// Where we are, and everything that can be done to the listing as a whole.
fn header(browser: &Browser) -> Element<'_, Message> {
	let folder = match &browser.folder {
		Some(folder) => folder.display().to_string(),
		None => "no folder".to_string(),
	};

	// Loading needs a media file selected: the buttons say what they will do, and go
	// dead rather than complaining afterwards.
	let loadable = browser
		.selection()
		.filter(|entry| entry.kind.is_media())
		.map(|entry| entry.path.clone());

	let load_into = |id: DeckId, path: &Option<std::path::PathBuf>| {
		button(text(format!("→ {}", id.label())).size(12))
			.padding([3, 8])
			.on_press_maybe(path.clone().map(|_| Message::LoadSelected(id)))
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
			load_into(DeckId::One, &loadable),
			load_into(DeckId::Two, &loadable),
		]
		.spacing(6)
		.align_y(iced::Center),
	]
	.spacing(6)
	.into()
}

/// One file. Single click selects, double click loads it into whichever player is idle —
/// the same rule an unaimed OS drop uses (PLAN §10).
fn file_row(entry: &Entry, selected: bool) -> Element<'_, Message> {
	let cells = row![
		text(entry.kind.glyph()).size(13).width(GLYPH_WIDTH),
		text(&entry.name).size(13).width(Fill),
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
		.style(move |theme: &Theme| row_style(theme, selected));

	let area = mouse_area(body).on_press(Message::RowSelected(entry.path.clone()));

	// Only media loads. A double click on a `.txt` selects it and does nothing else,
	// which is the honest outcome — the row is listed so the user can see the folder is
	// the right one (PLAN §9).
	if entry.kind.is_media() {
		area.on_double_click(Message::LoadUnaimed(entry.path.clone()))
			.into()
	} else {
		area.into()
	}
}

/// A selected row gets a filled background; every other row gets nothing, so the
/// selection is the only thing the eye is drawn to.
pub fn row_style(theme: &Theme, selected: bool) -> container::Style {
	if !selected {
		return container::Style::default();
	}

	let palette = theme.extended_palette();
	container::Style {
		background: Some(palette.primary.weak.color.into()),
		text_color: Some(palette.primary.weak.text),
		..container::Style::default()
	}
}

/// The virtualization's arithmetic, which is the only thing in this file a test can reach
/// (PLAN §9). A range that is wrong by one row leaves a blank strip where a row should be;
/// one that is wrong by a lot shows an empty pane over a folder full of files.
#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_short_folder_is_built_whole() {
		// Arrange / Act / Assert: the ordinary case — fewer files than the cap, so nothing is
		// virtualized at all and the pane behaves exactly as it did before.
		assert_eq!(visible_rows(0.0, 0), 0..0, "an empty folder");
		assert_eq!(visible_rows(0.0, 40), 0..40, "a music folder");
		assert_eq!(
			visible_rows(0.0, ROWS_BUILT),
			0..ROWS_BUILT,
			"exactly the cap"
		);
	}

	#[test]
	fn scrolling_moves_the_window_by_whole_rows() {
		// Arrange: a folder long enough that the window can move freely inside it.
		let total = 5_000;

		// Act / Assert: the row under the top edge is the first one built, and it is built
		// even when only its bottom pixel shows — a partly visible row is a visible row.
		assert_eq!(visible_rows(0.0, total).start, 0);
		assert_eq!(visible_rows(ROW_HEIGHT - 1.0, total).start, 0, "part way");
		assert_eq!(visible_rows(ROW_HEIGHT, total).start, 1, "exactly one down");
		assert_eq!(visible_rows(100.0 * ROW_HEIGHT, total).start, 100);

		// And the count is the cap wherever it sits.
		for row in [0, 1, 100, 4_000] {
			let range = visible_rows(row as f32 * ROW_HEIGHT, total);
			assert_eq!(range.len(), ROWS_BUILT, "at row {row}");
		}
	}

	#[test]
	fn the_end_of_a_list_still_fills_the_pane() {
		// Arrange: scrolled to the very bottom, and then past it — which is what a stale
		// offset from a longer listing looks like.
		let total = 5_000;

		// Act / Assert: the window stops at the last full pane instead of running off the
		// end, so the bottom of a folder is never a blank pane.
		let bottom = visible_rows(total as f32 * ROW_HEIGHT, total);
		assert_eq!(bottom, (total - ROWS_BUILT)..total, "scrolled to the end");

		let past = visible_rows(1_000_000.0, total);
		assert_eq!(past, bottom, "an offset left over from a longer folder");
	}

	#[test]
	fn an_impossible_offset_still_names_real_rows() {
		// Arrange / Act / Assert: `as usize` saturates rather than wrapping, which is what
		// this leans on — a negative or a `NaN` offset must land on the top of the list and
		// not on an index that would panic the `skip`/`take` below it.
		for scroll in [-1.0, -1_000_000.0, f32::NAN, f32::NEG_INFINITY] {
			assert_eq!(visible_rows(scroll, 5_000).start, 0, "scroll {scroll}");
		}

		// The other end: an infinite offset is just a very stale one.
		assert_eq!(visible_rows(f32::INFINITY, 300), 100..300);
	}
}
