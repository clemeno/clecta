//! The band above the players and the mixer (PLAN §14f): each player's rhythm close up,
//! scrolling past one shared playhead, with its beats and bars marked.
//!
//! What it is for is a comparison the two strips in the panels cannot make. They show a whole
//! track each, six hundred pixels apart, at whatever scale fits it; this shows the same few
//! seconds of *both*, one above the other, at one scale — so two kicks that land together line
//! up, and two that do not are visibly apart, and which way.
//!
//! The scale is **real time**, not the file's: a player running at 104 % passes 4.16 seconds
//! of its file under the playhead in four seconds, so its beats are drawn closer together.
//! That is the whole point of the band — two tracks at different tempi, matched with the rate
//! sliders, show matched spacing here — and it is why `Lane` carries the rate.
//!
//! A picture and nothing else: no events, no state, the same three methods the strip began
//! with (`ui/waveform.rs`), drawn with the same `fill_quad`.

use std::time::Duration;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Tree, Widget};
use iced::{Element, Length, Rectangle, Size, Theme, mouse};

use crate::waveform::RHYTHM_STEP;

/// The band's height: two lanes and the gap between them.
pub const HEIGHT: f32 = 2.0 * LANE + GAP;

/// One lane's height. Enough for a kick to stand clearly taller than what is between kicks,
/// and small enough that the band costs the players below it less than a queue row or two.
pub const LANE: f32 = 34.0;

/// The gap between the two lanes.
pub const GAP: f32 = 4.0;

/// How much real time the band shows either side of the playhead, in seconds.
///
/// Four: about six beats each way at 90 BPM and eight at 128 — enough to see a drift coming a
/// bar ahead, few enough that one beat is still forty-odd pixels wide in a window of a
/// thousand, which is what lets two kicks a sixteenth apart be seen to be apart.
const WINDOW: f64 = 4.0;

/// The slowest rate the band will draw at. A slider at zero is a pause, and its lane would
/// otherwise divide by it; this slow, four seconds of output is a fifth of a second of file,
/// which reads as "stopped" without being a special case.
const MIN_RATE: f64 = 0.05;

/// The gap kept between a lane's loudest bar and its edges, like the strip's (PLAN Q73).
const PADDING: f32 = 1.0;

/// Widths, in pixels: the shared playhead, a bar line, a beat tick.
const PLAYHEAD: f32 = 2.0;
const BAR_LINE: f32 = 2.0;
const BEAT_TICK: f32 = 1.0;

/// How far a beat tick reaches in from each edge of its lane, as a fraction of the lane. A bar
/// line runs the lane's full height; a beat is marked at the edges only, so the loudness it
/// sits on stays readable between the two halves of the mark.
const TICK: f32 = 0.22;

/// A track's beat grid, as the band needs it: how fast, and where a bar starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
	/// After any correction (PLAN §14d), so the grid agrees with the number the panel shows.
	pub bpm: f32,
	pub downbeat: Duration,
}

/// One player's lane: what to draw, and where the playhead is in it.
#[derive(Debug, Clone, Copy)]
pub struct Lane<'a> {
	/// The loudness every `RHYTHM_STEP`, empty for an empty player or a scan still running.
	pub envelope: &'a [u8],
	pub position: Duration,
	/// The player's rate slider: how many seconds of file pass in one second of output.
	pub rate: f32,
	pub grid: Option<Grid>,
}

/// Both lanes, Player 1 above Player 2.
struct Band<'a> {
	lanes: [Lane<'a>; 2],
}

/// The band, ready to drop into the layout.
pub fn view<'a, Message: 'a>(lanes: [Lane<'a>; 2]) -> Element<'a, Message> {
	Element::new(Band { lanes })
}

/// The colours are roles from the palette, like the strip's — and the same roles where the
/// meaning is the same, so the two read as one design: played in `primary`, to come in
/// `secondary`, the playhead in `danger`. A beat tick is the background's stronger shade, at
/// the lane's edges and under the music; a bar line is the text colour at half strength, over
/// it — checked on a screenshot, where a bar line drawn under the music read as a beat tick.
impl<Message, Renderer> Widget<Message, Theme, Renderer> for Band<'_>
where
	Renderer: renderer::Renderer,
{
	fn size(&self) -> Size<Length> {
		Size::new(Length::Fill, Length::Fixed(HEIGHT))
	}

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
		let centre = (bounds.width / 2.0).floor();
		let per_second = f64::from(bounds.width) / (2.0 * WINDOW);
		let fill = |renderer: &mut Renderer, rect: Rectangle, colour| {
			renderer.fill_quad(
				renderer::Quad {
					bounds: rect,
					..Default::default()
				},
				colour,
			);
		};

		for (index, lane) in self.lanes.iter().enumerate() {
			let top = bounds.y + index as f32 * (LANE + GAP);
			let middle = top + LANE / 2.0;
			let rate = f64::from(lane.rate).max(MIN_RATE);
			let position = lane.position.as_secs_f64();
			// Where a moment of the file is drawn, in pixels from the band's left edge.
			let place = |time: f64| centre as f64 + (time - position) / rate * per_second;

			renderer.fill_quad(
				renderer::Quad {
					bounds: Rectangle {
						x: bounds.x,
						y: top,
						width: bounds.width,
						height: LANE,
					},
					border: iced::border::rounded(2.0),
					..Default::default()
				},
				palette.background.weakest.color,
			);

			let reach = WINDOW * rate;
			let grid: Vec<(f32, bool)> = lane.grid.map_or_else(Vec::new, |grid| {
				beats(grid, position - reach, position + reach)
					.into_iter()
					.map(|(time, bar)| (place(time) as f32, bar))
					.collect()
			});

			// The beat ticks first, at the lane's edges, so the loudness is drawn over them.
			for (x, _) in grid.iter().filter(|(_, bar)| !bar) {
				for y in [top, top + LANE * (1.0 - TICK)] {
					fill(
						renderer,
						Rectangle {
							x: bounds.x + x,
							y,
							width: BEAT_TICK,
							height: LANE * TICK,
						},
						palette.background.strong.color,
					);
				}
			}

			// One bar per pixel column, each the loudest step of file time it covers.
			let room = LANE / 2.0 - PADDING;
			for column in 0..bounds.width.max(0.0) as usize {
				let from = position + (column as f64 - centre as f64) / per_second * rate;
				let to = from + rate / per_second;
				let loudness = level(lane.envelope, from, to);
				if loudness == 0 {
					continue;
				}
				let half = (contrast(loudness) * room).max(0.5);
				fill(
					renderer,
					Rectangle {
						x: bounds.x + column as f32,
						y: middle - half,
						width: 1.0,
						height: half * 2.0,
					},
					if (column as f32) < centre {
						palette.primary.base.color
					} else {
						palette.secondary.base.color
					},
				);
			}

			// The bar lines over the music, in the text's own colour at half strength: they are
			// what two lanes are lined up by, so they must read through a loud passage, where a
			// line under the loudness shows only as two stubs no different from a beat's.
			let text = palette.background.base.text;
			for (x, _) in grid.iter().filter(|(_, bar)| *bar) {
				fill(
					renderer,
					Rectangle {
						x: bounds.x + x - BAR_LINE / 2.0,
						y: top,
						width: BAR_LINE,
						height: LANE,
					},
					iced::Color { a: 0.5, ..text },
				);
			}
		}

		// Last, and across both lanes: one playhead, which is what the two are compared at.
		fill(
			renderer,
			Rectangle {
				x: bounds.x + centre - PLAYHEAD / 2.0,
				y: bounds.y,
				width: PLAYHEAD,
				height: bounds.height,
			},
			palette.danger.base.color,
		);
	}
}

/// How much of a lane's half-height a loudness fills, `0..=1`: squared, which is what makes a
/// kick stand out of a loud passage. The envelope is a mean level, and in a loud passage the
/// space between kicks is not much quieter than the kicks — 0.8 against 1.0, say, which drawn
/// straight is a ragged block. Squared it is 0.64 against 1.0, and the quiet stays quiet.
fn contrast(loudness: u8) -> f32 {
	let level = f32::from(loudness) / 255.0;
	level * level
}

/// The loudest envelope step between two moments of the file, in seconds — `0` where the file
/// is not, before its start or past its end, so a track's edges show as the band going quiet.
fn level(envelope: &[u8], from: f64, to: f64) -> u8 {
	let step = RHYTHM_STEP.as_secs_f64();
	if to <= 0.0 || envelope.is_empty() || !from.is_finite() || !to.is_finite() {
		return 0;
	}
	let first = (from.max(0.0) / step) as usize;
	// At least the step `from` is in, however narrow the column: zoomed in this far, several
	// columns share one step, and each must still draw it.
	let last = ((to / step).ceil() as usize)
		.max(first + 1)
		.min(envelope.len());
	envelope
		.get(first..last)
		.map_or(0, |steps| steps.iter().copied().max().unwrap_or(0))
}

/// Every beat of a grid between two moments of the file, as `(time, is the first of a bar)`.
///
/// Counted from the downbeat in both directions, so the beats before it — the first bar of a
/// track often starts a beat or three in — are on the grid too, and a bar is every fourth one
/// either way. Nothing before the file's start: there is no music there to mark.
fn beats(grid: Grid, from: f64, to: f64) -> Vec<(f64, bool)> {
	let period = 60.0 / f64::from(grid.bpm);
	if !(period.is_finite() && period > 0.0) {
		return Vec::new();
	}
	let downbeat = grid.downbeat.as_secs_f64();
	let first = ((from - downbeat) / period).ceil() as i64;
	let last = ((to - downbeat) / period).floor() as i64;
	(first..=last)
		.map(|beat| (downbeat + beat as f64 * period, beat.rem_euclid(4) == 0))
		.filter(|(time, _)| *time >= 0.0)
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_column_shows_the_loudest_step_it_covers() {
		// Arrange: four steps of 10 ms.
		let envelope = [10, 200, 30, 40];

		// Act / Assert: a column spanning the first two takes the loud one; one inside a single
		// step — zoomed in, several columns share a step — still draws that step.
		assert_eq!(level(&envelope, 0.0, 0.02), 200);
		assert_eq!(level(&envelope, 0.021, 0.022), 30);
	}

	#[test]
	fn outside_the_file_the_band_is_quiet() {
		// Arrange / Act / Assert: before the start and past the end draw nothing, and so does a
		// player with no envelope yet — a scan still running.
		let envelope = [255, 255];
		assert_eq!(level(&envelope, -1.0, -0.5), 0, "before the start");
		assert_eq!(level(&envelope, 5.0, 6.0), 0, "past the end");
		assert_eq!(level(&[], 0.0, 1.0), 0, "nothing scanned");
		assert_eq!(level(&envelope, f64::NAN, 0.01), 0, "not a number");
	}

	#[test]
	fn every_fourth_beat_from_the_downbeat_starts_a_bar_both_ways() {
		// Arrange: 120 BPM, a beat every half second, the first bar starting 1.25 s in.
		let grid = Grid {
			bpm: 120.0,
			downbeat: Duration::from_millis(1250),
		};

		// Act: from the top of the file to 3.3 s.
		let found = beats(grid, -1.0, 3.3);

		// Assert: the beats before the downbeat are there and belong to the bar before it, and
		// nothing is placed before the file starts.
		assert_eq!(
			found,
			[
				(0.25, false),
				(0.75, false),
				(1.25, true),
				(1.75, false),
				(2.25, false),
				(2.75, false),
				(3.25, true),
			]
		);
	}

	#[test]
	fn a_grid_with_no_tempo_marks_nothing() {
		// Arrange / Act / Assert: a period that is not a number must not become a loop over it.
		for bpm in [0.0, -120.0, f32::NAN] {
			let grid = Grid {
				bpm,
				downbeat: Duration::ZERO,
			};
			assert!(beats(grid, 0.0, 10.0).is_empty(), "{bpm} BPM");
		}
	}
}
