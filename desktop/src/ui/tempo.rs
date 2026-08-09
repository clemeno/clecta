//! The row menu and the tempo editor (PLAN §14d): the app's first two panels drawn *over* the
//! window rather than in it.
//!
//! Every other dialog in clecta is a native one (`rfd`) — a folder picker, a warning with two
//! buttons — because that is what the OS draws better than we can. This one cannot be: it holds
//! a value that changes while it is open, and `rfd` offers buttons and a message and nothing
//! else. So it is built out of the same widgets as the rest of the app and laid over the top
//! with `stack`.

use iced::widget::{Space, button, column, container, mouse_area, row, stack, text};
use iced::{Center, Color, Element, Fill, Theme};

use crate::app::Message;
use crate::ui;

/// How wide the two panels are. Fixed rather than shrink-to-fit: the number in the editor
/// changes width as it is halved and doubled, and a panel that resized under the pointer would
/// move the buttons out from under it.
const PANEL_WIDTH: f32 = 260.0;

/// Lay a panel over the window, with the rest of it dimmed and clickable to dismiss.
///
/// The dimming is not decoration — it is the hit target. A panel that can only be closed by its
/// own buttons is a panel that traps the app if one of them is ever missed, so everything outside
/// it means "never mind", which is what Escape means too.
///
/// `ponytail:` centred, not at the pointer. iced's press messages carry no position and a pane
/// cannot ask how big it is, so the alternative was a cursor subscription publishing a message on
/// every mouse move in the app — which is the one thing §6 already refuses to leave switched on.
/// The upgrade is that subscription, armed by the press and disarmed by the menu.
pub fn over<'a>(body: Element<'a, Message>, panel: Element<'a, Message>) -> Element<'a, Message> {
	let shade = mouse_area(container(Space::new()).width(Fill).height(Fill).style(
		|_theme: &Theme| container::Style {
			background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.45).into()),
			..container::Style::default()
		},
	))
	.on_press(Message::MenuDismissed);

	let centred = container(panel)
		.width(Fill)
		.height(Fill)
		.align_x(Center)
		.align_y(Center);

	stack![body, shade, centred].into()
}

/// What can be done to one row. One entry today, and it is why this is a menu rather than a
/// second click target: the next thing that belongs to a row — a cue point, a rename — goes here
/// rather than on the row.
///
/// The entry is dead when the row has no tempo to work from, because `/2` and `×2` need a number
/// to start from and there is nothing here to type one with. A file nothing has scanned is a file
/// to prepare first, and the menu says so rather than opening an editor with nothing in it.
pub fn menu<'a>(name: &'a str, tempo: Option<f32>) -> Element<'a, Message> {
	let entry = |label: &'a str, message: Option<Message>| {
		button(text(label).size(12))
			.width(Fill)
			.padding([4, 8])
			.style(button::text)
			.on_press_maybe(message)
	};

	panel(column![
		text(name).size(12).width(Fill),
		entry("Edit BPM…", tempo.map(|_| Message::TempoEditOpened)),
		text(match tempo {
			Some(_) => "",
			None => "nothing has scanned this file yet",
		})
		.size(10),
	])
}

/// The editor: what the tempo is now, the two buttons that are the whole of the editing, and a
/// footer that decides.
///
/// Halving and doubling and nothing else, because that is what a wrong tempo is wrong by (PLAN
/// §14d). It is also exactly reversible — `×2` after `/2` gives back the number that was there,
/// bit for bit, since both are powers of two — which is why there is no third button to undo
/// with and no need to remember what the detector said.
pub fn editor<'a>(name: &'a str, value: f32, detected: Option<f32>) -> Element<'a, Message> {
	let scale = |label: &'a str, factor: f32| {
		button(text(label).size(14))
			.padding([4, 12])
			.on_press(Message::TempoScaled(factor))
	};

	// Said only when it differs, and said quietly: it is the answer to "what was it before I
	// started", which is the one question the two buttons cannot answer on their own.
	let was = match detected {
		Some(detected) if (detected - value).abs() > f32::EPSILON => {
			format!("detected {}", ui::format_tempo(Some(Some(detected))))
		}
		_ => String::new(),
	};

	panel(column![
		text(name).size(12).width(Fill),
		row![
			scale("/ 2", 0.5),
			container(text(ui::format_tempo(Some(Some(value)))).size(20))
				.width(Fill)
				.align_x(Center),
			scale("× 2", 2.0),
		]
		.align_y(Center)
		.spacing(8),
		text(was).size(10),
		row![
			Space::new().width(Fill),
			button(text("Cancel").size(12))
				.padding([4, 10])
				.on_press(Message::MenuDismissed),
			button(text("Apply").size(12))
				.padding([4, 10])
				.on_press(Message::TempoApplied),
		]
		.spacing(6),
	])
}

/// The box both panels sit in: one width, one border, one padding, so they cannot drift apart.
fn panel<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
	container(column![content.into()].spacing(8))
		.width(PANEL_WIDTH)
		.padding(12)
		.style(container::bordered_box)
		.into()
}
