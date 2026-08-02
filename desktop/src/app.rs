//! The iced application (PLAN §5): the state, the messages, and the two functions that
//! move between them.
//!
//! Everything interesting lives in the modules this one calls. What is left here is the
//! wiring: which message changes which field, which change needs a `Task`, and how the
//! three panes are arranged. That is deliberate — an `update` that is only ever a
//! dispatch table stays readable as the app grows.

use std::path::PathBuf;
use std::time::Duration;

use iced::widget::pane_grid::Axis;
use iced::widget::{Space, button, column, container, mouse_area, pane_grid, row, text};
use iced::{Element, Fill, Size, Subscription, Task, Theme, event, mouse, time, window};

use crate::audio::Engine;
use crate::browser::{self, Browser, Entry};
use crate::deck::{self, Deck, DeckId, DropOutcome, Track};
use crate::mixer::{self, Curve};
use crate::settings::Settings;
use crate::tree::Tree;
use crate::{fsio, ui};

/// The playhead poll (PLAN §4). 20 Hz: fast enough that a time readout never looks stuck,
/// slow enough that it is not a battery bug — and it only runs while something plays.
const TICK: Duration = Duration::from_millis(50);

/// How long a changed setting may sit unsaved (PLAN §11). The window closing is *not* a
/// reliable last chance: macOS ⌘Q and the app menu's **Quit** run
/// `applicationWillTerminate`, which winit turns into `LoopExiting` — an event iced 0.14
/// never surfaces, so `CloseRequested` simply does not arrive. Two seconds is long enough
/// that sweeping a fader writes a handful of times rather than once per pixel, and short
/// enough that the worst a quit or a crash costs is the last couple of seconds.
const SAVE_AFTER: Duration = Duration::from_secs(2);

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
	// Read before the window exists, because its size is one of the things restored. A
	// blocking read of one small file, once, before anything is on screen.
	let settings = Settings::load();
	let window = settings.window;

	iced::application(
		// `boot` is `Fn`, not `FnOnce`, so the settings are cloned into each call rather
		// than moved. One clone of five fields, once.
		move || Clecta::boot(settings.clone()),
		Clecta::update,
		Clecta::view,
	)
	.title(Clecta::title)
	.subscription(Clecta::subscription)
	.theme(Clecta::theme)
	.window_size(window)
	// Closing has to run through `update` so the settings are written before the process
	// goes away. Every path out of `CloseRequested` ends in `iced::exit`, or the window
	// would refuse to close (PLAN §11). This catches the close *button* only — ⌘Q is why
	// there is also a debounced autosave.
	.exit_on_close_request(false)
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
	/// The window was resized. Recorded, then saved with everything else once the app has
	/// been still for `SAVE_AFTER`.
	WindowResized(Size),
	/// The autosave timer fired: something changed `SAVE_AFTER` ago. Only ever sent while
	/// `dirty`, because the subscription that sends it does not otherwise exist.
	SaveSettings,
	/// The window's close button. Writes `settings.json` and exits. **Not** ⌘Q, which
	/// never reaches the app at all — see `SAVE_AFTER`.
	CloseRequested,
	/// Files from outside are hovering the window, or have left again. No position comes
	/// with either event, which is why the target has to be derived (PLAN §10).
	FilesHovered(bool),
	/// One file of an OS drop. A multi-file drop arrives as one of these per file.
	FileDropped(PathBuf),
	/// During an in-app drag, the pointer entered (`true`) or left (`false`) a player.
	DragOver(DeckId, bool),
	/// The left button came up, wherever it is. The end of any in-app drag.
	DragReleased,
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
	/// The window's current size, tracked so it can be restored next run (PLAN §11).
	window: (f32, f32),
	/// The file an in-app drag is carrying. Armed by a press on a media row, disarmed by
	/// the release — so a plain click is a drag that landed on nothing (PLAN §10).
	drag: Option<PathBuf>,
	/// The player the pointer is over. Only ever set while a drag is in flight, because
	/// that is the only time the panels are drop targets at all.
	hover: Option<DeckId>,
	/// Whether files from outside are hovering the window right now.
	os_hover: bool,
	/// The one line of feedback the app gives: what loaded, what would not decode, what
	/// the audio device is (PLAN §7).
	notice: String,
	/// A persisted setting has changed and is not on disk yet. Drives the autosave
	/// subscription, which exists only while this is true.
	dirty: bool,
}

impl Clecta {
	fn boot(settings: Settings) -> (Self, Task<Message>) {
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

		let mut decks = [Deck::default(), Deck::default()];
		for (deck, fader) in decks.iter_mut().zip(settings.faders) {
			deck.fader = fader;
		}

		let mut app = Self {
			engine,
			decks,
			crossfader: settings.crossfader,
			curve: settings.curve,
			browser: Browser::default(),
			tree: Tree::new(fsio::roots()),
			panes,
			tree_split: Some(tree_split),
			tree_ratio: TREE_RATIO,
			window: settings.window,
			drag: None,
			hover: None,
			os_hover: false,
			notice,
			dirty: false,
		};
		app.apply_gains();

		// Open where the last run left off, or on the home folder — so the first thing on
		// screen is a real listing rather than an empty pane with a button in it. The
		// folder is known to exist: `Settings` drops one that does not.
		let task = app.select_folder(settings.folder.unwrap_or_else(fsio::home));
		// Restoring is not a change. Without this, every launch would write the file back
		// two seconds later having altered nothing.
		app.dirty = false;
		(app, task)
	}

	/// The state worth keeping, gathered for the one write at exit (PLAN §11).
	fn settings(&self) -> Settings {
		Settings {
			curve: self.curve,
			faders: [self.decks[0].fader, self.decks[1].fader],
			crossfader: self.crossfader,
			folder: self.browser.folder.clone(),
			window: self.window,
		}
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

	fn subscription(&self) -> Subscription<Message> {
		// The tick runs only while something plays (PLAN §4).
		let tick = if self.decks.iter().any(Deck::is_playing) {
			time::every(TICK).map(|_| Message::Tick)
		} else {
			Subscription::none()
		};

		// The autosave timer, which exists only while there is something to save: the
		// subscription is rebuilt after every `update`, so clearing `dirty` in the save
		// arm below ends it. That makes this a throttle rather than a true debounce — the
		// file is written at most `SAVE_AFTER` after the *first* change of a burst, not
		// after the last — which is the behaviour we want. A debounce would postpone the
		// write for as long as a fader keeps moving; this one caps the exposure instead.
		// Nothing ticks at rest.
		let autosave = if self.dirty {
			time::every(SAVE_AFTER).map(|_| Message::SaveSettings)
		} else {
			Subscription::none()
		};

		Subscription::batch([
			tick,
			autosave,
			window::resize_events().map(|(_, size)| Message::WindowResized(size)),
			window::close_requests().map(|_| Message::CloseRequested),
			gestures(),
		])
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

			Message::RowSelected(path) => {
				// The press both selects the row and arms a drag with it. A release that
				// is not over a player disarms it and nothing else happens — which is
				// exactly what a plain click is (PLAN §10).
				self.drag = browser::kind_of(&path).is_media().then(|| path.clone());
				self.browser.selected = Some(path);
			}

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
				self.dirty = true;
				self.apply_gains();
			}

			Message::CrossfaderChanged(value) => {
				self.crossfader = value;
				self.dirty = true;
				self.apply_gains();
			}

			Message::CurveSelected(curve) => {
				self.curve = curve;
				self.dirty = true;
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

			// Only a real change counts. Creating the window emits a resize event carrying
			// the size it was just asked for, so marking this dirty unconditionally made
			// every launch rewrite the file it had just read.
			Message::WindowResized(size) => {
				if self.window != (size.width, size.height) {
					self.window = (size.width, size.height);
					self.dirty = true;
				}
			}

			Message::SaveSettings => {
				self.settings().save();
				self.dirty = false;
			}

			Message::FilesHovered(hovering) => self.os_hover = hovering,

			Message::FileDropped(path) => {
				// Taking the flag is what makes the *first* file of the drop the one that
				// counts: every later event in the same burst sees `false` (PLAN §10).
				//
				// `ponytail:` this leans on the hover arriving before the drop, which is
				// the only burst boundary the event stream has. True on macOS and Windows
				// (checked in winit's `performDragOperation:` and `IDropTarget::Drop`,
				// neither of which cancels the hover first). A platform that skipped the
				// hover would decline the drop with a notice, not swallow it.
				let first = std::mem::take(&mut self.os_hover);
				let target = deck::idle_target(&self.decks[0], &self.decks[1]);
				self.accept_drop(target, path, first);
			}

			Message::DragOver(id, inside) => {
				// Compared rather than merely cleared: both panels see the same cursor
				// move, and the one being left is not always the one updated first.
				if inside {
					self.hover = Some(id);
				} else if self.hover == Some(id) {
					self.hover = None;
				}
			}

			Message::DragReleased => {
				let target = self.hover.take();
				// Disarmed on every release, whatever it was over, so nothing is left
				// armed behind a drag the user thought better of.
				if let Some(path) = self.drag.take()
					&& let Some(id) = target
				{
					self.accept_drop(id, path, true);
				}
			}

			Message::CloseRequested => {
				self.settings().save();
				// Unconditional, and it has to be: `exit_on_close_request(false)` means
				// nothing else will close the window.
				return iced::exit();
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
			self.notice = format!("{} is not a media file", fsio::name_of(&path));
			return;
		}

		let Some(engine) = self.engine.as_ref() else {
			self.notice = "no audio device — press Reconnect audio".to_string();
			return;
		};

		match engine.load(id, &path) {
			Ok(duration) => {
				let name = fsio::name_of(&path);
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

	/// Apply the drop policy, then load or explain. Both gestures come through here, so a
	/// folder is declined the same way whichever way it arrived (PLAN §10).
	fn accept_drop(&mut self, id: DeckId, path: PathBuf, first: bool) {
		match deck::drop_outcome(path, first) {
			DropOutcome::Load(path) => self.load(id, path),
			DropOutcome::Decline(reason) => self.notice = reason,
		}
	}

	/// The player a release would land on right now, or `None` when nothing is being
	/// dragged. Exactly one, or the ring would promise aiming the app cannot do.
	fn drop_ring(&self) -> Option<DeckId> {
		if self.os_hover {
			// An OS drag carries no position, so the target is derived — and shown
			// before the release rather than discovered after it (PLAN §10).
			Some(deck::idle_target(&self.decks[0], &self.decks[1]))
		} else if self.drag.is_some() {
			// An in-app drag is truly aimed: the pointer is ours the whole way.
			self.hover
		} else {
			None
		}
	}

	/// Show a folder: list its files, and open the tree down to it so the two panes agree
	/// about where the user is.
	fn select_folder(&mut self, folder: PathBuf) -> Task<Message> {
		// Set eagerly, so the header shows the destination while the listing is in
		// flight and the stale-listing guard above has something to compare against.
		self.browser.folder = Some(folder.clone());
		self.browser.selected = None;
		// Set here rather than at the two call sites, so a third one cannot forget. `boot`
		// clears it again, because restoring the last folder is not a change.
		self.dirty = true;

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
		let ring = self.drop_ring();

		row![
			self.deck_view(DeckId::One, ring),
			container(ui::mixer::view(
				&self.decks[0],
				&self.decks[1],
				self.crossfader,
				self.curve,
			))
			.width(MIXER_WIDTH),
			self.deck_view(DeckId::Two, ring),
		]
		.spacing(8)
		.padding(8)
		.into()
	}

	/// One player, made a drop target only while an in-app drag is in flight.
	///
	/// Attached conditionally because `mouse_area` reports every crossing of the panel,
	/// dragging or not, and outside a drag there is nothing to do with that.
	fn deck_view(&self, id: DeckId, ring: Option<DeckId>) -> Element<'_, Message> {
		let panel = ui::deck::view(id, &self.decks[id.index()], ring == Some(id));

		if self.drag.is_none() {
			return panel;
		}

		mouse_area(panel)
			.on_enter(Message::DragOver(id, true))
			.on_exit(Message::DragOver(id, false))
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

/// The two drop gestures, both of which need raw events rather than a widget (PLAN §10).
///
/// The status is ignored on purpose. A left release over a button is *captured*, and
/// `listen()` would drop it — which would leave a drag armed for ever the first time
/// someone let go over the Play button.
fn gestures() -> Subscription<Message> {
	event::listen_with(|event, _status, _window| match event {
		// The OS drop, exactly the three events cmote used. Not one of them carries a
		// position, and winit is where that is lost (PLAN §10).
		iced::Event::Window(window::Event::FileHovered(_)) => Some(Message::FilesHovered(true)),
		iced::Event::Window(window::Event::FilesHoveredLeft) => Some(Message::FilesHovered(false)),
		iced::Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
		// The end of an in-app drag. `ponytail:` this fires on every left release in the
		// app, including the hundreds that are ordinary clicks; the handler leaves in one
		// comparison when no drag is armed, which is cheaper than gating the subscription.
		iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
			Some(Message::DragReleased)
		}
		_ => None,
	})
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
