//! The mixer strip (PLAN §6, §8): a volume fader per player, the crossfader, and the
//! curve selector.
//!
//! The gain numbers shown beside each fader are `mixer::gains`' own output, not a second
//! calculation — so what the strip displays is exactly what the players were set to.

use iced::widget::{button, column, container, pick_list, row, slider, space, text};
use iced::{Center, Element, Fill};

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
				// The two end labels were already here as text; making them buttons costs a
				// widget each and saves dragging the whole travel to reach an end you can
				// name. The centre one has no label to inherit and is the one that could not
				// be done by hand at all: 0.5 exactly is a value a mouse lands on by luck.
				row![
					preset("◄ 1", Message::CrossfaderChanged(0.0)),
					space::horizontal(),
					preset("centre", Message::CrossfaderChanged(0.5)),
					space::horizontal(),
					preset("2 ►", Message::CrossfaderChanged(1.0)),
				]
				.align_y(Center),
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
		row![
			preset("0", Message::FaderChanged(id, 0.0)),
			slider(0.0..=1.0, value, move |value| Message::FaderChanged(
				id, value
			))
			.step(STEP),
			preset("max", Message::FaderChanged(id, 1.0)),
		]
		.spacing(6)
		.align_y(Center),
	]
	.spacing(4)
	.into()
}

/// A button that jumps a fader straight to one exact value.
///
/// It sends the **same message the slider sends**, which is the whole design: there is no
/// second way into the mixer state, nothing to keep in sync, and every value these buttons
/// can produce is one `mixer::gains` is already tested at (PLAN §8, §12).
///
/// `button::text` rather than a chip: these sit inside a control, and a row of filled
/// rectangles would read as more important than the fader they belong to.
fn preset(label: &'static str, message: Message) -> Element<'static, Message> {
	button(text(label).size(11))
		.padding([2, 6])
		.style(button::text)
		.on_press(message)
		.into()
}
