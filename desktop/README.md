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
  §14b).
- **The mixer strip.** A volume fader per player and a crossfader, with a
  **Power / Linear** curve selector. The number beside each fader is the gain actually
  sent to that player, so the cubic taper and the crossfade are visible as they move.
  Every slider has its ends on buttons — **0** and **max** either side of each volume
  fader, **◄ 1 / centre / 2 ►** under the crossfader. The centre one earns its place: the
  ends can be reached by shoving the knob into the wall, but `0.5` exactly is a value a
  mouse lands on by luck (PLAN §8).
- **The browser.** A files pane and a folder tree, each in its own pane with a draggable
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
- **It is portable.** Both faders, the crossfader, the curve, the folder, the window size
  and the players' height come back next launch, from **`clecta-data/settings.json` beside the executable** —
  beside the `.app`, not inside it, on macOS. Nothing is written anywhere else: no
  registry keys, no `~/Library` unless the app itself sits somewhere unwritable. Delete
  the folder and you have deleted clecta. The file is written **two seconds after you
  change something**, and again when the window closes, so quitting any way at all — ⌘Q
  included — keeps your settings. Changing folder does not wait: it is saved the moment the
  listing appears, because navigating and quitting straight after is the ordinary way to use
  a browser, not a corner case. It is plain JSON you can edit: a value that makes no
  sense falls back to its default on its own, and a file that will not parse at all reads
  as defaults rather than stopping the app (PLAN §11).

## What does not, yet

- **No aiming an OS drop.** A file dragged in from Finder goes to the idle player, not to
  the one under the cursor — the position is thrown away by winit and is not recoverable
  in clecta's own code. The upgrade path is upstream, and PLAN §10 spells it out.
- **No video picture** — the audio track of an `.mp4` / `.mkv` plays, and that is v1
  (PLAN §14). No cue points, no tempo.

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
| `deck.rs` | every edge of the transport state machine, that an unaimed load never interrupts a playing player while an idle one exists, and the drop policy — a folder, a non-media file and the rest of a multi-file drop each declined by name |
| `browser.rs` | extension → kind, the natural-numeric sort, the hidden filter, a selection surviving (or not) a refresh |
| `tree.rs` | expand asks for a re-list, reveal asks only for what was never listed, collapse keeps its cache, `None` ≠ `Some(vec![])` |
| `paths.rs` | `clecta-data/` beside an ordinary binary, beside the `.app` for a bundled one, and no walk-up for a folder that merely looks like a bundle |
| `settings.rs` | a round trip, four kinds of broken file reading as defaults, a missing field keeping its default, one bad value falling back without taking the good ones with it |
| `app.rs` | what a divider drag may store: a drag inside the bounds kept to the pixel, a drag past the bottom still leaving the browser its minimum at every window height, a drag above the top reading as the floor, and an impossible window storing a finite height; plus which key means refresh — F5 and ⌘R yes, a bare `r` no |
| `waveform.rs` | a scan staying bounded for a file of any length, a halving keeping the loudest sample, a `NaN` not blanking its column, every pixel column of every width in range, the scanning band never drawn outside the strip, and a click never producing a fraction that would panic a `Duration` |
| `ui/waveform.rs` | the scrub's three rules: a press arms only over the strip, a move seeks only while the button is held and follows it outside the strip too, a release disarms wherever it happens |
| `ui/browser.rs` | which rows the pane builds: a short folder built whole, the window moving by whole rows as it scrolls, the end of a long list still filling the pane, and a negative or `NaN` scroll offset still naming real rows |
| `audio.rs` | one scan of a real file — a generated WAV, silent for a second then loud for a second — which is the only thing in this module that needs no audio device |
| `ui/mod.rs` | eliding, sizes, the calendar (including a leap day), the clock |

Nine things the suite cannot reach, all of which need a window a person can look at. **Five
of the nine pass on macOS, by hand**; the last four are new and unchecked. The last two
rounds are why the list exists: three of the app's defects so far were found here and none
of them by a passing test.

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
  over an empty one. Confirmed, first try — the arithmetic had already been tested and the
  only question was the wiring (PLAN §14b).
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
  Python loop (PLAN §9).
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

None of that says anything about **Windows**, which is the shipped target no one has ever
run. CI type-checks it on every push, and type-checking is not running.
