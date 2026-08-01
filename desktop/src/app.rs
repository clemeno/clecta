//! The iced application (PLAN §5): the state, the messages, and the two functions that
//! move between them.
//!
//! Everything interesting lives in the modules this one calls. What is left here is the
//! wiring: which message changes which field, which change needs a `Task`, and how the
//! three panes are arranged. That is deliberate — an `update` that is only ever a
//! dispatch table stays readable as the app grows.

use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::widget::pane_grid::Axis;
use iced::widget::{Space, button, column, container, pane_grid, row, text};
use iced::{Element, Fill, Subscription, Task, Theme, time};

use crate::audio::Engine;
use crate::browser::{self, Browser, Entry};
use crate::deck::{self, Deck, DeckId, Track};
use crate::mixer::{self, Curve};
use crate::tree::Tree;
use crate::{fsio, ui};

/// The playhead poll (PLAN §4). 20 Hz: fast enough that a time readout never looks stuck,
/// slow enough that it is not a battery bug — and it only runs while something plays.
const TICK: Duration = Duration::from_millis(50);

/// Height of the top section, and width of the files pane, as fractions.
const DECKS_RATIO: f32 = 0.42;
const TREE_RATIO: f32 = 0.68;

/// The smallest a pane may be dragged to, in pixels. One value for every pane on both
/// axes — the ceiling noted in PLAN §6.
const MIN_PANE: f32 = 170.0;

/// Width of the mixer strip. Fixed, because the two players should keep the width they
/// are given as the window resizes; the mixer's controls do not grow usefully.
const MIXER_WIDTH: f32 = 240.0;

pub fn run() -> iced::Result {
	iced::application(Clecta::boot, Clecta::update, Clecta::view)
		.title(Clecta::title)
		.subscription(Clecta::subscription)
		.theme(Clecta::theme)
		.window_size((1180.0, 760.0))
		.centered()
		.run()
}

/// Which region of the window a pane holds. Fixed at boot: the layout is not
/// user-managed, only its two split ratios and whether the tree is folded (PLAN §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
	Decks,
	Files,
	Tree,
}

#[derive(Debug, Clone)]
pub enum Message {
	/// The playhead poll.
	Tick,
	/// A transport button on one player.
	Transport(DeckId, deck::Event),
	/// The per-player **Load…** button — the third door that always works (PLAN §10).
	LoadPressed(DeckId),
	/// Load the selected row into a named player.
	LoadSelected(DeckId),
	/// Load a file into the player an unaimed gesture lands on.
	LoadUnaimed(PathBuf),
	/// A row was clicked.
	RowSelected(PathBuf),
	/// Show a folder in the files pane, from the tree or the dialog.
	FolderSelected(PathBuf),
	/// The **Open folder…** button.
	OpenFolderPressed,
	/// Re-read the folder currently shown.
	RefreshPressed,
	/// A disclosure arrow in the tree.
	FolderToggled(PathBuf),
	/// The tree's fold button.
	TreeFolded,
	/// The `.*` toggle.
	HiddenToggled(bool),
	FaderChanged(DeckId, f32),
	CrossfaderChanged(f32),
	CurveSelected(Curve),
	/// A splitter was dragged.
	Resized(pane_grid::ResizeEvent),
	/// Re-open the audio device after it went away (PLAN §11).
	ReconnectPressed,
	/// A directory listing came back off the GUI thread.
	FilesListed(PathBuf, Result<Vec<Entry>, String>),
	FoldersListed(PathBuf, Result<Vec<PathBuf>, String>),
}

pub struct Clecta {
	/// `None` when there is no usable output device. The app still runs — it browses,
	/// and offers a **Reconnect audio** button (PLAN §11).
	engine: Option<Engine>,
	decks: [Deck; 2],
	crossfader: f32,
	curve: Curve,
	browser: Browser,
	tree: Tree,
	panes: pane_grid::State<Section>,
	/// The vertical split, so the fold button can find it. `None` while folded, because
	/// closing the tree pane destroys the split with it (PLAN §6).
	tree_split: Option<pane_grid::Split>,
	/// The tree's last width, remembered across a fold for the same reason.
	tree_ratio: f32,
	/// The one line of feedback the app gives: what loaded, what would not decode, what
	/// the audio device is (PLAN §7).
	notice: String,
}

impl Clecta {
	fn boot() -> (Self, Task<Message>) {
		// Built by splitting rather than from a `Configuration`, because
		// `with_configuration` does not return the `Split` handles and the fold needs one.
		let (mut panes, decks_pane) = pane_grid::State::new(Section::Decks);
		let (files_pane, decks_split) = panes
			.split(Axis::Horizontal, decks_pane, Section::Files)
			.expect("splitting the only pane always succeeds");
		let (_tree_pane, tree_split) = panes
			.split(Axis::Vertical, files_pane, Section::Tree)
			.expect("splitting an existing pane always succeeds");
		panes.resize(decks_split, DECKS_RATIO);
		panes.resize(tree_split, TREE_RATIO);

		let (engine, notice) = match Engine::new() {
			Ok(engine) => {
				let notice = format!("audio device: {}", engine.description());
				(Some(engine), notice)
			}
			// Not fatal. The browser still works, and the button to try again is in the
			// status bar.
			Err(error) => (None, format!("no audio: {error:#}")),
		};

		let mut app = Self {
			engine,
			decks: [Deck::default(), Deck::default()],
			crossfader: 0.5,
			curve: Curve::default(),
			browser: Browser::default(),
			tree: Tree::new(fsio::roots()),
			panes,
			tree_split: Some(tree_split),
			tree_ratio: TREE_RATIO,
			notice,
		};
		app.apply_gains();

		// Open on the home folder, so the first thing on screen is a real listing rather
		// than an empty pane with a button in it.
		let task = app.select_folder(fsio::home());
		(app, task)
	}

	/// The window title. Shows the loaded tracks once there are any, so the app is
	/// identifiable in a window switcher without bringing it to the front.
	fn title(&self) -> String {
		let loaded: Vec<&str> = self
			.decks
			.iter()
			.filter(|deck| deck.transport.has_track())
			.map(Deck::title)
			.collect();

		if loaded.is_empty() {
			"clecta".to_string()
		} else {
			format!("clecta — {}", loaded.join(" / "))
		}
	}

	fn theme(&self) -> Theme {
		Theme::Dark
	}

	/// The tick runs only while something plays (PLAN §4).
	fn subscription(&self) -> Subscription<Message> {
		if self.decks.iter().any(Deck::is_playing) {
			time::every(TICK).map(|_| Message::Tick)
		} else {
			Subscription::none()
		}
	}

	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Tick => self.poll_players(),

			Message::Transport(id, event) => self.transport(id, event),

			Message::LoadPressed(id) => {
				// `ponytail:` the native dialog is modal and blocks the GUI thread while
				// it is open, which is what a modal dialog looks like anyway. rfd's async
				// form would need the panel driven from the main thread regardless.
				if let Some(file) = rfd::FileDialog::new()
					.set_directory(self.browser.folder.clone().unwrap_or_else(fsio::home))
					.pick_file()
				{
					self.load(id, file);
				}
			}

			Message::LoadSelected(id) => {
				if let Some(path) = self.browser.selection().map(|entry| entry.path.clone()) {
					self.load(id, path);
				}
			}

			Message::LoadUnaimed(path) => {
				let id = deck::idle_target(&self.decks[0], &self.decks[1]);
				self.load(id, path);
			}

			Message::RowSelected(path) => self.browser.selected = Some(path),

			Message::FolderSelected(folder) => return self.select_folder(folder),

			Message::OpenFolderPressed => {
				if let Some(folder) = rfd::FileDialog::new()
					.set_directory(self.browser.folder.clone().unwrap_or_else(fsio::home))
					.pick_folder()
				{
					return self.select_folder(folder);
				}
			}

			Message::RefreshPressed => {
				if let Some(folder) = self.browser.folder.clone() {
					return list_files(folder);
				}
			}

			Message::FolderToggled(path) => {
				let needed = self.tree.toggle(&path);
				return Task::batch(needed.into_iter().map(list_folders));
			}

			Message::TreeFolded => self.fold_tree(),

			Message::HiddenToggled(show) => self.browser.show_hidden = show,

			Message::FaderChanged(id, value) => {
				self.decks[id.index()].fader = value;
				self.apply_gains();
			}

			Message::CrossfaderChanged(value) => {
				self.crossfader = value;
				self.apply_gains();
			}

			Message::CurveSelected(curve) => {
				self.curve = curve;
				self.apply_gains();
			}

			Message::Resized(event) => {
				if Some(event.split) == self.tree_split {
					self.tree_ratio = event.ratio;
				}
				self.panes.resize(event.split, event.ratio);
			}

			Message::ReconnectPressed => return self.reconnect(),

			Message::FilesListed(folder, result) => {
				// A listing for a folder the user has already navigated away from is
				// stale: it must not overwrite the one they are looking at.
				if self.browser.folder.as_deref() != Some(folder.as_path()) {
					return Task::none();
				}
				match result {
					Ok(entries) => self.browser.show(folder, entries),
					Err(error) => {
						self.notice = error.clone();
						self.browser.fail(error);
					}
				}
			}

			Message::FoldersListed(folder, result) => match result {
				Ok(children) => self.tree.set_children(&folder, children),
				// A folder that cannot be listed is left with whatever it had. Saying so
				// in the notice is enough; the tree is navigation, not the task at hand.
				Err(error) => self.notice = error,
			},
		}

		Task::none()
	}

	/// Read the playhead and watch for the end of a track — the whole GUI↔audio bridge
	/// (PLAN §4).
	fn poll_players(&mut self) {
		let Some(engine) = self.engine.as_ref() else {
			return;
		};

		for id in DeckId::ALL {
			if !self.decks[id.index()].is_playing() {
				continue;
			}

			if engine.finished(id) {
				// There is no end-of-track callback in rodio; `empty()` going true on the
				// tick is the signal (PLAN §7).
				let deck = &mut self.decks[id.index()];
				deck.transport = deck::transition(deck.transport, deck::Event::Ended);
				deck.position = Duration::ZERO;
			} else {
				self.decks[id.index()].position = engine.position(id);
			}
		}
	}

	/// Apply a transport event to both the model and the audio thread, in that order of
	/// authority: the state machine decides, the engine follows.
	fn transport(&mut self, id: DeckId, event: deck::Event) {
		let current = self.decks[id.index()].transport;
		let next = deck::transition(current, event);
		if next == current && current == deck::Transport::Empty {
			return;
		}

		if let Some(engine) = self.engine.as_ref() {
			let outcome = match event {
				deck::Event::Play => {
					engine.play(id);
					Ok(())
				}
				deck::Event::Pause => {
					engine.pause(id);
					Ok(())
				}
				deck::Event::Stop => engine.stop(id),
				deck::Event::Loaded | deck::Event::Ended => Ok(()),
			};

			if let Err(error) = outcome {
				// `ponytail:` a stream that cannot seek fails to rewind. PLAN §7's
				// fallback is to re-open and re-append the file; for now the transport
				// still stops and the notice says the position stayed put.
				self.notice = format!("{}: {error:#}", id.label());
			}
		}

		let deck = &mut self.decks[id.index()];
		deck.transport = next;
		if event == deck::Event::Stop {
			deck.position = Duration::ZERO;
		}
	}

	/// Decode a file into a player. A failure leaves the previous track alone and says so
	/// — it must never wipe a loaded track to show an error (PLAN §7).
	fn load(&mut self, id: DeckId, path: PathBuf) {
		if !browser::kind_of(&path).is_media() {
			self.notice = format!("{} is not a media file", display_name(&path));
			return;
		}

		let Some(engine) = self.engine.as_ref() else {
			self.notice = "no audio device — press Reconnect audio".to_string();
			return;
		};

		match engine.load(id, &path) {
			Ok(duration) => {
				let name = display_name(&path);
				self.notice = format!("{}: {name}", id.label());

				let deck = &mut self.decks[id.index()];
				deck.transport = deck::transition(deck.transport, deck::Event::Loaded);
				deck.position = Duration::ZERO;
				deck.track = Some(Track {
					path,
					name,
					duration,
				});
			}
			// `{error:#}` prints the anyhow chain on one line, which is what turns
			// "cannot decode x.mp4" into "cannot decode x.mp4: unsupported codec".
			Err(error) => self.notice = format!("{}: {error:#}", id.label()),
		}
	}

	/// Show a folder: list its files, and open the tree down to it so the two panes agree
	/// about where the user is.
	fn select_folder(&mut self, folder: PathBuf) -> Task<Message> {
		// Set eagerly, so the header shows the destination while the listing is in
		// flight and the stale-listing guard above has something to compare against.
		self.browser.folder = Some(folder.clone());
		self.browser.selected = None;

		let mut tasks = vec![list_files(folder.clone())];
		tasks.extend(self.tree.reveal(&folder).into_iter().map(list_folders));
		Task::batch(tasks)
	}

	/// Close the tree pane, or bring it back at the width it had.
	fn fold_tree(&mut self) {
		match self.pane_holding(Section::Tree) {
			Some(tree) => {
				let _ = self.panes.close(tree);
				self.tree_split = None;
			}
			None => {
				let Some(files) = self.pane_holding(Section::Files) else {
					return;
				};
				if let Some((_, split)) = self.panes.split(Axis::Vertical, files, Section::Tree) {
					self.panes.resize(split, self.tree_ratio);
					self.tree_split = Some(split);
				}
			}
		}
	}

	/// Find the pane holding a section. Needed because panes are opaque handles and a
	/// re-created one gets a new handle (PLAN §6).
	fn pane_holding(&self, section: Section) -> Option<pane_grid::Pane> {
		self.panes
			.iter()
			.find(|(_, held)| **held == section)
			.map(|(pane, _)| *pane)
	}

	/// Re-open the audio device and put both tracks back where they were, paused at 0.
	fn reconnect(&mut self) -> Task<Message> {
		match Engine::new() {
			Ok(engine) => {
				self.notice = format!("audio device: {}", engine.description());
				self.engine = Some(engine);
			}
			Err(error) => {
				self.notice = format!("no audio: {error:#}");
				return Task::none();
			}
		}

		self.apply_gains();

		// A new device means new players, so whatever was loaded is gone from the audio
		// side even though the model still lists it.
		for id in DeckId::ALL {
			let Some(path) = self.decks[id.index()]
				.track
				.as_ref()
				.map(|track| track.path.clone())
			else {
				continue;
			};
			self.load(id, path);
		}

		Task::none()
	}

	/// Push the collapsed gains to both players. Called after anything that can change
	/// one, because the crossfader always changes both (PLAN §8).
	fn apply_gains(&self) {
		if let Some(engine) = self.engine.as_ref() {
			engine.set_gains(mixer::gains(
				self.decks[0].fader,
				self.decks[1].fader,
				self.crossfader,
				self.curve,
			));
		}
	}

	fn view(&self) -> Element<'_, Message> {
		let panes = pane_grid(&self.panes, |_pane, section, _maximized| {
			let body = match section {
				Section::Decks => self.decks_view(),
				Section::Files => ui::browser::view(&self.browser),
				Section::Tree => ui::tree::view(&self.tree, self.browser.folder.as_deref()),
			};
			pane_grid::Content::new(body).style(container::bordered_box)
		})
		.spacing(6)
		.min_size(MIN_PANE)
		.on_resize(8, Message::Resized);

		column![panes, self.status_bar()]
			.spacing(4)
			.padding(6)
			.into()
	}

	/// The top section: two players with the mixer strip between them (PLAN §6).
	fn decks_view(&self) -> Element<'_, Message> {
		row![
			ui::deck::view(DeckId::One, &self.decks[0]),
			container(ui::mixer::view(
				&self.decks[0],
				&self.decks[1],
				self.crossfader,
				self.curve,
			))
			.width(MIXER_WIDTH),
			ui::deck::view(DeckId::Two, &self.decks[1]),
		]
		.spacing(8)
		.padding(8)
		.into()
	}

	/// One line: what just happened, and the way out when the audio device is gone.
	fn status_bar(&self) -> Element<'_, Message> {
		let mut bar = row![
			text(&self.notice).size(12),
			Space::new().width(Fill),
			// The fold button lives here rather than in the tree pane, because a button
			// inside the tree cannot bring the tree back once the pane is closed.
			button(
				text(if self.tree_split.is_some() {
					"◧ hide tree"
				} else {
					"◨ show tree"
				})
				.size(12)
			)
			.padding([2, 8])
			.on_press(Message::TreeFolded),
		]
		.spacing(8)
		.padding([0, 6])
		.align_y(iced::Center);

		if self.engine.is_none() {
			bar = bar.push(
				button(text("Reconnect audio").size(12))
					.padding([2, 8])
					.on_press(Message::ReconnectPressed),
			);
		}

		bar.into()
	}
}

/// List one folder's files, off the GUI thread (PLAN §4).
///
/// `ponytail:` the blocking `read_dir` runs inside the async block, so it occupies an
/// executor thread rather than a dedicated blocking pool. One directory read is short
/// enough for that; a recursive walk would not be, and there is not one.
fn list_files(folder: PathBuf) -> Task<Message> {
	Task::perform(
		async move {
			let result = fsio::list_files(&folder);
			(folder, result)
		},
		|(folder, result)| Message::FilesListed(folder, result),
	)
}

/// The same, for the tree's subfolders.
fn list_folders(folder: PathBuf) -> Task<Message> {
	Task::perform(
		async move {
			let result = fsio::list_folders(&folder);
			(folder, result)
		},
		|(folder, result)| Message::FoldersListed(folder, result),
	)
}

fn display_name(path: &Path) -> String {
	path.file_name()
		.map(|name| name.to_string_lossy().into_owned())
		.unwrap_or_else(|| path.display().to_string())
}
