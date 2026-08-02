# clecta — desktop

Two players, a mixer between them, and a file browser below. See [PLAN.md](PLAN.md) for
the design and the decision log.

## Running it

```sh
cargo run
```

The window opens where you left it last time, or on your home folder on a first run.

## What works

- **Two players.** Play / pause / stop, a `M:SS / M:SS` readout, and a **Load…** button
  each. Stop is a rewind, not a `Player::stop()`: the track stays loaded and plays again
  from the top (PLAN §7).
- **The mixer strip.** A volume fader per player and a crossfader, with a
  **Power / Linear** curve selector. The number beside each fader is the gain actually
  sent to that player, so the cubic taper and the crossfade are visible as they move
  (PLAN §8).
- **The browser.** A files pane and a folder tree, each in its own pane with a draggable
  splitter. Click a folder name in the tree to show it, the arrow to open it. Click a
  file row to select it, **double-click** a media row to load it into whichever player is
  idle, or use **→ Player 1 / → Player 2**. **◧ hide tree** in the status bar folds the
  tree away and brings it back at the width it had (PLAN §6, §9).
- **No audio device is survivable.** The app still browses, says so in the status bar,
  and offers **Reconnect audio** (PLAN §11).
- **It is portable.** Both faders, the crossfader, the curve, the folder and the window
  size come back next launch, from **`clecta-data/settings.json` beside the executable** —
  beside the `.app`, not inside it, on macOS. Nothing is written anywhere else: no
  registry keys, no `~/Library` unless the app itself sits somewhere unwritable. Delete
  the folder and you have deleted clecta. The file is written when the window closes, and
  is plain JSON you can edit — a value that makes no sense falls back to its default on
  its own, and a file that will not parse at all reads as defaults rather than stopping
  the app (PLAN §11).

## What does not, yet

- **No drag and drop**, neither in-app nor from Finder / Explorer (PLAN §10). The
  **Load…** buttons and the double-click are the ways in.
- **No video picture** — the audio track of an `.mp4` / `.mkv` plays, and that is v1
  (PLAN §14). No waveform, no cue points, no tempo.
- **No list virtualization.** A folder of five thousand files builds five thousand
  widgets per frame. Fine for a music folder; the upgrade path is in PLAN §9.

## Checks

The same four CI runs (PLAN §12), all of which must be clean:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

`cargo test` covers the pure modules, and only those — anything needing a device or a
real folder is manual (PLAN §12):

| Module | What is checked |
|---|---|
| `mixer.rs` | both curves at both ends and the centre, each curve's defining identity at the midpoint, the fader-at-zero invariant, clamping |
| `deck.rs` | every edge of the transport state machine, and that an unaimed load never interrupts a playing player while an idle one exists |
| `browser.rs` | extension → kind, the natural-numeric sort, the hidden filter, a selection surviving (or not) a refresh |
| `tree.rs` | expand asks for a re-list, reveal asks only for what was never listed, collapse keeps its cache, `None` ≠ `Some(vec![])` |
| `paths.rs` | `clecta-data/` beside an ordinary binary, beside the `.app` for a bundled one, and no walk-up for a folder that merely looks like a bundle |
| `settings.rs` | a round trip, four kinds of broken file reading as defaults, a missing field keeping its default, one bad value falling back without taking the good ones with it |
| `ui/mod.rs` | eliding, sizes, the calendar (including a leap day), the clock |

One thing the suite cannot reach: **the save fires when the window closes**, so killing
the process or force-quitting loses the last changes. Worth a manual check that ⌘Q saves
too — it may go around the close request (PLAN §11).
