//! The iced application (PLAN §5): the state, the messages, and the two functions that
//! move between them.
//!
//! Everything interesting lives in the modules this one calls. What is left here is the
//! wiring: which message changes which field, which change needs a `Task`, and how the
//! three sections are arranged. That is deliberate — an `update` that is only ever a
//! dispatch table stays readable as the app grows.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use iced::futures::Stream;
use iced::futures::channel::oneshot;
use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};
use iced::widget::pane_grid::Axis;
use iced::widget::scrollable::AbsoluteOffset;
use iced::widget::{
	Space, button, column, container, mouse_area, operation, pane_grid, row, scrollable, text,
};
use iced::{
	Element, Fill, Size, Subscription, Task, Theme, event, keyboard, mouse, stream, time, window,
};
use notify::{RecursiveMode, Watcher};

use crate::audio::{self, Engine, Scan};
use crate::browser::{self, Browser, Entry};
use crate::cache::{self, Cache};
use crate::deck::{self, Deck, DeckId, DropOutcome, Track};
use crate::mixer::{self, Curve};
use crate::playlist::{self, ListId, Playlist, Transition};
use crate::select::Click;
use crate::settings::Settings;
use crate::tree::Tree;
use crate::waveform::Trim;
use crate::{fsio, paths, ui};

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

/// How long the pane waits after the OS says the folder changed (PLAN §9).
///
/// Long enough that copying a hundred files in re-lists a handful of times instead of a
/// hundred, short enough that dragging one file in feels like it appeared by itself. The
/// same throttle shape as `SAVE_AFTER`, and for the same reason: the timer exists only
/// while there is something waiting, so nothing ticks at rest.
const WATCH_SETTLE: Duration = Duration::from_millis(500);

/// How a queue scrolls itself while a drag hovers one of its edges (PLAN §7a).
///
/// 30 ms is 33 fps, smooth for something the eye is tracking a target through, and eight
/// pixels a step is a little under four rows a second — slow enough to stop on the row you
/// meant, which is the whole reason the gesture exists. Like every other timer here, the
/// subscription exists only while an edge is armed.
const AUTOSCROLL: Duration = Duration::from_millis(30);
const AUTOSCROLL_STEP: f32 = 8.0;

/// Width of the files pane, as a fraction. The *height* of the top section is not a
/// fraction at all — see `Clecta::view` (PLAN §6).
const TREE_RATIO: f32 = 0.68;

/// The smallest a pane may be dragged to, in pixels. One value for every pane on both
/// axes — the ceiling noted in PLAN §6.
pub const MIN_PANE: f32 = 170.0;

/// The gap between panes, which is also the height of the hand-written divider above the
/// browser: the divider *is* the gap, so one constant serves both and they cannot drift.
const PANE_SPACING: f32 = 6.0;

/// The three numbers around the body, and the difference they add up to.
///
/// `CHROME` is what the window spends on everything that is not the body: padding twice,
/// the gap above the status bar, and the bar itself. The bar's height is *pinned* rather
/// than measured from its text so this stays a constant, which is what lets `dragged_height`
/// stop a divider drag before it pushes the browser off the bottom. The *layout* needs none
/// of it — iced compacts a too-tall panel itself (PLAN §6).
const WINDOW_PADDING: f32 = 6.0;
const STATUS_GAP: f32 = 4.0;
const STATUS_HEIGHT: f32 = 24.0;
const CHROME: f32 = 2.0 * WINDOW_PADDING + STATUS_GAP + STATUS_HEIGHT;

/// Width of the mixer strip. Fixed, because the two players should keep the width they
/// are given as the window resizes; the mixer's controls do not grow usefully.
const MIXER_WIDTH: f32 = 240.0;

/// How many files a folder scan decodes at once (PLAN §11b).
///
/// Four, and the number is the whole trade: a scan is a decode of an entire file, so one
/// thread at a time leaves a folder of two thousand tracks running for ten minutes, and one
/// thread per core leaves nothing for the audio callback while somebody is playing a set.
/// Four is fast enough to be worth pressing and still short of every machine's core count.
const SCAN_JOBS: usize = 4;

/// The cache's name inside `clecta-data/` (PLAN §11a). Beside `settings.json` rather than in a
/// per-user cache folder, so a portable install carries what it has already worked out with
/// it — and the extension says what it is to anyone looking at the folder.
const CACHE_FILE: &str = "cache.redb";

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

/// Which region of the browser a pane holds. Fixed at boot: the layout is not
/// user-managed, only the split ratio and whether the tree is folded (PLAN §6).
///
/// The players are *not* in here. They keep a height in pixels, and `pane_grid` can only
/// place a splitter at a ratio — see `Clecta::view`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
	Files,
	Tree,
}

/// Where a release would put what is being dragged (PLAN §10, §7a).
///
/// One type for both kinds of destination, so `hover` is one field and the rules about what
/// beats what live in one `match` rather than in two flags that can both be set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
	/// Load it into this player, now.
	Player(DeckId),
	/// Put it in this list, **above** this row. `index == len` is past the last row, which is
	/// the one index that names no row — the caret sits between rows, not on them.
	Row(ListId, usize),
}

/// The panel a pointer can leave, which is coarser than what it can land on: a list has as
/// many targets as it has rows, but leaving it is one event.
///
/// Leaving is separate from entering because the two arrive in an order nothing guarantees.
/// A leave that only clears *its own* zone cannot wipe an enter that has already landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
	Player(DeckId),
	List(ListId),
}

/// What an in-app drag is carrying, and where it came from.
///
/// `from` is the whole difference between a copy and a move: a drag out of the files pane
/// leaves the file where it is, and a drag out of a list takes the row with it.
impl DropTarget {
	/// The panel this target sits in, which is what a pointer leaves.
	fn zone(self) -> Zone {
		match self {
			DropTarget::Player(id) => Zone::Player(id),
			DropTarget::Row(list, _) => Zone::List(list),
		}
	}
}

/// What an in-app drag is carrying, and where it came from.
///
/// A whole selection rather than one row (PLAN §9a), in the order the rows are on screen — so a
/// drop puts them down in the order they were looked at. `from` is one list and the rows inside
/// it, because a drag starts with a press and a press is in one panel.
#[derive(Debug, Clone)]
pub struct Drag {
	items: Vec<playlist::Item>,
	from: Option<(ListId, Vec<usize>)>,
}

/// What a background job worked out about one file (PLAN §7a, §11a, §14c).
///
/// One type for two jobs that answer the same question at different depths: measuring a queue
/// *reads* what is known, and a folder scan *works it out*. The arm that receives them does
/// the same thing either way, which is the point — the app does not care which job found out
/// how long a track is or where its music starts.
#[derive(Debug, Clone)]
pub struct Facts {
	pub path: PathBuf,
	pub duration: Option<Duration>,
	pub trim: Option<Trim>,
}

/// What the duplicate warning answered (PLAN §9a).
///
/// Three, because a batch is what made two too few: a set of twenty where one is already
/// queued has an answer that is neither "all of them" nor "none of them", and it is the one
/// most people mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admission {
	/// Queue everything, repeats included.
	All,
	/// Queue only the tracks that are not already somewhere.
	Fresh,
	/// Queue nothing — the action did not happen.
	Nothing,
}

impl Admission {
	/// Whether the track at this position in the batch survives the answer.
	///
	/// A position rather than a path, so a caller can filter rows and tracks together with the
	/// same test — which is what the `←` / `→` buttons need, since what leaves one list has to
	/// be exactly what arrives in the other.
	fn keeps(self, index: usize, duplicates: &[(usize, ListId)]) -> bool {
		match self {
			Admission::All => true,
			Admission::Fresh => !duplicates.iter().any(|(duplicate, _)| *duplicate == index),
			Admission::Nothing => false,
		}
	}
}

/// A folder scan in flight (PLAN §11b).
///
/// `next` and `done` are separate counters because the files go out ahead of the answers:
/// `SCAN_JOBS` of them are always in the air, so the number on screen has to be the one that
/// has come *back*, and the scan is over when the last thread reports rather than when the
/// last file is handed out.
struct Scanning {
	files: Vec<PathBuf>,
	next: usize,
	done: usize,
	/// How many threads are out right now, which is what the fan-out is capped against.
	running: usize,
}

impl Scanning {
	/// How many more files may go out this moment: whatever the fan-out has free, or whatever
	/// is left of the list, whichever is smaller.
	fn slots(&self) -> usize {
		SCAN_JOBS
			.saturating_sub(self.running)
			.min(self.files.len() - self.next)
	}

	/// Whether the scan is over — when the last *thread* has reported, not when the last file
	/// went out. That is also what makes **Stop** work without a flag: it cuts the list down to
	/// what has already gone out, and this then becomes true as those last answers land.
	fn is_over(&self) -> bool {
		self.next == self.files.len() && self.running == 0
	}
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
	/// A row was clicked. What that does to the selection depends on what is held (PLAN §9a).
	RowSelected(PathBuf),
	/// The modifiers changed. Tracked because a `mouse_area` press does not carry them, and a
	/// click has to know whether it is a plain one, a toggle or a range.
	ModifiersChanged(Modifiers),
	/// ⌘A / Ctrl+A — every row the files pane is showing.
	SelectAll,
	/// Escape — nothing selected.
	SelectionCleared,
	/// Show a folder in the files pane, from the tree or the dialog.
	FolderSelected(PathBuf),
	/// The **Open folder…** button.
	OpenFolderPressed,
	/// Re-read the folder currently shown — the button, the refresh key, or the watcher's
	/// settle timer, which are three names for one thing.
	RefreshPressed,
	/// The OS says something in the shown folder changed. Not a re-list: it arms the settle
	/// timer, because one dropped folder is a burst of these (PLAN §9).
	FolderTouched,
	/// The files pane was scrolled. Not a preference — the view needs it to know which rows
	/// are worth building (PLAN §9).
	Scrolled(scrollable::Viewport),
	/// A queue row was clicked. The index, not the path: a queue may hold the same track
	/// twice (PLAN §7a).
	QueueSelected(ListId, usize),
	/// A queue row was double-clicked: play it now, out of turn.
	QueueLoad(ListId, usize),
	/// A queue was scrolled, to this absolute offset. Not a preference — the view needs it to
	/// know which rows are worth building, exactly as the files pane does (PLAN §9).
	QueueScrolled(ListId, f32),
	/// A drag entered (`true`) or left (`false`) one of a list's scroll edges: its header,
	/// which scrolls up, or its footer, which scrolls down (PLAN §7a).
	ScrollEdge(ListId, bool, bool),
	/// One step of that scroll. Sent only while an edge is armed.
	ScrollStep,
	/// What the queues' tracks turned out to be, looked up off the GUI thread. A batch rather
	/// than one message per file, because they are asked for as a batch and none of them is
	/// interesting alone.
	Measured(Vec<Facts>),
	/// Add the browser's selection to a queue — at the top when `true`, at the end when
	/// `false`.
	QueueAdd(ListId, bool),
	/// Take the selected row out of a queue.
	QueueRemove(ListId),
	/// Move the selected row one place up (`true`) or down (`false`).
	QueueMove(ListId, bool),
	/// Send the selected row to the neighbouring queue, right (`true`) or left (`false`).
	QueueShift(ListId, bool),
	/// A queue's **Auto-load** switch: whether it hands its top track to a player that has
	/// just run out (PLAN §7a).
	QueueAutoLoad(ListId, bool),
	/// Its **Auto-play** switch: whether a track handed over that way then starts by itself.
	QueueAutoPlay(ListId, bool),
	/// Its **transition**: whether the track it hands over waits for the file to run out, or
	/// takes over when the music stops (PLAN §7b).
	QueueTransition(ListId, Transition),
	/// A disclosure arrow in the tree.
	FolderToggled(PathBuf),
	/// The tree's fold button.
	TreeFolded,
	/// The `.*` toggle.
	HiddenToggled(bool),
	FaderChanged(DeckId, f32),
	CrossfaderChanged(f32),
	CurveSelected(Curve),
	/// The tree's splitter was dragged.
	Resized(pane_grid::ResizeEvent),
	/// The divider under the players was pressed. Arms the drag; the release that ends it
	/// is the same one that ends an in-app file drag.
	DecksGrabbed,
	/// The pointer moved while the divider is held, this far down the window. Sent only
	/// during a drag, because a cursor-move message rebuilds the whole view.
	DecksDragged(f32),
	/// Re-open the audio device after it went away (PLAN §11).
	ReconnectPressed,
	/// A directory listing came back off the GUI thread.
	FilesListed(PathBuf, Result<Vec<Entry>, String>),
	FoldersListed(PathBuf, Result<Vec<PathBuf>, String>),
	/// A track's scan finished. Carries the path it was started for, because a player can
	/// be given another track while the scan runs (PLAN §14a).
	Scanned(DeckId, PathBuf, Result<Scan, String>),
	/// Advance the scanning animation one step. Sent only while a scan is running.
	Sweep,
	/// The waveform was clicked, at this fraction of the track. Moves the playhead and
	/// nothing else — a playing player keeps playing, a paused one stays paused (PLAN §14).
	Seeked(DeckId, f32),
	/// One of the two buttons above a waveform: the top of the file, or the top of the music
	/// (PLAN §14c). The same seek as a click on the strip, aimed at a place rather than a
	/// fraction — which is what makes it exact.
	Jumped(DeckId, Duration),
	/// Work out the waveform, the length and the music's edges for every media file in the
	/// shown folder and everything under it (PLAN §11b).
	ScanFolderPressed,
	/// The same, for the files pane's selection and nothing else (PLAN §9a).
	ScanSelectedPressed,
	/// The walk came back with the files to work through.
	ScanFolderFound(Result<Vec<PathBuf>, String>),
	/// One of them is done. The unit of progress, and one message per file.
	ScanFolderStepped(Facts),
	/// Stop handing out files. Whatever is already on a thread finishes and is kept.
	ScanFolderCancelled,
	/// Throw away everything the app has worked out about other people's files (PLAN §11a).
	ClearCachePressed,
	CacheCleared,
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
	/// During an in-app drag, the pointer entered somewhere a release could land.
	DragOver(DropTarget),
	/// …and left a panel it could have landed in. Coarser than `DragOver` on purpose — see
	/// `Zone`.
	DragOut(Zone),
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
	/// The browser's two panes only. The players sit above the grid, not in it.
	panes: pane_grid::State<Section>,
	/// How tall the players and the mixer want to be, in **pixels** rather than a fraction
	/// of the window (PLAN §6). Kept as what was asked for, not as what fits: a window too
	/// short to grant it compacts the panel, and growing the window again gives it back.
	decks_height: f32,
	/// Whether the divider under the players is being held. Gates the cursor subscription,
	/// so nothing listens to the pointer at rest.
	decks_drag: bool,
	/// The vertical split, so the fold button can find it. `None` while folded, because
	/// closing the tree pane destroys the split with it (PLAN §6).
	tree_split: Option<pane_grid::Split>,
	/// The tree's last width, remembered across a fold for the same reason.
	tree_ratio: f32,
	/// The window's current size, tracked so it can be restored next run (PLAN §11).
	window: (f32, f32),
	/// What an in-app drag is carrying. Armed by a press on a media row — in the files pane
	/// or in a queue — and disarmed by the release, so a plain click is a drag that landed on
	/// nothing (PLAN §10, §7a).
	drag: Option<Drag>,
	/// Where the pointer is, of the places a release could land. Only ever set while a drag
	/// is in flight, because
	/// that is the only time the panels are drop targets at all.
	hover: Option<DropTarget>,
	/// Whether files from outside are hovering the window right now.
	os_hover: bool,
	/// The one line of feedback the app gives: what loaded, what would not decode, what
	/// the audio device is (PLAN §7).
	notice: String,
	/// A persisted setting has changed and is not on disk yet. Drives the autosave
	/// subscription, which exists only while this is true — and is what lets a successful
	/// folder listing flush early without turning every refresh into a write.
	dirty: bool,
	/// The watcher saw the folder change and the listing has not caught up yet. Drives the
	/// settle timer the same way `dirty` drives the autosave: it exists only while this is
	/// true, so a folder nothing is happening in costs nothing (PLAN §9).
	stale: bool,
	/// The three queues, indexed by `ListId::index` (PLAN §7a): one in front of each player,
	/// and the shared one between them.
	queues: [Playlist; 3],
	/// How far down each of them is scrolled, in the same order. Not persisted, for the same
	/// reason the files pane's offset is not: where a list is scrolled to is not a setting.
	queue_scroll: [f32; 3],
	/// The list whose edge a drag is resting on, and which way it is scrolling. `None` at
	/// rest, which is what gates the timer that does the scrolling.
	autoscroll: Option<(ListId, bool)>,
	/// The tracks whose length is being looked up right now. A row stays *unmeasured* until
	/// its answer lands, so without this a second edit arriving mid-lookup would send the same
	/// file off to be opened and parsed all over again (PLAN §7a).
	measuring: HashSet<PathBuf>,
	/// What has already been worked out about a file (PLAN §11a). Shared with every job that
	/// reads or writes it: an `Arc` because those jobs are threads, and the whole point is
	/// that the cache is touched *there* rather than on the GUI thread, where a commit is an
	/// `fsync`.
	cache: Arc<Cache>,
	/// Where the music sits inside the files this run has been told about (PLAN §14c).
	///
	/// Filled by every job that finds out — a track's own scan, a queue measurement reading
	/// the cache, a folder scan working it out — and read by the three places that need it:
	/// the handover's early cut, the track it starts next, and the button above the strip.
	///
	/// A map rather than a field on `Deck` and another on `playlist::Item`, because the same
	/// answer serves a loaded track and a queued one and a track that is neither yet. A miss
	/// is the ordinary state and means *play this whole*: nothing here is required for the app
	/// to work, which is what makes the folder scan an optimization rather than a step.
	trims: HashMap<PathBuf, Trim>,
	/// The folder scan, while one is running (PLAN §11b). `None` at rest, which is also what
	/// the Cancel button leaves behind.
	scanning: Option<Scanning>,
	/// What is being held on the keyboard right now (PLAN §9a).
	///
	/// Kept because a press on a row arrives through a `mouse_area`, which does not carry the
	/// modifiers — so the only way to know whether a click is a plain one, a toggle or a range
	/// is to have been listening.
	///
	/// `ponytail:` this can go stale if the app loses focus with a key held, since what arrives
	/// then is a focus event rather than a release. winit sends `ModifiersChanged` on the way
	/// back in, so the worst case is one click on the way back into the window.
	modifiers: Modifiers,
	/// The scanning animation's step counter. A plain integer rather than a timestamp, so
	/// nothing in `view` has to read the clock — the phase is whatever the last `Sweep`
	/// left behind.
	sweep: u32,
}

impl Clecta {
	fn boot(settings: Settings) -> (Self, Task<Message>) {
		// Built by splitting rather than from a `Configuration`, because
		// `with_configuration` does not return the `Split` handle and the fold needs one.
		let (mut panes, files_pane) = pane_grid::State::new(Section::Files);
		let (_tree_pane, tree_split) = panes
			.split(Axis::Vertical, files_pane, Section::Tree)
			.expect("splitting the only pane always succeeds");
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

		// The three lists, each with its own two switches (PLAN §7a). Built here rather than
		// inside the struct below because a restored list is its paths *and* its switches, and
		// the switches are stored in draw order while the paths are not — `cues` is per player
		// and has no slot for the shared list.
		let mut queues = [
			Playlist::from_paths(settings.cues[0].clone()),
			Playlist::from_paths(settings.common.clone()),
			Playlist::from_paths(settings.cues[1].clone()),
		];
		for (index, queue) in queues.iter_mut().enumerate() {
			queue.auto_load = settings.auto_load[index];
			queue.auto_play = settings.auto_play[index];
			queue.transition = settings.transition[index];
		}

		let mut app = Self {
			engine,
			decks,
			crossfader: settings.crossfader,
			curve: settings.curve,
			browser: Browser::default(),
			tree: Tree::new(fsio::roots()),
			panes,
			decks_height: settings.decks_height,
			decks_drag: false,
			tree_split: Some(tree_split),
			tree_ratio: TREE_RATIO,
			window: settings.window,
			drag: None,
			hover: None,
			os_hover: false,
			notice,
			dirty: false,
			stale: false,
			queues,
			queue_scroll: [0.0; 3],
			autoscroll: None,
			measuring: HashSet::new(),
			// Opened here rather than lazily, for the same reason `Settings::load` is read
			// before the window exists: it is one small file, once, and everything after it
			// wants to know whether there is a cache to ask (PLAN §11a).
			cache: Arc::new(Cache::open(&paths::data_dir().join(CACHE_FILE))),
			trims: HashMap::new(),
			scanning: None,
			modifiers: Modifiers::empty(),
			sweep: 0,
		};
		app.apply_gains();

		// Drop entries whose file is gone. The one job in the app that gets a bare thread
		// rather than `off_thread`: nothing waits for the answer and nothing on screen
		// changes, so there is no message to send and no `Task` to carry it (PLAN §11a).
		let pruning = app.cache.clone();
		std::thread::spawn(move || {
			pruning.prune();
		});

		// Open where the last run left off, or on the home folder — so the first thing on
		// screen is a real listing rather than an empty pane with a button in it. The
		// folder is known to exist: `Settings` drops one that does not.
		//
		// The restored queues have paths and no lengths, so the running times are measured
		// alongside the first listing rather than after it: both are file reads on threads of
		// their own, and neither is in the other's way.
		let task = Task::batch([
			app.select_folder(settings.folder.unwrap_or_else(fsio::home)),
			app.measure_queues(),
		]);
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
			cues: [
				self.queues[ListId::Cue(DeckId::One).index()].paths(),
				self.queues[ListId::Cue(DeckId::Two).index()].paths(),
			],
			common: self.queues[ListId::Common.index()].paths(),
			// In draw order, which is the order the lists are stored in here — unlike `cues`
			// above, which is per player.
			auto_load: std::array::from_fn(|index| self.queues[index].auto_load),
			auto_play: std::array::from_fn(|index| self.queues[index].auto_play),
			transition: std::array::from_fn(|index| self.queues[index].transition),
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

		// The pointer, while the divider is held and at no other time — the same rule as the
		// three above. A cursor-move message rebuilds the whole view, and the files pane
		// builds every row when it does (PLAN §9), so this is the one gesture where gating
		// the subscription is cheaper than ignoring the message.
		let divider = if self.decks_drag {
			divider_drag()
		} else {
			Subscription::none()
		};

		// The folder the pane is showing, watched by the OS (PLAN §9). Keyed on the path, so
		// choosing another folder tears this watcher down and builds one for the new place —
		// which is the whole reason `run_with` takes something hashable.
		let watcher = match &self.browser.folder {
			Some(folder) => Subscription::run_with(folder.clone(), watch),
			None => Subscription::none(),
		};

		// …and the timer that turns a burst of file events into one re-listing. Only while
		// something is actually waiting, the same rule as every subscription above.
		let settle = if self.stale {
			time::every(WATCH_SETTLE).map(|_| Message::RefreshPressed)
		} else {
			Subscription::none()
		};

		// The queue scroll a drag is holding an edge down for. Same rule again: it exists
		// only while a pointer is actually resting on an edge.
		let scrolling = if self.autoscroll.is_some() {
			time::every(AUTOSCROLL).map(|_| Message::ScrollStep)
		} else {
			Subscription::none()
		};

		Subscription::batch([
			tick,
			autosave,
			sweep,
			divider,
			watcher,
			settle,
			scrolling,
			keyboard(),
			window::resize_events().map(|(_, size)| Message::WindowResized(size)),
			window::close_requests().map(|_| Message::CloseRequested),
			gestures(),
		])
	}

	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Tick => return self.poll_players(),

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

			Message::LoadSelected(id) => return self.load_batch(id, self.browser.selected_media()),

			// The double click acts on the **selection** when the row it landed on is part of
			// one, and on that row alone when it is not (PLAN §9a). Both cases are reachable:
			// the press before it selects the row, unless it was a command-click that
			// deselected it — and a gesture that names a file should still do something with
			// that file.
			Message::LoadUnaimed(path) => {
				let id = deck::idle_target(&self.decks[0], &self.decks[1]);
				let batch = if self.browser.is_selected(&path) {
					self.browser.selected_media()
				} else {
					vec![path]
				};
				return self.load_batch(id, batch);
			}

			Message::RowSelected(path) => {
				// The press both moves the selection and arms a drag with it. A release that
				// is not over a player disarms it and nothing else happens — which is
				// exactly what a plain click is (PLAN §10).
				//
				// **A plain press on a row that is already selected leaves the selection
				// alone**, which is what makes a multi-row drag possible at all: collapsing to
				// one row here would destroy the very selection the drag is about to carry
				// (PLAN §9a).
				if !(self.browser.is_selected(&path) && self.click_kind() == Click::Replace) {
					self.browser.click(&path, self.click_kind());
				}

				// From the pane, so a drop *copies*: the files stay in the folder.
				let items: Vec<playlist::Item> = self
					.browser
					.selected_media()
					.into_iter()
					.map(playlist::Item::new)
					.collect();
				self.drag = (!items.is_empty()).then_some(Drag { items, from: None });
			}

			Message::ModifiersChanged(modifiers) => self.modifiers = modifiers,

			Message::SelectAll => self.browser.select_all(),

			Message::SelectionCleared => self.browser.clear_selection(),

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
				// Cleared here rather than only in the timer's own arm, so a *manual* refresh
				// also satisfies a pending automatic one: the listing that is about to be
				// read is the current one either way.
				self.stale = false;
				if let Some(folder) = self.browser.folder.clone() {
					return list_files(folder);
				}
			}

			Message::FolderTouched => self.stale = true,

			Message::QueueSelected(id, index) => {
				// The press both moves the selection and arms a drag with it, exactly as a
				// press in the files pane does (PLAN §10, §9a) — and for the same reason:
				// there is no separate gesture to start a drag with, so the press has to do
				// both jobs, and a plain press on an already-selected row leaves the selection
				// alone so that a multi-row drag has something to carry.
				let kind = self.click_kind();
				let list = &mut self.queues[id.index()];
				if !(list.is_selected(index) && kind == Click::Replace) {
					list.click(index, kind);
				}

				let list = &self.queues[id.index()];
				let rows: Vec<usize> = list.selection().collect();
				// From a list, so a drop *moves*: the rows leave here when they land.
				self.drag = (!rows.is_empty()).then(|| Drag {
					items: list.selected_items(),
					from: Some((id, rows)),
				});
			}

			// A double click plays queued rows now, out of turn — the one way from a list to
			// a player that is neither a drag nor waiting for a track to end (PLAN §7a). It
			// takes them with it, exactly as a drag onto a player does and as the handover
			// itself does: the queue is what is still to come.
			Message::QueueLoad(list, index) => {
				// A cue plays on the player it belongs to; the shared list has no player of its
				// own, so it uses the same "whichever is free" rule an unaimed drop does.
				let id = match list {
					ListId::Cue(id) => id,
					ListId::Common => deck::idle_target(&self.decks[0], &self.decks[1]),
				};

				// The selection when the double-clicked row is part of it, that row alone when
				// it is not — the same rule the files pane's double click follows (PLAN §9a).
				let taken = if self.queues[list.index()].is_selected(index) {
					self.queues[list.index()].take_selected()
				} else {
					self.queues[list.index()]
						.remove(index)
						.into_iter()
						.collect()
				};
				if taken.is_empty() {
					return Task::none();
				}

				// The press that opened this double click armed a drag, and the rows it was
				// carrying have just left the list. Disarmed here rather than left to the
				// release, which would otherwise drop rows that no longer exist.
				self.drag = None;

				let paths = taken.into_iter().map(|item| item.path).collect();
				return Task::batch([self.queued(), self.load_batch(id, paths)]);
			}

			// Not persisted and deliberately not `dirty`, exactly like the files pane's own
			// offset: where a list is scrolled to is not a setting.
			Message::QueueScrolled(id, offset) => self.queue_scroll[id.index()] = offset,

			Message::ScrollEdge(id, up, entering) => {
				// The same shape as `DragOut`, and for the same reason: entering one edge and
				// leaving another arrive in an order nothing guarantees, so a leave clears only
				// the edge it is actually about.
				if entering {
					self.autoscroll = Some((id, up));
				} else if self.autoscroll == Some((id, up)) {
					self.autoscroll = None;
				}
			}

			Message::ScrollStep => {
				if let Some((id, up)) = self.autoscroll {
					// `scroll_by` rather than a computed offset: it clamps against the pane's
					// real bounds, which the app does not know and would have to guess at. The
					// stored offset catches up on the next frame — the `scrollable` republishes
					// its viewport on every redraw where it has moved, which is what keeps the
					// virtualized rows following a scroll nobody asked the pointer for.
					let step = if up {
						-AUTOSCROLL_STEP
					} else {
						AUTOSCROLL_STEP
					};
					return operation::scroll_by(
						ui::playlist::scroll_id(id),
						AbsoluteOffset { x: 0.0, y: step },
					);
				}
			}

			Message::Measured(measured) => {
				for facts in measured {
					// Let go of it first: this batch is done with the file whatever the lists
					// have done with it, and a path left behind here would never be looked up
					// again. Every path that went out comes back, so the set empties.
					self.measuring.remove(&facts.path);
					self.learned(&facts);
				}
			}

			// Every arm below edits a queue, and every queue is persisted, so each ends in
			// `queued` rather than repeating the `dirty` flag four times.
			Message::QueueAdd(id, prepend) => {
				// Read from the *browser*, not from a payload on the message: the buttons were
				// drawn from this same selection, so carrying the paths would be carrying a copy
				// of something that cannot have changed in between.
				let items: Vec<playlist::Item> = self
					.browser
					.selected_media()
					.into_iter()
					.map(playlist::Item::new)
					.collect();

				let duplicates = playlist::duplicates(&self.queues, &items, &[]);
				let admission = self.admits(&items, &duplicates);
				let items: Vec<playlist::Item> = items
					.into_iter()
					.enumerate()
					.filter(|(index, _)| admission.keeps(*index, &duplicates))
					.map(|(_, item)| item)
					.collect();
				if items.is_empty() {
					return Task::none();
				}

				let queue = &mut self.queues[id.index()];
				// Top or end, and either way in the pane's order — top to bottom, so a batch
				// plays in the order it was looked at.
				let at = if prepend { 0 } else { queue.items().len() };
				queue.insert_many(at, items);
				return self.queued();
			}

			Message::QueueRemove(id) => {
				if self.queues[id.index()].has_selection() {
					self.queues[id.index()].take_selected();
					return self.queued();
				}
			}

			Message::QueueMove(id, up) => {
				if self.queues[id.index()].shift_selected(up) {
					return self.queued();
				}
			}

			Message::QueueShift(id, right) => {
				// Both halves or neither: tracks taken out of one list and not put into
				// another are tracks the user just lost. Which is also why the rows are *looked
				// at* before they are taken — a warning the user cancels must leave them where
				// they were, and putting them back afterwards would have to rebuild the
				// selection too.
				let Some(target) = id.neighbour(right) else {
					return Task::none();
				};

				let rows: Vec<usize> = self.queues[id.index()].selection().collect();
				let items = self.queues[id.index()].selected_items();
				let moving: Vec<(ListId, usize)> = rows.iter().map(|&index| (id, index)).collect();

				// The warning can leave *some* of them standing, so the rows and the tracks are
				// filtered together — what leaves the list is exactly what arrives in the other.
				let duplicates = playlist::duplicates(&self.queues, &items, &moving);
				let admission = self.admits(&items, &duplicates);
				let (rows, items): (Vec<usize>, Vec<playlist::Item>) = rows
					.into_iter()
					.zip(items)
					.enumerate()
					.filter(|(index, _)| admission.keeps(*index, &duplicates))
					.map(|(_, pair)| pair)
					.unzip();
				if items.is_empty() {
					return Task::none();
				}

				self.queues[id.index()].take_rows(&rows);
				let at = self.queues[target.index()].items().len();
				self.queues[target.index()].insert_many(at, items);
				return self.queued();
			}

			// The two switches, which are settings rather than edits: `dirty` alone, without
			// the measuring `queued` also does — nothing was added, so there is nothing new to
			// look up.
			Message::QueueAutoLoad(id, on) => {
				self.queues[id.index()].auto_load = on;
				self.dirty = true;
			}

			Message::QueueAutoPlay(id, on) => {
				self.queues[id.index()].auto_play = on;
				self.dirty = true;
			}

			// The third of them, and the same kind of thing: it changes what happens at the end
			// of a track, not what is in the list.
			Message::QueueTransition(id, transition) => {
				self.queues[id.index()].transition = transition;
				self.dirty = true;
			}

			// No `dirty`: where the pane is scrolled to is not something a restart should
			// restore, and marking it would write `settings.json` every time a wheel turned.
			Message::Scrolled(viewport) => self.browser.scroll = viewport.absolute_offset().y,

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

			Message::DecksGrabbed => self.decks_drag = true,

			Message::DecksDragged(y) => {
				// Stored already clamped, so what is remembered is a height the user could
				// see. The pointer sits mid-divider rather than at its top edge, which is
				// what makes the panel follow the cursor instead of jumping by the
				// divider's height the moment it is grabbed.
				self.decks_height =
					dragged_height(self.window.1, y - WINDOW_PADDING - PANE_SPACING / 2.0);
				self.dirty = true;
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

			Message::DragOver(target) => self.hover = Some(target),

			Message::DragOut(zone) => {
				// Compared rather than merely cleared: every panel sees the same cursor move,
				// and the one being left is not always the one updated first. A leave that
				// only clears *its own* zone cannot wipe an enter that has already landed.
				if self.hover.is_some_and(|target| target.zone() == zone) {
					self.hover = None;
				}
			}

			Message::DragReleased => {
				// One release ends both drags, because a release is a release: the divider
				// needs no listener of its own, and `gestures` already takes every one.
				self.decks_drag = false;
				// Cleared here rather than left to a leave: the edges are only `mouse_area`s
				// while a drag is in flight, so releasing *on* one destroys the widget that
				// would have reported the pointer leaving it, and the list would scroll for
				// ever (PLAN §7a).
				self.autoscroll = None;
				let target = self.hover.take();
				// Disarmed on every release, whatever it was over, so nothing is left
				// armed behind a drag the user thought better of.
				let Some(drag) = self.drag.take() else {
					return Task::none();
				};

				match target {
					// Onto a player: the first plays now, jumping whatever queue it came from,
					// and the rest go to the top of that player's cue (PLAN §9a) — so rows
					// dragged out of a list leave the list, exactly as they would have if the
					// list had handed them over itself (PLAN §7a).
					Some(DropTarget::Player(id)) => {
						let mut tasks = Vec::new();
						if let Some((list, rows)) = &drag.from {
							self.queues[list.index()].take_rows(rows);
							tasks.push(self.queued());
						}
						let paths = drag.items.into_iter().map(|item| item.path).collect();
						tasks.push(self.load_batch(id, paths));
						return Task::batch(tasks);
					}
					Some(DropTarget::Row(list, index)) => {
						self.drop_into(list, index, drag);
						return self.queued();
					}
					// A drag that landed on nothing is a plain click, and a click has already
					// done its work: the press selected the row.
					None => {}
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

			Message::Scanned(id, path, result) => {
				// Recorded before anything is checked: where a file's music sits is true about
				// the *file*, whatever is loaded in the player by now (PLAN §14c).
				let trim = result.as_ref().ok().and_then(|scan| scan.trim);
				self.remember(&path, trim);

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
					Ok(scan) => self.decks[id.index()].peaks = scan.peaks,
					// Odd but reachable: playback opened the file and the scan did not,
					// because it was replaced or unmounted between the two reads.
					Err(error) => self.notice = format!("{}: {error}", id.label()),
				}
			}

			// Wrapping, because this counter is only ever read modulo `SWEEP_STEPS`, and an
			// app left scanning for three years should not panic in a debug build.
			Message::Sweep => self.sweep = self.sweep.wrapping_add(1),

			Message::Seeked(id, fraction) => self.seek(id, fraction),

			Message::Jumped(id, to) => self.seek_to(id, to),

			Message::ScanFolderPressed => {
				let Some(folder) = self.browser.folder.clone() else {
					return Task::none();
				};
				self.notice = format!("looking for media under {}…", folder.display());

				// The walk before the work: the count on screen has to be a real total rather
				// than one that grows as folders are discovered, or the progress would run
				// backwards on a deep folder.
				return off_thread(
					move || fsio::media_tree(&folder),
					|found| {
						Message::ScanFolderFound(found.unwrap_or_else(|| {
							Err("the folder walk stopped unexpectedly".to_string())
						}))
					},
				);
			}

			// The same work as a folder scan, on the rows the user pointed at instead of on a
			// walk (PLAN §9a). It goes through the same message, so there is one driver, one
			// progress line and one Stop button rather than two of each.
			Message::ScanSelectedPressed => {
				let files = self.browser.selected_media();
				return Task::done(Message::ScanFolderFound(Ok(files)));
			}

			Message::ScanFolderFound(result) => match result {
				Ok(files) if files.is_empty() => {
					self.notice =
						"nothing to prepare — no media files under that folder".to_string();
				}
				Ok(files) => {
					self.scanning = Some(Scanning {
						files,
						next: 0,
						done: 0,
						running: 0,
					});
					return self.scan_step();
				}
				Err(error) => self.notice = error,
			},

			Message::ScanFolderStepped(facts) => {
				// Kept whatever the scan is doing by now: the work is done and the answer is
				// true, so a cancel that arrived while this file was on a thread does not throw
				// its answer away.
				self.learned(&facts);
				if let Some(scanning) = self.scanning.as_mut() {
					scanning.done += 1;
					scanning.running -= 1;
				}
				return self.scan_step();
			}

			// The threads already out are left to finish — they are decoding a file each, and
			// there is no way to interrupt a decode that does not also mean checking a flag in
			// the sample loop. What stops is the *handing out*, which is the part that would
			// otherwise run for another nine minutes.
			//
			// Cutting the list down to what has already gone out, rather than dropping the scan
			// where it stands, is what keeps the counters honest: the four answers still coming
			// belong to this scan and are counted by it, and the state clears itself when the
			// last of them lands. Dropping it here would leave four jobs reporting into a scan
			// that no longer exists — and starting another one before they landed would count
			// their files twice and run `running` past zero.
			Message::ScanFolderCancelled => {
				if let Some(scanning) = self.scanning.as_mut() {
					scanning.files.truncate(scanning.next);
				}
			}

			Message::ClearCachePressed => {
				// Asked first, with the same modal the duplicate warning uses: this is the one
				// button in the app that throws work away, and it is next to the one that makes
				// it. `ponytail:` modal, so the playhead stops while it is open (PLAN §7a).
				let confirmed = rfd::MessageDialog::new()
					.set_level(rfd::MessageLevel::Warning)
					.set_title("Clear the cache")
					.set_description(
						"Throw away every waveform, length and music-start clecta has worked out?\n\n\
						 Nothing is lost but the time to work them out again.",
					)
					.set_buttons(rfd::MessageButtons::OkCancel)
					.show() == rfd::MessageDialogResult::Ok;
				if !confirmed {
					return Task::none();
				}

				// Off the GUI thread like every other touch of the store: a commit is an
				// `fsync` (PLAN §11a).
				let cache = self.cache.clone();
				return off_thread(move || cache.clear(), |_| Message::CacheCleared);
			}

			// The in-memory answers are deliberately kept: they are still true, and throwing
			// them away would blank the waveform of a track that is playing. What was cleared is
			// the *disk*, so the next launch works everything out again — which is what a clean
			// start means.
			Message::CacheCleared => {
				self.notice = "cache cleared".to_string();
			}
		}

		Task::none()
	}

	/// Read the playhead and watch for the end of a track — the whole GUI↔audio bridge
	/// (PLAN §4).
	fn poll_players(&mut self) -> Task<Message> {
		if self.engine.is_none() {
			return Task::none();
		}

		let mut ended = Vec::new();

		for id in DeckId::ALL {
			if !self.decks[id.index()].is_playing() {
				continue;
			}

			// Re-borrowed each time round rather than held: the loop now calls `&mut self`
			// methods, and the engine is only read.
			let engine = self.engine.as_ref().expect("checked above");

			if engine.finished(id) {
				// There is no end-of-track callback in rodio; `empty()` going true on the
				// tick is the signal (PLAN §7).
				//
				// `empty()` also means the source has been *consumed*: rodio has dropped it
				// and there is nothing left in the player to start. The app is about to show
				// that track stopped at 0:00 with a live Play button, so the file has to go
				// back in, or that button is a lie — `play()` on an empty player is silence
				// (PLAN §7).
				//
				// `ponytail:` this re-opens the file even when the handover below replaces it
				// a moment later, which costs one header parse and one `clear()` per track.
				// Cheap, and it is the one place where the model and rodio can disagree —
				// worth splitting only if a handover ever feels like it hitches.
				if let Some(track) = &self.decks[id.index()].track {
					let path = track.path.clone();
					if let Err(error) = engine.load(id, &path) {
						self.notice = format!("{}: {error:#}", id.label());
					}
				}

				let deck = &mut self.decks[id.index()];
				deck.transport = deck::transition(deck.transport, deck::Event::Ended);
				deck.position = Duration::ZERO;
				ended.push(id);
			} else {
				let position = engine.position(id);
				self.decks[id.index()].position = position;

				// The *other* end of a track (PLAN §7b): the music has stopped, and the list
				// waiting behind this player asked not to sit through the run-out.
				if self.cuts_early(id, position) {
					// Stopped rather than re-appended, which is the whole difference from the
					// branch above: nothing has been consumed here — the file is still in the
					// player and still playing — so it is rewound and paused, and the handover
					// replaces it a moment later. Doing nothing would leave the tail audible
					// under the next track if the load failed.
					if let Err(error) = engine.stop(id) {
						self.notice = format!("{}: {error:#}", id.label());
					}

					let deck = &mut self.decks[id.index()];
					deck.transport = deck::transition(deck.transport, deck::Event::Ended);
					deck.position = Duration::ZERO;
					ended.push(id);
				}
			}
		}

		// After the loop, not inside it: `advance` loads, and loading is a `&mut self` call
		// that would fight the engine borrow above.
		Task::batch(ended.into_iter().map(|id| self.advance(id)))
	}

	/// Whether the track playing on this player should give way now rather than at the end of
	/// its file (PLAN §7b).
	///
	/// Asked of the list that would *supply* the next track, which is the same rule the two
	/// switches follow (PLAN §7a): the list waiting behind a player is what says how it wants
	/// to take over. And it is asked at all only when there is something to take over — the
	/// last track of the evening plays its run-out, because cutting it short would leave a
	/// player stopped early for nothing.
	fn cuts_early(&self, id: DeckId, position: Duration) -> bool {
		let Some(track) = &self.decks[id.index()].track else {
			return false;
		};
		let Some(source) = playlist::next_source(
			id,
			&self.queues[ListId::Cue(id).index()],
			&self.queues[ListId::Common.index()],
		) else {
			return false;
		};

		playlist::hands_over_early(
			self.queues[source.index()].transition,
			position,
			self.trims.get(&track.path).copied(),
		)
	}

	/// Give a player that has just finished the next track from a queue (PLAN §7a).
	///
	/// **Only from here**, which is the whole dispatch rule: a track ending is the one event
	/// that pulls from a queue. A player sitting empty at startup is left alone, so adding
	/// files to a list never loads anything by itself and every automatic load can be traced
	/// back to a track the user heard end.
	///
	/// The track is *taken* rather than marked played: a queue is what is still to come, so
	/// the row leaving the list as it reaches the player is what makes the list mean that.
	///
	/// Both switches belong to the **source list**, not to the player: the list that gave the
	/// track is the list that says whether it plays, so Cue 1 can run the evening by itself
	/// while the shared pool stays a shelf someone takes from by hand (PLAN §7a).
	fn advance(&mut self, id: DeckId) -> Task<Message> {
		let cue = &self.queues[ListId::Cue(id).index()];
		let common = &self.queues[ListId::Common.index()];

		// A list with **Auto-load** off offers nothing however full it is, so this is also
		// where switching every list off leaves the player simply stopped.
		let Some(source) = playlist::next_source(id, cue, common) else {
			return Task::none();
		};
		let Some(item) = self.queues[source.index()].take_next() else {
			return Task::none();
		};

		// Lands on `Stopped`, like every other load (PLAN §7): the next track is ready at
		// 0:00 and audible only when someone presses Play — unless this list was told to
		// start it. On a mixer an unrequested fade-in is a mistake that cannot be taken back,
		// which is why that is a switch and not the default.
		let list = &self.queues[source.index()];
		let (auto_play, transition) = (list.auto_play, list.transition);
		let path = item.path;
		let loading = Task::batch([self.queued(), self.load(id, path.clone())]);

		// Only if the file actually arrived. A load that failed leaves the *previous* track in
		// the player and says so in the notice (PLAN §7), and pressing Play on that would be
		// the app restarting a track nobody queued — the one way an automatic start could
		// play the wrong thing.
		if !self.decks[id.index()]
			.track
			.as_ref()
			.is_some_and(|track| track.path == path)
		{
			return loading;
		}

		// The other half of the transition setting (PLAN §7b): a list that skips the blanks at
		// the end of one track also skips them at the start of the next. Silently 0:00 for a
		// track nobody has scanned — the folder scan is what makes this exact, and until then
		// the app plays the file it was given from the top.
		if transition == Transition::Trimmed
			&& let Some(start) = self.trims.get(&path).map(|trim| trim.start)
		{
			self.seek_to(id, start);
		}

		if auto_play {
			self.transport(id, deck::Event::Play);
		}

		loading
	}

	/// Give a player a batch of tracks: **the first plays, the rest go to the top of its cue**
	/// (PLAN §9a).
	///
	/// Every aimed door comes through here — the two `→ Player` buttons, a double click, and a
	/// drag onto a panel — so five files selected mean the same thing however they were sent.
	/// A player holds one track, and the queue in front of it is what plays next, so "load
	/// these five" already had an answer in the app: it just had to be five separate steps
	/// before.
	///
	/// The **top** of the cue rather than the end, which is the one place this reads the intent
	/// over the word: the promise is that the batch plays back to back, and appending would let
	/// whatever was already queued play in the middle of it.
	///
	/// No duplicate warning, deliberately — the same silence a single drag onto a player has
	/// always kept (PLAN §7a). This gesture means *now*, and a modal that appeared after the
	/// first track had already started would be asking about something that has happened.
	fn load_batch(&mut self, id: DeckId, paths: Vec<PathBuf>) -> Task<Message> {
		let mut paths = paths.into_iter();
		let Some(first) = paths.next() else {
			return Task::none();
		};

		let rest: Vec<playlist::Item> = paths.map(playlist::Item::new).collect();
		if !rest.is_empty() {
			self.queues[ListId::Cue(id).index()].insert_many(0, rest);
			return Task::batch([self.load(id, first), self.queued()]);
		}

		self.load(id, first)
	}

	/// Remember what a job worked out about one file, and settle every queued row holding it.
	///
	/// Applied to all three lists, because a track can be moved between them while the answer
	/// is being looked up — and by path, so one answer settles every row holding that file. A
	/// row that has been removed in the meantime simply matches nothing.
	fn learned(&mut self, facts: &Facts) {
		self.remember(&facts.path, facts.trim);
		for queue in &mut self.queues {
			queue.measured(&facts.path, facts.duration);
		}
	}

	/// Where a file's music sits, if the job that looked found any (PLAN §14c).
	///
	/// A `None` is *not* stored as "there is no trim", and the distinction matters: it means
	/// this job could not say, and another one still might — a queue measurement only reads the
	/// cache, where a folder scan decodes. Overwriting a known answer with silence would make
	/// queueing a track un-learn what scanning it taught.
	fn remember(&mut self, path: &Path, trim: Option<Trim>) {
		if let Some(trim) = trim {
			self.trims.insert(path.to_path_buf(), trim);
		}
	}

	/// Hand files to threads until the fan-out is full, and notice when the last one lands
	/// (PLAN §11b).
	///
	/// The whole driver, and it is called from exactly two places: the moment the walk comes
	/// back, and every time a file reports. There is no timer and no subscription — a scan is a
	/// chain of messages that refills itself, so it stops costing anything the moment it ends.
	fn scan_step(&mut self) -> Task<Message> {
		let Some(scanning) = self.scanning.as_mut() else {
			return Task::none();
		};

		if scanning.is_over() {
			let total = scanning.files.len();
			self.scanning = None;
			self.notice = format!("prepared {total} files");
			return Task::none();
		}

		let mut tasks = Vec::new();
		for _ in 0..scanning.slots() {
			let path = scanning.files[scanning.next].clone();
			scanning.next += 1;
			scanning.running += 1;
			tasks.push(scan_file(path, self.cache.clone()));
		}

		Task::batch(tasks)
	}

	/// May these tracks go into a queue — asking first about any that are already in one
	/// (PLAN §7a, §9a).
	///
	/// `All` without a word when none of them is queued anywhere, which is the ordinary case
	/// and costs a scan of three short lists. Otherwise the app **asks**, because queueing a
	/// track twice is sometimes exactly what someone means and sometimes a mistake, and nothing
	/// in the app can tell those apart. A silent refusal would be the app deciding; a silent
	/// accept is what left the mistake possible.
	///
	/// **One dialog for a batch, and three answers**, which is what a batch made necessary: the
	/// old two — do it, or do not — could not say the thing most people want when nineteen
	/// tracks are fine and one is a repeat. So *Yes* queues them all again, *No* takes the ones
	/// that are not repeats and leaves the repeats behind, and *Cancel* does nothing at all.
	/// Asking once per duplicate was the alternative, and three modals in a row for one button
	/// press is not an improvement on the mistake it prevents.
	///
	/// `ponytail:` the buttons are the platform's Yes / No / Cancel with their meaning in the
	/// text, rather than rfd's custom labels — those need a Cargo feature that is off by
	/// default and, by rfd's own documentation, work on Windows only with it. A dialog whose
	/// buttons might be unlabelled on the one target nobody has run is not worth three words.
	///
	/// A modal, which blocks the GUI thread while it is open — the playhead stops with it,
	/// exactly as it does for the **Load…** dialog.
	fn admits(&self, items: &[playlist::Item], duplicates: &[(usize, ListId)]) -> Admission {
		if duplicates.is_empty() {
			return Admission::All;
		}

		// Named rather than counted, up to a point: a list of forty names is not a question
		// anyone reads, and the count is the part that decides the answer.
		const NAMED: usize = 6;
		let named: Vec<String> = duplicates
			.iter()
			.take(NAMED)
			.filter_map(|(index, list)| {
				items
					.get(*index)
					.map(|item| format!("    {} ({})", item.name, list.label()))
			})
			.collect();
		let rest = duplicates.len().saturating_sub(named.len());

		let description = format!(
			"{} of {} already queued:\n\n{}{}\n\nYes — queue them again.\nNo — add only the {} that {} not.\nCancel — add nothing.",
			duplicates.len(),
			items.len(),
			named.join("\n"),
			if rest > 0 {
				format!("\n    …and {rest} more")
			} else {
				String::new()
			},
			items.len() - duplicates.len(),
			if items.len() - duplicates.len() == 1 {
				"is"
			} else {
				"are"
			},
		);

		match rfd::MessageDialog::new()
			.set_level(rfd::MessageLevel::Warning)
			.set_title("Already queued")
			.set_description(description)
			.set_buttons(rfd::MessageButtons::YesNoCancel)
			.show()
		{
			rfd::MessageDialogResult::Yes => Admission::All,
			rfd::MessageDialogResult::No => Admission::Fresh,
			_ => Admission::Nothing,
		}
	}

	/// Which of the three a press is, from whatever is being held right now (PLAN §9a).
	fn click_kind(&self) -> Click {
		Click::of(self.modifiers.command(), self.modifiers.shift())
	}

	/// A queue changed: the settings file is out of date, and something in it may not have
	/// been measured yet. One place, so the editing arms, `advance` and a drop cannot each
	/// remember half of it.
	fn queued(&mut self) -> Task<Message> {
		self.dirty = true;
		self.measure_queues()
	}

	/// Look up the length of every queued track nobody has measured yet (PLAN §7a).
	///
	/// One job for the whole batch rather than one per file: they are wanted together, and a
	/// thread each would be dozens of threads for a restored queue. Off the GUI thread because
	/// it opens files, which is the rule `off_thread` exists to keep — a queue on a network
	/// mount would otherwise freeze the app for as long as the mount took to answer.
	///
	/// Empty when there is nothing to measure, which is the usual case: every call that edits
	/// a queue comes through here, and only the ones that *added* something have work to do.
	///
	/// Nothing is ever asked about twice, because a row counts as unmeasured until its answer
	/// *lands* — long after the job that will produce it started. `measuring` is what a job
	/// takes with it on the way out and gives back on the way in, and `to_measure` subtracts
	/// it. Without that, two edits a few milliseconds apart would send the same file to be
	/// opened and parsed twice, and twenty edits would send the first one twenty times.
	///
	/// The invariant that makes the giving-back safe: **the arm is handed exactly the batch
	/// that was asked about**, whatever happened to the job — so a batch cannot be lost from
	/// the set and leave a file nothing will ever look at again.
	fn measure_queues(&mut self) -> Task<Message> {
		let paths = playlist::to_measure(&self.queues, &self.measuring);
		if paths.is_empty() {
			return Task::none();
		}
		self.measuring.extend(paths.iter().cloned());

		// The batch survives the job, because the arm needs it whether or not the job answers.
		let asked = paths.clone();
		let cache = self.cache.clone();

		off_thread(
			move || {
				paths
					.into_iter()
					.map(|path| cached_facts(&cache, &path))
					.collect::<Vec<_>>()
			},
			// A job can only fail to answer by panicking. Its batch is then reported as
			// measured-and-no-length rather than left alone, which is the safer of the two
			// wrongs: forgetting it would strand those files in `measuring` for the rest of
			// the run, and re-queueing them would retry a panic that is probably deterministic
			// on every edit for ever. The footer already says what this looks like — the
			// running time keeps its `+`.
			move |measured| {
				Message::Measured(measured.unwrap_or_else(|| {
					asked
						.iter()
						.map(|path| Facts {
							path: path.clone(),
							duration: None,
							trim: None,
						})
						.collect()
				}))
			},
		)
	}

	/// Put what a drag was carrying into a list, above row `index` (PLAN §7a, §9a).
	///
	/// Three cases, and the first is the one worth separating: rows dragged **within their own
	/// list** are a reorder, not a remove followed by an insert. Doing it as two steps would
	/// need the caret index adjusting for the holes the rows left behind, which is what
	/// `relocate` exists to get right — so it is called rather than reimplemented here.
	fn drop_into(&mut self, list: ListId, index: usize, drag: Drag) {
		// A reorder is the one drop that cannot produce a duplicate: the rows are already
		// somewhere in this list and are only moving inside it. Asked before anything is
		// touched, because a cancelled drop must leave every list exactly as it was.
		if let Some((source, rows)) = &drag.from
			&& *source == list
		{
			self.queues[list.index()].relocate(rows, index);
			return;
		}

		// The rows on their way out, so a track dragged from one list to another does not warn
		// about colliding with itself.
		let moving: Vec<(ListId, usize)> = match &drag.from {
			Some((source, rows)) => rows.iter().map(|&row| (*source, row)).collect(),
			None => Vec::new(),
		};

		let duplicates = playlist::duplicates(&self.queues, &drag.items, &moving);
		let admission = self.admits(&drag.items, &duplicates);
		let items: Vec<playlist::Item> = drag
			.items
			.into_iter()
			.enumerate()
			.filter(|(position, _)| admission.keeps(*position, &duplicates))
			.map(|(_, item)| item)
			.collect();
		if items.is_empty() {
			return;
		}

		// Across two lists: taken out of one and put into the other, in that order. The index
		// is the caret in the *destination*, which the removal cannot have moved. From the
		// files pane there is nothing to take out — a drop from there is a copy, so the folder
		// keeps its files.
		if let Some((source, rows)) = &drag.from {
			// Only what the warning left standing, by position, so what leaves is what lands.
			let rows: Vec<usize> = rows
				.iter()
				.copied()
				.enumerate()
				.filter(|(position, _)| admission.keeps(*position, &duplicates))
				.map(|(_, row)| row)
				.collect();
			self.queues[source.index()].take_rows(&rows);
		}

		self.queues[list.index()].insert_many(index, items);
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

		self.seek_to(id, total.mul_f32(fraction));
	}

	/// Move a player's playhead to a place, which is what both gestures come down to: the
	/// strip works in fractions of a width, the two buttons above it work in seconds
	/// (PLAN §14c).
	///
	/// A stopped player that is asked to move **becomes paused**, and that is the one thing
	/// here that touches the transport. Q14 said a seek changes nothing about it, and that is
	/// still right for `Playing` and `Paused` — but `Stopped` in this app means *at the top of
	/// the track*, which is what Stop rewinds to and what every load lands on. A player
	/// labelled "stopped" sitting at 1:30 is the label lying about where Play would start,
	/// which was already true of a click on the strip and is why the rule lives here rather
	/// than on the buttons that made it obvious.
	fn seek_to(&mut self, id: DeckId, to: Duration) {
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
		let deck = &mut self.decks[id.index()];
		deck.position = to;
		if deck.transport == deck::Transport::Stopped {
			deck.transport = deck::Transport::Paused;
		}
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

				scan_track(id, path, self.cache.clone())
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
			// An in-app drag is truly aimed: the pointer is ours the whole way. A drag headed
			// for a *list* lights no player — the caret in that list is showing where it goes,
			// and two indicators at once would each be half a lie.
			match self.hover {
				Some(DropTarget::Player(id)) => Some(id),
				_ => None,
			}
		} else {
			None
		}
	}

	/// Where a release would put the dragged row in this list, if that is where it is headed.
	///
	/// `None` at rest and for the two lists the pointer is not over, so exactly one caret is
	/// ever drawn — the same "one indicator, and it tells the truth" rule as the drop ring.
	fn insertion(&self, list: ListId) -> Option<usize> {
		match self.hover {
			Some(DropTarget::Row(id, index)) if id == list && self.drag.is_some() => Some(index),
			_ => None,
		}
	}

	/// Show a folder: list its files, and open the tree down to it so the two panes agree
	/// about where the user is.
	fn select_folder(&mut self, folder: PathBuf) -> Task<Message> {
		// Set eagerly, so the header shows the destination while the listing is in
		// flight and the stale-listing guard above has something to compare against.
		self.browser.folder = Some(folder.clone());
		self.browser.clear_selection();
		// A new folder is read from the top. Both halves are needed and neither is enough:
		// the field is what `view` builds rows from, and the `scroll_to` is what moves the
		// `scrollable` itself, which otherwise keeps the offset it had and would show the
		// new listing scrolled to wherever the old one was left. A *refresh* deliberately
		// does neither (PLAN §9).
		self.browser.scroll = 0.0;
		// Set here rather than at the two call sites, so a third one cannot forget. `boot`
		// clears it again, because restoring the last folder is not a change.
		self.dirty = true;

		let mut tasks = vec![
			operation::scroll_to(ui::browser::scroll_id(), AbsoluteOffset { x: 0.0, y: 0.0 }),
			list_files(folder.clone()),
		];
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
				// How far the folder scan has got, and `None` when none is running — which is
				// what turns the pane's progress line back into the two buttons that start one
				// (PLAN §11b).
				Section::Files => ui::browser::view(
					&self.browser,
					self.scanning
						.as_ref()
						.map(|scanning| (scanning.done, scanning.files.len())),
				),
				Section::Tree => ui::tree::view(&self.tree, self.browser.folder.as_deref()),
			};
			pane_grid::Content::new(body).style(container::bordered_box)
		})
		.spacing(PANE_SPACING)
		.min_size(MIN_PANE)
		.on_resize(8, Message::Resized);

		// The whole rule, and there is deliberately nothing else to it: the players get the
		// height they were given, the browser gets `Fill`, so a window resize moves the
		// bottom of the file list and nothing else (PLAN §6).
		//
		// Note what is *not* here — the window's height. Every earlier version needed it,
		// to turn pixels into `pane_grid`'s ratio or to work out when to compact, and every
		// one of them wobbled: iced lays out at the new window size a frame before the app
		// is told the window changed, so any height derived from `self.window` is a frame
		// stale on exactly the frames a resize is being watched. A literal cannot be stale.
		// Compacting a window too short for this height is left to iced, whose own
		// `Limits::height` clamps a `Fixed` to what is actually available — measured at
		// layout time, so it is never a frame behind.
		let body = column![
			container(self.decks_view())
				.style(container::bordered_box)
				.height(self.decks_height),
			self.divider(),
			panes,
		];

		column![body, self.status_bar()]
			.spacing(STATUS_GAP)
			.padding(WINDOW_PADDING)
			.into()
	}

	/// The horizontal splitter, hand-written — the one thing `pane_grid` could not be bent
	/// into, since it places a splitter only at a ratio (PLAN §6).
	///
	/// It has no look of its own: it is the same gap `pane_grid` leaves between its own
	/// panes, `PANE_SPACING` wide, with the resize cursor as its whole affordance. The drag
	/// it starts is carried by `divider_drag`, and ended by the release every gesture shares.
	fn divider(&self) -> Element<'_, Message> {
		mouse_area(Space::new().width(Fill).height(PANE_SPACING))
			.on_press(Message::DecksGrabbed)
			.interaction(mouse::Interaction::ResizingVertically)
			.into()
	}

	/// The top section: two players with the mixer strip between them (PLAN §6).
	/// The players, the mixer, and the queue under each of them (PLAN §6, §7a).
	///
	/// Three columns, and each column is its control above its list. The controls are all
	/// fixed-size rows, so they take what they need and the **list takes the rest** — which
	/// is what makes dragging the divider grow the queues rather than pad the players with
	/// empty space.
	fn decks_view(&self) -> Element<'_, Message> {
		let ring = self.drop_ring();
		// Asked once for all three lists: they each draw the same two add buttons, and the
		// answer is a property of the files pane rather than of any list.
		let addable = self.browser.has_media_selection();

		let queue = |id: ListId| {
			ui::playlist::view(
				id,
				&self.queues[id.index()],
				addable,
				self.queue_scroll[id.index()],
				// One value rather than three arguments, and `None` at rest: everything a
				// list does differently during a drag arrives together.
				self.drag.is_some().then(|| ui::playlist::Dragging {
					insertion: self.insertion(id),
					edge: self
						.autoscroll
						.and_then(|(list, up)| (list == id).then_some(up)),
				}),
			)
		};

		row![
			column![
				self.deck_view(DeckId::One, ring),
				queue(ListId::Cue(DeckId::One))
			]
			.spacing(8),
			column![
				container(ui::mixer::view(
					&self.decks[0],
					&self.decks[1],
					self.crossfader,
					self.curve,
				)),
				queue(ListId::Common),
			]
			.spacing(8)
			.width(MIXER_WIDTH),
			column![
				self.deck_view(DeckId::Two, ring),
				queue(ListId::Cue(DeckId::Two))
			]
			.spacing(8),
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
		// Looked up by path rather than kept on the `Deck`, because the same answer is wanted
		// for tracks that are in no player at all (PLAN §14c).
		let trim = self.decks[id.index()]
			.track
			.as_ref()
			.and_then(|track| self.trims.get(&track.path))
			.copied();
		let panel = ui::deck::view(id, &self.decks[id.index()], trim, ring == Some(id), sweep);

		if self.drag.is_none() {
			return panel;
		}

		mouse_area(panel)
			.on_enter(Message::DragOver(DropTarget::Player(id)))
			.on_exit(Message::DragOut(Zone::Player(id)))
			.into()
	}

	/// One line: what just happened, and the way out when the audio device is gone.
	///
	/// Its height is pinned rather than measured, so the body above it has a height that can
	/// be worked out from the window's — see `CHROME`.
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

/// Where a drag of the divider is allowed to leave the players, in a window this tall.
///
/// The **only** place the window's height is still used, and the one place it is safe to:
/// a divider drag is not a window resize, so `self.window` is current rather than a frame
/// behind. What it buys is that the divider cannot be dragged off the bottom of its own
/// window — the browser keeps `MIN_PANE`, so there is always something left to grab.
///
/// `view` deliberately does *not* apply this. Re-clamping on every frame is what would put
/// the stale window height back into the layout, and it is not needed: iced clamps a
/// `Fixed` height to the room it actually has (`Limits::height`), so a window too short
/// compacts the panel on its own and pulling it open again restores what the user chose.
fn dragged_height(window_height: f32, wanted: f32) -> f32 {
	// `max` and `min` rather than comparisons: they also flatten a `NaN`, since `f32::max`
	// returns the other operand when one is not a number. A window height that is not a
	// number therefore reads as the smallest usable one instead of being stored.
	let ceiling = (window_height - CHROME - PANE_SPACING - MIN_PANE).max(MIN_PANE);
	wanted.max(MIN_PANE).min(ceiling)
}

/// The pointer, for as long as the divider is held (PLAN §6).
///
/// Its own subscription rather than an arm in `gestures`, because that one is always on: a
/// cursor-move arm there would publish a message on every mouse move in the app, each of
/// which rebuilds every row of the files pane.
fn divider_drag() -> Subscription<Message> {
	event::listen_with(|event, _status, _window| match event {
		iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
			Some(Message::DecksDragged(position.y))
		}
		_ => None,
	})
}

/// The keyboard: the shortcuts the app has, and what is being held (PLAN §9, §9a).
///
/// Always on, unlike `divider_drag`: a key press is rare where a cursor move is constant, and
/// this one publishes nothing at all unless the key was one of the three. The modifiers are
/// the exception and are published whenever they change — which is on the way down and the way
/// up of every Shift, and no more often than a person can press one.
fn keyboard() -> Subscription<Message> {
	event::listen_with(|event, _status, _window| match event {
		iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
			shortcut(&key, modifiers)
		}
		iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
			Some(Message::ModifiersChanged(modifiers))
		}
		_ => None,
	})
}

/// What a key press means, or nothing at all (PLAN §9, §9a).
///
/// Refresh is two keys, because neither of them travels: **F5** is *the* refresh key on
/// Windows, and on a Mac laptop it is a system key the app is never sent unless the
/// function-key preference is flipped; **⌘R** is what a Mac reaches for and means nothing on
/// Windows. One arm covers both, since `Modifiers::command` is already Cmd on macOS and Ctrl
/// everywhere else — which is what makes **⌘A** one arm too.
///
/// Split out from the subscription because a `Key` can be built in a test and a real key press
/// cannot.
fn shortcut(key: &Key, modifiers: Modifiers) -> Option<Message> {
	match key.as_ref() {
		Key::Named(Named::F5) => Some(Message::RefreshPressed),
		Key::Character("r") if modifiers.command() => Some(Message::RefreshPressed),
		Key::Character("a") if modifiers.command() => Some(Message::SelectAll),
		// Bare, and the one key here that is: Escape means "never mind" everywhere, and there
		// is nothing else in the app for it to mean.
		Key::Named(Named::Escape) => Some(Message::SelectionCleared),
		_ => None,
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

/// Watch one folder, so a file appearing in it shows up without anyone pressing Refresh
/// (PLAN §9).
///
/// The OS does the watching — FSEvents on macOS, `ReadDirectoryChangesW` on Windows — so
/// nothing here polls and nothing runs while nothing changes. `NonRecursive`, because the
/// pane shows one folder and the tree lists its own.
///
/// **What changed is deliberately not read.** Every event means the same thing: the listing
/// might be out of date, and the answer is a whole re-list either way. That also makes the
/// four-slot channel a feature rather than a limit — a burst of a hundred events fills it,
/// the rest are dropped, and dropping them costs nothing because the re-list reads the
/// folder as it is now rather than replaying a diff.
///
/// A watcher that cannot be created says so on stderr and gives up, like `settings.rs` does
/// with a file it cannot write. The Refresh button and the refresh key still work, so the
/// failure costs a convenience rather than a capability — and a status line about it on
/// every folder change would be noise for something the user never asked for.
///
/// The `&PathBuf` is iced's, not a slip: `Subscription::run_with` takes a `fn(&D) -> S`, and
/// `D` is the value it hashes to decide whether this is the same subscription as last frame.
/// A `&Path` here would make `D = Path`, which is unsized. Hence the `allow` — clippy is
/// right in general and wrong about this one signature.
///
/// `use<>` for the same kind of reason: in edition 2024 an `impl Trait` captures every
/// lifetime in scope, which would tie the stream to the borrowed folder and stop it
/// matching the `fn` pointer at all. Nothing is borrowed past the clone on the first line.
#[allow(clippy::ptr_arg)]
fn watch(folder: &PathBuf) -> impl Stream<Item = Message> + use<> {
	let folder = folder.clone();

	stream::channel(4, async move |sender| {
		let handler = move |event: notify::Result<notify::Event>| {
			if event.is_ok() {
				// Cloned per event because the handler is `Fn` and `try_send` needs `&mut`.
				// A clone of an `mpsc::Sender` is a refcount bump.
				let _ = sender.clone().try_send(Message::FolderTouched);
			}
		};

		let mut watcher = match notify::recommended_watcher(handler) {
			Ok(watcher) => watcher,
			Err(error) => return eprintln!("clecta: cannot watch folders: {error}"),
		};

		if let Err(error) = watcher.watch(&folder, RecursiveMode::NonRecursive) {
			return eprintln!("clecta: cannot watch {}: {error}", folder.display());
		}

		// Park here for as long as the subscription lives, because the watcher has to live
		// exactly that long: dropping it stops the watching, and it is dropped when this
		// future is — when the folder changes, or when the app quits.
		std::future::pending::<()>().await
	})
}

/// Run one blocking job on a thread of its own, and hand the executor a `oneshot` to wait
/// on — the one thing an executor is actually good at (PLAN §4).
///
/// This exists because iced's smol executor runs on **one** thread unless `SMOL_THREADS`
/// says otherwise, so anything blocking inside a `Task::perform` async block does not merely
/// queue the next job: it stops every subscription in the app, the 20 Hz playhead tick
/// included. That was found the hard way by the waveform scan, and measuring the directory
/// reads afterwards said they were the same bug with a smaller number — a folder of 5 000
/// files takes 25 ms to read and one of 20 000 takes 95 ms, which is half a tick and two
/// ticks. A network mount has no upper bound at all.
///
/// `None` means the thread ended without answering, which it can only do by panicking. Each
/// caller says what that means in its own words rather than swallowing it, because a pane
/// that never fills in with nothing in the status bar is worse than a slow one.
fn off_thread<T, M>(
	job: impl FnOnce() -> T + Send + 'static,
	delivered: impl Fn(Option<T>) -> M + Send + 'static,
) -> Task<M>
where
	T: Send + 'static,
	M: Send + 'static,
{
	let (sender, receiver) = oneshot::channel();

	std::thread::spawn(move || {
		// The receiver is gone if the app quit mid-job, and there is nothing to do about
		// that but let this thread end.
		let _ = sender.send(job());
	});

	Task::perform(receiver, move |result| delivered(result.ok()))
}

/// List one folder's files, off both the GUI thread and the executor's (PLAN §4, §9).
fn list_files(folder: PathBuf) -> Task<Message> {
	let read = folder.clone();

	off_thread(
		move || fsio::list_files(&read),
		move |result| {
			let result = result
				.unwrap_or_else(|| Err("the folder listing stopped unexpectedly".to_string()));
			Message::FilesListed(folder.clone(), result)
		},
	)
}

/// The same, for the tree's subfolders.
fn list_folders(folder: PathBuf) -> Task<Message> {
	let read = folder.clone();

	off_thread(
		move || fsio::list_folders(&read),
		move |result| {
			let result = result
				.unwrap_or_else(|| Err("the folder listing stopped unexpectedly".to_string()));
			Message::FoldersListed(folder.clone(), result)
		},
	)
}

/// Scan a track's waveform on a thread of its own (PLAN §4, §14a).
///
/// This is the job that found the rule `off_thread` now keeps for everything. Measured both
/// ways from the moment a scan starts: **641 ms with no tick at all**, then a dozen
/// delivered in the same millisecond, against a steady **49–51 ms** once the decode moved
/// off the executor. Pressing Play during a scan gave audio and a frozen clock, which is
/// how this was found.
///
/// Two threads at most, one per player, each living exactly as long as the scan it was
/// spawned for.
///
/// The error is flattened to a `String` here because a `Message` has to be `Clone` and an
/// `anyhow::Error` is not — the same reason `fsio` returns one (PLAN §9).
///
/// The cache is asked *inside* the job rather than before it (PLAN §11a). It reads a file and
/// its answer arrives as a `fsync`-shaped write, so it belongs on the same side of the thread
/// boundary as the decode it is replacing — and the app above is unchanged either way, because
/// a hit and a scan produce the same message.
fn scan_track(id: DeckId, path: PathBuf, cache: Arc<Cache>) -> Task<Message> {
	let scanned = path.clone();

	off_thread(
		move || cached_scan(&cache, &scanned).map_err(|error| format!("{error:#}")),
		move |result| {
			// Reported rather than swallowed: a strip that stays flat for ever with no line
			// in the status bar is the one outcome worse than a slow scan.
			let result =
				result.unwrap_or_else(|| Err("the waveform scan stopped unexpectedly".to_string()));
			Message::Scanned(id, path.clone(), result)
		},
	)
}

/// The same decode, for a file nobody is playing (PLAN §11b).
///
/// The unit of the folder scan, and the reason it is worth a function of its own is what it
/// throws away: the peak array goes into the cache and is dropped on the way out, because the
/// point of preparing a folder is what will be *there* when a track is loaded, not what is on
/// screen now. The length comes along for the ride — it is a header parse next to a full
/// decode, and the queues want it.
fn scan_file(path: PathBuf, cache: Arc<Cache>) -> Task<Message> {
	let asked = path.clone();

	off_thread(
		move || Facts {
			trim: cached_scan(&cache, &path).ok().and_then(|scan| scan.trim),
			duration: cached_duration(&cache, &path),
			path,
		},
		move |facts| {
			// A file that would not decode still counts as done, or the progress would stop on
			// the first broken track in a folder and never reach the end.
			Message::ScanFolderStepped(facts.unwrap_or_else(|| Facts {
				path: asked.clone(),
				duration: None,
				trim: None,
			}))
		},
	)
}

/// A track's waveform and the edges of its music: from the cache if they are there, from the
/// file if they are not, and into the cache on the way past (PLAN §11a, §14c).
///
/// The two halves of the stamp rule live here and nowhere else. **No stamp, no caching** — a
/// file that cannot be stat'd is scanned every time, which is the behaviour there was before
/// the cache and is right for a path that is behaving strangely. And a scan that *fails* is
/// not stored: the cache holds answers about files, not the fact that one of them would not
/// open, which is a condition that can change under it.
///
/// Both tables have to answer for it to be a hit. That is what makes the trim table arrive
/// without a format bump: a file scanned by a build that knew nothing about trims has its
/// waveform on disk and no edges, so it is decoded once more and both are stored — where
/// bumping `FORMAT` would have thrown away every waveform in the cache to add a field.
fn cached_scan(cache: &Cache, path: &Path) -> anyhow::Result<Scan> {
	let stamp = cache::stamp(path);

	if let Some(stamp) = stamp
		&& let Some(peaks) = cache.peaks(path, stamp)
		&& let Some(trim) = cache.trim(path, stamp)
	{
		return Ok(Scan { peaks, trim });
	}

	let scan = audio::scan(path)?;
	if let Some(stamp) = stamp {
		cache.store_peaks(path, stamp, &scan.peaks);
		cache.store_trim(path, stamp, scan.trim);
	}
	Ok(scan)
}

/// What is already known about a queued track, without decoding it (PLAN §7a, §14c).
///
/// The trim is **read, never worked out**: finding it means decoding the whole file, and a
/// queue edit that decoded fifty tracks would freeze four threads for half a minute for a
/// setting the user may not even have switched on. So a queue learns where a track's music
/// starts only if something has already scanned it — the folder scan, or the track's own turn
/// in a player — and plays it whole until then.
fn cached_facts(cache: &Cache, path: &Path) -> Facts {
	Facts {
		duration: cached_duration(cache, path),
		trim: cache::stamp(path)
			.and_then(|stamp| cache.trim(path, stamp))
			.flatten(),
		path: path.to_path_buf(),
	}
}

/// The same, for a track's length (PLAN §7a, §11a).
///
/// The one difference from the waveform is what happens to a failure, and it is not an
/// inconsistency: `audio::duration` has no error to report, only the *absence* of a length,
/// and "this file has no length" is an answer worth remembering — it is exactly what stops the
/// queues re-opening an unreadable file on every edit for the rest of the run.
fn cached_duration(cache: &Cache, path: &Path) -> Option<Duration> {
	let Some(stamp) = cache::stamp(path) else {
		return audio::duration(path);
	};

	if let Some(length) = cache.duration(path, stamp) {
		return length;
	}

	let length = audio::duration(path);
	cache.store_duration(path, stamp, length);
	length
}

/// The two testable things in this module: what a divider drag is allowed to store
/// (PLAN §6), and which key means refresh (PLAN §9). The *layout* is no longer arithmetic
/// at all — the height goes to the widget as a literal — so there is nothing left there for
/// a test to have an opinion about, which is the point of the rewrite rather than a gap in
/// it.
#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_drag_stores_the_height_the_pointer_asked_for() {
		// Arrange: a window with room to spare, so nothing should be clamped.
		let window = 1000.0;

		// Act / Assert: exactly, not approximately. A drag that is inside the bounds must
		// survive untouched, or the panel would creep by a pixel every time it was grabbed.
		for wanted in [MIN_PANE, 200.0, 300.0, 500.0, 780.0] {
			let stored = dragged_height(window, wanted);
			assert_eq!(stored, wanted, "a drag to {wanted} was stored as {stored}");
		}
	}

	#[test]
	fn a_drag_can_never_push_the_browser_off_the_bottom() {
		// Arrange: every window worth having, dragged well past the bottom of each.
		for window in (400..=2000).step_by(37) {
			let window = window as f32;

			// Act: the pointer is far below the window's own bottom edge.
			let stored = dragged_height(window, window * 2.0);

			// Assert: the browser keeps its minimum, so the divider stays on screen and
			// stays grabbable — the whole reason this clamp exists.
			let browser = window - CHROME - stored - PANE_SPACING;
			assert!(
				browser >= MIN_PANE,
				"window {window}: a drag left the browser {browser}"
			);
		}
	}

	#[test]
	fn a_drag_above_the_top_still_leaves_a_usable_panel() {
		// Arrange / Act / Assert: the pointer above the window's top edge is a negative
		// height, which must read as the floor rather than as a panel of nothing.
		assert_eq!(dragged_height(1000.0, -50.0), MIN_PANE);
	}

	#[test]
	fn an_impossible_window_still_produces_a_usable_height() {
		// Arrange / Act / Assert: a window shorter than its own chrome gives a negative
		// ceiling, and a `NaN` one would otherwise store a `NaN` as the panel's height.
		for window in [0.0, 1.0, CHROME, CHROME + 1.0, f32::NAN] {
			let stored = dragged_height(window, 300.0);
			assert!(
				stored.is_finite() && stored >= MIN_PANE,
				"window {window} stored a height of {stored}"
			);
		}
	}

	/// The folder scan's bookkeeping, walked through a whole run (PLAN §11b). It is three
	/// counters and no timer, and the only way to see that they add up is to run one — the
	/// `Task`s the driver returns need a window, but the arithmetic that decides how many
	/// there are does not.
	#[test]
	fn a_folder_scan_hands_out_exactly_what_it_finishes() {
		// Arrange: ten files and a fan-out of four.
		let mut scanning = Scanning {
			files: (0..10).map(|n| PathBuf::from(format!("{n}.mp3"))).collect(),
			next: 0,
			done: 0,
			running: 0,
		};

		// Act / Assert: it fills the fan-out, and never more than the fan-out.
		assert!(!scanning.is_over(), "nothing has been done yet");
		assert_eq!(scanning.slots(), SCAN_JOBS);

		// The whole run, one answer at a time: hand out what there is room for, take one back.
		let mut handed = 0;
		while !scanning.is_over() {
			let slots = scanning.slots();
			assert!(scanning.running + slots <= SCAN_JOBS, "over the fan-out");
			scanning.next += slots;
			scanning.running += slots;
			handed += slots;

			scanning.running -= 1;
			scanning.done += 1;
		}

		// Assert: every file went out exactly once, and every one came back.
		assert_eq!(handed, 10, "every file, once");
		assert_eq!(scanning.done, 10);
		assert_eq!(scanning.running, 0, "no thread left unaccounted for");
	}

	#[test]
	fn stopping_a_scan_still_waits_for_the_files_already_out() {
		// Arrange: four files out on threads, six still to come.
		let mut scanning = Scanning {
			files: (0..10).map(|n| PathBuf::from(format!("{n}.mp3"))).collect(),
			next: 4,
			done: 0,
			running: 4,
		};

		// Act: Stop, which cuts the list down to what has gone out rather than dropping the
		// scan — the four answers still coming belong to it and have to be counted by it.
		scanning.files.truncate(scanning.next);

		// Assert: nothing more goes out, and it is not over until the four report.
		assert_eq!(scanning.slots(), 0, "nothing more goes out");
		assert!(!scanning.is_over(), "four are still decoding");

		for _ in 0..4 {
			scanning.running -= 1;
			scanning.done += 1;
		}
		assert!(scanning.is_over(), "the last one landed");
		assert_eq!(scanning.done, scanning.files.len(), "counted once each");
	}

	#[test]
	fn the_shortcuts_are_the_three_keys_and_nothing_near_them() {
		// Arrange: the keys that must work, and the near-misses that must not — an unmodified
		// `r` above all, since a bare letter that re-listed the folder would fire on any stray
		// key press. Written in `Modifiers::COMMAND` rather than in `LOGO` or `CTRL`, so the
		// same table asserts Cmd on macOS and Ctrl on Windows.
		let command = Modifiers::COMMAND;
		let refresh = Some(Message::RefreshPressed);
		let cases = [
			(Key::Named(Named::F5), Modifiers::empty(), refresh.clone()),
			(Key::Named(Named::F5), command, refresh.clone()),
			(Key::Character("r".into()), command, refresh.clone()),
			(
				Key::Character("a".into()),
				command,
				Some(Message::SelectAll),
			),
			(
				Key::Named(Named::Escape),
				Modifiers::empty(),
				Some(Message::SelectionCleared),
			),
			(Key::Character("r".into()), Modifiers::empty(), None),
			(Key::Character("a".into()), Modifiers::empty(), None),
			(Key::Character("t".into()), command, None),
			(Key::Named(Named::F4), Modifiers::empty(), None),
		];

		// Act / Assert: by name rather than by value, because a `Message` is not `PartialEq` —
		// and the name is the whole of what this decides.
		for (key, modifiers, expected) in cases {
			let got = shortcut(&key, modifiers);
			assert_eq!(
				format!("{got:?}"),
				format!("{expected:?}"),
				"{key:?} with {modifiers:?}"
			);
		}
	}
}
