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

/// The scanning band's thickness. Thicker than the flat line it rides on, so it reads as
/// something happening rather than as a defect in the line.
const SWEEP_BAR: f32 = 4.0;

/// One player's waveform: the scan, how far through it the playhead is, and whether a scan
/// is still running.
///
/// Borrows the scan rather than owning it — the array lives in the `Deck` and is rebuilt
/// only when a track is loaded, so cloning it into the view every frame would be a copy of
/// a couple of thousand floats for nothing.
struct Waveform<'a> {
	peaks: &'a [f32],
	/// `0.0..=1.0`, or `None` when there is nothing to measure against: an empty player,
	/// or a stream whose length the decoder could not work out (PLAN §7).
	progress: Option<f32>,
	/// The scanning animation's phase, `0.0..=1.0`, or `None` when no scan is running —
	/// which covers an empty player and a scan that failed as well as a finished one.
	sweep: Option<f32>,
}

/// The strip, ready to drop into a panel.
///
/// A function rather than a public struct, because there is nothing to configure: every
/// choice it could offer is a constant above, decided once for both players.
pub fn view<'a, Message: 'a>(
	peaks: &'a [f32],
	progress: Option<f32>,
	sweep: Option<f32>,
) -> Element<'a, Message> {
	Element::new(Waveform {
		peaks,
		progress,
		sweep,
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
