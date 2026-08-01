# clecta — desktop

Two players, a mixer between them, and a file browser below. See [PLAN.md](PLAN.md) for
the design and the decision log.

## Current state: spikes only

**The app does not exist yet.** What is built is two throwaway spikes, each proving one
part of the plan against a compiler instead of documentation, plus the one module that
survives them both, `src/mixer.rs`.

| Binary | Proves | Plan |
|---|---|---|
| `clecta` (`src/main.rs`) | the rodio transport, headless — no GUI to blame | §7, §8 |
| `ui_spike` (`src/bin/ui_spike.rs`) | the window layout: `pane_grid` splitters, `table` rows | §6, §9 |

Both get deleted when the real `main.rs` and `app.rs` land.

## Running the audio spike

`cargo run` with no arguments prints usage and exits — that is expected, not a broken
build. It needs two audio files, one per player:

```sh
cargo run -- <file1> <file2>
```

Any of `.mp3` `.flac` `.wav` `.ogg` `.m4a` `.mp4` `.mkv` `.webm` (PLAN §3). It then:

1. opens the default output device and connects two players to its mixer,
2. loads both files with a **seekable** decoder and prints each track's duration,
3. plays both and sweeps the crossfader across its full travel on **both curves** —
   `Power` first, then `Linear`, which should sound audibly different in the middle,
4. seeks player 1 to 30 s,
5. stops both (rewind + pause, PLAN §7) and replays player 1 to prove the reset is real.

Roughly nine seconds of audio, at full volume. It exits on its own.

## Running the layout spike

```sh
cargo run --bin ui_spike
```

Opens a window with §6's layout: the three player/mixer boxes on top, a files table and
a folder tree below, two draggable splitters between them. Drag both, press **fold** to
close the tree and again to bring it back at the width it had, and click the file rows —
the line under the table reports which cell the click actually landed in, which is the
point of the exercise. Findings are at the top of `src/bin/ui_spike.rs`.

## Checks

The same four CI runs (PLAN §12), all of which must be clean:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

`cargo test` covers `mixer.rs`: both crossfader curves at both ends and the centre, each
curve's defining identity at the midpoint, the fader-at-zero invariant, and the clamping
of out-of-range values a corrupt `settings.json` could supply.
