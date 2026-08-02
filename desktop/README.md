# clecta — desktop

Two players, a mixer between them, and a file browser below. See [PLAN.md](PLAN.md) for
the design and the decision log.

## Running it

```sh
cargo run
```

The window opens where you left it last time, or on your home folder on a first run.

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
- **The mixer strip.** A volume fader per player and a crossfader, with a
  **Power / Linear** curve selector. The number beside each fader is the gain actually
  sent to that player, so the cubic taper and the crossfade are visible as they move
  (PLAN §8).
- **The browser.** A files pane and a folder tree, each in its own pane with a draggable
  splitter. Click a folder name in the tree to show it, the arrow to open it. Click a
  file row to select it, **double-click** a media row to load it into whichever player is
  idle, or use **→ Player 1 / → Player 2**. **◧ hide tree** in the status bar folds the
  tree away and brings it back at the width it had (PLAN §6, §9).
- **Drag and drop, both ways in.** Drag a row from the files pane onto either player and
  it loads *there* — the pointer is the app's the whole way, so the drag is truly aimed.
  Drag a file in from Finder / Explorer and it lands on **the idle player**, because the
  OS gives no position with the drop; a **green ring** lights the player that will receive
  it *before* you let go, so the rule is shown rather than sprung. A folder, a non-media
  file, and the second file of a multi-file drop are each declined in the status bar
  rather than ignored (PLAN §10).
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

- **No aiming an OS drop.** A file dragged in from Finder goes to the idle player, not to
  the one under the cursor — the position is thrown away by winit and is not recoverable
  in clecta's own code. The upgrade path is upstream, and PLAN §10 spells it out.
- **No video picture** — the audio track of an `.mp4` / `.mkv` plays, and that is v1
  (PLAN §14). No waveform, no cue points, no tempo.
- **No list virtualization.** A folder of five thousand files builds five thousand
  widgets per frame. Fine for a music folder; the upgrade path is in PLAN §9.

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

`cargo test` covers the pure modules, and only those — anything needing a device or a
real folder is manual (PLAN §12):

| Module | What is checked |
|---|---|
| `mixer.rs` | both curves at both ends and the centre, each curve's defining identity at the midpoint, the fader-at-zero invariant, clamping |
| `deck.rs` | every edge of the transport state machine, that an unaimed load never interrupts a playing player while an idle one exists, and the drop policy — a folder, a non-media file and the rest of a multi-file drop each declined by name |
| `browser.rs` | extension → kind, the natural-numeric sort, the hidden filter, a selection surviving (or not) a refresh |
| `tree.rs` | expand asks for a re-list, reveal asks only for what was never listed, collapse keeps its cache, `None` ≠ `Some(vec![])` |
| `paths.rs` | `clecta-data/` beside an ordinary binary, beside the `.app` for a bundled one, and no walk-up for a folder that merely looks like a bundle |
| `settings.rs` | a round trip, four kinds of broken file reading as defaults, a missing field keeping its default, one bad value falling back without taking the good ones with it |
| `ui/mod.rs` | eliding, sizes, the calendar (including a leap day), the clock |

Two things the suite cannot reach, both of which need a window a person can click:

- **The save fires when the window closes**, so killing the process or force-quitting
  loses the last changes. Worth a manual check that ⌘Q saves too — it may go around the
  close request (PLAN §11). The *folder* half of the portability check is confirmed both
  ways on macOS: a bare binary and a bundled `Clecta.app` each create `clecta-data/`
  beside themselves, and nothing appears in `~/Library`. That `settings.json` lands in it
  needs a window someone can shut.
- **The drop gestures themselves.** The policy and the targeting are pure and tested; the
  pointer bookkeeping around them — the ring lighting on one player, the drag disarming
  when it is let go over nothing, a release over a button not leaving it armed — is
  wiring that only a real drag exercises (PLAN §10).
