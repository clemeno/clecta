//! One player's panel (PLAN §6, §7): what is loaded, where the playhead is, and the
//! three transport buttons.

use std::time::Duration;

use iced::widget::{button, column, container, row, text};
use iced::{Center, Element, Fill, Theme};

use crate::app::Message;
use crate::deck::{self, Deck, DeckId, Transport};
use crate::ui;
use crate::waveform::Trim;

/// How much of a track name fits before it is elided. Fixed rather than measured: a
/// responsive elide needs the widget's real width, which only `advanced` gives (PLAN §3).
const TITLE_CHARS: usize = 34;

/// The drop ring's thickness. Thick enough to read as an answer to "where is this going?"
/// from across the window, which is the whole job it has.
const RING_WIDTH: f32 = 2.0;

/// `ring` lights this player as the one a release would land on — under the cursor for an
/// in-app drag, derived for an OS drop, and never on both (PLAN §10). `sweep` is the
/// scanning animation's phase, which the strip uses only while this player is being
/// scanned (PLAN §14a). `trim` is where the music inside the loaded track sits, if anything
/// has worked it out — the two jump buttons and the strip's two marks (PLAN §14c).
pub fn view(
	id: DeckId,
	deck: &Deck,
	trim: Option<Trim>,
	ring: bool,
	sweep: f32,
) -> Element<'_, Message> {
	let loaded = deck.transport.has_track();
	// Asked once and used three times below. The decoder cannot always answer it (PLAN §7),
	// and every one of the three has a different thing to do about that.
	let length = deck.track.as_ref().and_then(|track| track.duration);

	// Disabled rather than hidden: buttons that come and go make the panel jump, and the
	// user learns the layout from the empty state.
	let transport_button = |label: &'static str, event: deck::Event, enabled: bool| {
		button(text(label).size(18))
			.padding([4, 14])
			.on_press_maybe(enabled.then_some(Message::Transport(id, event)))
	};

	container(
		column![
			row![
				text(id.label()).size(13),
				text(state_label(deck.transport)).size(13),
			]
			.spacing(8),
			text(ui::elide_middle(deck.title(), TITLE_CHARS)).size(16),
			row![
				text(ui::format_transport(deck.position, length))
					.size(13)
					.width(Fill),
				jump("⇤ 0:00", loaded.then_some(Duration::ZERO), id),
				jump(
					"⇥ music",
					trim.map(|trim| trim.start).filter(|_| loaded),
					id
				),
			]
			.spacing(6)
			.align_y(Center),
			ui::waveform::view(
				&deck.peaks,
				progress(deck.position, length),
				music_span(length, trim),
				deck.scanning.then_some(sweep),
				move |fraction| Message::Seeked(id, fraction),
			),
			row![
				transport_button("▶", deck::Event::Play, loaded && !deck.is_playing()),
				transport_button("⏸", deck::Event::Pause, deck.is_playing()),
				transport_button("⏹", deck::Event::Stop, loaded),
				button(text("Load…").size(13))
					.padding([6, 12])
					.on_press(Message::LoadPressed(id)),
			]
			.spacing(6)
			.align_y(Center),
		]
		.spacing(8),
	)
	.style(move |theme: &Theme| panel_style(theme, ring))
	.padding(12)
	.width(Fill)
	// Shrink, not `Fill`: every row above is a fixed size, so filling would only pad the
	// panel with empty space — and the queue below it is what should take the room a
	// divider drag adds (PLAN §7a).
	.into()
}

/// The panel, plus the drop ring when this is the player a release would land on.
///
/// Green from the `success` palette rather than a hand-picked colour, so it stays legible
/// if the theme ever changes, and distinct from the focus ring, which is `primary`.
fn panel_style(theme: &Theme, ring: bool) -> container::Style {
	let base = container::rounded_box(theme);
	if !ring {
		return base;
	}

	container::Style {
		border: base
			.border
			.color(theme.extended_palette().success.strong.color)
			.width(RING_WIDTH),
		..base
	}
}

/// One of the two places above the strip a playhead can be sent (PLAN §14c).
///
/// Dead when there is nowhere to go: no track at all, or a track nothing has found the music
/// in yet. Dead rather than absent, like every other control in this panel — a button that
/// came and went as scans landed would make the row jump under the pointer.
fn jump(label: &'static str, to: Option<Duration>, id: DeckId) -> Element<'static, Message> {
	button(text(label).size(11))
		.padding([2, 6])
		.on_press_maybe(to.map(|to| Message::Jumped(id, to)))
		.into()
}

/// The music's two edges as fractions of the track, for the marks on the strip.
///
/// `None` unless everything needed is there — a length to divide by and a trim — because a
/// mark drawn against a length the app had to guess at would be a line in the wrong place,
/// which is worse than no line.
///
/// Takes the length rather than the `Deck` it came from, so the arithmetic can be checked
/// without one: this is a divide and two clamps, and every one of its interesting cases is a
/// number no player can be put into on purpose (PLAN §12).
fn music_span(length: Option<Duration>, trim: Option<Trim>) -> Option<(f32, f32)> {
	let total = length?.as_secs_f32();
	let trim = trim?;

	// The same positive test `progress` uses rather than a `<= 0.0` guard, and for the reason
	// `seek_fraction` learned the hard way (PLAN §14b): `f32::clamp` passes a `NaN` straight
	// through, so a guard that only refuses zero would draw two marks at nowhere.
	(total > 0.0).then(|| {
		(
			(trim.start.as_secs_f32() / total).clamp(0.0, 1.0),
			(trim.end.as_secs_f32() / total).clamp(0.0, 1.0),
		)
	})
}

/// How far through the track the playhead is, for the waveform's playhead line.
///
/// `None` whenever there is nothing to measure against — an empty player, or a stream the
/// decoder could not give a length for (PLAN §7) — which is the case the readout above
/// shows as `--:--` and the strip shows by drawing no playhead at all.
fn progress(position: Duration, length: Option<Duration>) -> Option<f32> {
	let total = length?.as_secs_f32();
	// A zero-length track would divide by nothing, and a playhead can overrun its total by
	// a tick's worth at the very end, so the ratio is clamped rather than trusted.
	(total > 0.0).then(|| (position.as_secs_f32() / total).clamp(0.0, 1.0))
}

/// The transport state, in the user's words rather than the enum's.
fn state_label(transport: Transport) -> &'static str {
	match transport {
		Transport::Empty => "",
		Transport::Stopped => "stopped",
		Transport::Playing => "playing",
		Transport::Paused => "paused",
	}
}

/// The two divisions in this panel. Everything else here builds widgets (PLAN §12).
#[cfg(test)]
mod tests {
	use super::*;

	/// Seconds as a `Duration`, since every case below is written in them.
	fn secs(seconds: f32) -> Duration {
		Duration::from_secs_f32(seconds)
	}

	#[test]
	fn the_marks_sit_where_the_music_does_and_nowhere_at_all_without_a_length() {
		// Arrange: four seconds of leader and two of run-out in a two-hundred-second track,
		// which is the shape §14c actually finds.
		let trim = Trim {
			start: secs(4.0),
			end: secs(198.0),
		};

		// Act / Assert: two fractions of the *file*, not of the music.
		let (start, end) = music_span(Some(secs(200.0)), Some(trim)).expect("a measured track");
		assert!((start - 0.02).abs() < 1e-6, "start {start}");
		assert!((end - 0.99).abs() < 1e-6, "end {end}");

		// Nothing to divide by, and nothing to divide: both are a strip with no marks on it
		// rather than marks in the wrong place.
		assert_eq!(music_span(None, Some(trim)), None, "a stream");
		assert_eq!(music_span(Some(secs(200.0)), None), None, "never scanned");
		assert_eq!(music_span(Some(Duration::ZERO), Some(trim)), None, "empty");
	}

	#[test]
	fn a_mark_can_never_be_drawn_off_the_end_of_the_strip() {
		// Arrange: edges past the end of the file, which a stale trim against a re-encoded
		// file would give — the caller is a `draw`, so this has to be a clamp and not a panic.
		let past = Trim {
			start: secs(500.0),
			end: secs(900.0),
		};

		// Act / Assert
		assert_eq!(music_span(Some(secs(200.0)), Some(past)), Some((1.0, 1.0)));
	}

	#[test]
	fn the_playhead_is_a_fraction_or_it_is_not_drawn() {
		// Arrange / Act / Assert: the ordinary case, then the three that must not divide.
		assert_eq!(progress(secs(50.0), Some(secs(200.0))), Some(0.25));
		assert_eq!(progress(secs(50.0), None), None, "a stream");
		assert_eq!(
			progress(Duration::ZERO, Some(Duration::ZERO)),
			None,
			"empty"
		);

		// The playhead can overrun its own total by a tick at the very end, which is a
		// position the app really does produce — 20 times a second, every track.
		assert_eq!(progress(secs(200.05), Some(secs(200.0))), Some(1.0));
	}
}
