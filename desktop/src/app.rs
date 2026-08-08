//! The iced application (PLAN §5): the state, the messages, and the two functions that
//! move between them.
//!
//! Everything interesting lives in the modules this one calls. What is left here is the
//! wiring: which message changes which field, which change needs a `Task`, and how the
//! three panes are arranged. That is deliberate — an `update` that is only ever a
//! dispatch table stays readable as the app grows.

use std::path::PathBuf;
use std::time::Duration;

use iced::futures::channel::oneshot;
use iced::widget::pane_grid::Axis;
use iced::widget::{Space, button, column, container, mouse_area, pane_grid, row, text};
use iced::{Element, Fill, Size, Subscription, Task, Theme, event, mouse, time, window};

use crate::audio::{self, Engine};
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

/// The scanning animation (PLAN §14a): how often the band moves, and how many steps it
/// takes to cross. 40 ms is 25 fps, which is smooth for a sliding shape without being the
/// playhead's 20 Hz; thirty steps makes a pass take 1.2 s, slow enough to read as
/// deliberate. Like the tick and the autosave, the subscription exists only while it is
/// needed — here, while a scan is actually running.
const SWEEP: Duration = Duration::from_millis(40);
const SWEEP_STEPS: u32 = 30;

/// Width of the files pane, as a fraction. The *height* of the top section is not a
/// fraction — see `decks_ratio` (PLAN §6).
const TREE_RATIO: f32 = 0.68;

/// The smallest a pane may be dragged to, in pixels. One value for every pane on both
/// axes — the ceiling noted in PLAN §6.
pub const MIN_PANE: f32 = 170.0;

/// The four numbers that make the pane grid's height derivable from the window's: the gap
/// the grid leaves between panes, the padding around the whole window, the gap above the
/// status bar, and the bar's own height.
///
/// Constants rather than literals in `view`, and the status bar is *pinned* to a height
/// rather than left to measure its own text, because the decks pane keeps a height in
/// **pixels** (PLAN §6): converting that to the ratio `pane_grid` wants needs the grid's
/// height, and iced 0.14 has no way to ask a widget how big it turned out. So everything
/// outside the grid is made a constant, and `CHROME` is the exact difference.
const PANE_SPACING: f32 = 6.0;
const WINDOW_PADDING: f32 = 6.0;
const STATUS_GAP: f32 = 4.0;
const STATUS_HEIGHT: f32 = 24.0;
const CHROME: f32 = 2.0 * WINDOW_PADDING + STATUS_GAP + STATUS_HEIGHT;

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
		// than moved. One clone of six fields, once.
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
	/// A waveform scan finished. Carries the path it was started for, because a player can
	/// be given another track while the scan runs (PLAN §14a).
	PeaksScanned(DeckId, PathBuf, Result<Vec<f32>, String>),
	/// Advance the scanning animation one step. Sent only while a scan is running.
	Sweep,
	/// The waveform was clicked, at this fraction of the track. Moves the playhead and
	/// nothing else — a playing player keeps playing, a paused one stays paused (PLAN §14).
	Seeked(DeckId, f32),
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
	/// The horizontal split, kept so a window resize can move it. Never `None`: unlike the
	/// tree, neither pane it separates can be folded away.
	decks_split: pane_grid::Split,
	/// How tall the players and the mixer want to be, in **pixels** rather than a fraction
	/// of the window (PLAN §6). Kept as what was asked for, not as what fits: a window too
	/// short to grant it compacts the panel, and growing the window again gives it back.
	decks_height: f32,
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
	/// subscription, which exists only while this is true — and is what lets a successful
	/// folder listing flush early without turning every refresh into a write.
	dirty: bool,
	/// The scanning animation's step counter. A plain integer rather than a timestamp, so
	/// nothing in `view` has to read the clock — the phase is whatever the last `Sweep`
	/// left behind.
	sweep: u32,
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
			decks_split,
			decks_height: settings.decks_height,
			tree_split: Some(tree_split),
			tree_ratio: TREE_RATIO,
			window: settings.window,
			drag: None,
			hover: None,
			os_hover: false,
			notice,
			dirty: false,
			sweep: 0,
		};
		app.apply_gains();
		// The split is placed from a pixel height and the window's, so it cannot be set while
		// building the grid above — the window size is not known there.
		app.apply_decks_height();

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
			decks_height: self.decks_height,
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

		// The scanning animation, on the same "only while it is needed" rule as the two
		// above. Nothing animates once both scans have landed.
		let sweep = if self.decks.iter().any(|deck| deck.scanning) {
			time::every(SWEEP).map(|_| Message::Sweep)
		} else {
			Subscription::none()
		};

		Subscription::batch([
			tick,
			autosave,
			sweep,
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
					return self.load(id, file);
				}
			}

			Message::LoadSelected(id) => {
				if let Some(path) = self.browser.selection().map(|entry| entry.path.clone()) {
					return self.load(id, path);
				}
			}

			Message::LoadUnaimed(path) => {
				let id = deck::idle_target(&self.decks[0], &self.decks[1]);
				return self.load(id, path);
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
				if event.split == self.decks_split {
					// Recorded in pixels and then re-derived, rather than stored as the ratio
					// the drag reported: `apply_decks_height` clamps, so the splitter stops at
					// the minimum instead of being draggable past where the panes can go.
					self.decks_height = decks_height(self.window.1, event.ratio);
					self.dirty = true;
					self.apply_decks_height();
				} else {
					self.panes.resize(event.split, event.ratio);
				}
			}

			Message::ReconnectPressed => return self.reconnect(),

			Message::FilesListed(folder, result) => {
				// A listing for a folder the user has already navigated away from is
				// stale: it must not overwrite the one they are looking at.
				if self.browser.folder.as_deref() != Some(folder.as_path()) {
					return Task::none();
				}
				match result {
					Ok(entries) => {
						self.browser.show(folder, entries);
						// A listing that arrives is the moment the new folder becomes real,
						// so it goes to disk now instead of in two seconds — quitting
						// straight after navigating is exactly when the throttle loses it.
						//
						// Guarded by `dirty` for the same reason `boot` clears it: a refresh
						// and the listing that opens the app both land here having changed
						// nothing, and neither should rewrite the file. A listing that
						// *failed* deliberately does not save: the folder is shown, so the
						// throttle will still store it, but not as this run's last word.
						if self.dirty {
							self.settings().save();
							self.dirty = false;
						}
					}
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
					// The players keep their height, so the split has to *move* as the window
					// grows rather than scale with it (PLAN §6).
					self.apply_decks_height();
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
				return self.accept_drop(target, path, first);
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
					return self.accept_drop(id, path, true);
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

			Message::PeaksScanned(id, path, result) => {
				// A scan takes a moment, and a player can be given a second track inside it.
				// An array that no longer belongs to what is loaded is dropped, or it would
				// draw the previous track under this one's playhead. `scanning` is left
				// alone in that case, because the *newer* scan is still running.
				if self.decks[id.index()]
					.track
					.as_ref()
					.map(|track| &track.path)
					!= Some(&path)
				{
					return Task::none();
				}
				self.decks[id.index()].scanning = false;
				match result {
					Ok(peaks) => self.decks[id.index()].peaks = peaks,
					// Odd but reachable: playback opened the file and the scan did not,
					// because it was replaced or unmounted between the two reads.
					Err(error) => self.notice = format!("{}: {error}", id.label()),
				}
			}

			// Wrapping, because this counter is only ever read modulo `SWEEP_STEPS`, and an
			// app left scanning for three years should not panic in a debug build.
			Message::Sweep => self.sweep = self.sweep.wrapping_add(1),

			Message::Seeked(id, fraction) => self.seek(id, fraction),
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
	/// Move a player's playhead to a fraction of its track, and change nothing else.
	///
	/// Not a `deck::Event`, and deliberately not routed through `transition`: seeking is
	/// the one thing that moves a player without moving its transport. Playing keeps
	/// playing from the new place, paused stays paused there. The state machine of §7 has
	/// no edge for this because there is no edge to have.
	fn seek(&mut self, id: DeckId, fraction: f32) {
		// No track, or a stream the decoder could give no length for (PLAN §7): there is
		// nothing for a fraction to be a fraction of. The widget already declines to send
		// this in that case; the guard is here because the message can outlive the frame
		// that produced it.
		let Some(total) = self.decks[id.index()]
			.track
			.as_ref()
			.and_then(|track| track.duration)
		else {
			return;
		};

		// A range test rather than a clamp: `f32::clamp` passes a `NaN` through and
		// `Duration::mul_f32` panics on one. Same reasoning as `waveform::seek_fraction`,
		// which is what makes this second check cheap rather than redundant — it is the
		// boundary of the module that does the multiplying.
		if !(0.0..=1.0).contains(&fraction) {
			return;
		}
		let to = total.mul_f32(fraction);

		if let Some(engine) = self.engine.as_ref()
			&& let Err(error) = engine.seek(id, to)
		{
			// `ponytail:` a stream that cannot seek keeps its old position, the same
			// failure Stop already has. PLAN §7's fallback — re-open and re-append — would
			// fix both at once if it is ever worth it.
			self.notice = format!("{}: {error:#}", id.label());
			return;
		}

		// Set here rather than left to the tick, which runs only while something plays: a
		// paused player would otherwise keep drawing its old playhead until it was started
		// again, and clicking a strip that visibly does nothing is worse than not clicking.
		self.decks[id.index()].position = to;
	}

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

	/// Decode a file into a player, and start the waveform scan behind it. A failure leaves
	/// the previous track alone and says so — it must never wipe a loaded track to show an
	/// error (PLAN §7).
	fn load(&mut self, id: DeckId, path: PathBuf) -> Task<Message> {
		if !browser::kind_of(&path).is_media() {
			self.notice = format!("{} is not a media file", fsio::name_of(&path));
			return Task::none();
		}

		let Some(engine) = self.engine.as_ref() else {
			self.notice = "no audio device — press Reconnect audio".to_string();
			return Task::none();
		};

		match engine.load(id, &path) {
			Ok(duration) => {
				let name = fsio::name_of(&path);
				self.notice = format!("{}: {name}", id.label());

				let deck = &mut self.decks[id.index()];
				deck.transport = deck::transition(deck.transport, deck::Event::Loaded);
				deck.position = Duration::ZERO;
				// Cleared now rather than when the new scan lands, so the strip is never
				// showing the outgoing track's shape under the incoming track's playhead.
				deck.peaks = Vec::new();
				deck.scanning = true;
				deck.track = Some(Track {
					path: path.clone(),
					name,
					duration,
				});

				scan_peaks(id, path)
			}
			// `{error:#}` prints the anyhow chain on one line, which is what turns
			// "cannot decode x.mp4" into "cannot decode x.mp4: unsupported codec".
			Err(error) => {
				self.notice = format!("{}: {error:#}", id.label());
				Task::none()
			}
		}
	}

	/// Apply the drop policy, then load or explain. Both gestures come through here, so a
	/// folder is declined the same way whichever way it arrived (PLAN §10).
	fn accept_drop(&mut self, id: DeckId, path: PathBuf, first: bool) -> Task<Message> {
		match deck::drop_outcome(path, first) {
			DropOutcome::Load(path) => self.load(id, path),
			DropOutcome::Decline(reason) => {
				self.notice = reason;
				Task::none()
			}
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

	/// Put the horizontal splitter where `decks_height` asks for, compacting the players only
	/// if the window is too short for it.
	///
	/// The wanted height is deliberately not written back when it is compacted: that is what
	/// lets a window squashed and then pulled open again come back to the panel the user
	/// chose, rather than to whatever the squashed one happened to fit.
	fn apply_decks_height(&mut self) {
		self.panes.resize(
			self.decks_split,
			decks_ratio(self.window.1, self.decks_height),
		);
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
		let mut tasks = Vec::new();
		for id in DeckId::ALL {
			let Some(path) = self.decks[id.index()]
				.track
				.as_ref()
				.map(|track| track.path.clone())
			else {
				continue;
			};
			tasks.push(self.load(id, path));
		}

		Task::batch(tasks)
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
		.spacing(PANE_SPACING)
		.min_size(MIN_PANE)
		.on_resize(8, Message::Resized);

		column![panes, self.status_bar()]
			.spacing(STATUS_GAP)
			.padding(WINDOW_PADDING)
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
		// One phase for both players, so two scans running at once sweep together rather
		// than drifting apart into something that looks like a rendering fault.
		let sweep = (self.sweep % SWEEP_STEPS) as f32 / SWEEP_STEPS as f32;
		let panel = ui::deck::view(id, &self.decks[id.index()], ring == Some(id), sweep);

		if self.drag.is_none() {
			return panel;
		}

		mouse_area(panel)
			.on_enter(Message::DragOver(id, true))
			.on_exit(Message::DragOver(id, false))
			.into()
	}

	/// One line: what just happened, and the way out when the audio device is gone.
	///
	/// Its height is pinned rather than measured, so the pane grid above it has a height that
	/// can be worked out from the window's — see `CHROME`.
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
		.height(STATUS_HEIGHT)
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

/// The pane grid's height inside a window this tall.
///
/// Exact rather than approximate, because everything the window holds outside the grid is a
/// constant (`CHROME`) — which is the whole reason the status bar has a pinned height.
fn grid_height(window_height: f32) -> f32 {
	// `max` and not a comparison: it also flattens a `NaN`, since `f32::max` returns the
	// other operand when one is not a number. Everything below can then assume a real
	// number without repeating the check.
	(window_height - CHROME).max(0.0)
}

/// The split ratio that gives the decks pane `wanted` pixels — and less than that only when
/// the window is too short to grant them (PLAN §6).
///
/// `pane_grid` stores a fraction, so a taller window would otherwise give the players a
/// taller panel, which is exactly what the fixed-height rows in it do not want. Inverting
/// `Axis::split`'s own arithmetic — `round(height * ratio - spacing / 2)` — is what keeps
/// the answer to the pixel.
///
/// The clamp repeats what `pane_grid` already does when it lays out, deliberately: the
/// widget clamps the *drawn* panes but keeps drawing the splitter at the stored ratio, so a
/// ratio left out of range would put the grab line somewhere the panes are not.
fn decks_ratio(window_height: f32, wanted: f32) -> f32 {
	let grid = grid_height(window_height);
	if grid <= 0.0 {
		return 0.5;
	}

	// Same order as `Axis::split`: the floor first, then the ceiling, so a window with no
	// room for both minimums gives the browser its share rather than the players.
	let height = wanted.max(MIN_PANE).min(grid - MIN_PANE - PANE_SPACING);
	((height + PANE_SPACING / 2.0) / grid).clamp(0.0, 1.0)
}

/// The inverse: what a dragged splitter means in pixels, which is what gets remembered.
fn decks_height(window_height: f32, ratio: f32) -> f32 {
	(grid_height(window_height) * ratio - PANE_SPACING / 2.0).max(0.0)
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

/// Scan a track's waveform on a thread of its own (PLAN §4, §14a).
///
/// **Not** the pattern the two directory reads above use, and the difference is the whole
/// point. iced's smol executor runs on **one** thread unless `SMOL_THREADS` says otherwise,
/// so a scan sitting inside an async block did not merely queue the next directory listing
/// — it stopped every subscription in the app, the 20 Hz playhead tick included. Measured
/// both ways from the moment a scan starts: **641 ms with no tick at all**, then a dozen
/// delivered in the same millisecond, against a steady **49–51 ms** once the decode moved
/// off the executor. Pressing Play during a scan gave audio and a frozen clock, which is
/// how this was found.
///
/// So the decode gets a real thread and the executor gets a `oneshot` to await, which is
/// the one thing it is good at. Two threads at most, one per player, each living exactly as
/// long as the scan it was spawned for.
///
/// The error is flattened to a `String` here because a `Message` has to be `Clone` and an
/// `anyhow::Error` is not — the same reason `fsio` returns one (PLAN §9).
fn scan_peaks(id: DeckId, path: PathBuf) -> Task<Message> {
	let (sender, receiver) = oneshot::channel();
	let scanned = path.clone();

	std::thread::spawn(move || {
		// The receiver is gone if the app quit mid-scan, and there is nothing to do about
		// that but let this thread end.
		let _ = sender.send(audio::peaks(&scanned).map_err(|error| format!("{error:#}")));
	});

	Task::perform(receiver, move |delivered| {
		// `Err` means the thread ended without answering, which it can only do by panicking.
		// Reported rather than swallowed: a strip that stays flat for ever with no line in
		// the status bar is the one outcome worse than a slow scan.
		let result =
			delivered.unwrap_or_else(|_| Err("the waveform scan stopped unexpectedly".to_string()));
		Message::PeaksScanned(id, path, result)
	})
}

/// The only testable thing in this module: the layout arithmetic above is pure, and it is
/// what a window resize is judged by (PLAN §6). Everything else here needs a window.
#[cfg(test)]
mod tests {
	use super::*;

	/// What `pane_grid` will actually draw the top pane as, given a ratio — `Axis::split`'s
	/// own formula, so the test measures the widget's behaviour and not the app's intent.
	fn drawn(window_height: f32, ratio: f32) -> f32 {
		let grid = grid_height(window_height);
		(grid * ratio - PANE_SPACING / 2.0)
			.round()
			.max(MIN_PANE)
			.min(grid - MIN_PANE - PANE_SPACING)
	}

	#[test]
	fn the_players_keep_their_height_however_tall_the_window_gets() {
		// Arrange: the panel the user chose, and every window height it fits in.
		let wanted = 300.0;

		// Act / Assert: within a pixel, because the widget rounds to one. From the shortest
		// window that can hold the panel *and* a minimum browser — below that the answer is
		// deliberately not the wanted height, which is the next test.
		let shortest = (wanted + MIN_PANE + PANE_SPACING + CHROME) as u32;
		for height in (shortest..=2000).step_by(37) {
			let height = height as f32;
			let drawn = drawn(height, decks_ratio(height, wanted));
			assert!(
				(drawn - wanted).abs() <= 1.0,
				"window {height}: panel drawn {drawn}, wanted {wanted}"
			);
		}
	}

	#[test]
	fn a_window_too_short_compacts_the_panel_and_then_gives_it_back() {
		// Arrange: a tall panel and a window with nowhere near the room for it.
		let wanted = 600.0;
		let short = MIN_PANE * 2.0 + PANE_SPACING + CHROME + 40.0;

		// Act
		let compacted = drawn(short, decks_ratio(short, wanted));

		// Assert: squashed, but never past the minimum, and the browser still has its own.
		assert!(compacted < wanted, "still {compacted} in a {short} window");
		assert!(compacted >= MIN_PANE, "compacted to {compacted}");
		let browser = grid_height(short) - compacted - PANE_SPACING;
		assert!(browser >= MIN_PANE, "browser left with {browser}");

		// Assert: the wanted height was not consumed by the squash — a window pulled open
		// again comes back to the panel the user chose.
		let restored = drawn(wanted + CHROME + MIN_PANE + PANE_SPACING, {
			decks_ratio(wanted + CHROME + MIN_PANE + PANE_SPACING, wanted)
		});
		assert!((restored - wanted).abs() <= 1.0, "restored to {restored}");
	}

	#[test]
	fn dragging_the_splitter_and_resizing_the_window_are_inverses() {
		// Arrange: a drag reports a fraction of the grid, and it has to survive the trip
		// through pixels or the panel would creep every time the window moved.
		let height = 900.0;

		// Act / Assert
		for ratio in [0.25, 0.4, 0.5, 0.75] {
			let round_trip = decks_ratio(height, decks_height(height, ratio));
			assert!(
				(round_trip - ratio).abs() < 0.001,
				"ratio {ratio} came back as {round_trip}"
			);
		}
	}

	#[test]
	fn an_impossible_window_still_produces_a_usable_ratio() {
		// Arrange / Act / Assert: a window shorter than its own chrome divides by nothing,
		// and a fraction outside `0..=1` puts `pane_grid`'s splitter off the screen.
		for height in [0.0, 1.0, CHROME, CHROME + 1.0, f32::NAN] {
			let ratio = decks_ratio(height, 300.0);
			assert!(
				(0.0..=1.0).contains(&ratio),
				"window {height} gave a ratio of {ratio}"
			);
		}
	}
}
