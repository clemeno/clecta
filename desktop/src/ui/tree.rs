//! The folder tree pane (PLAN §9): one row per visible folder, indented by depth.
//!
//! Two hit areas per row on purpose. The arrow opens and closes the folder; the name
//! *shows* it in the files pane. Collapsing those into one click would mean a user
//! cannot look inside a folder without also unfolding its subfolders underneath them.

use std::path::Path;

use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, text};
use iced::{Element, Fill, Theme};

use crate::app::Message;
use crate::tree::{Row, Tree};
use crate::ui;

/// Indent per level, in pixels.
const INDENT: f32 = 14.0;

/// Width of the disclosure arrow's hit area. Wide enough to hit without aiming, narrow
/// enough that it does not eat the name.
const ARROW_WIDTH: f32 = 18.0;

const ROW_PADDING: [u16; 2] = [2, 4];

pub fn view<'a>(tree: &'a Tree, current: Option<&'a Path>) -> Element<'a, Message> {
	let rows = tree
		.rows()
		.into_iter()
		.map(|row| folder_row(row, current))
		.collect::<Vec<_>>();

	column![
		text("FOLDERS").size(12),
		scrollable(column(rows).spacing(1))
			.spacing(ui::SCROLLBAR_GAP)
			.height(Fill),
	]
	.spacing(6)
	.padding(8)
	.into()
}

fn folder_row<'a>(node: Row, current: Option<&Path>) -> Element<'a, Message> {
	let selected = current == Some(node.path.as_path());

	// An unlisted folder shows a closed arrow on spec: whether it has children is not
	// known until it is read, and reading every folder up front is the thing the lazy
	// tree exists to avoid (PLAN §9).
	let arrow: Element<Message> = if node.expandable {
		button(text(if node.expanded { "▾" } else { "▸" }).size(12))
			.padding(0)
			.width(ARROW_WIDTH)
			.style(button::text)
			.on_press(Message::FolderToggled(node.path.clone()))
			.into()
	} else {
		Space::new().width(ARROW_WIDTH).into()
	};

	let body = container(
		row![
			Space::new().width(INDENT * node.depth as f32),
			arrow,
			text(node.name).size(13).width(Fill),
		]
		.spacing(2)
		.align_y(iced::Center),
	)
	.padding(ROW_PADDING)
	.width(Fill)
	.style(move |theme: &Theme| row_style(theme, selected));

	mouse_area(body)
		.on_press(Message::FolderSelected(node.path))
		.into()
}

/// The folder currently shown in the files pane is highlighted, so the two panes visibly
/// agree about where the user is.
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
