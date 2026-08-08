//! One queue's panel (PLAN §7a): a header that adds, rows that select, and a footer that
//! edits.
//!
//! Three of these are drawn, one under each column of the players row, and they differ only
//! in which `ListId` they carry — so this is one function and not three, and a rule added
//! here is added to all of them.

use iced::widget::{button, column, container, mouse_area, row, scrollable, text};
use iced::{Center, Element, Fill, Theme};

use crate::app::Message;
use crate::playlist::{ListId, Playlist};
use crate::ui;

/// One row's height, pinned for the same reason the files pane pins its own (PLAN §9): a
/// queue is a list that can grow, and a row whose height depends on its text is a row whose
/// position cannot be worked out.
const ROW_HEIGHT: f32 = 20.0;

/// How much of a track name fits before it is elided. Narrower than a player's title
/// (`ui/deck.rs`), because three of these share the width two players had.
const NAME_CHARS: usize = 26;

/// One list, drawn.
///
/// `addable` is what the files pane has selected, if it is something a queue can hold. It is
/// passed in rather than read here because all three panels ask the same question of the
/// same pane, and the answer should be worked out once.
pub fn view<'a>(id: ListId, list: &'a Playlist, addable: bool) -> Element<'a, Message> {
	container(
		column![
			header(id, addable),
			scrollable(rows(id, list)).height(Fill),
			footer(id, list),
		]
		.spacing(4),
	)
	.style(container::bordered_box)
	.padding(6)
	.height(Fill)
	.into()
}

/// The list's name, and the two ways a file from the browser gets into it.
///
/// The add buttons live here rather than in the files pane because there are three lists and
/// two ways in: six buttons in one header would each need a label saying which list they
/// meant, where a button sitting *on* the list needs none (PLAN §7a).
fn header(id: ListId, addable: bool) -> Element<'static, Message> {
	let add = |glyph: &'static str, prepend: bool| {
		button(text(glyph).size(12))
			.padding([1, 5])
			.on_press_maybe(addable.then_some(Message::QueueAdd(id, prepend)))
	};

	row![
		text(id.label()).size(12).width(Fill),
		add("⤒", true),
		add("⤓", false),
	]
	.spacing(4)
	.align_y(Center)
	.into()
}

/// The tracks, in the order they will play.
///
/// Every row is built, unlike the files pane (PLAN §9): a queue is something a person types
/// into one track at a time, so it is tens of rows where a folder is thousands. If a queue
/// ever grows to where that matters, `visible_rows` is next door and already tested.
fn rows<'a>(id: ListId, list: &'a Playlist) -> Element<'a, Message> {
	let rows = list.items().iter().enumerate().map(|(index, item)| {
		let selected = list.selected() == Some(index);

		let body = container(
			row![
				// The play order, which is the queue's whole point — without it the top row
				// is only "the one that happens to be first".
				text(format!("{}.", index + 1)).size(11).width(20.0),
				text(ui::elide_middle(&item.name, NAME_CHARS)).size(12),
			]
			.spacing(4),
		)
		.padding([0, 4])
		.width(Fill)
		.height(ROW_HEIGHT)
		.align_y(Center)
		.style(move |theme: &Theme| ui::browser::row_style(theme, selected));

		mouse_area(body)
			.on_press(Message::QueueSelected(id, index))
			.into()
	});

	column(rows).into()
}

/// Everything that can be done to the selected row: take it out, move it within the list, or
/// hand it to a neighbour.
///
/// The `←` and `→` sit at the outer edges, facing the list they send to, so the pair either
/// side of a gap reads as one control *between* two lists rather than two controls belonging
/// to one (PLAN §7a).
fn footer(id: ListId, list: &Playlist) -> Element<'static, Message> {
	let selected = list.selected();
	let count = list.items().len();

	let edit = |glyph: &'static str, message: Option<Message>| {
		button(text(glyph).size(12))
			.padding([1, 5])
			.on_press_maybe(message)
	};

	// Dead rather than absent, and dead for a *reason* each time: nothing selected, already
	// at the top, already at the bottom, or no neighbour in that direction.
	let send = |right: bool| {
		let glyph = if right { "→" } else { "←" };
		let allowed = selected.is_some() && id.neighbour(right).is_some();
		edit(glyph, allowed.then_some(Message::QueueShift(id, right)))
	};

	row![
		send(false),
		edit(
			"▲",
			selected
				.filter(|index| *index > 0)
				.map(|_| Message::QueueMove(id, true))
		),
		edit(
			"▼",
			selected
				.filter(|index| index + 1 < count)
				.map(|_| Message::QueueMove(id, false))
		),
		edit("✕", selected.map(|_| Message::QueueRemove(id))),
		text(format!("{count}"))
			.size(11)
			.width(Fill)
			.align_x(iced::Right),
		send(true),
	]
	.spacing(3)
	.align_y(Center)
	.into()
}
