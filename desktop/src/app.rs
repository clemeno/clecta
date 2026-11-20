//! The iced application (PLAN §5): the state, the messages, and the two functions that
//! move between them.
//!
//! Everything interesting lives in the modules this one calls. What is left here is the
//! wiring: which message changes which field, which change needs a `Task`, and how the
//! three sections are arranged. That is deliberate — an `update` that is only ever a
//! dispatch table stays readable as the app grows.

use std::collections::{BTreeMap, HashMap};
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
	Element, Fill, Point, Size, Subscription, Task, Theme, event, keyboard, mouse, stream, time,
	window,
};
use notify::{RecursiveMode, Watcher};

use crate::audio::Engine;
use crate::browser::{self, Browser, Entry};
use crate::cache::{self, Cache, Ready};
use crate::deck::{self, Deck, DeckId, DropOutcome};
use crate::drag::{Carried, Drag, DropTarget, Zone};
use crate::focus::Focus;
use crate::known::{Facts, Known};
use crate::measure;
use crate::mixer::{self, Curve};
use crate::prepare::Preparing;
use crate::queue::{self, Handover};
use crate::queues::{Admission, Layout, QueueId, Queues};
use crate::select::{Click, Collapse};
use crate::settings::Settings;
use crate::tree::Tree;
use crate::waveform::Scan;
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

/// How long a released click's collapse waits before it fires (PLAN §9a, Q50).
///
/// Just past iced's double-click window — 300 ms and six pixels, `mouse::Click` in
/// `iced_core` — because the release that owes the collapse may be the first half of a
/// double click whose load acts on the whole selection, and that message only arrives
/// with the second press. `ponytail:` the coupling is by value, not by name; if iced ever
/// exposes its click interval, this should read it instead.
const COLLAPSE_AFTER: Duration = Duration::from_millis(350);

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

/// The cache's name inside `clecta-data/` (PLAN §11a). Beside `settings.json` rather than in a
/// per-user cache folder, so a portable install carries what it has already worked out with
/// it — and the extension says what it is to anyone looking at the folder.
const CACHE_FILE: &str = "cache.redb";

pub fn run() -> iced::Result {
	// First, so that the settings read below has somewhere to complain to.
	#[cfg(windows)]
	crate::paths::log_stderr();

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

#[derive(Debug, Clone)]
pub enum Message {
	/// The playhead poll.
	Tick,
	/// A transport button on one player.
	Transport(DeckId, deck::Event),
	/// The per-player rate slider (PLAN §7c): 1.0 is the track's own speed, 0 is a pause.
	RateChanged(DeckId, f32),
	/// The **MT/KL** switch beside it (PLAN §7c): on, the slider changes the tempo alone
	/// and the key stays put.
	KeyLockToggled(DeckId, bool),
	/// The per-player **Load…** button — the third door that always works (PLAN §10).
	LoadPressed(DeckId),
	/// The file that dialog came back with. Cancel sends nothing.
	FilePicked(DeckId, PathBuf),
	/// The per-player **Unload** button: take the track out and leave the player empty.
	UnloadPressed(DeckId),
	/// **OK** on the question a playing track's Unload asks, naming the track it asked about.
	UnloadConfirmed(DeckId, PathBuf),
	/// Load the selected row into a named player.
	LoadSelected(DeckId),
	/// Load a file into the player an unaimed gesture lands on.
	LoadUnaimed(PathBuf),
	/// A row was clicked. What that does to the selection depends on what is held (PLAN §9a).
	RowSelected(PathBuf),
	/// The modifiers changed. Tracked because a `mouse_area` press does not carry them, and a
	/// click has to know whether it is a plain one, a toggle or a range.
	ModifiersChanged(Modifiers),
	/// A row was right-clicked: the files pane names it by path, a queue by queue and row, and
	/// both open the same menu (PLAN §14d).
	RowMenuOpened(PathBuf),
	QueueMenuOpened(QueueId, usize),
	/// Where a right press happened, from `ui::pointer` — published *before* whichever of the
	/// two above it belongs to, so the menu knows where to open (PLAN §14d).
	PointerPressed(Point),
	/// The menu or the editor was dismissed — its own button, a click outside it, or Escape.
	MenuDismissed,
	/// **Correct tempo…** on the open menu.
	TempoCorrectionOpened,
	/// `/2` or `×2`, as the factor itself: the two buttons differ by nothing else.
	TempoScaled(f32),
	/// **Apply** — the corrected tempo becomes this file's, until it is corrected again.
	TempoApplied,
	/// **Clear tempo corrections**, which asks first.
	ClearCorrectionsPressed,
	/// …and **OK** on its question.
	ClearCorrectionsConfirmed,
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
	QueueSelected(QueueId, usize),
	/// A queue row was double-clicked: play it now, out of turn.
	QueueLoad(QueueId, usize),
	/// A queue was scrolled, to this absolute offset. Not a preference — the view needs it to
	/// know which rows are worth building, exactly as the files pane does (PLAN §9).
	QueueScrolled(QueueId, f32),
	/// A drag entered (`true`) or left (`false`) one of a queue's scroll edges: its header,
	/// which scrolls up, or its footer, which scrolls down (PLAN §7a).
	ScrollEdge(QueueId, bool, bool),
	/// One step of that scroll. Sent only while an edge is armed.
	ScrollStep,
	/// What the queues' tracks turned out to be, looked up off the GUI thread. A batch rather
	/// than one message per file, because they are asked for as a batch and none of them is
	/// interesting alone.
	Measured(Vec<Facts>),
	/// Add the browser's selection to a queue — at the top when `true`, at the end when
	/// `false`.
	QueueAdd(QueueId, bool),
	/// Take the selected row out of a queue.
	QueueRemove(QueueId),
	/// Move the selected row one place up (`true`) or down (`false`).
	QueueMove(QueueId, bool),
	/// Send the selected row to the neighbouring queue, right (`true`) or left (`false`).
	QueueShift(QueueId, bool),
	/// The duplicate warning's **Yes** or **No**, for the queue edit it was holding (Q83).
	/// Cancel sends nothing.
	Admitted(Held, Admission),
	/// A queue's **Auto-load** switch: whether it hands its top track to a player that has
	/// just run out (PLAN §7a).
	QueueAutoLoad(QueueId, bool),
	/// Its **Auto-play** switch: whether a track handed over that way then starts by itself.
	QueueAutoPlay(QueueId, bool),
	/// Its **handover**: whether the track it hands over waits for the file to run out, or
	/// takes over when the music stops (PLAN §7b).
	QueueHandover(QueueId, Handover),
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
	/// Which files of a listing the store already answers for in full, and how long the music in
	/// each of them runs (PLAN §11c, §14c). The folder comes back with the answer for the same
	/// reason the listing does: to be thrown away if the user has moved on since.
	Prepared(PathBuf, HashMap<PathBuf, Ready>),
	FoldersListed(PathBuf, Result<Vec<PathBuf>, String>),
	/// A track's scan finished. Carries the path it was started for, because a player can
	/// be given another track while the scan runs (PLAN §14a), and the length its load read,
	/// because that is the same answer a queue's measuring would have looked up (Q59).
	Scanned(DeckId, PathBuf, Option<Duration>, Result<Scan, String>),
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
	/// …and **OK** on its question.
	ClearCacheConfirmed,
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
	/// The double-click window closed on a released click that still owes a collapse (Q50).
	CollapseDue,
	/// Any button went down, wherever it is. Only job: abandon a pending collapse (Q53).
	PressedAnywhere,
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
	/// A drag in flight, in-app or from outside: what it carries, where a release would land,
	/// and the queue edge it is scrolling (PLAN §10, §7a).
	drag: Drag,
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
	/// The three queues, indexed by `QueueId::index` (PLAN §7a): one in front of each player,
	/// and the shared one between them.
	queues: Queues,
	/// What has already been worked out about a file (PLAN §11a). Shared with every job that
	/// reads or writes it: an `Arc` because those jobs are threads, and the whole point is
	/// that the cache is touched *there* rather than on the GUI thread, where a commit is an
	/// `fsync`.
	cache: Arc<Cache>,
	/// What this run has learned about files, and the one door those answers arrive through
	/// (PLAN §14c, Q59). The fan-out — the players' trims, the pane's marks, the queues' rows
	/// — is the type's business, not the arms'.
	known: Known,
	/// The folder scan, while one is running (PLAN §11b). `None` at rest, which is also what
	/// **Stop** leaves behind once the files already out have come back.
	preparing: Option<Preparing>,
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
	/// Tempos corrected by hand, by file (PLAN §14d). Persisted in `settings.json` rather than
	/// the cache, because a person's answer about a track is not something a decode could work
	/// out again — and read only by the two views, which apply it to a row as it is drawn.
	tempos: BTreeMap<PathBuf, f32>,
	/// Which list ⌘A and Escape are talking to (PLAN §9a): the one a row was last pressed in,
	/// and the files pane until one has been. A press the user was making anyway, rather than a
	/// Tab order and a focus ring to learn.
	focus: Focus,
	/// Where the last right press landed, from the widget wrapped round the window
	/// (`ui::pointer`) — the one thing iced's press messages do not carry, and the whole of
	/// what a menu needs to open under the pointer (PLAN §14d). `None` until the first right
	/// press of the run, which is a menu centred as it used to be.
	pointer: Option<Point>,
	/// The row menu or the tempo editor, while one is open. `None` is the ordinary state and is
	/// also what every way of dismissing either leaves behind.
	overlay: Option<Overlay>,
	/// The collapse a plain press on a selected row still owes (Q50, Q58): armed by a
	/// deferring press (`Click::defers`), promoted to pending by its release, spent by the
	/// timer — and cancelled by anything that acts on the selection in the meantime, so a
	/// stale click can never fire under a newer gesture. The ordering rules live on the type,
	/// where they are tested; the arms below only say which gesture happened.
	collapse: Collapse<Pressed>,
}

/// A plain press on an already-selected row, remembered so the release can finish it.
///
/// The press itself must leave the selection alone — it is arming a drag that may want to
/// carry all of it (PLAN §9a) — so collapsing to one row is deferred twice: to the release,
/// and past it to `COLLAPSE_AFTER`, because the release may open a double click (Q50).
#[derive(Debug, Clone)]
enum Pressed {
	/// A files-pane row, by path, because a refresh renumbers rows underneath it (PLAN §9).
	Row(PathBuf),
	/// A queue row, by queue and index like every other queue message (PLAN §7a) — plus the
	/// path that was under the press, so the delayed click can prove the index still names
	/// the same row when it finally fires.
	Queue(QueueId, usize, PathBuf),
}

/// One of the three queue edits that can raise the duplicate warning (PLAN §7a, §9a).
#[derive(Debug, Clone)]
enum Edit {
	/// **⤒ ⤓**: the files pane's media at the top (`true`) or the end of a queue.
	Add(QueueId, Vec<queue::Item>, bool),
	/// **← →**: a queue's selected rows to its neighbour, right (`true`) or left.
	Shift(QueueId, bool),
	/// A drag released above row `usize` of a queue.
	Drop(QueueId, usize, Carried),
}

/// A queue edit the duplicate warning is holding until it answers (Q83).
#[derive(Debug, Clone)]
pub struct Held {
	edit: Edit,
	/// The queues as they stood when the warning opened. The window keeps running behind it, so
	/// a track that ends hands a row over and renumbers its queue — and an edit that names rows
	/// or a caret by number would then move other tracks, the drag's problem (Q80).
	layout: Layout,
}

/// Which row was right-clicked, and everything the menu over it needs to know.
///
/// The name and the tempo are **copied** rather than looked up again when the menu is drawn: the
/// menu is opened from two panes that name a row differently — the files pane by path, a queue by
/// queue and index — and a queue can be edited underneath an open menu.
#[derive(Debug, Clone)]
struct RowMenu {
	path: PathBuf,
	name: String,
	/// What the row is showing, corrections included, and `None` when nothing has scanned it.
	tempo: Option<f32>,
	/// What the detector said, as the pane the menu was opened in knows it — the files pane's
	/// mark or the queue row's own. Carried into the editor, because a queue row of a file from
	/// another folder is one the files pane has never heard of.
	detected: Option<f32>,
	/// Where the press that opened it was, so the panel appears under the pointer rather than
	/// in the middle of the window (PLAN §14d). Copied at open like the two fields above: a
	/// menu that moved because a later press landed somewhere else would be a menu that walks.
	at: Option<Point>,
}

/// A tempo being corrected: what it started as, and what the buttons have made of it so far.
///
/// The value lives here rather than in the map until **Apply**, which is the whole of what
/// Cancel means — halving a tempo four times and changing your mind leaves nothing behind.
#[derive(Debug, Clone)]
struct TempoCorrection {
	path: PathBuf,
	name: String,
	value: f32,
	/// What the detector said, for the line under the number. Kept so the editor can say what it
	/// is departing from without asking the store again.
	detected: Option<f32>,
	/// Where the menu it grew out of was, so the editor opens in its place rather than jumping
	/// to the middle of the window as the entry is clicked.
	at: Option<Point>,
}

/// What is drawn over the window, if anything (PLAN §14d): the row menu, or the tempo editor it
/// opens. One value because they are one panel in two states — never both at once, and one
/// Escape closes whichever is there.
#[derive(Debug, Clone)]
enum Overlay {
	Menu(RowMenu),
	Editor(TempoCorrection),
}

impl RowMenu {
	/// The menu over one row. `detected` is what that row's own pane knows about the file, and
	/// the tempo shown is that with any correction applied — by the one function every row is
	/// drawn with (Q41), so the menu cannot disagree with the row it was opened on.
	fn new(
		path: PathBuf,
		name: String,
		detected: Option<f32>,
		tempos: &BTreeMap<PathBuf, f32>,
		at: Option<Point>,
	) -> Self {
		Self {
			tempo: ui::corrected_tempo(tempos, &path, Some(detected)).flatten(),
			detected,
			path,
			name,
			at,
		}
	}

	/// **Correct tempo…**: the editor in the menu's place, starting from the number the row
	/// shows and saying what the detector said. `None` for a row with no tempo at all, which has
	/// nothing to correct.
	fn correct(self) -> Option<TempoCorrection> {
		Some(TempoCorrection {
			value: self.tempo?,
			detected: self.detected,
			path: self.path,
			name: self.name,
			at: self.at,
		})
	}
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
		for (index, deck) in decks.iter_mut().enumerate() {
			deck.fader = settings.faders[index];
			// The rate slider and its lock, restored together because the lock is what the
			// rate means (PLAN §7c). Only the model here — the audio is told by `apply_rates`
			// below, which is also what a reconnect calls.
			deck.rate = settings.rates[index];
			deck.key_lock = settings.key_locks[index];
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
			drag: Drag::default(),
			notice,
			dirty: false,
			stale: false,
			// The zip between the file's two orderings is the file's own business (Q55).
			queues: Queues::restored(settings.queues()),
			// Opened here rather than lazily, for the same reason `Settings::load` is read
			// before the window exists: it is one small file, once, and everything after it
			// wants to know whether there is a cache to ask (PLAN §11a).
			cache: Arc::new(Cache::open(&paths::data_dir().join(CACHE_FILE))),
			known: Known::default(),
			preparing: None,
			modifiers: Modifiers::empty(),
			sweep: 0,
			tempos: settings.tempos,
			focus: Focus::default(),
			pointer: None,
			overlay: None,
			collapse: Collapse::default(),
		};
		app.apply_gains();
		app.apply_rates();

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
		let mut settings = Settings {
			curve: self.curve,
			faders: [self.decks[0].fader, self.decks[1].fader],
			crossfader: self.crossfader,
			rates: [self.decks[0].rate, self.decks[1].rate],
			key_locks: [self.decks[0].key_lock, self.decks[1].key_lock],
			folder: self.browser.folder.clone(),
			window: self.window,
			decks_height: self.decks_height,
			tempos: self.tempos.clone(),
			// The queue fields, filled in below: the zip into the file's two orderings sits
			// beside the fields that define them (Q55).
			..Settings::default()
		};
		settings.record_queues(&self.queues);
		settings
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
		//
		// It drives two things now — the band crossing a player's strip and the spinner on
		// every files-pane row a thread is decoding (PLAN §11c) — off one counter, so the whole
		// window turns at the same rate rather than at two rates that beat against each other.
		let sweep = if self.decks.iter().any(|deck| deck.scanning) || self.preparing.is_some() {
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
		let scrolling = if self.drag.scrolling().is_some() {
			time::every(AUTOSCROLL).map(|_| Message::ScrollStep)
		} else {
			Subscription::none()
		};

		// The collapse a released click still owes (PLAN §9a, Q50), once the double-click
		// window has passed. Ticks only while one is actually pending — and that is at most
		// one tick, because the arm below takes the pending state that gates this.
		let collapse = if self.collapse.waiting() {
			time::every(COLLAPSE_AFTER).map(|_| Message::CollapseDue)
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
			collapse,
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

			Message::RateChanged(id, rate) => {
				self.decks[id.index()].rated(id, rate, &self.engine);
				// Remembered across a quit, like the fader it sits under (PLAN §7c).
				self.dirty = true;
			}

			Message::KeyLockToggled(id, on) => {
				self.decks[id.index()].locked(id, on, &self.engine);
				self.dirty = true;
			}

			Message::LoadPressed(id) => {
				let dialog = rfd::AsyncFileDialog::new()
					.set_directory(self.browser.folder.clone().unwrap_or_else(fsio::home));
				return ask(
					move |window| dialog.clone().set_parent(window).pick_file(),
					move |file| file.map(|file| Message::FilePicked(id, file.path().to_path_buf())),
				);
			}

			// Into whatever the player holds by now, which a handover may have changed while the
			// dialog was open — the same as pressing Load… at this moment would do.
			Message::FilePicked(id, file) => return self.load(id, file),

			Message::UnloadPressed(id) => {
				// Asked only while the sound is on: unloading a stopped track costs one Load
				// to undo, but cutting audible playback is the mistake a modal exists for —
				// the same dialog the two destructive buttons in the corrections pane use.
				let deck = &self.decks[id.index()];
				if deck.is_playing()
					&& let Some(track) = &deck.track
				{
					let path = track.path.clone();
					let dialog = warning(
						"Unload the playing track",
						format!(
							"{} is playing {}.\n\nUnload it and cut the sound?",
							id.label(),
							deck.title(),
						),
					);
					return ask(
						move |window| dialog.clone().set_parent(window).show(),
						move |answer| {
							confirmed(answer).then(|| Message::UnloadConfirmed(id, path.clone()))
						},
					);
				}
				self.unload(id);
			}

			Message::UnloadConfirmed(id, path) => {
				// The playhead ran on behind the question, so the track asked about may have
				// ended and handed over: unloading the one that replaced it would cut a track
				// nobody was asked about.
				if self.decks[id.index()]
					.track
					.as_ref()
					.is_some_and(|track| track.path == path)
				{
					self.unload(id);
				} else {
					self.notice = format!(
						"{}: the track changed while the question was open — nothing was unloaded",
						id.label()
					);
				}
			}

			Message::LoadSelected(id) => {
				return self.load_batch(id, self.browser.selected_media(), |_| false);
			}

			// The double click acts on the **selection** when the row it landed on is part of
			// one, and on that row alone when it is not (PLAN §9a). Both cases are reachable:
			// the press before it selects the row, unless it was a command-click that
			// deselected it — and a gesture that names a file should still do something with
			// that file.
			Message::LoadUnaimed(path) => {
				// The double click claims the press: this load acts on the whole selection,
				// so the collapse the first click deferred must not fire after it (Q50).
				self.collapse.cancel();

				let id = deck::idle_target(&self.decks[0], &self.decks[1]);
				let batch = if self.browser.is_selected(&path) {
					self.browser.selected_media()
				} else {
					vec![path]
				};
				return self.load_batch(id, batch, |_| false);
			}

			Message::RowSelected(path) => {
				// The press both moves the selection and arms a drag with it. A release that
				// is not over a player disarms it and nothing else happens — which is
				// exactly what a plain click is (PLAN §10).
				//
				// **A plain press on a row that is already selected leaves the selection
				// alone**, which is what makes a multi-row drag possible at all: collapsing to
				// one row here would destroy the very selection the drag is about to carry
				// (PLAN §9a). The release finishes what the press deferred, once nothing
				// else claims the click (Q50).
				// Arming and disarming both abandon what an older click still owed: a press
				// is a new gesture (Q50), and the type clears it synchronously (Q53, Q58).
				// The press is also what the keyboard follows (PLAN §9a): ⌘A and Escape act on
				// the list a row was last pressed in, so no new gesture had to be invented for
				// them and none has to be remembered.
				self.focus = Focus::Files;

				let kind = self.click_kind();
				if kind.defers(self.browser.is_selected(&path)) {
					self.collapse.arm(Pressed::Row(path.clone()));
				} else {
					self.browser.click(&path, kind);
					self.collapse.disarm();
				}

				// From the pane, so a drop *copies*: the files stay in the folder.
				let items: Vec<queue::Item> = self
					.browser
					.selected_media()
					.into_iter()
					.map(queue::Item::new)
					.collect();
				self.drag.arm(items, None);
			}

			Message::ModifiersChanged(modifiers) => self.modifiers = modifiers,

			Message::SelectAll => {
				// "All" must not narrow to one a third of a second later: the click is
				// claimed, at both stages (Q50).
				self.collapse.cancel();
				self.focus.select_all(&mut self.browser, &mut self.queues);
			}

			// Escape closes whatever is over the window first, and only clears the selection when
			// there is nothing over it: "never mind" means the most recent thing, which is the
			// panel the user is looking at.
			Message::SelectionCleared => {
				// Escape is "never mind", and that includes the collapse a click still owes —
				// at either stage, or a release after the Escape would revive it (Q50).
				self.collapse.cancel();
				if self.overlay.take().is_none() {
					self.focus.clear(&mut self.browser, &mut self.queues);
				}
			}

			// The pointer's own message arrives first, from the widget wrapped round the whole
			// window (`ui::pointer`), so `self.pointer` is this press and not the last one.
			Message::PointerPressed(at) => self.pointer = Some(at),

			Message::RowMenuOpened(path) => {
				let detected = self.browser.ready(&path).and_then(|ready| ready.tempo);
				let name = fsio::name_of(&path);
				let menu = RowMenu::new(path, name, detected, &self.tempos, self.pointer);
				self.overlay = Some(Overlay::Menu(menu));
			}

			// A queue names its rows by position, so the path is read here rather than carried in
			// the message: the row could have moved between the press and this arm.
			Message::QueueMenuOpened(id, index) => {
				if let Some(item) = self.queues.get(id).items().get(index) {
					let menu = RowMenu::new(
						item.path.clone(),
						item.name.clone(),
						item.ready.and_then(|ready| ready.tempo),
						&self.tempos,
						self.pointer,
					);
					self.overlay = Some(Overlay::Menu(menu));
				}
			}

			Message::MenuDismissed => self.overlay = None,

			// The menu closes as the editor opens: they are one panel in two states, and two
			// panels over each other would be two ways to dismiss one thing.
			Message::TempoCorrectionOpened => {
				self.overlay = match self.overlay.take() {
					Some(Overlay::Menu(menu)) => menu.correct().map(Overlay::Editor),
					other => other,
				};
			}

			// Held in the editor and nowhere else until **Apply**, which is what makes Cancel
			// free: nothing has been written, so there is nothing to put back.
			Message::TempoScaled(factor) => {
				if let Some(Overlay::Editor(correction)) = self.overlay.as_mut() {
					let scaled = correction.value * factor;
					// The guard is for the map and the column downstream, not for this button: a
					// tempo halved out of every sensible range is the user's business, but one
					// that stopped being a number would be drawn.
					if scaled.is_finite() && scaled > 0.0 {
						correction.value = scaled;
					}
				}
			}

			Message::TempoApplied => {
				if let Some(Overlay::Editor(correction)) = &self.overlay {
					self.tempos
						.insert(correction.path.clone(), correction.value);
					self.overlay = None;
					// A correction is a setting, so it is saved the way every other setting is —
					// on the throttle, not immediately (PLAN §11).
					self.dirty = true;
				}
			}

			Message::ClearCorrectionsPressed => {
				// The same warning **Clear cache** shows, and deliberately a different sentence:
				// that one costs time, this one costs answers nothing can work out again
				// (PLAN §14d).
				let dialog = warning(
					"Clear the tempo corrections",
					"Throw away every tempo you have corrected by hand?\n\n\
					 Every row goes back to what the detector said. Nothing else is touched.",
				);
				return ask(
					move |window| dialog.clone().set_parent(window).show(),
					|answer| confirmed(answer).then_some(Message::ClearCorrectionsConfirmed),
				);
			}

			Message::ClearCorrectionsConfirmed => {
				// No re-reading and no re-scanning: the corrections were applied as the rows
				// were drawn, so dropping them *is* putting the detected numbers back.
				let cleared = self.tempos.len();
				self.tempos.clear();
				self.dirty = true;
				self.notice = format!("cleared {cleared} tempo corrections");
			}

			Message::FolderSelected(folder) => return self.select_folder(folder),

			Message::OpenFolderPressed => {
				let dialog = rfd::AsyncFileDialog::new()
					.set_directory(self.browser.folder.clone().unwrap_or_else(fsio::home));
				return ask(
					move |window| dialog.clone().set_parent(window).pick_folder(),
					|folder| {
						folder.map(|folder| Message::FolderSelected(folder.path().to_path_buf()))
					},
				);
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
				// alone so that a multi-row drag has something to carry. The release finishes
				// what the press deferred, once nothing else claims the click (Q50).
				self.focus = Focus::Queue(id);

				let kind = self.click_kind();
				let queue = self.queues.get_mut(id);
				if kind.defers(queue.is_selected(index)) {
					// The pressed row's path rides along so the delayed click can prove the
					// index still names the same track when it finally fires (Q50).
					match queue.items().get(index) {
						Some(item) => {
							self.collapse
								.arm(Pressed::Queue(id, index, item.path.clone()));
						}
						None => self.collapse.disarm(),
					}
				} else {
					queue.click(index, kind);
					self.collapse.disarm();
				}

				let queue = self.queues.get(id);
				// From a queue, so a drop *moves*: the rows leave here when they land.
				self.drag.arm(
					queue.selected_items(),
					Some((id, queue.selection().collect())),
				);
			}

			// A double click plays queued rows now, out of turn — the one way from a queue to
			// a player that is neither a drag nor waiting for a track to end (PLAN §7a). It
			// takes them with it, exactly as a drag onto a player does and as the handover
			// itself does: the queue is what is still to come.
			Message::QueueLoad(queue, index) => {
				// The double click claims the press, exactly as in the files pane (Q50) —
				// and here the rows leave the queue, so a collapse held back by the timer
				// would land on an index that no longer names them.
				self.collapse.cancel();

				// A cue plays on the player it belongs to; the shared queue has no player of its
				// own, so it uses the same "whichever is free" rule an unaimed drop does.
				let id = match queue {
					QueueId::Cue(id) => id,
					QueueId::Shared => deck::idle_target(&self.decks[0], &self.decks[1]),
				};

				// The selection when the double-clicked row is part of it, that row alone when
				// it is not — the same rule the files pane's double click follows (PLAN §9a).
				let selected = self.queues.get(queue).is_selected(index);
				let paths: Vec<PathBuf> = if selected {
					self.queues.get(queue).selected_items()
				} else {
					self.queues
						.get(queue)
						.items()
						.get(index)
						.cloned()
						.into_iter()
						.collect()
				}
				.into_iter()
				.map(|item| item.path)
				.collect();
				if paths.is_empty() {
					return Task::none();
				}

				// The press that opened this double click armed a drag, and the double click has
				// claimed it. Disarmed here rather than left to the release, which would
				// otherwise drop the rows a second time — gone from the queue if the player took
				// them, or reordered under the pointer if it refused.
				self.drag.disarm();

				return self.load_batch(id, paths, |queues| {
					let rows = queues.get_mut(queue);
					if selected {
						rows.take_selected();
					} else {
						rows.remove(index);
					}
					true
				});
			}

			// Not persisted and deliberately not `dirty`, exactly like the files pane's own
			// offset: where a queue is scrolled to is not a setting.
			Message::QueueScrolled(id, offset) => self.queues.scrolled(id, offset),

			Message::ScrollEdge(id, up, entering) => self.drag.edge(id, up, entering),

			Message::ScrollStep => {
				if let Some((id, up)) = self.drag.scrolling() {
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
						ui::queue::scroll_id(id),
						AbsoluteOffset { x: 0.0, y: step },
					);
				}
			}

			Message::Measured(measured) => {
				for facts in measured {
					self.known
						.learned(&facts, &mut self.browser, &mut self.queues);
				}
			}

			// Every arm below edits a queue, and every queue is persisted, so each ends in
			// `queued` rather than repeating the `dirty` flag four times.
			Message::QueueAdd(id, prepend) => {
				// Read from the *browser*, not from a payload on the message: the buttons were
				// drawn from this same selection, so carrying the paths would be carrying a copy
				// of something that cannot have changed in between.
				let items: Vec<queue::Item> = self
					.browser
					.selected_media()
					.into_iter()
					.map(queue::Item::new)
					.collect();
				return self.edit(Edit::Add(id, items, prepend), None);
			}

			Message::QueueRemove(id) => {
				if self.queues.get(id).has_selection() {
					self.queues.get_mut(id).take_selected();
					return self.queued();
				}
			}

			Message::QueueMove(id, up) => {
				if self.queues.get_mut(id).shift_selected(up) {
					return self.queued();
				}
			}

			Message::QueueShift(id, right) => return self.edit(Edit::Shift(id, right), None),

			Message::Admitted(held, admission) => {
				// An add puts its tracks at the top or the end, which nothing can renumber; the
				// other two name rows and a caret, and run only on the queues they were asked
				// about (Q80, Q83).
				if !matches!(held.edit, Edit::Add(..)) && held.layout != self.queues.layout() {
					self.notice =
						"the queues changed while the warning was open — nothing was moved"
							.to_string();
					return Task::none();
				}
				return self.edit(held.edit, Some(admission));
			}

			// The two switches, which are settings rather than edits: `dirty` alone, without
			// the measuring `queued` also does — nothing was added, so there is nothing new to
			// look up.
			Message::QueueAutoLoad(id, on) => {
				self.queues.get_mut(id).auto_load = on;
				self.dirty = true;
			}

			Message::QueueAutoPlay(id, on) => {
				self.queues.get_mut(id).auto_play = on;
				self.dirty = true;
			}

			// The third of them, and the same kind of thing: it changes what happens at the end
			// of a track, not what is in the queue.
			Message::QueueHandover(id, handover) => {
				self.queues.get_mut(id).handover = handover;
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
						let asking = self.ask_prepared();
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
						return asking;
					}
					Err(error) => {
						self.notice = error.clone();
						self.browser.fail(error);
					}
				}
			}

			// The store's answer is the authority: it *replaces* what the pane believed rather
			// than adding to it, which is what makes an optimistic mark self-correcting — a scan
			// that could not be stored is marked here and unmarked by the next listing.
			Message::Prepared(folder, prepared) => {
				if self.browser.folder.as_deref() == Some(folder.as_path()) {
					self.browser.marked_prepared(prepared);
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

			Message::FilesHovered(hovering) => self.drag.files_hovered(hovering),

			Message::FileDropped(path) => {
				// Only the *first* file of the drop counts (PLAN §10); `files_dropped` says
				// which one that is.
				let first = self.drag.files_dropped();
				let target = deck::idle_target(&self.decks[0], &self.decks[1]);
				// The policy itself is deck.rs's (`drop_outcome`, PLAN §10); this arm only
				// carries its answer, which is why it no longer goes through a forwarder.
				return match deck::drop_outcome(path, first) {
					DropOutcome::Load(path) => self.load(target, path),
					DropOutcome::Decline(reason) => {
						self.notice = reason;
						Task::none()
					}
				};
			}

			Message::DragOver(target) => self.drag.over(target),

			Message::DragOut(zone) => self.drag.out(zone),

			Message::DragReleased => {
				// One release ends both drags, because a release is a release: the divider
				// needs no listener of its own, and `gestures` already takes every one.
				self.decks_drag = false;

				match self.drag.release() {
					// The queue it came out of changed under it — a track ending hands its top
					// row over — so the rows it remembers are other tracks by now (Q80). Nothing
					// moves, and the notice says why the drop did nothing.
					Some((_, carried)) if !carried.still_in(&self.queues) => {
						self.collapse.cancel();
						self.notice =
							"the queue changed during the drag — nothing was moved".to_string();
					}
					// Onto a player: the first plays now, jumping whatever queue it came from,
					// and the rest go to the top of that player's cue (PLAN §9a) — so rows
					// dragged out of a queue leave the queue, exactly as they would have if the
					// queue had handed them over itself (PLAN §7a).
					Some((DropTarget::Player(id), carried)) => {
						// A drag that lands takes the collapse with it: releasing a carried
						// selection must not also narrow it (Q50).
						self.collapse.cancel();
						let paths = carried.items.into_iter().map(|item| item.path).collect();
						return self.load_batch(id, paths, |queues| match carried.from {
							Some((queue, rows)) => {
								queues.get_mut(queue).take_rows(&rows);
								true
							}
							None => false,
						});
					}
					Some((DropTarget::Row(queue, index), carried)) => {
						self.collapse.cancel();
						return self.edit(Edit::Drop(queue, index, carried), None);
					}
					// A drag that landed on nothing is a plain click, and so is the release of
					// a press that armed none — on an already-selected row that is not media,
					// say. The press selected the row — unless it deferred (PLAN §9a), in which
					// case the collapse now waits out the double-click window rather than
					// firing here: the load a double click sends acts on the whole selection,
					// and its message only arrives with the second press (Q50).
					None => self.collapse.release(),
				}
			}

			Message::CollapseDue => {
				// A delayed plain click, and exactly that — every path that reshapes the
				// selection in the meantime has already cancelled it.
				match self.collapse.due() {
					Some(Pressed::Row(path)) => self.browser.click(&path, Click::Replace),
					// A queue can move under the timer — a handover takes the top row on
					// its own clock — so the click fires only if the index still holds the
					// very track that was under the press, and is dropped otherwise (Q50).
					Some(Pressed::Queue(id, index, path))
						if self.queues.get(id).holds(index, &path) =>
					{
						self.queues.get_mut(id).click(index, Click::Replace);
					}
					_ => {}
				}
			}

			// The net under every widget: a press anywhere abandons a pending collapse
			// (Q53). Pending only, and the reasoning is the type's now, tested where it
			// lives (Q58).
			Message::PressedAnywhere => self.collapse.pressed_anywhere(),

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

			Message::Scanned(id, path, duration, result) => {
				// Recorded before the player is asked: what a scan found is true about the
				// *file*, whatever is loaded in the player by now (PLAN §14c). Through the one
				// door the other two scans use (Q59), so a queued row of this file learns it
				// as the files pane's mark does — loading a track prepares it (PLAN §11c).
				let facts = Facts::scanned(path.clone(), duration, result.as_ref().ok());
				self.known
					.learned(&facts, &mut self.browser, &mut self.queues);
				let notice = self.decks[id.index()].scanned(id, &path, result);
				self.noted(notice);
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
					self.preparing = Some(Preparing::new(files));
					return self.scan_step();
				}
				Err(error) => self.notice = error,
			},

			Message::ScanFolderStepped(facts) => {
				// Kept whatever the scan is doing by now: the work is done and the answer is
				// true, so a cancel that arrived while this file was on a thread does not throw
				// its answer away.
				self.known
					.learned(&facts, &mut self.browser, &mut self.queues);
				if let Some(preparing) = self.preparing.as_mut() {
					preparing.landed(&facts.path);
				}
				return self.scan_step();
			}

			// What stops is the *handing out*, which is the part that would otherwise run for
			// another nine minutes; the files already out are waited for (`Preparing::stop`).
			Message::ScanFolderCancelled => {
				if let Some(preparing) = self.preparing.as_mut() {
					preparing.stop();
				}
			}

			Message::ClearCachePressed => {
				// Asked first, with the same modal the duplicate warning uses: this is the one
				// button in the app that throws work away, and it is next to the one that makes
				// it (PLAN §7a).
				let dialog = warning(
					"Clear the cache",
					"Throw away every waveform, length and music-start clecta has worked out?\n\n\
					 Nothing is lost but the time to work them out again.",
				);
				return ask(
					move |window| dialog.clone().set_parent(window).show(),
					|answer| confirmed(answer).then_some(Message::ClearCacheConfirmed),
				);
			}

			Message::ClearCacheConfirmed => {
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
				// The marks go, though — they are a report of what is on disk, and there is
				// nothing on disk any more (PLAN §11c). Leaving them would be the one place in
				// the app where the screen says work is saved that is not.
				self.browser.forget_prepared();
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

			// Re-borrowed each time round rather than held: the loop calls `&mut self`
			// methods, and the engine is only read.
			let engine = self.engine.as_ref().expect("checked above");

			if engine.finished(id) {
				// There is no end-of-track callback in rodio; `empty()` going true on the
				// tick is the signal (PLAN §7). What that then means for the player is the
				// deck's business, not this loop's.
				let notice = self.decks[id.index()].ended(id, &self.engine);
				self.noted(notice);
				ended.push(id);
			} else {
				let position = engine.position(id);
				self.decks[id.index()].position = position;

				// The *other* end of a track (PLAN §7b): the music has stopped, and the queue
				// waiting behind this player asked not to sit through the run-out.
				let trim = self.decks[id.index()]
					.track
					.as_ref()
					.and_then(|track| self.known.trim(&track.path));
				if self.queues.cuts_early(id, position, trim) {
					// **Stop**, not `Ended`, and that is the whole difference from the branch
					// above: nothing has been consumed here — the file is still in the player
					// and still playing — so it is the same rewind the button does, and the
					// handover replaces the track a moment later. Doing nothing would leave the
					// tail audible under the next track if the load failed.
					let notice = self.decks[id.index()].moved(id, deck::Event::Stop, &self.engine);
					self.noted(notice);
					ended.push(id);
				}
			}
		}

		// After the loop, not inside it: `advance` loads, and loading is a `&mut self` call
		// that would fight the engine borrow above.
		Task::batch(ended.into_iter().map(|id| self.advance(id)))
	}

	/// Give a player that has just finished the next track from a queue (PLAN §7a).
	///
	/// **Only from here**, which is the whole dispatch rule: a track ending is the one event
	/// that pulls from a queue. A player sitting empty at startup is left alone, so adding
	/// files to a queue never loads anything by itself and every automatic load can be traced
	/// back to a track the user heard end.
	///
	/// What the handover does is the queues' answer (`Queues::hand_over`); this carries it out,
	/// because the load is IO and what follows it depends on whether the file arrived.
	fn advance(&mut self, id: DeckId) -> Task<Message> {
		// Without a device every load is refused, and the skipping below would empty the
		// queues one refusal at a time. `poll_players` already stops short of here then; this
		// says so where it matters.
		if self.engine.is_none() {
			return Task::none();
		}

		// A file the player refuses is skipped for the next one at once (Q82), so one broken
		// track does not leave the player stopped with a full queue behind it. Each refusal
		// took its track out of the queue, so this ends when one loads or the queues run dry.
		let mut skipped: Vec<String> = Vec::new();
		let (next, loading) = loop {
			let Some(next) = self.queues.hand_over(id, |path| self.known.trim(path)) else {
				if skipped.is_empty() {
					return Task::none();
				}
				self.notice = format!(
					"{}: nothing queued would load — skipped {}",
					id.label(),
					skipped.join(", ")
				);
				return self.queued();
			};
			let name = fsio::name_of(&next.path);
			// Only if the file actually arrived. A load that failed leaves the *previous* track
			// in the player (PLAN §7), and pressing Play on that would be the app restarting a
			// track nobody queued — the one way an automatic start could play the wrong thing.
			match self.try_load(id, next.path.clone()) {
				Some(loading) => break (next, loading),
				None => skipped.push(name),
			}
		};
		if !skipped.is_empty() {
			self.notice = format!(
				"{} — skipped {}, which would not load",
				self.notice,
				skipped.join(", ")
			);
		}
		let loading = Task::batch([self.queued(), loading]);

		// The other half of the handover setting (PLAN §7b): silently 0:00 for a track nobody
		// has scanned — the folder scan is what makes this exact, and until then the app plays
		// the file it was given from the top.
		if let Some(start) = next.start {
			self.seek_to(id, start);
		}

		if next.auto_play {
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
	///
	/// **Loaded before anything moves** (Q81). `take` is what the door takes out of a queue —
	/// the dragged rows, the double-clicked ones, nothing from the files pane — and it runs only
	/// once the player has the first file, before the rest go in so a cue's own rows are taken
	/// by the numbers they had. A player that refuses the file leaves every queue as it was, and
	/// the notice says why: both halves or neither, as Cancel does (Q76).
	fn load_batch(
		&mut self,
		id: DeckId,
		paths: Vec<PathBuf>,
		take: impl FnOnce(&mut Queues) -> bool,
	) -> Task<Message> {
		let mut paths = paths.into_iter();
		let Some(first) = paths.next() else {
			return Task::none();
		};
		let Some(loading) = self.try_load(id, first) else {
			return Task::none();
		};

		let took = take(&mut self.queues);
		let rest: Vec<queue::Item> = paths.map(queue::Item::new).collect();
		let queued = !rest.is_empty();
		if queued {
			self.queues.get_mut(QueueId::Cue(id)).insert_many(0, rest);
		}

		if took || queued {
			Task::batch([loading, self.queued()])
		} else {
			loading
		}
	}

	/// Hand files to threads until the fan-out is full, and notice when the last one lands
	/// (PLAN §11b).
	///
	/// The whole driver, and it is called from exactly two places: the moment the walk comes
	/// back, and every time a file reports. There is no timer and no subscription — a scan is a
	/// chain of messages that refills itself, so it stops costing anything the moment it ends.
	fn scan_step(&mut self) -> Task<Message> {
		let Some(preparing) = self.preparing.as_mut() else {
			return Task::none();
		};

		if preparing.is_over() {
			let (_, total) = preparing.progress();
			self.preparing = None;
			self.notice = format!("prepared {total} files");
			return Task::none();
		}

		Task::batch(
			preparing
				.hand_out()
				.into_iter()
				.map(|path| scan_file(path, self.cache.clone())),
		)
	}

	/// Ask the store which of the listed files it already answers for in full (PLAN §11c).
	///
	/// Off the GUI thread like every other touch of the cache, and it costs no `stat`: the rows
	/// already carry the size and the modified time a stamp is made of, because the pane shows
	/// them. Media only — nothing ever scans a `.txt`, so asking about one is a lookup that can
	/// only miss.
	///
	/// A thread that dies answers "nothing is prepared", which is the right way round to be
	/// wrong: the column goes blank rather than promising work that may not be there.
	fn ask_prepared(&self) -> Task<Message> {
		let Some(folder) = self.browser.folder.clone() else {
			return Task::none();
		};

		let files: Vec<(PathBuf, cache::Stamp)> = self
			.browser
			.entries()
			.iter()
			.filter(|entry| entry.kind.is_media())
			.map(|entry| {
				(
					entry.path.clone(),
					cache::stamp_of(entry.size, entry.modified),
				)
			})
			.collect();

		let cache = self.cache.clone();
		off_thread(
			move || cache.prepared(&files),
			move |prepared| Message::Prepared(folder.clone(), prepared.unwrap_or_default()),
		)
	}

	/// Every file a thread is decoding this moment, whoever asked for it (PLAN §11c).
	///
	/// One list from two places on purpose: a folder scan and a player's own waveform scan are
	/// the same work, so a row spins for either. Short — at most `SCAN_JOBS` plus the two
	/// players — which is what makes a linear search per built row the whole of the lookup.
	fn working(&self) -> Vec<&Path> {
		let preparing = self
			.preparing
			.iter()
			.flat_map(|preparing| preparing.busy().iter());
		let loading = self
			.decks
			.iter()
			.filter(|deck| deck.scanning)
			.filter_map(|deck| deck.track.as_ref().map(|track| &track.path));

		preparing.chain(loading).map(PathBuf::as_path).collect()
	}

	/// Run a queue edit past the duplicate warning (PLAN §7a, §9a): at once when nothing in it is
	/// queued already, and otherwise once the warning has answered (Q83).
	///
	/// `answer` is `None` the first time. A batch with repeats then changes nothing — its edit is
	/// told `Nothing`, which leaves every queue as it was (Q76) — and the warning opens holding
	/// the edit; **Yes** or **No** comes back through `Admitted` and runs this again with that
	/// answer. Twice through the same `Queues` edit rather than a second copy of its rules, so
	/// the queues stay pure and their tests answer with a fixed `Admission`.
	fn edit(&mut self, edit: Edit, answer: Option<Admission>) -> Task<Message> {
		let mut asked = None;
		let admits = |items: &[queue::Item], duplicates: &[(usize, QueueId)]| {
			answer.unwrap_or_else(|| {
				asked = Some(Self::duplicate_warning(items, duplicates));
				Admission::Nothing
			})
		};
		let changed = match edit.clone() {
			Edit::Add(id, items, prepend) => self.queues.add(id, items, prepend, admits),
			Edit::Shift(id, right) => self.queues.shift(id, right, admits),
			Edit::Drop(into, at, carried) => {
				self.queues
					.dropped(into, at, carried.items, carried.from, admits)
			}
		};

		if let Some(dialog) = asked {
			let held = Held {
				edit,
				layout: self.queues.layout(),
			};
			return ask(
				move |window| dialog.clone().set_parent(window).show(),
				move |answer| {
					let admission = match answer {
						rfd::MessageDialogResult::Yes => Admission::All,
						rfd::MessageDialogResult::No => Admission::Fresh,
						_ => return None,
					};
					Some(Message::Admitted(held.clone(), admission))
				},
			);
		}
		if changed { self.queued() } else { Task::none() }
	}

	/// A player's track out, leaving it empty.
	fn unload(&mut self, id: DeckId) {
		self.decks[id.index()].unloaded(id, &self.engine);
		self.notice = format!("{}: unloaded", id.label());
	}

	/// The duplicate warning: what `Queues` asks when a batch it is adding has tracks already
	/// queued somewhere (PLAN §7a, §9a).
	///
	/// The app **asks** because queueing a track twice is sometimes exactly what someone means
	/// and sometimes a mistake, and nothing in the app can tell those apart. A silent refusal
	/// would be the app deciding; a silent accept is what left the mistake possible.
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
	fn duplicate_warning(
		items: &[queue::Item],
		duplicates: &[(usize, QueueId)],
	) -> rfd::AsyncMessageDialog {
		// Named rather than counted, up to a point: a queue of forty names is not a question
		// anyone reads, and the count is the part that decides the answer.
		const NAMED: usize = 6;
		let named: Vec<String> = duplicates
			.iter()
			.take(NAMED)
			.filter_map(|(index, queue)| {
				items
					.get(*index)
					.map(|item| format!("    {} ({})", item.name, queue.label()))
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

		warning("Already queued", description).set_buttons(rfd::MessageButtons::YesNoCancel)
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
	/// takes with it on the way out and gives back on the way in, and `take_unmeasured` subtracts
	/// it. Without that, two edits a few milliseconds apart would send the same file to be
	/// opened and parsed twice, and twenty edits would send the first one twenty times.
	///
	/// The invariant that makes the giving-back safe: **the arm is handed exactly the batch
	/// that was asked about**, whatever happened to the job — so a batch cannot be lost from
	/// the set and leave a file nothing will ever look at again.
	fn measure_queues(&mut self) -> Task<Message> {
		let paths = self.queues.take_unmeasured();
		if paths.is_empty() {
			return Task::none();
		}

		// The batch survives the job, because the arm needs it whether or not the job answers.
		let asked = paths.clone();
		let cache = self.cache.clone();

		off_thread(
			move || {
				paths
					.into_iter()
					.map(|path| measure::facts(&cache, &path))
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
							ready: None,
							prepared: false,
						})
						.collect()
				}))
			},
		)
	}

	/// The one door a device's refusal comes through (Q56).
	///
	/// `None` is the ordinary case and means nothing worth a line happened; a `Some` replaces
	/// whatever the bar was showing, because the newest failure is the one the user can still
	/// act on. Four arms each re-wrote this rule before it had a name.
	fn noted(&mut self, notice: Option<String>) {
		if let Some(notice) = notice {
			self.notice = notice;
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
		let notice = self.decks[id.index()].seek(id, to, &self.engine);
		self.noted(notice);
	}

	fn transport(&mut self, id: DeckId, event: deck::Event) {
		let notice = self.decks[id.index()].moved(id, event, &self.engine);
		self.noted(notice);
	}

	/// Decode a file into a player, and start the waveform scan behind it. A failure leaves
	/// the previous track alone and says so — it must never wipe a loaded track to show an
	/// error (PLAN §7).
	fn load(&mut self, id: DeckId, path: PathBuf) -> Task<Message> {
		self.try_load(id, path).unwrap_or_else(Task::none)
	}

	/// The same, saying whether the player took the file: `None` when it refused, with the
	/// notice already saying why. For the callers whose next step depends on it — a batch
	/// takes rows out of a queue only once the first file has arrived, and a handover plays
	/// only what arrived.
	fn try_load(&mut self, id: DeckId, path: PathBuf) -> Option<Task<Message>> {
		if !browser::kind_of(&path).is_media() {
			self.notice = format!("{} is not a media file", fsio::name_of(&path));
			return None;
		}

		let Some(engine) = self.engine.as_ref() else {
			self.notice = "no audio device — press Reconnect audio".to_string();
			return None;
		};

		match engine.load(id, &path) {
			Ok(duration) => {
				self.notice = format!("{}: {}", id.label(), fsio::name_of(&path));
				self.decks[id.index()].loaded(path.clone(), duration);
				Some(scan_track(id, path, duration, self.cache.clone()))
			}
			// `{error:#}` prints the anyhow chain on one line, which is what turns
			// "cannot decode x.mp4" into "cannot decode x.mp4: unsupported codec".
			Err(error) => {
				self.notice = format!("{}: {error:#}", id.label());
				None
			}
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
		// A fresh `Engine` starts both players at the track's own speed with no lock, so a
		// reconnect has to say what the sliders say — or the model would read 150% while the
		// audio ran at 100% (PLAN §7c).
		self.apply_rates();

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

	/// Push both rate sliders and both locks to a device that has not been told yet — a fresh
	/// `Engine` at startup, or a re-opened one (PLAN §7c).
	///
	/// Not called when a slider *moves*: that is `Deck::rated`'s job, which pairs the model and
	/// the audio the way every other control does (Q48). This is only the catching-up, and it
	/// goes through the same two methods rather than round the side of them, so a rate at the
	/// slider's pause end stays paused across a reconnect.
	fn apply_rates(&mut self) {
		for id in DeckId::ALL {
			let deck = &mut self.decks[id.index()];
			let (rate, locked) = (deck.rate, deck.key_lock);
			deck.locked(id, locked, &self.engine);
			self.decks[id.index()].rated(id, rate, &self.engine);
		}
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
					self.preparing.as_ref().map(Preparing::progress),
					&self.working(),
					self.phase(),
					&self.tempos,
				),
				Section::Tree => ui::tree::view(&self.tree, self.browser.folder.as_deref()),
			};
			// The files pane wears the same ring the queues do, and only it: the tree is
			// navigation, not a list the keyboard acts on (PLAN §9a).
			let focused = section == &Section::Files && self.focus == Focus::Files;
			pane_grid::Content::new(body)
				.style(move |theme: &Theme| ui::panel_style(theme, focused))
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

		// Wrapped so a right press says *where* it was, which no press message carries and no
		// pane can work out (PLAN §14d). It publishes on right presses only, so the app is as
		// idle as it was — which is what lets §6 keep refusing a cursor subscription.
		let window = ui::pointer::watch(
			column![body, self.status_bar()]
				.spacing(STATUS_GAP)
				.padding(WINDOW_PADDING),
			Message::PointerPressed,
		);

		// The one place anything is drawn over the window (PLAN §14d), and the window goes
		// through it whether or not there is: see `over` for why it must.
		let panel = self.overlay.as_ref().map(|overlay| match overlay {
			Overlay::Menu(menu) => (ui::tempo::menu(&menu.name, menu.tempo), menu.at),
			Overlay::Editor(correction) => (
				ui::tempo::editor(&correction.name, correction.value, correction.detected),
				correction.at,
			),
		});
		ui::tempo::over(window, panel, self.window)
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
	/// Three columns, and each column is its control above its queue. The controls are all
	/// fixed-size rows, so they take what they need and the **queue takes the rest** — which
	/// is what makes dragging the divider grow the queues rather than pad the players with
	/// empty space.
	fn decks_view(&self) -> Element<'_, Message> {
		let ring = self
			.drag
			.ring(deck::idle_target(&self.decks[0], &self.decks[1]));
		// Asked once for all three queues: they each draw the same two add buttons, and the
		// answer is a property of the files pane rather than of any queue.
		let addable = self.browser.has_media_selection();

		let queue = |id: QueueId| {
			ui::queue::view(
				id,
				self.queues.get(id),
				addable,
				self.focus == Focus::Queue(id),
				self.queues.scroll(id),
				&self.tempos,
				// One value rather than three arguments, and `None` at rest: everything a
				// queue does differently during a drag arrives together.
				self.drag.is_armed().then(|| ui::queue::Dragging {
					insertion: self.drag.caret(id),
					edge: self
						.drag
						.scrolling()
						.and_then(|(queue, up)| (queue == id).then_some(up)),
				}),
			)
		};

		row![
			column![
				self.deck_view(DeckId::One, ring),
				queue(QueueId::Cue(DeckId::One))
			]
			.spacing(8),
			column![
				container(ui::mixer::view(
					&self.decks[0],
					&self.decks[1],
					self.crossfader,
					self.curve,
				)),
				queue(QueueId::Shared),
			]
			.spacing(8)
			.width(MIXER_WIDTH),
			column![
				self.deck_view(DeckId::Two, ring),
				queue(QueueId::Cue(DeckId::Two))
			]
			.spacing(8),
		]
		.spacing(8)
		.padding(8)
		.into()
	}

	/// How far round the scanning animation is, as a fraction of one turn.
	///
	/// One phase for everything that turns — both players' strips and every spinning row — so
	/// two scans running at once move together rather than drifting apart into something that
	/// looks like a rendering fault.
	fn phase(&self) -> f32 {
		(self.sweep % SWEEP_STEPS) as f32 / SWEEP_STEPS as f32
	}

	/// One player, made a drop target only while an in-app drag is in flight.
	///
	/// Attached conditionally because `mouse_area` reports every crossing of the panel,
	/// dragging or not, and outside a drag there is nothing to do with that.
	fn deck_view(&self, id: DeckId, ring: Option<DeckId>) -> Element<'_, Message> {
		let sweep = self.phase();
		// Looked up by path rather than kept on the `Deck`, because the same answer is wanted
		// for tracks that are in no player at all (PLAN §14c).
		let trim = self.decks[id.index()]
			.track
			.as_ref()
			.and_then(|track| self.known.trim(&track.path));
		// The same correction rule as every row (PLAN §14d): the slider's readout multiplies
		// the number the user believes, which is the corrected one where there is one.
		let tempo = self.decks[id.index()].track.as_ref().and_then(|track| {
			ui::corrected_tempo(&self.tempos, &track.path, Some(track.tempo)).flatten()
		});
		// Not corrected, unlike the tempo beside it: a key cannot be typed in (PLAN §14d is
		// about halved and doubled tempos), so the detector's answer is the only one there is.
		let key = self.decks[id.index()]
			.track
			.as_ref()
			.and_then(|track| track.key);
		let panel = ui::deck::view(
			id,
			&self.decks[id.index()],
			trim,
			tempo,
			key,
			ring == Some(id),
			sweep,
		);

		if !self.drag.is_armed() {
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
		// Any press, any button. §9a promised "a new press cancels the pending collapse",
		// and only the row presses kept it — a press held on a deck button across the
		// 350 ms tick let the collapse fire under it, and the button then acted on one row
		// instead of the selection (Q53). Same ponytail trade as the release above.
		iced::Event::Mouse(mouse::Event::ButtonPressed(_)) => Some(Message::PressedAnywhere),
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

/// Show a native dialog on the app's window, and turn its answer into a message — or into none,
/// for a Cancel (PLAN §7a, Q83).
///
/// **Attached and awaited.** `show` builds and opens the dialog with the window as its parent,
/// which iced lends inside `window::run` alone, so the window takes no clicks while it is open —
/// that is what a modal is for. But `update` returns at once and the answer comes back as a
/// message, so the window goes on drawing: the playhead and the time run on, a track that ends
/// still hands over. Every arm the answer reaches has to allow for that.
///
/// rfd runs a Windows dialog on a thread of its own, which is also what keeps it visible: a
/// message box stays hidden until its thread's queue is empty, and the GUI thread's never is
/// (Q70). On macOS a parented dialog is a sheet on the window rather than `CFUserNotification`'s
/// alert drawn by another process, so the log line Q38 explained is gone with the blocking.
///
/// `Fn` and `Clone`, because iced may ask for the window more than once in principle: each
/// `show` clones its dialog, which is a few strings.
fn ask<F, T>(
	show: impl Fn(&dyn window::Window) -> F + Clone + Send + 'static,
	answered: impl Fn(T) -> Option<Message> + Send + 'static,
) -> Task<Message>
where
	F: Future<Output = T> + Send + 'static,
	T: Send + 'static,
{
	window::oldest()
		.and_then(move |id| window::run(id, show.clone()))
		.then(Task::future)
		.then(move |answer| answered(answer).map_or_else(Task::none, Task::done))
}

/// A warning with **OK** and **Cancel** — the shape of every question the app asks but one.
fn warning(title: &str, description: impl Into<String>) -> rfd::AsyncMessageDialog {
	rfd::AsyncMessageDialog::new()
		.set_level(rfd::MessageLevel::Warning)
		.set_title(title)
		.set_description(description)
		.set_buttons(rfd::MessageButtons::OkCancel)
}

/// Whether a warning was answered **OK**.
fn confirmed(answer: rfd::MessageDialogResult) -> bool {
	answer == rfd::MessageDialogResult::Ok
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
fn scan_track(
	id: DeckId,
	path: PathBuf,
	duration: Option<Duration>,
	cache: Arc<Cache>,
) -> Task<Message> {
	let scanned = path.clone();

	off_thread(
		move || measure::scan(&cache, &scanned).map_err(|error| format!("{error:#}")),
		move |result| {
			// Reported rather than swallowed: a strip that stays flat for ever with no line
			// in the status bar is the one outcome worse than a slow scan.
			let result =
				result.unwrap_or_else(|| Err("the waveform scan stopped unexpectedly".to_string()));
			Message::Scanned(id, path.clone(), duration, result)
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
		move || {
			let scan = measure::scan(&cache, &path).ok();
			let duration = measure::duration(&cache, &path);
			Facts::scanned(path, duration, scan.as_ref())
		},
		move |facts| {
			// A file that would not decode still counts as done, or the progress would stop on
			// the first broken track in a folder and never reach the end.
			Message::ScanFolderStepped(facts.unwrap_or_else(|| Facts {
				path: asked.clone(),
				duration: None,
				ready: None,
				prepared: false,
			}))
		},
	)
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

	/// The menu over a row, as either pane opens it: here with what the row's pane detected.
	fn menu(detected: Option<f32>, tempos: &BTreeMap<PathBuf, f32>) -> RowMenu {
		RowMenu::new(
			PathBuf::from("/elsewhere/a.mp3"),
			"a.mp3".to_string(),
			detected,
			tempos,
			None,
		)
	}

	#[test]
	fn the_editor_says_what_the_rows_own_pane_detected() {
		// Arrange: a queue row of a file from another folder, which the files pane has never
		// listed, so only the row itself knows what the detector said.
		let opened = menu(Some(87.0), &BTreeMap::new());

		// Act
		let editor = opened
			.correct()
			.expect("a row with a tempo has one to correct");

		// Assert
		assert_eq!(editor.value, 87.0);
		assert_eq!(editor.detected, Some(87.0));
	}

	#[test]
	fn a_corrected_row_shows_its_correction_and_the_editor_still_says_what_was_detected() {
		// Arrange
		let tempos = BTreeMap::from([(PathBuf::from("/elsewhere/a.mp3"), 174.0)]);

		// Act
		let opened = menu(Some(87.0), &tempos);
		let shown = opened.tempo;
		let editor = opened.correct().expect("a corrected row has a tempo");

		// Assert: the number the row is drawn with, and the editor starting from it.
		assert_eq!(shown, Some(174.0));
		assert_eq!((editor.value, editor.detected), (174.0, Some(87.0)));
	}

	#[test]
	fn a_row_with_no_tempo_opens_no_editor() {
		// Arrange / Act / Assert
		assert!(menu(None, &BTreeMap::new()).correct().is_none());
	}
}
