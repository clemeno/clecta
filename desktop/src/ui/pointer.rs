//! Where the pointer was when a right button went down (PLAN §6, §14d).
//!
//! iced's press messages carry no position: `mouse_area::on_right_press` takes a `Message`,
//! not a function of one, so a row that is right-clicked can say *which row* and nothing
//! about where on screen it was. The obvious fix — a cursor subscription — is the one thing
//! §6 refuses to leave switched on, because it publishes on every mouse move in the app and
//! every message rebuilds every row of the files pane.
//!
//! So the position comes from a *widget* instead. `watch` wraps the whole window in one
//! widget whose only job is to notice a right press and publish the cursor's position with
//! it. Nothing is published while the pointer merely moves, so the app is exactly as idle as
//! it was before this file existed — and the answer is exact rather than derived from
//! layout arithmetic the app does not have.
//!
//! The app's second custom widget, and the smaller of the two: `ui/waveform.rs` draws, this
//! one only listens, so `layout`, `draw` and the rest forward straight to the child.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, Widget};
use iced::advanced::{Clipboard, Shell, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Theme, Vector, mouse};

/// The window, plus one listener for the one event a `mouse_area` cannot describe.
pub struct Watched<'a, Message, Renderer> {
	content: Element<'a, Message, Theme, Renderer>,
	pressed: Box<dyn Fn(Point) -> Message + 'a>,
}

/// Wrap `content` so that every right press in it publishes where the pointer was.
///
/// One instance, at the root: a wrapper per row would be four hundred widgets to answer a
/// question one can answer, and the root sees every press in the window anyway.
pub fn watch<'a, Message, Renderer>(
	content: impl Into<Element<'a, Message, Theme, Renderer>>,
	pressed: impl Fn(Point) -> Message + 'a,
) -> Element<'a, Message, Theme, Renderer>
where
	Message: 'a,
	Renderer: renderer::Renderer + 'a,
{
	Element::new(Watched {
		content: content.into(),
		pressed: Box::new(pressed),
	})
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for Watched<'_, Message, Renderer>
where
	Renderer: renderer::Renderer,
{
	fn children(&self) -> Vec<Tree> {
		vec![Tree::new(&self.content)]
	}

	fn diff(&self, tree: &mut Tree) {
		tree.diff_children(std::slice::from_ref(&self.content));
	}

	fn size(&self) -> Size<Length> {
		self.content.as_widget().size()
	}

	fn layout(
		&mut self,
		tree: &mut Tree,
		renderer: &Renderer,
		limits: &layout::Limits,
	) -> layout::Node {
		self.content
			.as_widget_mut()
			.layout(&mut tree.children[0], renderer, limits)
	}

	/// The position goes out **before** the child is given the event, which is the whole
	/// contract of this widget: the row's own `RowMenuOpened` is published by a `mouse_area`
	/// underneath, so publishing first is what guarantees the app knows where the press was
	/// by the time it hears which row was pressed. Ordering hazards between two messages of
	/// one gesture are what Q53 cost, and this one is settled here rather than guessed at
	/// in `update`.
	///
	/// The event is deliberately not captured: this widget answers a question, it does not
	/// claim the press. Everything under it still sees it.
	fn update(
		&mut self,
		tree: &mut Tree,
		event: &Event,
		layout: Layout<'_>,
		cursor: mouse::Cursor,
		renderer: &Renderer,
		clipboard: &mut dyn Clipboard,
		shell: &mut Shell<'_, Message>,
		viewport: &Rectangle,
	) {
		if matches!(
			event,
			Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
		) && let Some(position) = cursor.position()
		{
			shell.publish((self.pressed)(position));
		}

		self.content.as_widget_mut().update(
			&mut tree.children[0],
			event,
			layout,
			cursor,
			renderer,
			clipboard,
			shell,
			viewport,
		);
	}

	fn operate(
		&mut self,
		tree: &mut Tree,
		layout: Layout<'_>,
		renderer: &Renderer,
		operation: &mut dyn Operation,
	) {
		self.content
			.as_widget_mut()
			.operate(&mut tree.children[0], layout, renderer, operation);
	}

	fn mouse_interaction(
		&self,
		tree: &Tree,
		layout: Layout<'_>,
		cursor: mouse::Cursor,
		viewport: &Rectangle,
		renderer: &Renderer,
	) -> mouse::Interaction {
		self.content.as_widget().mouse_interaction(
			&tree.children[0],
			layout,
			cursor,
			viewport,
			renderer,
		)
	}

	fn draw(
		&self,
		tree: &Tree,
		renderer: &mut Renderer,
		theme: &Theme,
		style: &renderer::Style,
		layout: Layout<'_>,
		cursor: mouse::Cursor,
		viewport: &Rectangle,
	) {
		self.content.as_widget().draw(
			&tree.children[0],
			renderer,
			theme,
			style,
			layout,
			cursor,
			viewport,
		);
	}

	fn overlay<'b>(
		&'b mut self,
		tree: &'b mut Tree,
		layout: Layout<'b>,
		renderer: &Renderer,
		viewport: &Rectangle,
		translation: Vector,
	) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
		self.content.as_widget_mut().overlay(
			&mut tree.children[0],
			layout,
			renderer,
			viewport,
			translation,
		)
	}
}
