//! The mixer strip (PLAN §6, §8): a volume fader per player, the crossfader, and the
//! curve selector.
//!
//! The gain numbers shown beside each fader are `mixer::gains`' own output, not a second
//! calculation — so what the strip displays is exactly what the players were set to.

use iced::widget::{column, container, pick_list, row, slider, text};
use iced::{Element, Fill};

use crate::app::Message;
use crate::deck::{Deck, DeckId};
use crate::mixer::{Curve, gains};

/// Slider granularity. 1/100 of the travel is finer than the ear can follow in one
/// movement and keeps the printed gain from flickering.
const STEP: f32 = 0.01;

pub fn view(
	deck1: &Deck,
	deck2: &Deck,
	crossfader: f32,
	curve: Curve,
) -> Element<'static, Message> {
	let (gain1, gain2) = gains(deck1.fader, deck2.fader, crossfader, curve);

	container(
		column![
			text("MIXER").size(13),
			fader(DeckId::One, deck1.fader, gain1),
			fader(DeckId::Two, deck2.fader, gain2),
			column![
				text("crossfader").size(12),
				slider(0.0..=1.0, crossfader, Message::CrossfaderChanged).step(STEP),
				row![
					text("◄ 1").size(11),
					text("2 ►").size(11).width(Fill).align_x(iced::Right),
				],
			]
			.spacing(4),
			pick_list(Curve::ALL, Some(curve), Message::CurveSelected)
				.text_size(12)
				.width(Fill),
		]
		.spacing(12),
	)
	.style(container::rounded_box)
	.padding(12)
	.height(Fill)
	.into()
}

/// One volume fader, with the gain it currently produces.
fn fader(id: DeckId, value: f32, gain: f32) -> Element<'static, Message> {
	column![
		row![
			text(id.label()).size(12).width(Fill),
			// Two decimals: enough to see the cubic taper bite in the bottom half, which
			// is the thing worth noticing (PLAN §8).
			text(format!("{gain:.2}")).size(12),
		],
		slider(0.0..=1.0, value, move |value| Message::FaderChanged(
			id, value
		))
		.step(STEP),
	]
	.spacing(4)
	.into()
}
