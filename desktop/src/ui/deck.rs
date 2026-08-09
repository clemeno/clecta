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
				text(ui::format_transport(
					deck.position,
					deck.track.as_ref().and_then(|track| track.duration)
				))
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
				progress(deck),
				music(deck, trim),
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
/// `None` unless everything needed is there — a track, a length to divide by, and a trim —
/// because a mark drawn against a length the app had to guess at would be a line in the wrong
/// place, which is worse than no line.
fn music(deck: &Deck, trim: Option<Trim>) -> Option<(f32, f32)> {
	let total = deck.track.as_ref()?.duration?.as_secs_f32();
	let trim = trim?;
	if total <= 0.0 {
		return None;
	}

	Some((
		(trim.start.as_secs_f32() / total).clamp(0.0, 1.0),
		(trim.end.as_secs_f32() / total).clamp(0.0, 1.0),
	))
}

/// How far through the track the playhead is, for the waveform's playhead line.
///
/// `None` whenever there is nothing to measure against — an empty player, or a stream the
/// decoder could not give a length for (PLAN §7) — which is the case the readout above
/// shows as `--:--` and the strip shows by drawing no playhead at all.
fn progress(deck: &Deck) -> Option<f32> {
	let total = deck.track.as_ref()?.duration?.as_secs_f32();
	// A zero-length track would divide by nothing, and a playhead can overrun its total by
	// a tick's worth at the very end, so the ratio is clamped rather than trusted.
	(total > 0.0).then(|| (deck.position.as_secs_f32() / total).clamp(0.0, 1.0))
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
