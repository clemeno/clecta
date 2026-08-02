//! The waveform strip in a player's panel (PLAN §14a) — the app's one custom widget.
//!
//! Everything else under `ui/` composes widgets iced already has. This one cannot: a bar
//! per pixel column is not a `row` of four hundred elements, and the shape has to be
//! re-fitted to whatever width the panel happens to have this frame. So it implements
//! `advanced::Widget` directly, which turns out to be three methods — `size`, `layout`,
//! `draw` — because every other one has a default that is already right for a widget with
//! no children, no state and no events.
//!
//! `fill_quad` is the whole drawing API used here, and it is the same primitive every
//! built-in widget's background is made of: there is no second, lower rendering layer
//! being reached for.
//!
//! Display only, deliberately: no `update`, so clicking it does not seek yet (PLAN §14).

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Tree, Widget};
use iced::{Element, Length, Rectangle, Size, Theme, mouse};

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

/// One player's waveform: the scan, and how far through it the playhead is.
///
/// Borrows the scan rather than owning it — the array lives in the `Deck` and is rebuilt
/// only when a track is loaded, so cloning it into the view every frame would be a copy of
/// a couple of thousand floats for nothing.
struct Waveform<'a> {
	peaks: &'a [f32],
	/// `0.0..=1.0`, or `None` when there is nothing to measure against: an empty player,
	/// or a stream whose length the decoder could not work out (PLAN §7).
	progress: Option<f32>,
}

/// The strip, ready to drop into a panel.
///
/// A function rather than a public struct, because there is nothing to configure: every
/// choice it could offer is a constant above, decided once for both players.
pub fn view<'a, Message: 'a>(peaks: &'a [f32], progress: Option<f32>) -> Element<'a, Message> {
	Element::new(Waveform { peaks, progress })
}

/// Implemented for the concrete `Theme` rather than a generic one, because the colours are
/// read from its palette — the played part is `primary`, the rest `background.strong`, and
/// the playhead `danger`. Naming roles instead of colours is what keeps the strip legible
/// if the theme ever stops being `Dark`.
impl<Message, Renderer> Widget<Message, Theme, Renderer> for Waveform<'_>
where
	Renderer: renderer::Renderer,
{
	fn size(&self) -> Size<Length> {
		Size::new(Length::Fill, Length::Fixed(HEIGHT))
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
		// running, has to read as a control rather than as a gap in the panel.
		renderer.fill_quad(
			renderer::Quad {
				bounds,
				border: iced::border::rounded(2.0),
				..Default::default()
			},
			palette.background.weak.color,
		);

		// One bar per pixel of the width actually laid out — which is why the scan's own
		// length is not the loop bound (PLAN §14a).
		let columns = bounds.width.max(0.0) as usize;
		let middle = bounds.y + bounds.height / 2.0;
		let played = self.progress.unwrap_or(0.0) * bounds.width;

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
				palette.primary.strong.color
			} else {
				palette.background.strong.color
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
