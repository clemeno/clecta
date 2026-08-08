//! One queue's panel (PLAN §7a): a header that adds, rows that select, and a footer that
//! edits.
//!
//! Three of these are drawn, one under each column of the players row, and they differ only
//! in which `ListId` they carry — so this is one function and not three, and a rule added
//! here is added to all of them.

use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, text};
use iced::{Center, Element, Fill, Theme};

use crate::app::{DropTarget, Message, Zone};
use crate::playlist::{ListId, Playlist};
use crate::ui;

/// One row's height, pinned for the same reason the files pane pins its own (PLAN §9): a
/// queue is a list that can grow, and a row whose height depends on its text is a row whose
/// position cannot be worked out.
const ROW_HEIGHT: f32 = 20.0;

/// How much of a track name fits before it is elided. Narrower than a player's title
/// (`ui/deck.rs`), because three of these share the width two players had.
const NAME_CHARS: usize = 26;

/// The insertion caret's thickness, and the height of the strip past the last row that means
/// "append". The caret is reserved between every pair of rows whether it is lit or not, so
/// showing it never shifts the row under the pointer.
const CARET_HEIGHT: f32 = 2.0;
const TAIL_HEIGHT: f32 = 12.0;

/// One list, drawn.
///
/// `addable` is what the files pane has selected, if it is something a queue can hold. It is
/// passed in rather than read here because all three panels ask the same question of the same
/// pane, and the answer should be worked out once. `dragging` says whether a drag is in
/// flight, which is what turns the rows into drop targets; `insertion` is where the caret
/// goes, and is `Some` for at most one of the three lists (PLAN §7a).
pub fn view<'a>(
	id: ListId,
	list: &'a Playlist,
	addable: bool,
	dragging: bool,
	insertion: Option<usize>,
) -> Element<'a, Message> {
	let panel = container(
		column![
			header(id, addable),
			scrollable(rows(id, list, dragging, insertion)).height(Fill),
			footer(id, list),
		]
		.spacing(4),
	)
	.style(container::bordered_box)
	.padding(6)
	.height(Fill);

	if !dragging {
		return panel.into();
	}

	// Leaving the panel is what clears the caret. Only the leave is handled here: entering is
	// a row's business, and a panel-level enter would fight the rows for the same pointer.
	mouse_area(panel)
		.on_exit(Message::DragOut(Zone::List(id)))
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
fn rows<'a>(
	id: ListId,
	list: &'a Playlist,
	dragging: bool,
	insertion: Option<usize>,
) -> Element<'a, Message> {
	let mut column = column![].width(Fill);

	for (index, item) in list.items().iter().enumerate() {
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

		// A whole row is one target and it means *above this row*, so the caret above it is
		// what shows where the drop lands. Attached only while a drag is in flight, like the
		// player panels: `mouse_area` reports every crossing otherwise, and a list of rows
		// would report a great many.
		let row: Element<'a, Message> = if dragging {
			mouse_area(body)
				.on_press(Message::QueueSelected(id, index))
				.on_enter(Message::DragOver(DropTarget::Row(id, index)))
				.into()
		} else {
			mouse_area(body)
				.on_press(Message::QueueSelected(id, index))
				.into()
		};

		column = column.push(caret(insertion == Some(index))).push(row);
	}

	// The tail: the caret past the last row, and the target that means *append*. It is a real
	// strip rather than the empty space below the rows, because empty space inside a
	// `scrollable` is not a widget and cannot be entered — and dropping at the end of a list
	// is the commonest drop there is. It is also the only target an *empty* list has.
	let tail = container(Space::new().width(Fill).height(TAIL_HEIGHT));
	let tail: Element<'a, Message> = if dragging {
		mouse_area(tail)
			.on_enter(Message::DragOver(DropTarget::Row(id, list.items().len())))
			.into()
	} else {
		tail.into()
	};

	column
		.push(caret(insertion == Some(list.items().len())))
		.push(tail)
		.into()
}

/// The line showing where a dropped row would land.
///
/// Drawn between every pair of rows at all times and merely *coloured* when it is the target,
/// so a caret appearing never moves the row under the pointer — which would change the target
/// as a side effect of showing it.
fn caret(lit: bool) -> Element<'static, Message> {
	container(Space::new().width(Fill).height(CARET_HEIGHT))
		.style(move |theme: &Theme| {
			container::Style::default().background(if lit {
				// The same green the drop ring uses, because it answers the same question.
				theme.extended_palette().success.strong.color
			} else {
				iced::Color::TRANSPARENT
			})
		})
		.into()
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
