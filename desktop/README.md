# clecta — desktop

Two players, a mixer between them, and a file browser below. See [PLAN.md](PLAN.md) for
the design and the decision log.

## Running it

```sh
cargo run
```

The window opens where you left it last time, or on your home folder on a first run.

A debug build optimizes its **dependencies** but not clecta itself
([Cargo.toml](Cargo.toml)). That is not tidiness: decoding a track for its waveform took
16.7 s under a plain `cargo run` and 0.5 s with it, because the work happens inside
symphonia and unoptimized arithmetic is fifty times arithmetic. clecta's own code stays
unoptimized, so it is still debuggable and still rebuilds in seconds (PLAN §14a).

### As a macOS app

```sh
cargo build --release
./bundle-macos.sh
```

Gives `target/release/Clecta.app`, which Finder launches as an app rather than through
Terminal, and which keeps `clecta-data/` *beside* the bundle instead of inside it
(PLAN §11). The shipped binary is Intel, so on an Apple Silicon Mac bundle that instead:

```sh
cargo build --release --target x86_64-apple-darwin
./bundle-macos.sh target/x86_64-apple-darwin/release/clecta
```

## What works

- **Two players.** Play / pause / stop, a `M:SS / M:SS` readout, and a **Load…** button
  each. Stop is a rewind, not a `Player::stop()`: the track stays loaded and plays again
  from the top (PLAN §7).
- **A waveform per player.** The whole file is scanned once when it loads, on a thread of
  its own, and the strip fills in when it lands — around a third of a second for a
  three-minute track. The player is playable the whole time, and a band sweeps the strip
  while the scan runs so the wait is visible rather than mysterious. The part already
  played is coloured, and a playhead crosses it. **Click it to jump there, or hold and drag
  along it to scrub** — the transport does not change either way, so a playing track carries
  on from the new place and a paused one stays paused there. A scrub follows the pointer
  past either end of the strip and off the panel, and stops wherever you let go (PLAN §14a,
  §14b). Two buttons sit above it — **⇤ 0:00** and **⇥ music** — which send the playhead to
  the top of the file and to the top of the *music*, wherever the silence at the front ends;
  both edges of the music are drawn on the strip as green hairlines, so **⇥ music** lands
  somewhere you can see. A stopped player asked to move becomes **paused** at the new place,
  because "stopped" in this app means at the top of the track (PLAN §14c).
- **Three queues.** A cue under each player, and a shared **Next up** under the mixer.
  When a track ends, that player takes the next one — **its own cue first, the shared queue
  second** — and loads it *stopped* at 0:00, so nothing ever becomes audible without a Play
  press. A track ending is the only thing that pulls from a queue: adding files never starts a
  sound. **⤒ ⤓** on each queue add whatever the files pane has selected, to the top or the end;
  **✕ ▲ ▼** edit the selection; **← →** hand it to the neighbouring queue. Cue 1 and Cue 2 are
  not neighbours, so a track crosses through the shared queue rather than jumping.
  **Double-click a row to play it now**, out of turn — a cue goes to the player it sits under,
  the shared queue to whichever player is free — and the row leaves the queue, exactly as it
  would have when its turn came. Each footer shows how many tracks and **how long they run
  for**, with a `+` while something in the queue is still being measured or has no length the
  decoder can give. Queueing a track that is **already in one of the three queues** asks first,
  naming where it is — playing something twice is deliberate as often as it is a slip, so the
  app asks rather than deciding. All three queues survive a restart, and a queued file that has
  been deleted or renamed since is dropped rather than left to fail at the moment it is due
  (PLAN §7a).
- **Each queue decides what it does at the end of a track.** Two checkboxes on every queue:
  **Auto-load**, on to begin with, hands its top track to a player that has just run out;
  **Auto-play**, off to begin with, starts that track instead of leaving it stopped at 0:00.
  They are per queue, which is the point — Cue 1 can run the evening by itself while **Next up**
  stays a shelf you take from by hand. A queue with **Auto-load** off is skipped rather than
  blocking, so switching a cue off still lets the shared queue feed that player, and switching
  every queue off leaves the player stopped with full queues in front of it. **Auto-play** is
  drawn dead while **Auto-load** is off, since nothing is handed over for it to start. Both
  survive a restart, and a settings file written before they existed keeps the old behaviour
  (PLAN §7a).
- **…and when it does it.** A third control on every queue, under the two checkboxes.
  **Whole track**, the default, waits for the file to run out and starts the next one at 0:00.
  **Skip blanks** hands over when the *music* stops — skipping the run-out, the fade to nothing
  and the padding an encoder left — and starts the next track where **its** music starts. Per
  queue, like the switches, so Cue 1 can run an evening back to back while **Next up** plays
  what it is handed, whole. It needs to know where a track's music is, which is what the
  **Prepare folder** button below is for: a track nothing has scanned simply plays whole, and
  the last track of the evening always plays its run-out, since cutting it short would only
  stop the player early (PLAN §7b, §14c).
- **Select as many rows as you like.** Click to select, ⌘/Ctrl-click to add or remove one,
  Shift-click for everything in between, ⌘A for every row the pane is showing, Escape for none
  — in the files pane and in all three queues. Clicking a selected row narrows the selection
  to it on release, file-manager style — after a beat, because the click might still turn out
  to be a drag or a double click, both of which carry the whole selection (PLAN §9a). Everything that worked on one file works on the
  selection, **top to bottom**: **→ Player 1** plays the first and puts the rest at the top of
  that player's cue, **⤒ ⤓** queue all of them in order, a drag carries the lot, and
  **✕ ▲ ▼ ← →** act on every selected row. Queueing a batch where some tracks are already
  queued asks **once**, with three answers: queue them all again, queue only the ones that are
  not, or do nothing (PLAN §9a).
- **Drag anything anywhere.** Drag a file from the browser into a queue, drag a row up or down
  inside its queue, drag it across to another queue, or drag it straight onto a player to play
  it now — jumping the queue, and leaving the queue as it goes. A **green caret** shows the
  exact gap the row will land in, and the strip below the last row means "append", which is
  also the only place to aim at in an empty queue. Exactly one indicator is ever lit: a drag
  headed for a queue lights no player ring. To reach a row that is off screen, **rest the drag
  on a queue's header to scroll it up, or on its footer to scroll it down** (PLAN §7a, §10).
- **The mixer strip.** A volume fader per player and a crossfader, with a
  **Power / Linear** curve selector. The number beside each fader is the gain actually
  sent to that player, so the cubic taper and the crossfade are visible as they move.
  Every slider has its ends on buttons — **0** and **max** either side of each volume
  fader, **◄ 1 / centre / 2 ►** under the crossfader. The centre one earns its place: the
  ends can be reached by shoving the knob into the wall, but `0.5` exactly is a value a
  mouse lands on by luck (PLAN §8).
- **The files pane and the folder tree.** Each in its own pane with a draggable
  splitter. Click a folder name in the tree to show it, the arrow to open it. Click a
  file row to select it, **double-click** a media row to load it into whichever player is
  idle, or use **→ Player 1 / → Player 2**. **◧ hide tree** in the status bar folds the
  tree away and brings it back at the width it had (PLAN §6, §9).
- **The folder keeps itself up to date.** Save a file into the folder you are looking at and
  the row appears; delete one and it goes. The OS does the telling — FSEvents on macOS,
  `ReadDirectoryChangesW` on Windows — so nothing polls and nothing runs while nothing
  changes, and a burst of twenty files arriving is one re-listing rather than twenty.
  **Refresh** and the keys **F5** / **⌘R** are still there, because a permission or a network
  mount can take the watching away and the manual door has to work when it does. Two keys
  because F5 is the refresh key on Windows and is swallowed by a Mac laptop's own function
  keys, while ⌘R is what a Mac reaches for (PLAN §9).
- **A folder of any size costs the same.** Only the rows on screen are built, so a
  20 000-file folder draws for what a 200-file one does. It was worth doing rather than
  assumed to be: 5 000 files cost **70 % of a core** at the playing tick before, and **9 %**
  after. Choosing a folder opens it at the top; a refresh leaves you where you were reading.
  The reading itself now happens on a thread of its own, like the waveform scan — 5 000
  files take 25 ms to list and 20 000 take 95 ms, and on the executor that is a frozen
  playhead rather than a late listing (PLAN §4, §9).
- **The players keep their height.** Drag the divider under them and that panel stays that
  tall whatever the window does — resizing the window moves the bottom of the *file list* and
  nothing else, since the players' rows are all fixed-size and would only gain empty space.
  Squash the window past the panel's own height and it compacts; pull it open again and it
  comes back to the height you chose, because the height you asked for is remembered
  separately from the height that fits. It holds *still* while you drag, which took three
  goes — PLAN §6 tells that story, and it is the best one in the file (PLAN §6).
- **Drag and drop, both ways in.** Drag a row from the files pane onto either player and
  it loads *there* — the pointer is the app's the whole way, so the drag is truly aimed.
  Drag a file in from Finder / Explorer and it lands on **the idle player**, because the
  OS gives no position with the drop; a **green ring** lights the player that will receive
  it *before* you let go, so the rule is shown rather than sprung. A folder, a non-media
  file, and the second file of a multi-file drop are each declined in the status bar
  rather than ignored (PLAN §10).
- **No audio device is survivable.** The app still browses, says so in the status bar,
  and offers **Reconnect audio** (PLAN §11).
- **It is portable.** Both faders, the crossfader, the curve, the folder, the window size,
  the players' height and the three queues come back next launch, from **`clecta-data/settings.json` beside the executable** —
  beside the `.app`, not inside it, on macOS. Nothing is written anywhere else: no
  registry keys, no `~/Library` unless the app itself sits somewhere unwritable. Delete
  the folder and you have deleted clecta. The file is written **two seconds after you
  change something**, and again when the window closes, so quitting any way at all — ⌘Q
  included — keeps your settings. Changing folder does not wait: it is saved the moment the
  listing appears, because navigating and quitting straight after is the ordinary way to use
  a browser, not a corner case. It is plain JSON you can edit: a value that makes no
  sense falls back to its default on its own, and a file that will not parse at all reads
  as defaults rather than stopping the app (PLAN §11).
- **A waveform is scanned once, not once per launch.** What has been worked out about a file
  — its shape and its length — is kept in **`clecta-data/cache.redb`**, beside the settings
  and travelling with them, so a portable install carries its own history. Reading a stored
  waveform takes **54 µs against the 73 ms** the scan it replaces took, and the array is
  bit-identical to the one a fresh scan would produce. An entry is good only for the file it
  was written for: change the file and it is scanned again, on the next launch. It is only a
  cache — **delete it whenever you like** and the app rebuilds it as it goes. Entries whose
  file has gone are dropped at startup, so it stays the size of the library it describes
  (PLAN §11a).
- **Prepare a whole folder in one click.** **Prepare folder** under the files pane walks the
  shown folder *and everything under it*, and works out the waveform, the length and the
  music's two edges for every media file it finds — four at a time, with a count of how many
  of how many are done and a **Stop** that takes effect on the next file rather than killing
  the four already decoding. Everything it finds goes in the cache, so the waveforms appear
  instantly afterwards and **Skip blanks** has something to work with. **Prepare selected**
  does the same for just the rows you picked. Beside them, **Clear cache** throws all of it
  away again, for every file, after asking — nothing is lost but the time to work it out again
  (PLAN §11b, §9a).
- **Every row says whether it is ready.** A column at the left of the files pane, before the
  `♪`: a **turning glyph** while a thread is decoding that file, and a green **✓** once the
  cache holds a full scan of it — waveform, music edges *and* tempo, for that exact version of
  the file, which is the same test a load uses to decide it can skip the work. So a prepared folder
  is readable at a glance, still is after a relaunch, and stops being one the moment you edit a
  track. The spinner covers any decode, not just a batch one: load a track into a player and its
  row turns too. Clearing the cache empties the column (PLAN §11c).
- **How long the music actually runs.** The same scan that finds where a track's music starts
  and stops knows how long it lasts between the two, so once a file is prepared the files pane
  shows that in a column before the size, and a queue row shows it in front of the file's own
  length — `2:58 / 3:12`, the music first, because that is the number a set is planned against.
  It costs nothing extra: both are read from the edges the cache was already holding, so no file
  is opened for it. Blank until something has scanned the file (PLAN §14c).
- **How fast it beats.** The same decode counts the beats, so a prepared file also gets a tempo:
  `128.00`, two decimals, in a column before the playing time in the files pane and leading a
  queue row. Onsets from a loudness envelope, an autocorrelation for the beat and one bin of a
  Fourier transform to sharpen it — no new dependency, because the obvious crate is C and the
  portable build bans a C toolchain. Reported raw between 65 and 200 BPM rather than folded into
  a narrower window: half-time and double-time are both genuinely true about a lot of music, so
  the app says what it measured and you overrule it. `--` for a file with no tempo in it at all,
  blank until it is scanned (PLAN §14d).
- **Correcting a tempo by hand.** Right-click a row — in the files pane or in a queue — for a menu
  with **Correct tempo…**, and a panel with `/2`, the number, and `×2`. That is the whole editor,
  because an octave is the only error a detector makes that you can see at a glance, and the two
  buttons are exactly reversible. Nothing is written until **Apply**; Cancel, Escape and a click
  outside are the same thing. A correction lives in `settings.json`, not the cache, because
  nothing can work it out again: **Clear cache** takes the detected numbers, **Clear tempo corrections**
  takes yours, and each asks first (PLAN §14d).

## What does not, yet

- **No aiming an OS drop.** A file dragged in from Finder goes to the idle player, not to
  the one under the cursor — the position is thrown away by winit and is not recoverable
  in clecta's own code. The upgrade path is upstream, and PLAN §10 spells it out.
- **No video picture** — the audio track of an `.mp4` / `.mkv` plays, and that is v1
  (PLAN §14). No cue points, and no *changing* a tempo: reading one is §14d, playing a track at
  a different one needs a time-stretch stage rodio does not have.
- **The row menu is centred, not at the pointer.** iced's press messages carry no position, and
  the only route to one is a subscription that fires on every mouse move — which would rebuild
  every row of the files pane each time (PLAN §14d, §6).

## Checks

What CI runs (PLAN §12), all of which must be clean:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny check bans licenses sources
cargo audit
```

CI runs clippy and the tests on **both** shipped targets — natively on Windows, and
cross-compiled against `x86_64-apple-darwin` on the macOS runner, which is Apple Silicon.
The supply-chain pair guards the dependency tree: the licence allow-list in
[deny.toml](deny.toml) is the minimal set the tree actually needs, and `cc` is banned so
that the day something wants a C compiler on a shipped target, CI says so rather than the
portable single-binary property quietly dying.

The release build is deliberately *not* a CI job — clippy already type-checks everything,
and `lto` costs minutes to re-prove something that only matters when shipping. Run it
yourself for that:

```sh
cargo build --release
```

`cargo test` covers the pure modules, plus the pure arithmetic inside the ones that are not
— anything needing a device or a real folder is manual (PLAN §12):

| Module | What is checked |
|---|---|
| `mixer.rs` | both curves at both ends and the centre, each curve's defining identity at the midpoint, the fader-at-zero invariant, clamping |
| `deck.rs` | every edge of the transport state machine, including a seek — which leaves a playing player playing and a paused one paused, but turns a *stopped* one into a paused one, since stopped in this app means at the top of the track; then the same edges driven with no audio device at all, which is what the app does when an interface is unplugged: Pause keeping the playhead where the listener last heard it while Stop rewinds, the file running out and the Stop button agreeing about where the player ends up, a seek on a stopped player moving the position and the label together, and an empty player refusing every one of them including the seek. Plus: an unaimed load never interrupting a playing player while an idle one exists, and the drop policy — a folder, a non-media file and the rest of a multi-file drop each declined by name |
| `select.rs` | what a press means — plain replaces, ⌘ toggles, Shift takes the range, and Shift wins when both are held — a range that is inclusive at both ends whichever way it was dragged, and only a plain press on a selected row deferring to its release |
| `browser.rs` | extension → kind, the natural-numeric sort, the hidden filter, a selection surviving (or not) a refresh — and the selection itself: each of the three click kinds, a shift-click adjusted by a second one, a range with no anchor falling back to a plain click, the selection coming back in **row order** however it was clicked, ⌘A meaning every row *on screen*, and a `.txt` that can be selected but is never handed to a player; then the ready marks — a refresh keeping the mark of a row it still lists and dropping the one it lost, a file worked out while the pane shows it getting one, a path the pane has never heard of not, and clearing the store clearing the column — and the playing time and tempo riding in the same map, a scanned file's seconds and beats coming back together, a scanned *silent* one coming back marked with neither, and a file nobody has scanned told apart from both |
| `tree.rs` | expand asks for a re-list, reveal asks only for what was never listed, collapse keeps its cache, `None` ≠ `Some(vec![])` |
| `paths.rs` | `clecta-data/` beside an ordinary binary, beside the `.app` for a bundled one, and no walk-up for a folder that merely looks like a bundle |
| `cache.rs` | the record encoding without a database — an exact round trip, a changed size or timestamp reading as a miss, another format byte read as a miss rather than as noise, a length told apart from the absence of one, a tempo told apart from the absence of one with a stored `NaN` or negative read as a miss, and a payload that is not a whole number of columns thrown away rather than truncated — then one pass over a real database in a temporary folder: store a whole scan and get the same one back, rewrite the fixture and watch its entry go stale, delete another and watch pruning drop exactly its three entries — one per table, which is what says the whole scan was written and not just its first third — then clear the lot and confirm the store still works; and what the files pane asks: "prepared" naming the file that has every table and not the one still missing a trim — a state the interface can no longer produce, so the test writes it through the private door, which is what an older build effectively is — and a listing's own size and modified time building the same stamp a `stat` does, with the playing time and the tempo read back out of the same answer — the seconds between a scanned file's edges and the beats it runs at, a mark with neither for one scanned and found silent, and nothing at all for the one still missing a table |
| `app.rs` | what a divider drag may store: a drag inside the bounds kept to the pixel, a drag past the bottom still leaving the browser its minimum at every window height, a drag above the top reading as the floor, and an impossible window storing a finite height; plus what each key means — F5, ⌘R, ⌘A and Escape yes, a bare `r` or `a` no — and the folder scan's three counters, run through a whole ten-file scan an answer at a time: the fan-out never exceeded, every file out exactly once, no thread unaccounted for, and a **Stop** that hands out nothing more while still waiting for the four already decoding, reporting those four **out of order** so that exactly one row stops spinning per answer and none is left turning |
| `waveform.rs` | a scan staying bounded for a file of any length, a halving keeping the loudest sample, a `NaN` not blanking its column, and every pixel column of every width in range; then the music's edges — a leader and a run-out found to the sample, a file of silence having no edges rather than edges at its ends, one loud sample being enough to be one, and a channel not being a second (a stereo file holds twice the sample rate per second, and getting that backwards puts every trim at twice its real depth), and how long the music runs between them — four seconds of leader and two of run-out subtracted away, and edges the wrong way round giving `0:00` rather than the panic a `Duration` subtraction would otherwise be; then the tempo — a click track built at 100, 128 and 174 BPM read back to a fiftieth of a BPM (174 on purpose, since 87 fits every other click just as well and the faster reading has to win), a channel not being a beat either, and the two kinds of nothing: silence, a clip too short to hold two of the slowest beats it is asked about, a rate the decoder would not answer for, and a stream of `NaN`s all having *no* tempo rather than a number somebody would act on |
| `queues.rs` | what is true of the three queues together: the arrows reaching a neighbour and only a neighbour, the next track coming from a player's own cue before the shared queue and from the shared queue when the cue is empty, a queue with **Auto-load** off being skipped rather than ending the handover (a cue switched off still lets the shared queue feed that player) while both switched off stops the player with full queues in front of it, a duplicate found in whichever queue actually holds it rather than only the one being added to, a row on its way out of a queue not counting as its own duplicate while a second copy elsewhere still does, which tracks get sent off to be measured (one entry per file rather than per row, and asking twice in a row giving the batch and then nothing, because recording what went out is part of asking), one answer settling every row holding that track, and three panels keeping three scroll offsets |
| `fsio.rs` | the recursive walk: media at every depth and nothing that is not media, in the pane's own order, with an unreadable root an error and an unreadable folder deeper down skipped in silence |
| `ui/waveform.rs` | the strip's geometry — the scanning band sliding in and out rather than appearing, and never drawn outside the strip at any phase, and a click mapping to the fraction under it and never to one that would panic a `Duration`; plus the scrub's three rules — a press arms only over the strip, a move seeks only while the button is held and follows it outside the strip too, a release disarms wherever it happens — and the one they could not see: the four events macOS really sends for a single click, replayed in order, adding up to exactly one seek |
| `queue.rs` | when a track gives way early — the music stopped *and* this queue asked to skip the blanks *and* somebody has scanned the track, so a queue set to **Whole track** never cuts and an unscanned one plays whole rather than being cut at zero — plus what every queue edit does to the selection — an insert above it carries it down, a remove above it pulls it up, removing the selected row lands on what slid into its place, a shift takes the highlight with the track — plus a drag within a queue landing where the caret was (both directions, past the last row, the two carets that touch the row itself, and every from × to keeping the contents unchanged) — and the same again for a whole block of rows, which keeps its own order, lands as a block, and comes out highlighted, checked over every pair of rows to every caret — plus the multi-row gestures: each of the three click kinds, a block and a scattered pair each shifting together, a selection touching the end it moves towards blocking the *whole* move, and taking only some of a selection leaving the rest highlighted; the running time (a measured row with no length keeps the total's `+` for ever); and a scan landing on a row that was measured long ago, since that is the order it happens in, without a later queue edit taking it away again — one fact now rather than two, since a row with a playing time and no tempo beside it was never a state anything could produce; and an index only holding the track that is still under it, so a delayed click on a queue that shifted meanwhile drops rather than firing on the row below |
| `ui/queue.rs` | the footer's count and running time, and an empty queue saying nothing at all rather than `0 · 0:00`; and which way the two arrows may move a selection — the top row, the last row, the middle, a block touching either end, the whole queue at once, and the single row of a single-row queue |
| `ui/browser.rs` | which mark a row wears — nothing, the spinner, the `✓` — and that a file being *re-*scanned spins rather than keeping the `✓` it already earned; plus a load button counting its rows only once there is more than one |
| `ui/deck.rs` | the strip's two divides: the music's edges as fractions of the file, a stale trim past the end clamped to the strip rather than drawn off it, a stream with no length and a zero-length track both drawing no marks at all, and a playhead a tick past its own total — which the app produces twenty times a second at the end of every track — reading as exactly the end |
| `ui/tempo.rs` | the quiet line under the editor: what the detector said, shown only while it differs from what the panel shows — compared as the two decimals on screen, so halving and doubling back says nothing rather than "detected 128.00" under a 128.00 |
| `audio.rs` | the two things here that need no audio device — one scan of a real file (a generated WAV, silent for a second then loud for a second, whose music is asserted to start at exactly one second and run to two), and one measurement of another, plus a missing file answering "no length" rather than failing |
| `settings.rs` | a round trip, every broken file reading as defaults, a missing field keeping its default (including the newest: a file written before tempos could be corrected has corrected none), an out-of-range value falling back alone, a folder or a queued track that no longer exists being forgotten, and a corrected tempo surviving only if it could be drawn — a deleted file, a non-media file, a `0`, a negative and a `NaN` all dropped, the good one beside them kept |
| `ui/mod.rs` | which rows a pane builds: a short list built whole, the window moving by whole rows as it scrolls, the end of a long list still filling the pane, a negative or `NaN` offset still naming real rows, and the same offset naming a different row at the queues' 22-pixel pitch than at the files pane's 24 — plus the spinner showing all four of its frames in order over one turn, and no phase indexing past the last of them — plus eliding, sizes, the calendar (including a leap day), the clock, and the six states of a row's times — blank, `--:--`, a plain length, `2:58 / 3:15`, and the one order that would print a separator with nothing after it — plus a tempo's three: blank, `--`, and always two decimals, including a round 128, a 127.999 that rounds up to it, and a hundredth landing on the boundary; and how a hand correction reaches a row: it replaces whatever the detector said, including the `--` of a file found to beat at nothing, it shows even on a file nothing has scanned, and a file nobody corrected comes back exactly as it went in |

Twenty-three things the suite cannot reach, all of which need a window a person can look at — or,
once, listen to. **Five of the twenty-three pass on macOS, by hand**; the last eighteen are new and
unchecked. The last rounds are why the list exists: four of the app's defects so far were
found here and none of them by a passing test. The fourth is the sharpest — an item already
marked *confirmed* was confirmed by eye, and the defect was audible only.

- **The close-button save.** ⌘Q *was* the open question here, and the answer was that it
  never reaches the app at all — so the settings are now written on a two-second throttle
  as well, which is what a kill, a crash or a ⌘Q relies on (PLAN §11). Both halves of the
  portability check are confirmed on macOS without a click: a bare binary and a bundled
  `Clecta.app` each create `clecta-data/` beside themselves with nothing in `~/Library`,
  and a run started with an oversized window writes the resized value while still running.
  The close *button* is confirmed with one.
- **The drop gestures themselves.** The policy and the targeting are pure and tested; the
  pointer bookkeeping around them — the ring lighting on one player, the drag disarming
  when it is let go over nothing, a release over a button not leaving it armed — is
  wiring that only a real drag exercises (PLAN §10).
- **What the waveform looks like.** Its numbers are checked three ways — unit tests, a real
  decode, and a printout from the running app — and the first version was still invisible,
  because the bars were painted thirteen levels of grey away from the panel behind them.
  Numbers cannot catch that; screen capture from a script is blocked by the same permission
  wall as scripted clicking, so it takes an eye (PLAN §14a).
- **The seek gesture.** The playhead lands where the pointer was, a playing track carries on
  and a paused one stays put, and the cursor turns into a hand over a loaded strip and not
  over an empty one. Confirmed, first try — and still wrong, which is why this list exists.
  Everything *visible* about it was right; what a look could not catch was that a click
  seeked twice, replaying a tenth of a second of audio from the click target. It took an ear
  on a playing track, and the cause was an event nobody knew the platform sent (PLAN §14b).
  **Worth one more listen** now that a click publishes one seek.
- **The scrub.** *Not checked yet.* Its rules are a tested pure function, so what is left is
  everything the rules assume: that a drag off the strip keeps sending moves, that the
  release still arrives when the button comes up outside the window, and that a seek per
  pointer move does not make a playing track stutter — the one `ponytail:` note this feature
  left behind (PLAN §14b).
- **A long folder, scrolled.** *Not checked yet.* The arithmetic is tested and the CPU is
  measured, but neither can see the pane: that the scrollbar is the length the folder
  deserves, that rows do not jump or blank out while the wheel turns, that the row under the
  pointer is the row that gets selected, and that choosing a folder opens it at the top while
  a refresh does not move. `/tmp/clecta-bench-5000`-style folders are made with a one-line
  Python loop (PLAN §9). Now also: that nothing hides under the scrollbar. iced paints the bar
  over the rows unless a gap is reserved, which a queue's running time found first, and the
  same gap is now reserved in all three queues — so the check is that a long name, a date and a
  running time all end before the bar begins, and that anything too short to scroll still uses
  its full width.
- **Dragging the divider, and then the window edge.** This is the one that failed, twice.
  The height had been measured with a probe widget and was exact every time, and the panel
  still *wobbled* as the window's bottom edge moved: iced lays out at the new window size a
  frame before the app hears about the resize, so any height worked out from the window is
  one frame stale. The first fix removed most of those and left one — the ceiling that
  decides when to compact — which binds precisely when the edge is being dragged, so the
  wobble survived it. The height is now a literal that never consults the window at all, and
  iced does the compacting (PLAN §6).
- **The refresh key.** *Not checked yet.* Which key is refresh is a tested branch; whether
  the OS hands that key to the app is not, and it is exactly the doubt that made this two
  keys — F5 on a Mac laptop is the keyboard-brightness key unless the function-key
  preference is flipped, so ⌘R may be the only one of the pair that works here, and F5 the
  only one that matters on Windows (PLAN §9).
- **The watcher across a folder change.** Watching itself *is* confirmed from a script — a
  file added, three added at once, one deleted, then an idle folder, all read back from the
  running app. What a script cannot do is click a folder in the tree, and that is the case
  worth an eye: the watcher is keyed on the path, so choosing another folder has to tear the
  old one down and start one on the new place, with no watcher left behind (PLAN §9).
- **The four queue drags, and the scroll edges.** *Not checked yet.* The arithmetic of a
  reorder is tested exhaustively, but the pointer bookkeeping around it is not, and it is the
  half that has always broken here: that the caret lights in the gap the row will actually
  land in, that moving between two rows never leaves both or neither lit, that leaving a queue
  clears its caret without clearing the ring of the player the pointer has moved onto, and
  that a drag released over nothing leaves everything exactly as it was. The autoscroll adds
  three of its own: that resting on a header or footer scrolls at a speed you can stop on the
  row you meant, that the rows keep up with a scroll no pointer asked for, and — the one that
  cannot be tested by trying it once and it working — that **letting go while resting on an
  edge stops the scrolling**, since the widget that would report the pointer leaving is
  destroyed by the same release (PLAN §7a, §10).
- **A queue handing over at the end of a track.** *Not checked yet*, and the one worth
  waiting for: every edit to a queue is tested, but the handover itself is not — that a track
  running out really does load the next one, that it lands *stopped* rather than playing, that
  the title and the waveform follow it, and that the shared queue is reached only when that
  player's own cue is empty. It needs two short tracks and a minute of listening (PLAN §7a).

  The two switches are checked in the same minute, and they are the half a pure test cannot
  reach: that **Auto-load** off really leaves the player stopped with a full queue under it,
  that a cue switched off still lets **Next up** feed that player, that **Auto-play** starts
  the handed-over track and *only* the handed-over one — a double-clicked row still lands
  stopped — and that **Auto-play** greys out the moment **Auto-load** is unticked (PLAN §7a).

  And the end of the *last* track, which is the same minute again: a track that runs out with
  nothing queued behind it must still be playable, because rodio consumes the source it
  finishes and the app has to put the file back before **Play** means anything (PLAN §7).
- **Selecting several rows.** *Not checked yet*, and the gestures are the half a pure test
  cannot reach: that ⌘-click adds without the pane scrolling or the row jumping, that a
  Shift-click over a *virtualized* list selects the rows between and not the rows built — the
  pane only builds what is on screen (PLAN §9), so a range across a thousand rows is the case
  worth trying — that ⌘A and Escape reach the app at all, and that the modifiers are the ones
  the app thinks they are after clicking away to another window and back, which is the
  `ponytail:` note this leaves behind (PLAN §9a).
- **The batch actions.** *Not checked yet.* That **→ Player 1** with five selected really does
  play the first and leave four at the top of Cue 1 in order; that a drag of five carries all
  five and drops them where the caret is; that **▲** with a block moves the block; and the
  duplicate dialog's three answers, which is the one to try deliberately — Yes queueing all,
  No queueing only the fresh ones, Cancel leaving every queue exactly as it was, with the
  buttons reading sensibly on the platform's own dialog (PLAN §9a).
- **A handover that skips the blanks.** *Not checked yet*, and it is the one this whole
  feature is judged by, because everything about it is a matter of taste rather than of
  arithmetic: that the cut lands where the music actually stopped rather than a beat early,
  that the next track comes in at its own first note, and that the two together sound like one
  set rather than two files. It needs a track with a real run-out — a fade, or the four seconds
  of nothing an album's last track ends with — and the folder prepared first, since a track
  nothing has scanned deliberately plays whole. Then the case with nothing behind it: the last
  track of a queue must play its run-out rather than stopping early (PLAN §7b).
- **The two buttons above the strip.** *Not checked yet.* That **⇥ music** lands on the green
  hairline and not near it, that **⇤ 0:00** goes back to the very top, that a playing track
  keeps playing from the new place and a paused one stays paused there — and the new rule,
  that a *stopped* player becomes **paused** rather than staying labelled stopped somewhere it
  is not (PLAN §14c).
- **Preparing a folder.** *Not checked yet.* The counters are tested and the walk is tested;
  what is not is the minute it runs for — that the count climbs steadily rather than in jumps
  of four, that the app stays usable and a track keeps playing without stuttering while four
  threads decode, that **Stop** stops it within a file or so, and that pressing it and starting
  again immediately does not double-count. Then the point of it: load a track from that folder
  and watch the waveform appear with no sweep at all (PLAN §11b).
- **Clearing the cache.** *Not checked yet.* That the dialog appears and **Cancel** really does
  nothing, that the waveform of a *playing* track is not blanked by it, and that the next track
  loaded after it sweeps again — which is the whole visible difference between a cache that was
  cleared and a button that lied (PLAN §11a, §11b).
- **Playing a queued row on demand.** *Not checked yet.* That a double click on a cue row
  loads it into the player it sits under and on a **Next up** row into whichever player is
  free; that the row leaves the queue either way; and that the press which opened the double
  click has not left a drag armed behind it — the release lands after the row is gone
  (PLAN §7a).
- **The duplicate warning.** *Not checked yet*, and it is the search that is tested rather
  than the dialog: that the box actually appears on all four ways in — **⤒ ⤓**, a drop from
  the files pane, a drop from another queue, **← →** — that **Cancel** leaves every queue
  exactly as it was, that a plain reorder and a drag onto a player never ask, and the one
  worth watching for, that a modal opened on a mouse *release* mid-drop leaves nothing armed
  behind it (PLAN §7a).
- **The ready column.** *Not checked yet*, and half of it is a font question a test cannot ask:
  that `◐ ◓ ◑ ◒` and `✓` actually render on both targets rather than as a missing-glyph box.
  Then the behaviour — that exactly the four files being decoded turn and they turn together
  with the players' strips, that ticks appear behind the scan as it goes, that they are still
  there after a relaunch, that touching a file on disk takes its tick away on the next refresh,
  and that **Clear cache** empties the column while the playing track's waveform stays
  (PLAN §11c).
- **The playing times.** *Not checked yet.* The arithmetic is tested and the six states of the
  string are pinned, but nothing can see the columns: that the files pane's new column lines up
  and does not squeeze the name out of a narrow pane, that a number appears on a row at the same
  moment its `✓` does, that it is still there after a relaunch, and that a queue row shows
  `2:58 / 3:12` without wrapping in the narrowest of the three panels. Worth doing against a
  real track with a long fade — the point of the number is that it should be visibly shorter
  than the file, and a folder where every row reads the same as its length means the threshold,
  not the display, is what to look at (PLAN §14c).
- **The tempos.** *Not checked yet.* Nothing in the suite has seen a real track: that the numbers
  in a folder of one genre agree with each other and with a tapped foot, that the two-decimal
  column lines up and does not squeeze the name out of a narrow pane or wrap in the narrowest
  queue panel, and that a file with no beat in it — a spoken intro, an ambient wash — shows `--`
  rather than a confident number. Also the cost of the change itself: **every folder prepared by
  an older build has lost its `✓` and needs one more Prepare folder run**, which is the price of
  a mark that means one thing (PLAN §14d, §11c).
- **Correcting one.** *Not checked yet.* The rule is tested and the panel is not: that a
  right-click opens the menu from both panes and never from a `.txt`, that **Correct tempo…** is dead
  on a row nothing has scanned, that `/2` then `×2` puts the number back exactly, that Cancel,
  Escape and a click outside all leave the row as it was, that **Apply** changes the number in the
  files pane *and* in every queue holding that file at once, that it survives a relaunch and a
  **Clear cache**, and that **Clear tempo corrections** asks first and puts every detected number back
  (PLAN §14d).

- **Narrowing a selection.** *Not checked yet.* The rule (`Click::defers`) is tested and the
  timing is not: that clicking one row of five narrows to it a beat later, that dragging the
  five to a queue does **not** narrow them, that double-clicking one still loads all five, and
  that ⌘A or Escape inside the beat wins over the pending collapse (PLAN §9a, Q50).

None of that says anything about **Windows**, which is the shipped target no one has ever
run. CI type-checks it on every push, and type-checking is not running.
