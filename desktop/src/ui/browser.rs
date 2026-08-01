//! The files pane (PLAN §9): a header, then one row per file.
//!
//! Rows are built by hand — `scrollable(column(rows))` — rather than with
//! `widget::table`, because `table` has no row element to carry a click or a selected
//! background. The layout spike is what settled that; PLAN §9 records why.

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

/// Vertical padding inside a row. Small: the pane is scanned, so more rows on screen
/// beats a roomier row.
const ROW_PADDING: [u16; 2] = [3, 6];

pub fn view(browser: &Browser) -> Element<'_, Message> {
	let rows = browser.visible().map(|entry| {
		let selected = browser.selected.as_deref() == Some(entry.path.as_path());
		file_row(entry, selected)
	});

	column![
		header(browser),
		scrollable(column(rows).spacing(1)).height(Fill),
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
fn row_style(theme: &Theme, selected: bool) -> container::Style {
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
