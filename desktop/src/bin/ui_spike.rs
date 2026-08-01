//! THROWAWAY layout spike (PLAN §6, §9) — deleted when the real `app.rs` lands.
//!
//! Two `ponytail:` notes in the plan say "verify at scaffold time" rather than deciding
//! on paper. This binary is that scaffold. It answers exactly two questions and nothing
//! else — no audio, no filesystem, no state worth keeping:
//!
//! 1. **§6** — can `widget::pane_grid` express clecta's *fixed* layout (one horizontal
//!    split, one vertical split inside the bottom half) without dragging its
//!    user-managed-layout baggage along? And what does folding the tree cost, given that
//!    `pane_grid` models it as closing a pane rather than flipping a `bool`?
//! 2. **§9** — does `widget::table` take a per-row click, which the drag-to-player
//!    gesture (§10) needs? If not, the fallback is `scrollable(column(rows))`.
//!
//! Run it: `cargo run --bin ui_spike`. Drag both splitters, press **fold**, click the
//! file rows and watch the status line under the table.
//!
//! # What it found
//!
//! **`pane_grid`: yes, keep it.** The fixed layout is ~40 lines here against the ~150
//! cmote spreads over six files, and the drag-to-reorder baggage never appears because
//! it is opt-in through `on_drag`. Three costs, all priced into the code below and
//! written up in PLAN §6.
//!
//! **`table`: no, fall back.** It has no row — `Column::view` produces one *cell*, and
//! `Table` flattens the lot into a flat cell list. So the click has to be attached per
//! cell (leaving the inter-column padding dead, which the status line shows), and
//! `table::Style` carries only separator colours, so a selected or hovered *row* cannot
//! be drawn at all. That second one is what settles it. PLAN §9.

use iced::widget::pane_grid::Axis;
use iced::widget::{button, center, column, container, mouse_area, pane_grid, row, table, text};
use iced::{Element, Fill, Right};

/// Height of the top section as a fraction of the window, before any drag.
const DECKS_RATIO: f32 = 0.4;

/// Width of the files pane as a fraction of the bottom section, before any drag.
const TREE_RATIO: f32 = 0.65;

/// Smallest a pane may be dragged to, in pixels. One number for **both** axes and every
/// pane — see the findings in `main`.
const MIN_PANE: f32 = 140.0;

/// A stand-in files listing, shaped like §6's sketch: glyph, name, size, modified.
const ENTRIES: [(&str, &str, &str, &str); 5] = [
	("♪", "01 opener.flac", "8.2 MB", "2026-07-14"),
	("♪", "02 build.mp3", "6.1 MB", "2026-07-14"),
	("▶", "03 clip.mp4", "41.0 MB", "2026-07-02"),
	("♪", "04 closer.flac", "9.7 MB", "2026-07-14"),
	("", "notes.txt", "1.1 KB", "2026-06-30"),
];

fn main() -> iced::Result {
	println!("layout spike — findings are in the module docs of src/bin/ui_spike.rs");
	iced::run(Spike::update, Spike::view)
}

/// Which of the three regions a pane holds. The whole point of the fixed layout is that
/// this set never grows at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
	Decks,
	Files,
	Tree,
}

#[derive(Debug, Clone)]
enum Message {
	/// A splitter was dragged.
	Resized(pane_grid::ResizeEvent),
	/// The fold button was pressed.
	ToggleTree,
	/// A table cell was clicked: which row, and which column it landed in.
	CellClicked(usize, &'static str),
}

struct Spike {
	panes: pane_grid::State<Section>,
	/// The vertical split, kept so the fold button can find it. `None` while folded,
	/// because closing the tree pane destroys the split with it.
	tree_split: Option<pane_grid::Split>,
	/// The tree's last width, remembered across a fold. `pane_grid` cannot remember it
	/// for us: the split it was stored in no longer exists.
	tree_ratio: f32,
	/// What the last table click reported, or the prompt if there has not been one.
	clicked: String,
}

impl Default for Spike {
	fn default() -> Self {
		// Built by splitting rather than from a `Configuration`, purely because
		// `with_configuration` does not hand back the `Split` handles and the fold
		// button needs one of them.
		let (mut panes, decks) = pane_grid::State::new(Section::Decks);
		let (files, decks_split) = panes
			.split(Axis::Horizontal, decks, Section::Files)
			.expect("splitting the only pane always succeeds");
		let (_tree, tree_split) = panes
			.split(Axis::Vertical, files, Section::Tree)
			.expect("splitting an existing pane always succeeds");

		panes.resize(decks_split, DECKS_RATIO);
		panes.resize(tree_split, TREE_RATIO);

		Self {
			panes,
			tree_split: Some(tree_split),
			tree_ratio: TREE_RATIO,
			clicked: "click a file row".to_string(),
		}
	}
}

impl Spike {
	fn update(&mut self, message: Message) {
		match message {
			Message::Resized(event) => {
				if Some(event.split) == self.tree_split {
					self.tree_ratio = event.ratio;
				}
				self.panes.resize(event.split, event.ratio);
			}
			Message::ToggleTree => self.toggle_tree(),
			Message::CellClicked(index, field) => {
				self.clicked = format!("row {index} ({}) — hit the {field} cell", ENTRIES[index].1);
			}
		}
	}

	/// Fold the tree away, or bring it back at the width it had. Six lines and a
	/// remembered ratio, because `pane_grid` has no "hide this pane" — see findings.
	fn toggle_tree(&mut self) {
		match self.pane_holding(Section::Tree) {
			Some(tree) => {
				let _ = self.panes.close(tree);
				self.tree_split = None;
			}
			None => {
				let files = self
					.pane_holding(Section::Files)
					.expect("the files pane is never closed");
				if let Some((_, split)) = self.panes.split(Axis::Vertical, files, Section::Tree) {
					self.panes.resize(split, self.tree_ratio);
					self.tree_split = Some(split);
				}
			}
		}
	}

	/// Find the pane holding a section. Needed because `pane_grid` identifies panes by an
	/// opaque handle, and the handle for a pane that gets closed and re-created changes.
	fn pane_holding(&self, section: Section) -> Option<pane_grid::Pane> {
		self.panes
			.iter()
			.find(|(_, held)| **held == section)
			.map(|(pane, _)| *pane)
	}

	fn view(&self) -> Element<'_, Message> {
		pane_grid(&self.panes, |_pane, section, _maximized| {
			let body = match section {
				Section::Decks => decks(),
				Section::Files => self.files(),
				Section::Tree => tree(),
			};
			pane_grid::Content::new(body).style(container::bordered_box)
		})
		.spacing(6)
		// The clamp §6 wanted, for free — but one value for every pane on both axes.
		.min_size(MIN_PANE)
		.on_resize(8, Message::Resized)
		.into()
	}

	/// The files pane: the `table` experiment, plus the status line that shows where the
	/// click actually landed.
	fn files(&self) -> Element<'_, Message> {
		// Every cell is its own widget — `table` has no row element to hang a
		// `mouse_area` on, so the click has to be attached per cell instead. Filling the
		// cell width is the fairest version of that: it closes the gap inside a column,
		// leaving only the padding and separators between columns dead.
		let cell = |index: usize, field: &'static str, value: &'static str| -> Element<Message> {
			mouse_area(container(text(value)).width(Fill))
				.on_press(Message::CellClicked(index, field))
				.into()
		};

		let listing = table::table(
			[
				table::column(text(""), move |i: usize| cell(i, "glyph", ENTRIES[i].0)).width(24),
				table::column(text("Name"), move |i: usize| cell(i, "name", ENTRIES[i].1))
					.width(Fill),
				table::column(text("Size"), move |i: usize| cell(i, "size", ENTRIES[i].2))
					.align_x(Right),
				table::column(text("Modified"), move |i: usize| {
					cell(i, "modified", ENTRIES[i].3)
				}),
			],
			0..ENTRIES.len(),
		)
		.width(Fill);

		column![
			row![
				text("FILES — /Users/cme/Music/set").width(Fill),
				button(text("◧ fold")).on_press(Message::ToggleTree),
			]
			.spacing(8),
			listing,
			text(&self.clicked).size(12),
		]
		.spacing(8)
		.padding(8)
		.into()
	}
}

/// The top section: three boxes in a row, not three panes. §6 splits the window twice,
/// not four times — the decks and mixer keep fixed proportions.
fn decks() -> Element<'static, Message> {
	let box_ = |label: &'static str| {
		container(center(text(label)))
			.style(container::rounded_box)
			.width(Fill)
			.height(Fill)
	};

	row![box_("PLAYER 1"), box_("MIXER"), box_("PLAYER 2")]
		.spacing(8)
		.padding(8)
		.into()
}

/// The folder tree, as static text. Its real content is §9's job.
fn tree() -> Element<'static, Message> {
	column![
		text("▾ /"),
		text("  ▾ Users"),
		text("    ▾ cme"),
		text("      ▸ Documents"),
		text("      ▾ Music  ◄──"),
	]
	.spacing(2)
	.padding(8)
	.into()
}
