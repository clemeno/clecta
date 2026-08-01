//! One player's panel (PLAN §6, §7): what is loaded, where the playhead is, and the
//! three transport buttons.

use iced::widget::{button, column, container, row, text};
use iced::{Center, Element, Fill};

use crate::app::Message;
use crate::deck::{self, Deck, DeckId, Transport};
use crate::ui;

/// How much of a track name fits before it is elided. Fixed rather than measured: a
/// responsive elide needs the widget's real width, which only `advanced` gives (PLAN §3).
const TITLE_CHARS: usize = 34;

pub fn view(id: DeckId, deck: &Deck) -> Element<'_, Message> {
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
	.style(container::rounded_box)
	.padding(12)
	.width(Fill)
	.height(Fill)
	.into()
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
