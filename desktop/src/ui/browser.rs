//! The files pane (PLAN §9): a header, then one row per file.
//!
//! Rows are built by hand — `scrollable(column(rows))` — rather than with
//! `widget::table`, because `table` has no row element to carry a click or a selected
//! background. The layout spike is what settled that; PLAN §9 records why.

use std::path::Path;

use iced::widget::{Space, button, checkbox, column, container, mouse_area, row, scrollable, text};
use iced::{Element, Fill, Left, Right, Theme};

use crate::app::Message;
use crate::browser::{Browser, Entry};
use crate::deck::DeckId;
use crate::ui;

/// Fixed column widths, in pixels. The name column takes what is left.
const MARK_WIDTH: f32 = 14.0;
const GLYPH_WIDTH: f32 = 18.0;
const SIZE_WIDTH: f32 = 80.0;
const DATE_WIDTH: f32 = 92.0;

/// A file the store already holds a full scan of (PLAN §11c). Leading the row rather than
/// trailing it so the marks line up in a column at the pane's edge, which is what makes a
/// prepared folder readable without reading any of it.
///
/// A character, like the `♪` beside it, and it leans on the same font fallback that one
/// already does on both targets.
const PREPARED: &str = "✓";

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
			let mark = if working.contains(&entry.path.as_path()) {
				Some(ui::spinner(sweep))
			} else if browser.is_prepared(&entry.path) {
				Some(PREPARED)
			} else {
				None
			};
			file_row(entry, browser.is_selected(&entry.path), mark)
		});

	// The rows above and below, as their height and nothing else.
	let above = Space::new().height(range.start as f32 * ROW_HEIGHT);
	let below = Space::new().height((total - range.end) as f32 * ROW_HEIGHT);

	column![
		header(browser, scanning),
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
fn header(browser: &Browser, scanning: Option<(usize, usize)>) -> Element<'_, Message> {
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
		let label = match selected {
			0 | 1 => format!("→ {}", id.label()),
			count => format!("→ {} ({count})", id.label()),
		};
		button(text(label).size(12))
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
		preparation(browser, scanning),
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
fn preparation(browser: &Browser, scanning: Option<(usize, usize)>) -> Element<'_, Message> {
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
fn file_row<'a>(
	entry: &'a Entry,
	selected: bool,
	mark: Option<&'static str>,
) -> Element<'a, Message> {
	// Green for a file that is ready, the same green the waveform draws the music's edges in;
	// the spinner keeps the row's own colour, since it is saying "wait", not "good".
	let prepared = mark == Some(PREPARED);
	let mark = text(mark.unwrap_or(""))
		.size(12)
		.width(MARK_WIDTH)
		.style(move |theme: &Theme| iced::widget::text::Style {
			color: prepared.then(|| theme.extended_palette().success.base.color),
		});

	let cells = row![
		mark,
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

// The one testable thing that was in this file moved to `ui/mod.rs` with `visible_rows`
// itself, which the queues now share (PLAN §9). Everything left here builds widgets.
