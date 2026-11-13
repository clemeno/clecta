//! The row menu and the tempo editor (PLAN §14d): the app's first two panels drawn *over* the
//! window rather than in it.
//!
//! Every other dialog in clecta is a native one (`rfd`) — a folder picker, a warning with two
//! buttons — because that is what the OS draws better than we can. This one cannot be: it holds
//! a value that changes while it is open, and `rfd` offers buttons and a message and nothing
//! else. So it is built out of the same widgets as the rest of the app and laid over the top
//! with `stack`.

use iced::widget::{Space, button, column, container, mouse_area, row, stack, text};
use iced::{Center, Color, Element, Fill, Point, Theme};

use crate::app::Message;
use crate::ui;

/// How wide the two panels are. Fixed rather than shrink-to-fit: the number in the editor
/// changes width as it is halved and doubled, and a panel that resized under the pointer would
/// move the buttons out from under it.
const PANEL_WIDTH: f32 = 260.0;

/// How tall each panel is, near enough to place it by — **pinned, not measured**, like the
/// status bar's height and the row pitches (PLAN §6, §9): a widget cannot be asked how tall it
/// is until it has been laid out, and the placement has to be decided before that. Only the
/// flip decision below reads these, so a few pixels out moves a panel a few pixels, and the
/// panel itself is still laid out at whatever size it really needs.
const MENU_HEIGHT: f32 = 92.0;
const EDITOR_HEIGHT: f32 = 148.0;

/// One of the two panels, and how the window behind it is treated while it is up.
///
/// A struct rather than two loose arguments so the caller cannot get the pairing wrong: the
/// height belongs to the panel that was built, and `over` is the only thing that reads it.
pub struct Panel<'a> {
	body: Element<'a, Message>,
	height: f32,
	/// Whether the window behind is dimmed. The **editor** is a decision with an Apply button
	/// and dims, the way a modal does; the **menu** is a context menu at the pointer, and a
	/// veil over the whole window for a three-line menu would read as far more than it is.
	dim: bool,
}

/// Lay a panel over the window **at the pointer**, with the rest of it clickable to dismiss.
///
/// The full-window catcher behind the panel is not decoration — it is the hit target. A panel
/// that can only be closed by its own buttons is a panel that traps the app if one of them is
/// ever missed, so everything outside it means "never mind", which is what Escape means too.
/// Whether that catcher is *visible* is the panel's own business (`Panel::dim`).
///
/// `at` is where the right press happened, from `ui::pointer` — `None` only if the cursor had
/// no position at the press, which leaves the panel centred, the way both panels used to be.
///
/// With no panel up, the window is still put in a stack, alone: iced keeps a widget's state
/// only while the same kind of widget stays at the same place in its tree, and a window that
/// was the root without a panel and a stack's first layer with one lost every list's scroll
/// offset each time a menu opened or closed.
pub fn over<'a>(
	body: Element<'a, Message>,
	panel: Option<(Panel<'a>, Option<Point>)>,
	window: (f32, f32),
) -> Element<'a, Message> {
	let Some((panel, at)) = panel else {
		return stack![body].into();
	};
	let dim = panel.dim;
	let catcher = mouse_area(container(Space::new()).width(Fill).height(Fill).style(
		move |_theme: &Theme| container::Style {
			background: dim.then(|| Color::from_rgba(0.0, 0.0, 0.0, 0.45).into()),
			..container::Style::default()
		},
	))
	.on_press(Message::MenuDismissed);

	let placed: Element<'a, Message> = match at {
		Some(at) => {
			let (left, top) = placed_at(at, (PANEL_WIDTH, panel.height), window);
			// Offsets as space rather than padding, because a `container` padding is inside
			// its own width and this has to push the panel across the whole window.
			column![
				Space::new().height(top),
				row![Space::new().width(left), panel.body],
			]
			.into()
		}
		None => container(panel.body)
			.width(Fill)
			.height(Fill)
			.align_x(Center)
			.align_y(Center)
			.into(),
	};

	stack![body, catcher, placed].into()
}

/// Where the panel's top-left goes, given where the pointer was pressed.
///
/// Down and to the right of the pointer, like every menu on both platforms — **flipped** to the
/// other side of the pointer when that would not fit, rather than slid along the edge: a menu
/// pinned to the bottom of the window sits over the rows below the one that was clicked, and
/// the row that was clicked is the one thing the menu must not hide.
///
/// Clamped to zero last, so a window too small for the panel puts it in the corner instead of
/// off the top-left — which is the same thing every other measurement here does with a window
/// nobody expected (PLAN §6).
fn placed_at(at: Point, panel: (f32, f32), window: (f32, f32)) -> (f32, f32) {
	let flipped = |position: f32, panel: f32, window: f32| {
		if position + panel <= window {
			position
		} else {
			(position - panel).max(0.0)
		}
	};

	(
		flipped(at.x, panel.0, window.0),
		flipped(at.y, panel.1, window.1),
	)
}

/// What can be done to one row. One entry today, and it is why this is a menu rather than a
/// second click target: the next thing that belongs to a row — a cue point, a rename — goes here
/// rather than on the row.
///
/// The entry is dead when the row has no tempo to work from, because `/2` and `×2` need a number
/// to start from and there is nothing here to type one with. A file nothing has scanned is a file
/// to prepare first, and the menu says so rather than opening an editor with nothing in it.
pub fn menu<'a>(name: &'a str, tempo: Option<f32>) -> Panel<'a> {
	let entry = |label: &'a str, message: Option<Message>| {
		button(text(label).size(12))
			.width(Fill)
			.padding([4, 8])
			.style(button::text)
			.on_press_maybe(message)
	};

	Panel {
		body: panel(column![
			text(name).size(12).width(Fill),
			entry(
				"Correct tempo…",
				tempo.map(|_| Message::TempoCorrectionOpened)
			),
			text(match tempo {
				Some(_) => "",
				None => "nothing has scanned this file yet",
			})
			.size(10),
		]),
		height: MENU_HEIGHT,
		dim: false,
	}
}

/// The editor: what the tempo is now, the two buttons that are the whole of the editing, and a
/// footer that decides.
///
/// Halving and doubling and nothing else, because that is what a wrong tempo is wrong by (PLAN
/// §14d). It is also exactly reversible — `×2` after `/2` gives back the number that was there,
/// bit for bit, since both are powers of two — which is why there is no third button to undo
/// with and no need to remember what the detector said.
pub fn editor<'a>(name: &'a str, value: f32, detected: Option<f32>) -> Panel<'a> {
	let scale = |label: &'a str, factor: f32| {
		button(text(label).size(14))
			.padding([4, 12])
			.on_press(Message::TempoScaled(factor))
	};

	let body = panel(column![
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
		text(detected_line(value, detected)).size(10),
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
	]);

	Panel {
		body,
		height: EDITOR_HEIGHT,
		dim: true,
	}
}

/// The box both panels sit in: one width, one border, one padding, so they cannot drift apart.
fn panel<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
	container(column![content.into()].spacing(8))
		.width(PANEL_WIDTH)
		.padding(12)
		.style(container::bordered_box)
		.into()
}

/// What the detector said, when that is not what is on screen — the one question `/2` and `×2`
/// cannot answer between them.
///
/// Blank while the two agree, so opening the editor and closing it again shows nothing new, and
/// blank on a file nothing has scanned, where the number on screen is a correction and there is
/// no "before" to go back to.
///
/// The comparison is against the *shown* two decimals rather than the bits, because that is
/// what the line is for: `f32::EPSILON` is about 1.2e-7, so halving and doubling a tempo back to
/// where it started can leave a difference that passes an exact test and prints identically —
/// a line reading "detected 128.00" under a 128.00.
fn detected_line(value: f32, detected: Option<f32>) -> String {
	let shown = ui::format_tempo(Some(Some(value)));
	match detected.map(|detected| ui::format_tempo(Some(Some(detected)))) {
		Some(was) if was != shown => format!("detected {was}"),
		_ => String::new(),
	}
}

/// The two decisions in this file. Everything else builds widgets (PLAN §12).
#[cfg(test)]
mod tests {
	use super::*;
	use iced::advanced::widget::Tree;
	use iced::widget::scrollable;

	/// A panel opening, or closing, must leave the window under it where it was in iced's
	/// widget tree. iced keeps a widget's state — a list's scroll offset among it — only
	/// while the same kind of widget stays at the same place in the tree, and rebuilds it
	/// otherwise. The window was the root with no panel up and a stack's first layer with
	/// one, so a right-click put the files list back at its top while the app still drew the
	/// rows it had scrolled to, and it jumped. Checked as the tree's own test, the kinds:
	/// seeing the offset survive needs a renderer.
	#[test]
	fn a_panel_opening_leaves_the_window_where_it_was_in_the_tree() {
		// Arrange
		const WINDOW: (f32, f32) = (1000.0, 700.0);
		let window = || -> Element<'static, Message> { scrollable(Space::new()).into() };

		// Act
		let closed = Tree::new(over(window(), None, WINDOW));
		let open = Tree::new(over(
			window(),
			Some((menu("a.mp3", None), Some(Point::new(10.0, 10.0)))),
			WINDOW,
		));

		// Assert
		assert_eq!(closed.tag, open.tag, "what the window sits in");
		assert_eq!(
			closed.children.first().map(|window| window.tag),
			open.children.first().map(|window| window.tag),
			"the window itself"
		);
	}

	#[test]
	fn a_panel_opens_below_and_right_of_the_pointer_until_that_would_not_fit() {
		// Arrange: a panel of 260 × 92 — the menu's own size — in an ordinary window.
		const PANEL: (f32, f32) = (260.0, 92.0);
		const WINDOW: (f32, f32) = (1000.0, 700.0);

		// Act / Assert: the ordinary case is the pointer itself, so the panel hangs off the
		// press the way a menu does on both platforms.
		assert_eq!(
			placed_at(Point::new(300.0, 200.0), PANEL, WINDOW),
			(300.0, 200.0)
		);

		// Near the right edge it flips to the *other side* of the pointer rather than sliding
		// along the edge, and near the bottom likewise — a menu slid up the edge would cover
		// the row that was clicked, which is the one row it must not hide.
		let corner = placed_at(Point::new(900.0, 650.0), PANEL, WINDOW);
		assert_eq!(corner, (640.0, 558.0), "flipped both ways");

		// Exactly fitting is fitting: 740 + 260 == 1000 stays put rather than flipping.
		assert_eq!(placed_at(Point::new(740.0, 100.0), PANEL, WINDOW).0, 740.0);

		// And a window too small for the panel at all puts it in the corner rather than off
		// the top-left, where nothing could be clicked.
		assert_eq!(
			placed_at(Point::new(40.0, 30.0), PANEL, (200.0, 80.0)),
			(0.0, 0.0)
		);
	}

	#[test]
	fn the_detected_tempo_is_said_only_while_it_differs_from_what_is_shown() {
		// Arrange / Act / Assert: the ordinary correction, and the ordinary non-correction.
		assert_eq!(detected_line(87.0, Some(174.0)), "detected 174.00");
		assert_eq!(detected_line(174.0, Some(174.0)), "", "nothing changed yet");

		// A correction on a file nothing has scanned has no "before" to offer.
		assert_eq!(detected_line(128.0, None), "");

		// And the case the two buttons make: halved and doubled back, which is exact for a
		// power of two — and would still have to read as unchanged if it were not, since the
		// line compares the two decimals the panel is showing.
		assert_eq!(detected_line(174.0 / 2.0 * 2.0, Some(174.0)), "");
		assert_eq!(
			detected_line(97.464_9, Some(97.464_1)),
			"",
			"same to a hundredth"
		);
		assert_eq!(detected_line(97.47, Some(97.46)), "detected 97.46");
	}
}
