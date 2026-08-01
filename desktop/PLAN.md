# clecta (desktop) — Design Plan

A **native, portable two-player audio mixer** written in Rust. One window, two sections:

- **Top** — **Player 1**, a **mixer** strip, **Player 2**. Each player loads one media
  file (audio *or* video — **audio track only** in v1) and can play / pause / stop. The
  mixer between them carries a **volume fader per player** and a **crossfader**.
- **Bottom** — a **files view** of one selected folder, with a **foldable folder-tree
  pane on the right** for navigation (the same shape as cmote's remote browser strip,
  §18/§19 there — local filesystem here).

**Drag a single media file onto a player** and it loads there, ready to play.

This is a **learning project**, the second after [cmote](../../cmote). Same contract: the
code is meant to be read as much as run, so this plan is didactic — it explains *why*
each choice was made, and every deliberate shortcut carries a `ponytail:` note so
"simple" reads as intent, not ignorance.

Status: **planning — v0. Nothing is implemented.** Every decision is locked; §15 is the
log. The plan is buildable as written.

---

## 1. Locked decisions

| Area | Decision |
|---|---|
| Language | Rust, stable channel (verified: 1.97.1 on `x86_64-apple-darwin`) |
| Edition | 2024 (same as cmote) |
| Crate root | **`desktop/`** — this folder is the whole Rust project, so a later `mobile/` or `web/` sibling does not have to move it |
| Crate shape | **One binary crate**, not a workspace. Small cohesive modules (<800 lines each) |
| GUI | **iced 0.14** — pure Rust, Elm architecture (state / `Message` / `update` / `view`). Same as cmote: the pattern is already learned, so the *audio* is the new lesson. Re-examined against egui and the rest in §16, with the trigger that would overturn it |
| Audio | **rodio 0.22** over cpal — one device sink, its `Mixer`, one `rodio::Player` per deck (§4) |
| Decoding | rodio's **symphonia** backend: flac / mp3 / mp4+aac / vorbis / wav by default, plus **`symphonia-mkv`**. Pure Rust, no C toolchain |
| Crossfader | **Switchable curve** — constant-power (default) or linear, selectable in the mixer strip (§8) |
| Async runtime | **None.** cmote needed tokio for SSH; clecta has no network. Audio runs on cpal's own callback thread inside rodio, and directory reads go through `iced::Task` on a blocking thread |
| Filesystem | **`std::fs`** — `read_dir`, `metadata`. No `walkdir`, no `notify`: nothing here walks recursively or watches |
| File picker | **`rfd`** — native open-folder dialog, same crate cmote uses |
| Errors | **`anyhow`** at the app boundary; typed `thiserror` enums deferred until a module becomes a real API (same call as cmote) |
| Naming | **Idiomatic Rust** — `snake_case`, `SCREAMING_SNAKE` consts, no Hungarian prefixes. Same reasoning as cmote §15: the org's C-family rules fight `rustc`'s own lints. Tabs are honoured by `hard_tabs = true` in `rustfmt.toml` |
| Targets | **`x86_64-pc-windows-msvc`** (Windows 11) **and `x86_64-apple-darwin`** (macOS Sequoia, Intel) — both first-class, dual CI, same pair as cmote |
| Distribution | **Portable, as a hard requirement**: one self-contained binary, no installer, no registry / `plist` writes, and **every file clecta writes lives in `clecta-data/` beside the executable** (§11) |
| Persistence | One **`clecta-data/settings.json`**: crossfader curve, both faders, the crossfader, last folder, window size. Corrupt file → defaults, never a crash |
| Drop targeting | **In-app drag is aimed** (we own the pointer); an **OS drag lands on the idle player** — no track, else not playing, else Player 1 — and the hover ring shows which (§10) |

---

## 2. Why these choices (didactic)

- **iced again, on purpose.** cmote taught the Elm loop; repeating it here means the
  new material is the audio graph and the real-time thread, not the widget tree. The
  one genuinely new iced lesson is **animated state that no message drives** — a
  playhead moves because time passed, not because the user clicked — which is a
  `Subscription` on a timer, and §7 is where that lands.
- **rodio over hand-rolling cpal + symphonia (decided).** rodio *is* cpal + symphonia
  with the mixing, resampling and queueing already written. Two players sharing one
  mixer is its central example. Hand-rolling means owning a lock-free ring buffer, a
  resampler and the "never allocate on the audio thread" discipline — a deeper lesson,
  and a month of work before the first sound. clecta's lesson is the *app*: the audio
  graph, the polling bridge, the layout. `ponytail:` if the real-time layer later
  becomes the thing worth learning, rodio's `Source` trait is the seam to drop under —
  it is one trait, not an architecture.
- **No tokio, deliberately.** This is the clearest thing clecta *deletes* relative to
  cmote. There is no socket, so there is no async runtime, so there is no
  `Send`/`Sync`-across-a-runtime puzzle. The GUI ↔ audio boundary is instead a *thread
  you never see* (cpal's callback) plus **polling** (§4) — a different and simpler
  concurrency shape, worth recognising as such rather than reaching for tokio by habit.
- **No ffmpeg.** Video containers tempt one toward `ffmpeg-sys`, which is a large C
  dependency, a build-toolchain requirement on both targets, and an LGPL/GPL licensing
  question. symphonia reads the **audio track out of an MP4/MKV container** in pure
  Rust, which is exactly and only what v1 needs. When actual video *rendering* is
  wanted, that is the moment to pay for ffmpeg — not before (§14).
- **The crossfader is not a DSP stage.** It is arithmetic that collapses into the
  per-player gain each player already has (§8). Recognising that is worth more than
  writing a mixer node: the laziest correct signal graph here has *no extra nodes*.

---

## 3. Tech stack + versions (mid-2026)

| Crate | Version | Purpose | Notes |
|---|---|---|---|
| `iced` | 0.14.0 | GUI (Elm architecture, `Task`, `Subscription`) | Pure Rust, wgpu/tiny-skia renderer. `advanced` **not** needed in v1 — no custom widget yet (a waveform would be the first, §14). `lazy` if the files pane needs it (§9) |
| `rodio` | 0.22.2 | Audio output, mixing, decoding | Wraps `cpal` (device) + `symphonia` (decode). Default features give flac/mp3/mp4-aac/vorbis/wav; add **`symphonia-mkv`** for `.mkv`/`.webm` |
| `cpal` | 0.18 (via rodio) | OS audio device + callback thread | Not a direct dependency; named here because it owns the real-time thread |
| `symphonia` | 0.6 (via rodio) | Container demux + codec decode | Pure Rust. MPL-2.0 — permissive, file-level copyleft; fine to link, and `cargo deny`'s licence allow-list must include it |
| `rfd` | 0.17 | Native folder-open dialog | `NSOpenPanel` / Win32. Same crate as cmote |
| `serde` / `serde_json` | 1.0 | `clecta-data/settings.json` (§11) | `derive` on one small struct. A corrupt file is logged and treated as absent |
| `anyhow` | 1.0 | App-level error handling | Context-rich errors, `?` everywhere |

No `dirs` / `directories` crate: `std::env::current_exe()` plus a write-probe is the
whole portable-path rule (§11), the same call cmote's `paths.rs` makes.

Caret (`^`) requirements in `Cargo.toml`, **`Cargo.lock` committed** — the idiomatic
reproducibility guarantee for a binary crate, same call as cmote.

### What each format actually gives us

| Extension | Container | Works in v1? |
|---|---|---|
| `.mp3` `.flac` `.wav` `.ogg` | native audio | yes, default features |
| `.m4a` `.mp4` `.m4v` | ISO-BMFF | yes, `mp4` default feature (isomp4 + aac) |
| `.mkv` `.webm` | Matroska | yes, **once `symphonia-mkv` is enabled** |
| `.mov` | QuickTime | ISO-BMFF-adjacent; **verify at implementation time**, do not promise it |
| `.avi` `.wmv` | RIFF/ASF | **no.** Out of scope, and not worth ffmpeg |

`ponytail:` extension-sniffing decides only which files the browser *offers*; the actual
answer is whatever `Decoder::builder().build()` returns, so a rejected file is a notice
line (§7), never a crash.

---

## 4. Architecture — where the threads are

Three places code runs. Only two of them are ours.

```
   GUI thread (iced event loop)                  cpal callback thread (rodio owns it)
 ┌──────────────────────────────┐              ┌────────────────────────────────────┐
 │ App state                    │              │  pull samples from the Mixer        │
 │   update(Message) / view()   │              │    ├── Player 1 (gain g1)           │
 │                              │  set_volume  │    └── Player 2 (gain g2)           │
 │  Deck { player: rodio::Player┼──play/pause─►│  sum, write to the device buffer    │
 │         gain, state, pos }   │  try_seek    │                                     │
 │                              │              │  REAL-TIME: no alloc, no lock, no   │
 │  Subscription: 20 Hz tick ───┼──get_pos()──►│  I/O. rodio guarantees this for us. │
 └──────────────────────────────┘   empty()    └────────────────────────────────────┘
                │
                │ Task::perform (blocking pool)
                ▼
   ┌────────────────────────────┐
   │ std::fs::read_dir          │  one directory, off the GUI thread
   └────────────────────────────┘
```

- **Nothing pushes from the audio side.** `rodio::Player` is a *handle*: its methods are
  atomics and a lock the callback never blocks on. There is no event channel back, so
  the GUI **polls** — `get_pos()` for the playhead and `empty()` for end-of-track — on
  one timer subscription. This is the whole GUI↔audio bridge, and it is why there is no
  `bridge.rs` here to match cmote's.
- **The tick only runs while something plays.** `Subscription::none()` when both players
  are stopped or paused, otherwise `time::every(50ms)`. A UI that redraws 20×/s while
  idle is a laptop-battery bug, and iced makes the fix one `if`.
- **Directory reads are the only blocking work.** `read_dir` on a cold network mount can
  take seconds, so it goes through `Task::perform`, and the pane shows its previous
  contents until the new listing lands (cmote's "never flash empty" rule, §18 there).

---

## 5. Repo layout

```
clecta/
├── LICENSE
├── .gitignore
└── desktop/                     ← the Rust project root
    ├── Cargo.toml
    ├── Cargo.lock               (committed — reproducible, auditable builds)
    ├── PLAN.md                  this file
    ├── README.md
    ├── rustfmt.toml             edition 2024 + hard_tabs
    └── src/
        ├── main.rs      entry; #![windows_subsystem = "windows"] (inert on macOS); iced::run
        ├── app.rs       the iced App: State / Message / update / view; owns both decks + the browser
        ├── audio.rs     rodio wiring: the device sink, its Mixer, one Player per deck; load / play / pause / stop / seek
        ├── deck.rs      one deck's model: transport state machine, loaded track, position, duration (§7)
        ├── mixer.rs     PURE gain math: fader + crossfader + Curve → the two gains. No rodio, no iced (§8)
        ├── browser.rs   the files pane's model: one directory, its entries, media categories (§9)
        ├── tree.rs      the folder tree's model: nodes, expansion, path arithmetic (§9)
        ├── fsio.rs      std::fs reads run off the GUI thread: list a dir, list its subfolders, the roots (§9)
        ├── paths.rs     clecta-data/ beside the app if writable, else the per-user dir; the .app walk-up (§11)
        ├── settings.rs  load/save clecta-data/settings.json; a corrupt file reads as defaults (§11)
        └── ui/
            ├── mod.rs       shared view helpers (elide_middle, the section splitters)
            ├── deck.rs      one player's panel: title, transport buttons, time, drop ring
            ├── mixer.rs     the two faders and the crossfader
            ├── browser.rs   the files pane and its rows
            └── tree.rs      the folder tree pane, its splitter and its fold button
```

**Naming collision, resolved up front:** rodio's playback handle is called `Player`. The
UI calls the two halves **"Player 1" / "Player 2"** because that is the user's word for
them, but the *type* is `deck::Deck` — so `deck.player: rodio::Player` reads
unambiguously and no `use` has to be renamed.

`ponytail:` `PLAN.md` lives in `desktop/` because it plans the desktop app. If a second
front end ever appears, the shared product decisions move up to a root `PLAN.md` and
this one keeps the desktop-only ones.

---

## 6. The window

```
┌───────────────────────────────────────────────────────────────────────────┐
│  ┌─────────────────┐  ┌───────────────┐  ┌─────────────────┐              │
│  │   PLAYER 1      │  │    MIXER      │  │   PLAYER 2      │   §7, §8     │
│  │  track name     │  │  ▮ fader 1    │  │  track name     │              │
│  │  00:42 / 03:15  │  │  ▮ fader 2    │  │  ── no track ── │              │
│  │  ▶  ⏸  ⏹        │  │  ◄─ X-fade ─► │  │  ▶  ⏸  ⏹        │              │
│  └─────────────────┘  └───────────────┘  └─────────────────┘              │
├════════════════════════ draggable splitter ═══════════════════════════════┤
│  ┌──────────────────────────────────────────┐ ║ ┌───────────────────────┐ │
│  │ FILES — /Users/cme/Music/set             │ ║ │ ▾ /                   │ │
│  │  ♪ 01 opener.flac        8.2 MB  2026-… │ ║ │   ▾ Users             │ │
│  │  ♪ 02 build.mp3          6.1 MB  2026-… │ ║ │     ▾ cme             │ │
│  │  ▶ 03 clip.mp4          41.0 MB  2026-… │ ║ │       ▸ Documents     │ │
│  │    notes.txt             1.1 KB  2026-… │ ║ │       ▾ Music     ◄── │ │
│  └──────────────────────────────────────────┘ ║ └───────────────────────┘ │
│                                          §9   ▲ splitter    [◧ fold]      │
└───────────────────────────────────────────────────────────────────────────┘
```

- **Two sections, one draggable horizontal splitter.** The top section has a sensible
  minimum height (the transport must never be clipped); the drag is clamped so neither
  section can be squeezed to nothing — cmote's 60 % clamp rule, §18 there, for the same
  reason: a splitter with no ceiling leaves the user dragging their way back out.
- **The tree is a fixed-width column at the right of the bottom section**, with its own
  vertical splitter and a **fold button**. Folding it gives the files pane the full
  width; the pane never disappears, because a browser with no file list is not a
  browser. (cmote folds pane and tree together because the terminal owns the rest of
  the window; here the bottom section is the browser, so only the tree folds.)
- **Colours are set in pairs.** Every surface sets background *and* foreground together,
  so contrast never depends on the system light/dark preference — the trap cmote
  documents in §14 there.

### The splitters: try `pane_grid` before hand-rolling (§16)

cmote hand-rolled its two splitters — the drag state, the clamp, the grab/drag/release
messages and the width arithmetic are spread across `explorer.rs`, `files.rs`,
`ui/terminal.rs`, `ui/explorer.rs`, `ui/files.rs` and `app.rs`. **iced 0.14 ships
`widget::pane_grid`**, which is draggable splits as a built-in.

So the order of attack is: **`pane_grid` first**, and only fall back to cmote's approach
if it does not fit. The reason it might not: `pane_grid` is built for *user-managed
dynamic* layouts — it owns a tree of panes and brings drag-to-reorder and maximise with
it — whereas clecta wants a **fixed** two-section layout where the only variables are two
split ratios and whether the tree is folded. Bending it into that could cost more than the
~150 lines it saves, and folding the tree is "close a pane" in its model rather than a
`bool`.

`ponytail:` this is a build-time evaluation, not a decision to make on paper. Spike
`pane_grid` in the scaffold; if the fixed layout fights it, cmote's splitter code is a
known-good fallback sitting one directory over. Either way, **do not write a third
splitter implementation from scratch.**

---

## 7. A player (`deck.rs`, `audio.rs`, `ui/deck.rs`)

### The transport is a three-state machine

```
              load(path)                  Play                Pause
   Empty ─────────────────► Stopped ◄──┬──────────► Playing ◄────────► Paused
                              ▲        │                │                │
                              └────────┴── Stop ────────┴────────────────┘
                                   (position ← 0)
```

- **Stop means reset**, as asked: `try_seek(Duration::ZERO)` then `pause()`. It is *not*
  rodio's `Player::stop()`, which drops the queued source and would force a re-decode.
  `ponytail:` if `try_seek` returns `SeekError` (some streams cannot seek), fall back to
  re-opening the file and re-appending — correct either way, fast in the common case.
- **Loading needs a seekable decoder**, which is not the default. The builder form is
  the one to use, because `with_byte_len` is also what makes `total_duration()` answer:

  ```rust
  let file = File::open(&path)?;
  let len  = file.metadata()?.len();
  let src  = Decoder::builder()
      .with_data(file)
      .with_byte_len(len)
      .with_seekable(true)
      .build()?;
  ```

- **Read the duration *before* `append`.** `append(src)` consumes the source, so
  `src.total_duration()` must be captured first and stored on the `Deck`. Missing this
  is the classic first bug here: the UI ends up with a playhead and no track length.
- **Load replaces.** Loading into a busy player calls `clear()` first, then appends and
  leaves the new track **paused at 0** — loading is not playing. A file that fails to
  decode leaves the previous track alone and raises a notice line; it must never wipe a
  loaded track to show an error.
- **End of track** is `player.empty()` going true on the tick — transition to `Stopped`.
  There is no callback for this; polling is the mechanism (§4).
- **The playhead is `get_pos()`**, read on the same tick. It is the *decoder's* position,
  which leads the speaker by the device buffer (a few ms). Irrelevant for a readout;
  it would matter for beat-matching, which is not v1 (§14).

---

## 8. The mixer (`mixer.rs`)

The whole of it is one pure function, and that is the point.

```rust
/// The crossfader's shape. Neither curve is right for everything, which is why
/// hardware mixers ship the knob (§8).
pub enum Curve {
	/// g1 = cos(x·π/2), g2 = sin(x·π/2). g1² + g2² = 1 — loudness holds flat.
	/// Right for two DIFFERENT tracks. Both -3 dB at the centre. The default.
	Power,
	/// g1 = 1-x, g2 = x. g1 + g2 = 1 — amplitude holds flat. Right for the SAME
	/// beat-matched track on both players. Both -6 dB at the centre.
	Linear,
}

/// Volume fader (0..=1) and crossfader (0 = player 1 alone, 1 = player 2 alone)
/// collapsed into the linear gain each rodio Player is set to.
pub fn gains(fader1: f32, fader2: f32, crossfader: f32, curve: Curve) -> (f32, f32)
```

Three decisions inside it:

1. **The crossfader is arithmetic, not a node.** `player.set_volume(g)` already exists
   per player, so the crossfader multiplies into it. No mixer node, no extra source
   wrapper, no second gain stage to keep in sync.
2. **The curve is switchable** (decided), a two-way selector in the mixer strip,
   defaulting to `Power`. It is one `match` inside `gains` and one field in the state, so
   the cost is genuinely two lines — and the *reason* the two curves differ (uncorrelated
   signals sum by power, correlated ones by amplitude) is exactly the kind of thing this
   project exists to make visible. The choice persists in `settings.json` (§11).
3. **The volume fader is not linear.** A linear fader spends its top half on changes the
   ear barely hears and its bottom inch on everything else. `gain = fader³` approximates
   a proper dB taper closely enough (fader 0.5 → −18 dB) in one operation, and
   `fader = 0` is exactly silent. `ponytail:` cubic, not a true dB curve; upgrade path is
   `10^((fader − 1) · 3)` for a −60 dB..0 dB fader if the feel is wrong.

**This is where the one required test lives** (§12): each curve at both ends and at the
centre, each curve's defining identity at the midpoint (`g1² + g2² = 1` for `Power`,
`g1 + g2 = 1` for `Linear`), and the invariant that a fader at 0 is silent under either
curve no matter where the crossfader sits.

---

## 9. The browser (`browser.rs`, `tree.rs`, `fsio.rs`, `ui/`)

Same three-way split cmote uses for its remote browser — pure model, pure view,
I/O module — which is what keeps the interesting rules (path arithmetic, what collapsing
does, which files are offered) unit-testable with no filesystem.

### The folder tree (`tree.rs`)

- **Folders only, lazily listed.** Files are the pane's job. `expand()` *returns the
  paths that still need reading* rather than reading them, and `app` turns each into one
  `Task` — so an unreadable or enormous folder cannot stall the GUI.
- **`children: Option<Vec<PathBuf>>`.** `None` = never listed, `Some(vec![])` = listed
  and empty. Collapsing that distinction is what makes a permission-denied folder
  re-request itself on every redraw.
- **Collapse takes the subtree with it**, but keeps the cached listings, so re-opening
  draws instantly. Re-opening still re-lists in the background — a folder the user
  deliberately opens should show what is there now, not what was there before.
- **Roots are platform-shaped.** `/` on macOS; on Windows the tree has **one root per
  drive letter** (a `C:\` / `D:\` list, since there is no single filesystem root). This
  is the one place the two targets genuinely differ, so it is `fsio::roots()` and
  nowhere else.
- **Hidden entries are a filter, not a fetch** — always listed, drawn behind a `.*`
  toggle. Unlike cmote (a server's `.ssh`/`.config` are usually why the tree was
  opened), a *local* browser for media files should default them **off**.

### The files pane (`browser.rs`)

- **One directory, read whole.** No batching: `read_dir` on a local disk returns in
  milliseconds where cmote's SFTP round trips did not.
- **Rows, not icons** — name, size, modified date, and a leading glyph marking *audio* /
  *video* / *other*. A media browser is scanned by name and length, not by thumbnail.
- **Use `widget::table`.** iced 0.14 ships one, and three aligned columns with a header is
  exactly its job — cmote built its rows by hand because it needed an icon *grid* with
  rubber-band selection, which clecta does not. `ponytail:` if `table` turns out not to
  take a per-row click / `mouse_area` (needed for the drag-to-player gesture, §10), fall
  back to `scrollable(column(rows))`; verify at scaffold time.
- **Everything is listed, media is distinguished.** Hiding non-media would hide the
  `.cue` and the artwork that tell the user they are in the right folder. Only a media
  row is loadable; the rest are inert.
- **Sort: folders never appear here** (that is the tree) — files sort by name,
  case-insensitively, natural-numeric so `track2` precedes `track10`.
- **Choosing a folder**: click it in the tree, or the **Open folder…** button (`rfd`).
  Both funnel through one `select_folder(path)`.
- `ponytail:` **no list virtualization, and iced has none to offer** (§16). A
  `scrollable` builds every row on every `view()`, so a 5 000-entry folder builds 5 000
  widgets per frame — and at the 20 Hz playing tick (§4) that is 100 000 widgets a second
  for rows nobody can see. A music folder is normally tens to hundreds of files, so this
  is a real ceiling in the right place. Upgrade paths in order of cost: `widget::lazy` to
  cache the built rows between changes; then hand-rolled virtualization (measure the
  scrollable, build only the visible slice); then §16's framework question.

---

## 10. Loading a file into a player

### What cmote actually built, exactly

Worth stating precisely, because the shape of it is easy to misread as more than it was.
`src/app.rs:4759`:

```rust
fn file_drop_events() -> iced::Subscription<Message> {
	iced::event::listen_with(|event, _status, _window| match event {
		iced::Event::Window(iced::window::Event::FileHovered(_))  => Some(Message::FileHovered),
		iced::Event::Window(iced::window::Event::FilesHoveredLeft) => Some(Message::FileDropLeft),
		iced::Event::Window(iced::window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
		_ => None,
	})
}
```

…and the state it feeds is a **single global `bool`** (`app.rs:908`):

```rust
Message::FileHovered  => self.drop_hover = self.terminal.is_some(),
Message::FileDropLeft => self.drop_hover = false,
```

`drop_hover` is passed down to the view, which draws a green border on the files-pane
region. **That is the whole thing.** The pane is not a spatial drop target: the drop is a
*window* event, and the pane's own `current directory` is used as the destination because
it is a piece of app state that happens to be the only sensible destination in cmote. The
ring is drawn on the pane not because the pane was hit-tested but because that is where
the file is going regardless of where on the window it was released. cmote's own §29 says
so in its `ponytail:` note: *"the drop ring lights on any hover over the window while
connected, since the event has no position to test against the pane's bounds."*

So the honest summary: **cmote never aimed a drop. It had one destination, so it never
had to.** The mechanism ports to clecta perfectly; the *targeting* does not port, because
clecta has two destinations.

### Why aiming is not merely awkward but currently impossible in iced

Two layers, both verified against the docs rather than assumed:

- **iced 0.14** — `window::Event` has exactly `FileHovered(PathBuf)`,
  `FileDropped(PathBuf)`, `FilesHoveredLeft`. No coordinate field on any of them. (Every
  *other* variant that has a position carries one — `Opened { position, .. }`, `Moved(Point)`
  — so this is a gap, not a convention.)
- **winit** (iced's backend) — `WindowEvent::HoveredFile(PathBuf)` /
  `DroppedFile(PathBuf)` / `HoveredFileCancelled`. Same three, same missing position.

And the fallback trick does not work either: **the OS does not send cursor-move events to
the window during a native drag** — the pointer belongs to the drag session. So the
`mouse_area` `on_move` pointer-tracking that cmote uses to place a context menu under the
cursor (§18/§10 there) receives *nothing* while a file is being dragged in. There is no
stale last-known position worth trusting; it would be wherever the mouse was before the
drag started, which is very often over the files pane the drag *originated* from.

The information exists at the bottom of the stack and is thrown away on the way up: macOS
gives `draggingLocation` on `draggingUpdated:`, Windows gives `pt` to
`IDropTarget::DragOver`. winit discards both. **So the upgrade path is real but upstream**
(a winit PR adding the position, then iced surfacing it) — not something clecta can work
around in its own code.

### What this means for clecta

| Gesture | Position known? | Targeting |
|---|---|---|
| **In-app** — drag a row out of the files pane onto a player | **yes** — the pointer is ours the whole time, tracked by `mouse_area` exactly as cmote's grid and tree do | the player under the cursor highlights and receives the drop. Fully aimable, no compromise |
| **OS** — drag a file in from Finder / Explorer | **no**, and not fixable locally (above) | **the idle player wins** — a rule derived from state, below |

Note the in-app drag is *more* capable than cmote's OS drop, not less: because clecta owns
the pointer, the in-app gesture gets the per-player highlight and true aiming that cmote's
upload drop never had.

### The OS-drop rule (decided): the idle player wins

```rust
/// Which player an OS drop lands on, when the event carries no position.
/// Derived from state — no armed flag, no dialog, nothing to click.
fn os_drop_target(p1: &Deck, p2: &Deck) -> DeckId {
	// 1. an empty player is obviously where a new track goes
	// 2. else the one not playing — never interrupt audible playback
	// 3. else Player 1, so the rule is always total
}
```

Three properties make this the right shape rather than merely the smallest:

- **Nothing new to hold.** No armed field, no dialog, no persisted choice — so nothing to
  get out of sync with what the user sees, and nothing to migrate in `settings.json`.
- **It is self-teaching, because the ring already exists.** On `FileHovered` we light the
  ring on exactly the player `os_drop_target` names, so the answer is *visible before the
  release*. That turns a derived rule from "surprising" into "shown" — which is the whole
  reason a derived rule is acceptable here and would not be if the ring were on both.
- **It will not cut off a playing track**, which is the only genuinely destructive outcome
  a drop can have. Rule 2 exists for exactly that.

`ponytail:` when both players are playing, Player 1 loses its track. There is no
non-arbitrary answer at that point and the ring says which it will be, so a confirm
dialog would be a prompt for a case the user can already see coming. Upgrade path: if
that bites, `os_drop_target` is one pure function and one test away from any other tie-break.

`drop_outcome` stays a pure function free of `self`, exactly as cmote pulled it, and
`os_drop_target` sits beside it — so both the policy and the targeting are unit-tested
with no window and no audio device (§12).

Everything else about the drop is shared, and is decided:

- **One file.** A multi-file drop takes the first and says so in the notice line; there
  are two players and no queue, so "load them all" has no meaning yet.
- **A folder dropped is declined** with a notice, not silently ignored.
- **A non-media file dropped is declined** by extension first, then by whatever
  `Decoder::build()` actually says (§3).
- **The drop ring is green**, distinct from a focus ring, and it lights on **exactly one**
  player: the one under the cursor for an in-app drag, the one `os_drop_target` names for
  an OS drag. A ring on both would promise aiming the app cannot deliver — cmote's ring is
  honest for the same reason, it means "this is where the file goes", which is true.
- **A third door exists regardless**: the files pane's context menu offers **Load into
  Player 1 / Player 2**, and each player has a **Load…** button. Drag-and-drop is the
  convenient path, never the only one — a gesture that fails silently on a trackpad is
  not an interface.

---

## 11. Portability — a hard requirement (`paths.rs`)

**Copy the app anywhere, run it, leave no trace outside its own folder.** Not an
aesthetic: it is the requirement.

- **Every file clecta writes goes in `clecta-data/` beside the app.** One file in v1 —
  `settings.json` (curve, faders, crossfader, last folder, window size). No registry
  keys, no `plist` writes, no `~/Library` unless the portable spot is genuinely
  unwritable.
- **Resolution order** (`paths.rs`, plain `std`, no `dirs` crate — cmote's rule):
  1. `clecta-data/` beside the executable, if a create-dir + write-probe succeeds — true
     portable mode, USB stick included.
  2. else `%LOCALAPPDATA%\clecta\` (Windows) / `~/Library/Application Support/clecta/`
     (macOS) — only for an app dropped somewhere read-only (`Program Files`,
     `/Applications`).
- **The macOS `.app` wrinkle, and clecta fixes what cmote left.** Inside a bundle,
  `current_exe()` is `Clecta.app/Contents/MacOS/clecta`, so a naive "beside the exe" puts
  `clecta-data/` **inside the bundle** — which survives a copy but is wiped by any app
  replacement and invalidates a code signature. So: **when the exe path ends in
  `.app/Contents/MacOS/`, walk up three levels and put `clecta-data/` beside the `.app`**,
  which is what "next to it on the filesystem" actually means to a user looking at
  Finder. `ponytail:` a path-suffix test, not a bundle API. It is wrong only for a binary
  a user has manually placed in a directory tree that mimics `.app/Contents/MacOS`, which
  is not a case worth code.
- **A corrupt or unreadable `settings.json` yields defaults**, logged, never a crash or a
  refusal to start. A settings file must never be able to brick the app.
- **One binary, no installer.** Same release profile as cmote (`lto`,
  `codegen-units = 1`, `strip`, `panic = "abort"`). `#![windows_subsystem = "windows"]`
  in `main.rs` so no console window pops on Windows (inert on macOS).
- **`bundle-macos.sh`**, cmote's script adapted: wrap the release binary in a minimal
  `Clecta.app` (`Contents/MacOS/` + `Info.plist`) so Finder launches it as a GUI app
  instead of through Terminal.
- **No C toolchain**, and this too is real rather than aesthetic: cpal binds CoreAudio /
  WASAPI through `objc2` / `windows-sys`, both pure Rust, and symphonia is pure Rust.
  Nothing needs NASM or a vendored C library — the property cmote had to fight for with
  `ring` (§2 there) comes free here.
- **Building the shipped Intel binary on an Apple Silicon Mac**: add
  `--target x86_64-apple-darwin` (the Xcode CLT SDK carries both slices; Rosetta 2 only to
  *run* it locally). Same split CI uses.
- **Audio device changes are a real failure mode.** Unplugging the interface mid-set
  kills the cpal stream. v1 surfaces it as a notice line and a **Reconnect audio**
  button rather than pretending; auto-recovery is §14.

---

## 12. Testing

Rust's built-in `#[test]` / `#[cfg(test)]`, AAA pattern, no framework — same as cmote.
Pure logic is tested; anything needing a device or a real folder is manual.

- **`mixer.rs`** — the required one. Both curves at both ends and the centre, each curve's
  defining identity at the midpoint (`g1² + g2² = 1` / `g1 + g2 = 1`), and the
  fader-at-zero invariant under either curve.
- **`paths.rs`** — the `.app/Contents/MacOS` walk-up returns the directory beside the
  bundle, and an ordinary exe path returns the directory beside the binary. Pure string /
  path arithmetic, so no bundle needs to exist to test it.
- **`settings.rs`** — a round trip, and every broken input (absent, empty, truncated JSON,
  wrong types, out-of-range fader) reading as defaults rather than an error.
- **`tree.rs`** — expand returns exactly the unlisted paths; collapse takes the subtree;
  `None` vs `Some(vec![])` survives a collapse/expand round trip; path arithmetic on
  both path dialects.
- **`browser.rs`** — extension → category (audio / video / other), the natural-numeric
  sort, the hidden filter.
- **`deck.rs`** — the transport state machine as a pure `transition(state, event)`, so
  every edge is checked with no audio device in the room.
- **Drop policy** — `drop_outcome(...) -> DropOutcome` pulled free of `self`, exactly as
  cmote pulled `drop_outcome` and `plan_uploads`, and tested for the folder / non-media /
  multi-file cases. **`os_drop_target`** gets its own table: empty+empty → 1,
  loaded+empty → 2, playing+paused → the paused one, both playing → 1, and the invariant
  that it never names a playing player while an idle one exists.
- **Manual smoke test**, documented in the README: load both players, play both, sweep
  the crossfader on **both curves**, stop and re-play, load a `.mp4`, fold the tree, drag a
  row to a player, drag a file in from Finder, pull the audio device — and the
  **portability check**: copy the binary to an empty folder, run it, confirm
  `clecta-data/settings.json` appears *there* and nothing appears in `~/Library` or the
  registry.

**CI** mirrors cmote's `.github/workflows/ci.yml` — `cargo fmt --check`, `cargo clippy
-D warnings`, `cargo test`, `cargo deny` + `cargo audit` — with `working-directory:
desktop` and symphonia's **MPL-2.0** added to the licence allow-list. Set up once the
crate exists, not before.

---

## 13. Coding conventions

**Idiomatic Rust**, locked, for the reasons cmote's §15 records in full: the
organisation's C-family rules (`kConst`, `vLocal`, `inParam`, `fField`, Whitesmith
braces) trigger `rustc`'s own `non_upper_case_globals` / `non_snake_case` lints, and
suppressing those hides real ones. `rustfmt` defaults with `hard_tabs = true` honours
the tab-indent rule while leaving every other formatting question to the tool. Clippy
runs clean at `-D warnings`.

Every deliberate shortcut gets a `ponytail:` comment naming the ceiling and the upgrade
path, so `/ponytail-debt` can harvest them later.

---

## 14. Deferred (with upgrade paths)

- **Video rendering.** v1 decodes the audio track only. Upgrade path: an ffmpeg binding
  and a texture in a custom iced widget — a large C dependency and a licensing question,
  worth paying only when the picture is actually the feature.
- **Waveform / scrubbing.** Needs the whole file decoded to a peak array; the display is
  the first thing needing `iced` `advanced` and a custom `Widget`. Natural v2.
- **Cue points, loops, tempo / pitch, BPM detection.** Real DJ features. Tempo needs a
  time-stretch stage rodio does not have (`rubato` or a phase vocoder).
- **Headphone cue / pre-listen.** Needs a *second* output device and a second mixer —
  the point at which the "one shared stream" decision in §4 has to be revisited.
- **A queue / playlist per player.** rodio's `Player` is already a queue; v1 just never
  appends more than one source.
- **File watching.** `notify` so the pane updates when a folder changes underneath it.
  v1 has a Refresh button and F5, which is what cmote learned to ship first.
- **Recursive / multi-file drops**, and drag-*out* to the desktop (iced cannot originate
  an OS drag at all — cmote §29 there).
- **Positional OS drops** — blocked upstream in winit, not in clecta (§10). If winit ever
  surfaces `draggingLocation` / `IDropTarget`'s `pt`, `os_drop_target` demotes to the
  fallback for "released somewhere that is not a player" and the aiming starts working
  with no other change.
- **Auto-recovery from a device change**, rather than the notice line in §11.

---

## 15. Decision log

| # | Question | Decision | Landed in |
|---|---|---|---|
| Q1 | Audio engine | **rodio 0.22** — cpal + symphonia with the mixing already written; the real-time layer is a `Source` away if it later becomes the lesson | §1, §2, §3 |
| Q2 | Crossfader curve | **Switchable**, `Power` (constant-power) default, `Linear` for the same beat-matched track on both players. One `match`, one state field, persisted | §1, §8, §12 |
| Q3 | OS drop targeting | **The idle player wins** — no track, else not playing, else Player 1 — with the hover ring showing which. Derived from state: no armed flag, no dialog. In-app drags are aimed normally | §1, §10, §12 |
| Q4 | Targets + portability | **Windows 11 + macOS Sequoia Intel**, dual CI, and **portability as a hard requirement**: everything written goes to `clecta-data/` beside the app, including beside the `.app` rather than inside it | §1, §9, §11, §12 |

Nothing is open. The plan is buildable as written; the first commit is the scaffold
(`Cargo.toml`, `rustfmt.toml`, `main.rs`, and `mixer.rs` with its test).

---

## 16. The UI framework, re-examined

iced was taken from cmote rather than chosen for clecta, so it is worth writing down why
it survives the second look — and, more usefully, what would overturn it.

### Why it holds

The question is not "best Rust GUI toolkit"; it is "best for a project whose *new*
material is audio". Changing framework makes the UI the lesson again and discards the
part of cmote that actually transfers: it is a working reference for the four hard things
clecta needs — the OS file-drop subscription (§10), `mouse_area` pointer tracking, a
foldable panel with a clamped splitter (§6), and a custom `Widget` (the terminal grid,
which is the waveform's precedent, §14). Reading working code beats re-deriving it.

Everything clecta needs is in 0.14: `slider`, `vertical_slider`, `mouse_area`,
`scrollable`, `pane_grid` (§6), `table` (§9), `lazy`, and `Widget` behind the `advanced`
feature when the waveform lands.

### The honest counter-case: egui

One real advantage, and it lands exactly on clecta's weak spot: **`ScrollArea::show_rows`
virtualizes long lists** natively, which is the ceiling §9 documents. egui is also where
Rust's audio-tool community sits (nih-plug, egui-baseview), which would matter if clecta
ever grew a plugin build.

Against: a second framework to learn, no Elm-architecture lesson, and cmote stops being a
reference. Not worth paying for one list — **but that list is the trigger.** If the files
pane turns out to be the thing that hurts, and `lazy` plus hand-rolled virtualization do
not fix it, that is the moment to reopen this, not before.

### Ruled out, and why

| Option | Why not |
|---|---|
| **Tauri**, **Dioxus** (webview) | The UI would be JavaScript — the exact thing cmote rejected, for the exact same reason: this is a learn-Rust project |
| **Slint** | Its own DSL to learn, plus a licensing question; the DSL is the opposite of "read it as Rust" |
| **gpui** | Tied to Zed's ecosystem, weakest of the set on Windows — which is a first-class target (§1) |

`ponytail:` this section exists so the question is settled *with a trigger* rather than
re-litigated every time the UI is annoying. Annoying is not the trigger; the files pane
being measurably slow is.
