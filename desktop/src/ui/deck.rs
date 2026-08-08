//! One player's panel (PLAN §6, §7): what is loaded, where the playhead is, and the
//! three transport buttons.

use iced::widget::{button, column, container, row, text};
use iced::{Center, Element, Fill, Theme};

use crate::app::Message;
use crate::deck::{self, Deck, DeckId, Transport};
use crate::ui;

/// How much of a track name fits before it is elided. Fixed rather than measured: a
/// responsive elide needs the widget's real width, which only `advanced` gives (PLAN §3).
const TITLE_CHARS: usize = 34;

/// The drop ring's thickness. Thick enough to read as an answer to "where is this going?"
/// from across the window, which is the whole job it has.
const RING_WIDTH: f32 = 2.0;

/// `ring` lights this player as the one a release would land on — under the cursor for an
/// in-app drag, derived for an OS drop, and never on both (PLAN §10). `sweep` is the
/// scanning animation's phase, which the strip uses only while this player is being
/// scanned (PLAN §14a).
pub fn view(id: DeckId, deck: &Deck, ring: bool, sweep: f32) -> Element<'_, Message> {
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
			text(ui::format_transport(
				deck.position,
				deck.track.as_ref().and_then(|track| track.duration)
			))
			.size(13),
			ui::waveform::view(
				&deck.peaks,
				progress(deck),
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
