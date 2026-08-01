# clecta — desktop

Two players, a mixer between them, and a file browser below. See [PLAN.md](PLAN.md) for
the design and the decision log.

## Current state: audio spike only

**There is no window yet.** `iced` is not even a dependency at this point — the only
thing built so far is a headless spike that proves the rodio API the plan is written
against (PLAN §7, §8), plus the one module that survives it, `src/mixer.rs`.

Running the binary with no arguments therefore does nothing but print usage. That is
expected, not a broken build.

## Running the spike

It needs two audio files, one per player:

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

`src/main.rs` is throwaway and gets deleted when the real `main.rs` lands.

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
