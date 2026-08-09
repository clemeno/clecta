//! One queue's panel (PLAN §7a): a header that adds, rows that select, and a footer that
//! edits.
//!
//! Three of these are drawn, one under each column of the players row, and they differ only
//! in which `ListId` they carry — so this is one function and not three, and a rule added
//! here is added to all of them.

use iced::widget::{
	Space, button, checkbox, column, container, mouse_area, pick_list, row, scrollable, text,
};
use iced::{Center, Element, Fill, Theme};

use crate::app::{DropTarget, Message, Zone};
use crate::playlist::{ListId, Playlist, Transition};
use crate::ui;

/// One row's height, pinned for the same reason the files pane pins its own (PLAN §9): a
/// queue is a list that can grow, and a row whose height depends on its text is a row whose
/// position cannot be worked out.
const ROW_HEIGHT: f32 = 20.0;

/// How much of a track name fits before it is elided. Narrower than a player's title
/// (`ui/deck.rs`), because three of these share the width two players had.
const NAME_CHARS: usize = 22;

/// The insertion caret's thickness, and the height of the strip past the last row that means
/// "append". The caret is reserved between every pair of rows whether it is lit or not, so
/// showing it never shifts the row under the pointer.
const CARET_HEIGHT: f32 = 2.0;
const TAIL_HEIGHT: f32 = 12.0;

/// The pitch from one row to the next, which is *not* the row's height: every row carries the
/// caret reserved above it, and the pair moves as one. This is the number `visible_rows` needs
/// (PLAN §9) — getting it wrong by two pixels a row is a list that drifts out of its own
/// scrollbar.
const ROW_PITCH: f32 = ROW_HEIGHT + CARET_HEIGHT;

/// What a drag in flight is doing to *this* list.
///
/// `None` when nothing is being dragged, which is what turns the rows back into plain rows:
/// `mouse_area` reports every crossing otherwise, and three lists of rows would report a great
/// many. Passed as one value rather than three parameters because all of it arrives together
/// and none of it means anything on its own.
pub struct Dragging {
	/// Where a release would put the row, as the index of the caret it would land above.
	/// `Some` for at most one of the three lists (PLAN §7a).
	pub insertion: Option<usize>,
	/// Whether one of this list's scroll edges is armed, and which: `true` is the header,
	/// scrolling up.
	pub edge: Option<bool>,
}

/// The three `scrollable`s' names, so an autoscroll can reach one of them from `update`.
///
/// One id per list, because three panels sharing one would scroll together. Written out as an
/// array rather than built with `format!` because `Id::new` takes a `&'static str` — and
/// indexed by `ListId::index`, which is the same order everything else about the lists uses.
const SCROLL_IDS: [&str; 3] = ["queue-cue-1", "queue-common", "queue-cue-2"];

pub fn scroll_id(list: ListId) -> iced::advanced::widget::Id {
	iced::advanced::widget::Id::new(SCROLL_IDS[list.index()])
}

/// One list, drawn.
///
/// `addable` is what the files pane has selected, if it is something a queue can hold. It is
/// passed in rather than read here because all three panels ask the same question of the same
/// pane, and the answer should be worked out once. `scroll` is how far down the panel is,
/// which the view needs for the same reason the files pane does: to know which rows are worth
/// building (PLAN §9).
pub fn view<'a>(
	id: ListId,
	list: &'a Playlist,
	addable: bool,
	scroll: f32,
	dragging: Option<Dragging>,
) -> Element<'a, Message> {
	let insertion = dragging.as_ref().and_then(|drag| drag.insertion);
	let edge = dragging.as_ref().and_then(|drag| drag.edge);
	let held = dragging.is_some();

	let panel = container(
		column![
			edging(id, header(id, addable), true, held, edge == Some(true)),
			switches(id, list),
			scrollable(rows(id, list, held, insertion, scroll))
				.id(scroll_id(id))
				.on_scroll(move |viewport| Message::QueueScrolled(id, viewport.absolute_offset().y))
				.height(Fill),
			edging(id, footer(id, list), false, held, edge == Some(false)),
		]
		.spacing(4),
	)
	.style(container::bordered_box)
	.padding(6)
	.height(Fill);

	if !held {
		return panel.into();
	}

	// Leaving the panel is what clears the caret. Only the leave is handled here: entering is
	// a row's business, and a panel-level enter would fight the rows for the same pointer.
	mouse_area(panel)
		.on_exit(Message::DragOut(Zone::List(id)))
		.into()
}

/// The header and the footer, doubling as the list's scroll edges while a drag is in flight
/// (PLAN §7a).
///
/// They are the edges rather than two strips of their own for one reason: a strip that only
/// existed during a drag would shift every row the moment the drag began — the same feedback
/// loop the caret avoids by being reserved — and one reserved for ever would cost twenty
/// pixels of every list to be useful for a second at a time. The header and the footer are
/// already exactly the top and bottom edges of the rows, and mid-drag they have nothing else
/// to do.
///
/// Lit with the same fill a selected row gets, because it answers the same question: this is
/// the thing the pointer is on.
fn edging<'a>(
	id: ListId,
	body: Element<'a, Message>,
	up: bool,
	dragging: bool,
	armed: bool,
) -> Element<'a, Message> {
	// Wrapped whether or not anything is being dragged, so arming an edge changes its colour
	// and nothing else. A container that appeared with the drag would add its padding to the
	// panel and push every row down two pixels — the same feedback loop the caret is reserved
	// to avoid, and worse, because it would move the rows the moment a drag began.
	let lit = container(body)
		.padding([1, 2])
		.style(move |theme: &Theme| ui::browser::row_style(theme, armed));

	if !dragging {
		return lit.into();
	}

	mouse_area(lit)
		.on_enter(Message::ScrollEdge(id, up, true))
		.on_exit(Message::ScrollEdge(id, up, false))
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

/// What this list does when a player runs out of track (PLAN §7a).
///
/// A row of its own between the header and the rows, rather than two more controls in either
/// of them: the header already carries the two add buttons and the footer five edit buttons,
/// and three of these panels share the width two players had. Sitting on the list is what says
/// *this* list, which is the whole point of the setting being per list.
///
/// **Auto-play** goes dead while **Auto-load** is off, because nothing is handed over for it to
/// start — the same "dead rather than absent, and dead for a reason" rule the footer's buttons
/// follow. Dead rather than silently ignored: a ticked box that does nothing is a lie about
/// what the app will do at the end of the track.
/// …and *when* it does it (PLAN §7b), which is the third answer to the same question and
/// therefore sits with the other two rather than in the mixer or in a settings window.
///
/// A `pick_list` rather than a third checkbox, for the reason the crossfader's curve is one:
/// the two positions are not "on" and "off" but two different behaviours, both of which want
/// naming. It goes dead with the other switch for the same reason **Auto-play** does — a list
/// that hands nothing over has no transition to make.
fn switches(id: ListId, list: &Playlist) -> Element<'static, Message> {
	let auto_load = list.auto_load;

	let switch =
		|on: bool, label: &'static str| checkbox(on).label(label).text_size(11).size(12).spacing(4);

	column![
		row![
			switch(auto_load, "Auto-load").on_toggle(move |on| Message::QueueAutoLoad(id, on)),
			switch(list.auto_play, "Auto-play")
				.on_toggle_maybe(auto_load.then_some(move |on| Message::QueueAutoPlay(id, on))),
		]
		.spacing(10)
		.align_y(Center),
		pick_list(Transition::ALL, Some(list.transition), move |transition| {
			Message::QueueTransition(id, transition)
		})
		.text_size(11)
		.padding([1, 5])
		.width(Fill),
	]
	.spacing(4)
	.into()
}

/// The tracks, in the order they will play.
///
/// Virtualized exactly like the files pane, and with the same helper (PLAN §9) — a queue is
/// normally tens of rows where a folder is thousands, but "normally" is not a bound, and the
/// arithmetic was already written and already tested. The only difference is the pitch: a
/// queue reserves a caret above every row, so a row is 22 pixels from the next rather than 20.
fn rows<'a>(
	id: ListId,
	list: &'a Playlist,
	dragging: bool,
	insertion: Option<usize>,
	scroll: f32,
) -> Element<'a, Message> {
	let total = list.items().len();
	let range = ui::visible_rows(scroll, total, ROW_PITCH, ui::ROWS_BUILT);

	// The rows above and below the window, as their height and nothing else.
	let mut column = column![Space::new().height(range.start as f32 * ROW_PITCH)].width(Fill);

	for (index, item) in list
		.items()
		.iter()
		.enumerate()
		.skip(range.start)
		.take(range.len())
	{
		let selected = list.selected() == Some(index);

		let body = container(
			row![
				// The play order, which is the queue's whole point — without it the top row
				// is only "the one that happens to be first".
				text(format!("{}.", index + 1)).size(11).width(20.0),
				text(ui::elide_middle(&item.name, NAME_CHARS))
					.size(12)
					.width(Fill),
				// Blank until it has been measured, rather than a placeholder that would flick
				// to a number a moment later on every row of a freshly opened list.
				text(match item.duration {
					Some(Some(length)) => ui::format_clock(length),
					Some(None) => "--:--".to_string(),
					None => String::new(),
				})
				.size(11),
			]
			.spacing(4),
		)
		.padding([0, 4])
		.width(Fill)
		.height(ROW_HEIGHT)
		.align_y(Center)
		.style(move |theme: &Theme| ui::browser::row_style(theme, selected));

		// A press selects and arms a drag; a double click plays it now, jumping the queue.
		let area = mouse_area(body)
			.on_press(Message::QueueSelected(id, index))
			.on_double_click(Message::QueueLoad(id, index));

		// A whole row is one target and it means *above this row*, so the caret above it is
		// what shows where the drop lands. Attached only while a drag is in flight, like the
		// player panels: `mouse_area` reports every crossing otherwise, and a list of rows
		// would report a great many.
		let row: Element<'a, Message> = if dragging {
			area.on_enter(Message::DragOver(DropTarget::Row(id, index)))
				.into()
		} else {
			area.into()
		};

		column = column.push(caret(insertion == Some(index))).push(row);
	}

	column = column.push(Space::new().height((total - range.end) as f32 * ROW_PITCH));

	// The tail: the caret past the last row, and the target that means *append*. It is a real
	// strip rather than the empty space below the rows, because empty space inside a
	// `scrollable` is not a widget and cannot be entered — and dropping at the end of a list
	// is the commonest drop there is. It is also the only target an *empty* list has.
	let tail = container(Space::new().width(Fill).height(TAIL_HEIGHT));
	let tail: Element<'a, Message> = if dragging {
		mouse_area(tail)
			.on_enter(Message::DragOver(DropTarget::Row(id, total)))
			.into()
	} else {
		tail.into()
	};

	column
		.push(caret(insertion == Some(total)))
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
/// hand it to a neighbour — and how long the whole list runs for.
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
		text(running_time(list))
			.size(11)
			.width(Fill)
			.align_x(iced::Right),
		send(true),
	]
	.spacing(3)
	.align_y(Center)
	.into()
}

/// How many tracks, and how long they run for.
///
/// The `+` is the honest part: a list whose rows are still being measured, or that holds a
/// file the decoder could give no length for, adds up to *at least* this much. A running time
/// exists to be planned against, so one that quietly leaves rows out is worse than one that
/// says it is still counting.
fn running_time(list: &Playlist) -> String {
	let count = list.items().len();
	if count == 0 {
		return String::new();
	}

	let (total, whole) = list.total();
	format!(
		"{count} · {}{}",
		ui::format_clock(total),
		if whole { "" } else { "+" }
	)
}

/// The one thing in this file that is arithmetic rather than widgets. The rows' own
/// virtualization is tested where the helper lives (`ui/mod.rs`).
#[cfg(test)]
mod tests {
	use super::*;
	use std::path::PathBuf;
	use std::time::Duration;

	#[test]
	fn a_running_time_says_when_it_is_still_counting() {
		// Arrange
		let mut list = Playlist::from_paths(vec![PathBuf::from("/m/a.mp3")]);

		// Act / Assert: an empty list says nothing at all — a `0 · 0:00` in every empty panel
		// is three pieces of furniture saying there is nothing there.
		assert_eq!(running_time(&Playlist::default()), "");

		// Measured or not, the count is right; the `+` is what changes.
		assert_eq!(running_time(&list), "1 · 0:00+", "not measured yet");

		list.measured(&PathBuf::from("/m/a.mp3"), Some(Duration::from_secs(215)));
		assert_eq!(running_time(&list), "1 · 3:35", "measured");
	}
}
