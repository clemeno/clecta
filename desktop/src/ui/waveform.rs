//! The waveform strip in a player's panel (PLAN §14a) — the app's one custom widget.
//!
//! Everything else under `ui/` composes widgets iced already has. This one cannot: a bar
//! per pixel column is not a `row` of four hundred elements, and the shape has to be
//! re-fitted to whatever width the panel happens to have this frame. So it implements
//! `advanced::Widget` directly, which as a *picture* was three methods — `size`, `layout`,
//! `draw` — because every other one has a default that is already right for a widget with
//! no children, no state and no events.
//!
//! `fill_quad` is the whole drawing API used here, and it is the same primitive every
//! built-in widget's background is made of: there is no second, lower rendering layer
//! being reached for.
//!
//! It is now a control as well as a picture: `update` turns a press into a seek and a drag
//! into a scrub, which cost the four methods PLAN §14 said they would — `update` for the
//! events, `mouse_interaction` so the pointer says the strip is a control before it is
//! touched, and `tag` / `state` for the one bit the gesture has to remember.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::{Element, Event, Length, Rectangle, Size, Theme, mouse};

use crate::waveform;

/// The strip's height. Fixed rather than `Fill`: the panel's other rows are single lines
/// of text, so a waveform that grew with the window would swallow the space the browser
/// below is competing for (PLAN §6).
const HEIGHT: f32 = 56.0;

/// The playhead's width in pixels. One is invisible against a dense waveform.
const PLAYHEAD: f32 = 2.0;

/// The shortest bar drawn for a column that has *any* signal in it. Digital silence stays
/// flat, but a quiet passage still reads as "there is a track here", which is the
/// difference between a fade-out and a scan that has not landed yet.
const MIN_BAR: f32 = 1.0;

/// The scanning band's thickness. Thicker than the flat line it rides on, so it reads as
/// something happening rather than as a defect in the line.
const SWEEP_BAR: f32 = 4.0;

/// One player's waveform: the scan, how far through it the playhead is, and whether a scan
/// is still running.
///
/// Borrows the scan rather than owning it — the array lives in the `Deck` and is rebuilt
/// only when a track is loaded, so cloning it into the view every frame would be a copy of
/// a couple of thousand floats for nothing.
struct Waveform<'a, Message> {
	peaks: &'a [f32],
	/// `0.0..=1.0`, or `None` when there is nothing to measure against: an empty player,
	/// or a stream whose length the decoder could not work out (PLAN §7).
	///
	/// Doubles as the answer to "is this strip clickable?", which is not a coincidence: a
	/// strip with no total to place a playhead against has no total to seek within either.
	progress: Option<f32>,
	/// The scanning animation's phase, `0.0..=1.0`, or `None` when no scan is running —
	/// which covers an empty player and a scan that failed as well as a finished one.
	sweep: Option<f32>,
	/// What a click means, as a fraction of the track. The widget deliberately does not
	/// know what a second is: turning the fraction into a `Duration` needs the track's
	/// length, which lives in the `Deck`.
	on_seek: Box<dyn Fn(f32) -> Message + 'a>,
}

/// The one thing the strip remembers, and the reason it has a `Tree` state at all: whether
/// the pointer is dragging along it. A *click* needs no memory, which is why this widget had
/// none until scrubbing was added (PLAN §14).
#[derive(Debug, Default)]
struct State {
	scrubbing: bool,
}

/// What a mouse event does to a strip, given whether the pointer is over it and whether a
/// scrub is already under way.
///
/// Pure and separate from `update` for the same reason `deck::transition` is separate from
/// the app: the rules of the gesture are three lines of `match` and every one of them is
/// wrong in a way a window would have to be opened to notice. Tested at the bottom of this
/// file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scrub {
	/// Arm the drag, and seek to where the pointer went down.
	Start,
	/// Already armed: follow the pointer, inside the strip or not.
	Follow,
	/// Disarm. Not a seek — the playhead is already where the drag left it.
	Stop,
	Ignore,
}

/// The gesture, as a function of the event and the two bits of context around it.
fn scrub(event: &Event, over: bool, scrubbing: bool) -> Scrub {
	match event {
		// A press *inside* the strip only. Elsewhere in the window it belongs to something
		// else, and arming on it would make the next mouse move seek out of nowhere.
		Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if over => Scrub::Start,
		// A move is followed wherever it goes once armed — over the panel above, past either
		// end of the strip, outside the window. `seek_fraction` clamps, so leaving the strip
		// parks the playhead at the edge it left by, which is what makes a scrub forgiving of
		// a hand that wanders.
		Event::Mouse(mouse::Event::CursorMoved { .. }) if scrubbing => Scrub::Follow,
		// A release disarms wherever it happens, `over` or not: a button let go outside the
		// strip is still let go, and a strip left armed would scrub on the next stray move.
		Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Scrub::Stop,
		_ => Scrub::Ignore,
	}
}

/// The strip, ready to drop into a panel.
///
/// A function rather than a public struct, because there is nothing to configure: every
/// choice it could offer is a constant above, decided once for both players.
pub fn view<'a, Message: 'a>(
	peaks: &'a [f32],
	progress: Option<f32>,
	sweep: Option<f32>,
	on_seek: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
	Element::new(Waveform {
		peaks,
		progress,
		sweep,
		on_seek: Box::new(on_seek),
	})
}

/// Implemented for the concrete `Theme` rather than a generic one, because the colours are
/// read from its palette. Naming roles instead of colours is what keeps the strip legible
/// if the theme ever stops being `Dark` — but a *role* is not automatically a contrast, and
/// the first attempt at this widget proved it by drawing nothing anyone could see: the bed
/// was `background.weak`, which is exactly what `container::rounded_box` paints the panel
/// behind it, and the bars were `background.strong`, thirteen levels of grey above that in
/// a bar one pixel wide. The four roles below were picked by printing the palette and
/// comparing, which is the only way to know:
///
/// | part | role | dark theme |
/// |---|---|---|
/// | the bed | `background.weakest` | `#323439`, darker than the panel's `#43464e` |
/// | not yet played | `secondary.base` | `#878a90` |
/// | already played | `primary.base` | `#5865f2` |
/// | the playhead | `danger.base` | `#c3423f` |
impl<Message, Renderer> Widget<Message, Theme, Renderer> for Waveform<'_, Message>
where
	Renderer: renderer::Renderer,
{
	fn size(&self) -> Size<Length> {
		Size::new(Length::Fill, Length::Fixed(HEIGHT))
	}

	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<State>()
	}

	fn state(&self) -> tree::State {
		tree::State::new(State::default())
	}

	/// A left press inside the strip is a seek, and holding it is a scrub (PLAN §14).
	///
	/// Press, not release: a transport control should answer the instant the button goes
	/// down, and waiting for the release would make a click that drifted a few pixels feel
	/// like it went somewhere else. A scrub is then the same seek repeated, which is why
	/// adding it needed no new message and no new arm in the app — the widget publishes the
	/// fraction it always published, just more often.
	///
	/// `ponytail:` one seek per pointer move, and `Engine::seek` blocks the GUI thread until
	/// the audio thread has done it. Fine for a local file, where the seek is a format-level
	/// jump rather than a decode; if a slow source ever makes a scrub stutter, the fix is to
	/// coalesce the moves within a frame rather than to make the widget cleverer.
	fn update(
		&mut self,
		tree: &mut Tree,
		event: &Event,
		layout: Layout<'_>,
		cursor: mouse::Cursor,
		_renderer: &Renderer,
		_clipboard: &mut dyn Clipboard,
		shell: &mut Shell<'_, Message>,
		_viewport: &Rectangle,
	) {
		let bounds = layout.bounds();
		let state = tree.state.downcast_mut::<State>();

		// The strip's guard rather than the gesture's, so it is checked before the state
		// machine and an empty player can never arm one: a strip with no length has nothing
		// for a fraction to be a fraction of. Disarming rather than merely returning covers
		// the track being unloaded mid-drag.
		if self.progress.is_none() {
			state.scrubbing = false;
			return;
		}

		match scrub(event, cursor.is_over(bounds), state.scrubbing) {
			Scrub::Start => state.scrubbing = true,
			Scrub::Follow => {}
			Scrub::Stop => {
				state.scrubbing = false;
				return;
			}
			Scrub::Ignore => return,
		}

		// The event's own position for a move, because that is the one the move is *about*;
		// the cursor for the press, which is over the strip by the time `Start` is returned.
		let position = match event {
			Event::Mouse(mouse::Event::CursorMoved { position }) => Some(*position),
			_ => cursor.position(),
		};
		let Some(position) = position else {
			return;
		};

		if let Some(fraction) = waveform::seek_fraction(bounds.width, position.x - bounds.x) {
			shell.publish((self.on_seek)(fraction));
			// Nothing under this strip handles a left press today — the panel's `mouse_area`
			// exists only while a drag is armed and only watches enter/exit. This is the
			// contract every built-in control keeps anyway: a widget that acted on a click
			// says so, rather than letting it fall through and be acted on twice.
			shell.capture_event();
		}
	}

	/// A pointer over a strip that can be seeked, and nothing over one that cannot — so an
	/// empty player is visibly not a control, without needing a disabled look.
	///
	/// It stays a pointer for as long as a scrub is held, wherever the cursor has wandered
	/// to: the gesture still belongs to this strip, and a cursor that changed shape halfway
	/// through would say it had been dropped.
	fn mouse_interaction(
		&self,
		tree: &Tree,
		layout: Layout<'_>,
		cursor: mouse::Cursor,
		_viewport: &Rectangle,
		_renderer: &Renderer,
	) -> mouse::Interaction {
		let scrubbing = tree.state.downcast_ref::<State>().scrubbing;

		if self.progress.is_some() && (scrubbing || cursor.is_over(layout.bounds())) {
			mouse::Interaction::Pointer
		} else {
			mouse::Interaction::None
		}
	}

	/// `atomic`, because this widget has no children and no intrinsic size to negotiate:
	/// it takes the width it is given and the height it asked for.
	fn layout(
		&mut self,
		_tree: &mut Tree,
		_renderer: &Renderer,
		limits: &layout::Limits,
	) -> layout::Node {
		layout::atomic(limits, Length::Fill, Length::Fixed(HEIGHT))
	}

	fn draw(
		&self,
		_tree: &Tree,
		renderer: &mut Renderer,
		theme: &Theme,
		_style: &renderer::Style,
		layout: Layout<'_>,
		_cursor: mouse::Cursor,
		_viewport: &Rectangle,
	) {
		let bounds = layout.bounds();
		let palette = theme.extended_palette();

		// The bed, drawn first and always: an empty player, or one whose scan is still
		// running, has to read as a control rather than as a gap in the panel. Which means
		// it has to be *darker* than the panel, not merely a different name for it.
		renderer.fill_quad(
			renderer::Quad {
				bounds,
				border: iced::border::rounded(2.0),
				..Default::default()
			},
			palette.background.weakest.color,
		);

		// One bar per pixel of the width actually laid out — which is why the scan's own
		// length is not the loop bound (PLAN §14a).
		let columns = bounds.width.max(0.0) as usize;
		let middle = bounds.y + bounds.height / 2.0;
		let played = self.progress.unwrap_or(0.0) * bounds.width;

		// A flat line while there is no scan: an empty player, or the moment before a track's
		// shape arrives. Without it the strip is a bare rectangle, which reads as broken
		// rather than as waiting — which is exactly how the first version of this widget was
		// reported, when a contrast mistake made every bar invisible.
		if self.peaks.is_empty() {
			renderer.fill_quad(
				renderer::Quad {
					bounds: Rectangle {
						x: bounds.x,
						y: middle - MIN_BAR / 2.0,
						width: bounds.width,
						height: MIN_BAR,
					},
					..Default::default()
				},
				palette.secondary.base.color,
			);

			// A band travelling along that line while the decode runs. It says "working"
			// rather than "empty", and it is drawn here rather than written in the status
			// bar because this is the thing being waited for (PLAN §14a).
			if let Some((start, end)) = self
				.sweep
				.and_then(|phase| waveform::sweep_band(bounds.width, phase))
			{
				renderer.fill_quad(
					renderer::Quad {
						bounds: Rectangle {
							x: bounds.x + start,
							y: middle - SWEEP_BAR / 2.0,
							width: end - start,
							height: SWEEP_BAR,
						},
						border: iced::border::rounded(SWEEP_BAR / 2.0),
						..Default::default()
					},
					palette.primary.base.color,
				);
			}
		}

		for column in 0..columns {
			let peak = waveform::column_peak(self.peaks, column, columns);
			// Silence is drawn as nothing at all, not as a hairline: a gap between tracks
			// should look like a gap.
			if peak <= 0.0 {
				continue;
			}

			// Mirrored about the middle, because a waveform is an envelope and not a bar
			// chart growing off the floor.
			let half = (peak * bounds.height / 2.0).max(MIN_BAR / 2.0);
			let colour = if (column as f32) < played {
				palette.primary.base.color
			} else {
				palette.secondary.base.color
			};

			renderer.fill_quad(
				renderer::Quad {
					bounds: Rectangle {
						x: bounds.x + column as f32,
						y: middle - half,
						width: 1.0,
						height: half * 2.0,
					},
					..Default::default()
				},
				colour,
			);
		}

		// Last, so it is never buried under a loud bar — and only when there is a length to
		// place it against, since a position with no total means no position on screen.
		if self.progress.is_some() {
			renderer.fill_quad(
				renderer::Quad {
					bounds: Rectangle {
						// Clamped so the head stays inside the strip at the very end of a
						// track rather than hanging over the panel's edge.
						x: bounds.x + played.clamp(0.0, (bounds.width - PLAYHEAD).max(0.0)),
						y: bounds.y,
						width: PLAYHEAD,
						height: bounds.height,
					},
					..Default::default()
				},
				palette.danger.base.color,
			);
		}
	}
}

/// The gesture's rules, which are the only thing in this file a test can reach: everything
/// else here is a `fill_quad` (PLAN §14).
#[cfg(test)]
mod tests {
	use super::*;

	fn pressed() -> Event {
		Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
	}

	fn released() -> Event {
		Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
	}

	fn moved() -> Event {
		Event::Mouse(mouse::Event::CursorMoved {
			position: iced::Point::new(10.0, 10.0),
		})
	}

	#[test]
	fn a_press_arms_a_scrub_only_over_the_strip() {
		// Arrange / Act / Assert: a press elsewhere in the window must leave the strip
		// disarmed, or the next mouse move anywhere would seek a track nobody touched.
		assert_eq!(scrub(&pressed(), true, false), Scrub::Start, "over");
		assert_eq!(scrub(&pressed(), false, false), Scrub::Ignore, "elsewhere");
	}

	#[test]
	fn a_move_seeks_only_while_the_button_is_held() {
		// Arrange / Act / Assert: the whole difference between a scrub and a hover. Note
		// the second case — armed and *outside* the strip still follows, which is what lets
		// a drag run past either end.
		assert_eq!(scrub(&moved(), true, true), Scrub::Follow, "held, over");
		assert_eq!(scrub(&moved(), false, true), Scrub::Follow, "held, outside");
		assert_eq!(
			scrub(&moved(), true, false),
			Scrub::Ignore,
			"merely hovering"
		);
	}

	#[test]
	fn a_release_disarms_wherever_it_happens() {
		// Arrange / Act / Assert: a scrub that ended over the mixer, the browser or off the
		// window is still ended. A strip left armed would scrub on the next stray move.
		for over in [true, false] {
			assert_eq!(scrub(&released(), over, true), Scrub::Stop, "over: {over}");
		}
	}

	#[test]
	fn nothing_else_touches_the_playhead() {
		// Arrange: the events that pass through this widget in quantity and must cost it
		// nothing — the right button, and the wheel over a strip.
		let others = [
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)),
			Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Right)),
			Event::Mouse(mouse::Event::CursorLeft),
		];

		// Act / Assert: including while armed, since a right-click mid-scrub must not end it.
		for event in others {
			assert_eq!(scrub(&event, true, true), Scrub::Ignore, "{event:?}");
		}
	}
}
