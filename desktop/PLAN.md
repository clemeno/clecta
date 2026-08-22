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

The product's **vocabulary** lives in [`../CONTEXT.md`](../CONTEXT.md): this plan wins on
decisions, that file wins on words, and a term defined in both places is a bug here (Q49).

Status: **v0.1 complete, and the first piece of v2 landed.** Two players with a working
transport, the mixer strip, the browser (files pane + folder tree), portable persistence,
both drop gestures, `bundle-macos.sh` (§11) and the CI workflow with its supply-chain gate
(§12) are all built and green. Running v0.1 found the two things the plan got wrong:
**⌘Q never reaches the app**, so saving at exit saved nothing for the way most Mac users
quit, and the window ceiling in `settings.rs` was **above what wgpu can render**, so a
hand-edited file crashed the app at launch. Both are fixed, and §11 records what each
cost. On top of that sits the **waveform** (§14a), which is where the plan promised the
app would need its own `Widget`. The **manual smoke test** (§12) is now clean on macOS,
every item of it — and the last pass earned its keep: the seek gesture and the mixer's
preset buttons were right, and the players' section **wobbled** on a vertical resize, which
is Q16 and the second half of §6. What is left before a v0.1 tag is **one run on Windows**,
which is the shipped target that has never been executed: CI type-checks it every push, and
type-checking is not running. §15 is the log.

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
| Async runtime | **None of ours.** cmote needed tokio for SSH; clecta has no network, and no `async fn` is written here. iced still needs *an* executor for its own `Task`s and timers, so it gets `smol` — see §3 |
| Filesystem | **`std::fs`** — `read_dir`, `metadata`. No `walkdir`: nothing here walks recursively. **`notify`** was added later, once the pane earned it: the OS tells us the shown folder changed rather than the app polling or the user pressing Refresh (§9) |
| File picker | **`rfd`** — native open-folder dialog, same crate cmote uses |
| Errors | **`anyhow`** at the app boundary; typed `thiserror` enums deferred until a module becomes a real API (same call as cmote) |
| Naming | **Idiomatic Rust** — `snake_case`, `SCREAMING_SNAKE` consts, no Hungarian prefixes. Same reasoning as cmote §15: the org's C-family rules fight `rustc`'s own lints. Tabs are honoured by `hard_tabs = true` in `rustfmt.toml` |
| Targets | **`x86_64-pc-windows-msvc`** (Windows 11) **and `x86_64-apple-darwin`** (macOS Sequoia, Intel) — both first-class, dual CI, same pair as cmote |
| Distribution | **Portable, as a hard requirement**: one self-contained binary, no installer, no registry / `plist` writes, and **every file clecta writes lives in `clecta-data/` beside the executable** (§11) |
| Persistence | One **`clecta-data/settings.json`**: crossfader curve, both faders, the crossfader, last folder, window size, the players' height, and the three queues — the only unbounded thing in it (§7a). Corrupt file → defaults, never a crash |
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
| `iced` | 0.14.0 | GUI (Elm architecture, `Task`, `Subscription`) | Pure Rust, wgpu/tiny-skia renderer. **Feature `smol`** — see below. `advanced` **not** needed in v1 — no custom widget yet (a waveform would be the first, §14). `lazy` if the files pane needs it (§9) |
| `rodio` | 0.22.2 | Audio output, mixing, decoding | Wraps `cpal` (device) + `symphonia` (decode). Default features give flac/mp3/mp4-aac/vorbis/wav; add **`symphonia-mkv`** for `.mkv`/`.webm` |
| `cpal` | 0.17.3 (via rodio) | OS audio device + callback thread | Not a direct dependency; named here because it owns the real-time thread. Version confirmed by the spike, not guessed |
| `symphonia` | 0.5.5 (via rodio) | Container demux + codec decode | Pure Rust. MPL-2.0 — permissive, file-level copyleft; fine to link, and `cargo deny`'s licence allow-list must include it |
| `rfd` | 0.17.2 | Native folder-open dialog | `NSOpenPanel` / Win32. Same crate as cmote |
| `notify` | 8.2.0 | Watching the shown folder (§9) | FSEvents / `ReadDirectoryChangesW` — the OS pushes, nothing polls. CC0-1.0, already on the allow-list, and no `cc` on either target. Added after v1, on the strength of the pane being cheap to redraw |
| `redb` | 4.1.0 | The file cache (§11a) | An embedded ACID key/value store in pure Rust, one dependency of its own (`libc` — declarations, not a compilation). MIT OR Apache-2.0, both already on the allow-list, and no `cc` on either target. **SQLite is what this is instead of**: `libsqlite3-sys` compiles C, which is the one thing `deny.toml` bans |
| `serde` / `serde_json` | 1.0.229 / 1.0.151 | `clecta-data/settings.json` (§11) | `derive` on one small struct. A corrupt file is logged and treated as absent |
| `anyhow` | 1.0.104 | App-level error handling | Context-rich errors, `?` everywhere |

Every version above is the **latest stable as of 2026-08-01**, checked against the
crates.io index rather than remembered. The two that are not are transitive and pinned by
rodio: it depends on `symphonia ^0.5` and `cpal ^0.17`, so newer majors of either cannot
be taken without rodio taking them first. Nothing to do — a `[patch]` override to force
them would be a compatibility break bought for no feature we need.

**The one surprise, found at build time: iced's `smol` feature is not optional.** §1 says
there is no async runtime, and no `async fn` of ours exists — but iced's default backend
is `thread-pool`, whose `time` module is *empty*. `time::every`, which the 20 Hz playhead
tick in §4 is built on, simply does not compile without `tokio` or `smol`. `smol` is the
smaller of the two for the one function needed. It changes nothing about how the code is
written; it is a dependency of the framework, surfaced as a feature flag.

No `dirs` / `directories` crate: `std::env::current_exe()` plus a write-probe is the
whole portable-path rule (§11), the same call cmote's `paths.rs` makes.

Caret (`^`) requirements in `Cargo.toml` written to the exact patch level in use
(`rodio = "0.22.2"`, not `"0.22"`) so the manifest records what was actually tested, and
**`Cargo.lock` committed** — the idiomatic reproducibility guarantee for a binary crate,
same call as cmote. `cargo update` then moves the whole tree forward within semver.

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
   │ audio::peaks               │  a whole file decoded for its shape (§14a)
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
- **Nothing that blocks goes on the executor. Not one thing.** Every blocking job — both
  directory reads and the waveform scan — goes through one `off_thread(job, delivered)`
  helper: a `std::thread` for the work, a `oneshot` for the executor to wait on. iced's smol
  executor is *one thread* by default, so blocking it stops every subscription in the app,
  the playhead tick included. §14a has the measurement that found this.

  The reads were the last holdout, and the note that kept them there said the bet was a
  measured one: a local `read_dir` is milliseconds. It was measured properly in the end and
  it is not — **25 ms for 5 000 files, 95 ms for 20 000**, which is half a tick and two
  ticks, on a *local* disk. The network mount the old note was waiting for never had to be
  found; the local case was already over budget. Applying the pattern the scan already used
  cost fewer lines than the note explaining why it had not been applied, which is usually
  the sign that a deferral has gone stale.

  The pane still shows its previous contents until a new listing lands — cmote's "never
  flash empty" rule (§18 there) is about *what is drawn*, and is untouched by where the
  reading happens.

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
        ├── queue.rs  PURE arithmetic for **one** queue: its rows, its selection, and what
        │                every edit does to that selection (§7a)
        ├── queues.rs    PURE, and about the **set** of three: `QueueId`, the queues themselves,
        │                where each is scrolled to, which files are out being measured, and the
        │                three questions only the set can answer — is this a duplicate, what
        │                needs measuring, where does the next track come from (§7a, Q47)
        ├── settings.rs  load/save clecta-data/settings.json; a corrupt file reads as defaults (§11)
        ├── waveform.rs  PURE, and about what is in a *file*: `Scanner` folds its samples to a
        │                bounded array, finds the music's edges and works out its tempo, all in
        │                one pass; `Scan` is the three answers, declared here so `cache` can name
        │                them without depending on the module that knows rodio exists (§14a, Q44,
        │                Q46). The strip's pixel geometry lives with the widget, not here
        └── ui/
            ├── mod.rs       what more than one pane shares: the formatting helpers, the row
            │                window, and the selected-row fill all three queues are drawn with
            ├── deck.rs      one player's panel: title, transport buttons, time, drop ring
            ├── mixer.rs     the two faders and the crossfader
            ├── queue.rs  one queue's panel: add, select, reorder, send to a neighbour, the two switches (§7a)
            ├── browser.rs   the files pane and its rows
            ├── tree.rs      the folder tree pane, its splitter and its fold button
            └── waveform.rs  the custom advanced::Widget: a bar per pixel column, the playhead, the scrub (§14a)
```

**Built so far:** all of it. `ui/mod.rs` also carries the formatting helpers (elide, size,
date, clock) and their tests — small pure functions with no home of their own, and the
kind where a subtly wrong answer survives a hundred glances at the screen.

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
  reason: a splitter with no ceiling leaves the user dragging their way back out. The top
  section's height is a **number of pixels**, not a share of the window, and it is
  remembered across a restart — see "The players keep a height, not a share" below.
- **The tree is a fixed-width column at the right of the bottom section**, with its own
  vertical splitter and a **fold button**. Folding it gives the files pane the full
  width; the pane never disappears, because a browser with no file list is not a
  browser. (cmote folds pane and tree together because the terminal owns the rest of
  the window; here the bottom section is the browser, so only the tree folds.)
- **Colours are set in pairs.** Every surface sets background *and* foreground together,
  so contrast never depends on the system light/dark preference — the trap cmote
  documents in §14 there.

### The splitters: `pane_grid`, verified (§16)

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

**Spiked, and it fits — `pane_grid` it is.** `src/bin/ui_spike.rs` builds this exact
layout in about forty lines against cmote's ~150 across six files, and the fear above
turns out to be unfounded: drag-to-reorder and maximise are **opt-in** through `on_drag`,
so never calling it costs nothing. Three real costs, all small, all paid in the spike:

- **The fold is `close(pane)`, as feared, and closing destroys the split with it.** So
  the tree's width lives in app state and is restored with `resize` after the re-`split`.
  Add a lookup helper too: panes are opaque handles and a re-created pane gets a new one,
  so the fold button has to *find* the tree rather than remember it. Fourteen lines
  total.
- **`State::with_configuration` does not return the `Split` handles**, and the fold needs
  one. Build the layout with `State::new` + two `split()` calls instead, which do return
  them — the declarative `Configuration` enum reads better but cannot be used here.
- **`min_size` is one number, in pixels, for every pane on both axes.** §6 wants "the
  transport must never be clipped", which is a *per-section* floor; what the widget
  offers is a global one. Setting it to the transport's height also imposes that as the
  tree's minimum width. Acceptable, and simpler than cmote's 60 % clamp, but it is not
  the same rule. `ponytail:` if a section genuinely needs its own floor later, the
  upgrade is clamping the ratio in the `Resized` handler — where the app already sits.

**Do not write a third splitter implementation from scratch.** That held for the tree's
splitter, which is still `pane_grid`'s. It did not survive the section below — for a reason
worth reading, because the ban was right and the exception is not a relaxation of it.

### The players keep a height, not a share

`pane_grid` stores a **ratio**, so a taller window gave the players a taller panel. That is
the wrong answer for this layout and not merely a preference: the panel's rows are a name,
a clock, a 56-pixel waveform and a row of buttons, all fixed. Every pixel a ratio hands it
is empty space taken from the file list, which is the one part of the window that can
actually use more room. So the top section keeps a **pixel height**, dragged to taste,
persisted (§11), and compacted only when the window is too short to grant it.

**The first attempt kept `pane_grid` and converted.** It is worth telling in full, because
every number in it was right and the result was still visibly wrong — the third time on this
project that has happened, after ⌘Q (§11) and the invisible waveform (§14a).

A ratio is `pixels / grid_height`, and the grid's height is decided during layout — iced
0.14 has no `on_resize` on a container, and `responsive` needs the `lazy` feature. Rather
than measure the layout the chrome was *decided*: 6 px of padding twice, a 4 px gap and a
status bar **pinned** to 24 px instead of sizing itself to its text, so the body's height is
`window height − 40` by construction, with `view` using the same constants so the two cannot
drift. The ratio was then the exact inverse of `Axis::split`'s own arithmetic, read out of
`iced_widget`'s source rather than guessed — it lays the top pane out at
`round(height × ratio − spacing / 2)`, so the ratio wanted is `(pixels + spacing / 2) /
height`. A probe widget confirmed it: the pane's real bounds came back `height: 300.0` for a
wanted 300 in a 932-pixel window, and `420.0` after the settings file asked for 420. Exact
both times.

**And the panel wobbled on every vertical resize.** Not drifted — wobbled, scaling with the
window and snapping back, continuously, for as long as the edge was held.

The cause is a race, and no arithmetic can win it. In `iced_winit`'s event loop, a
`WindowEvent::Resized` calls `window.raw.request_redraw()` **immediately** (`lib.rs:1065`)
and only *later* broadcasts the event to subscriptions (`lib.rs:1202`), from where our
message crosses a channel and the single-threaded smol executor (§14a) before it reaches
`update`. So the frame is laid out at the **new** window height with the **old** ratio, and
the correction lands a frame or more afterwards. A ratio derived from the window size is
always one frame stale, by construction, and during a live drag that is every frame.

So the players stopped being a pane. The layout is now a plain column:

```rust
column![
    container(decks).height(self.decks_height),  // a literal, straight from state
    divider(),                                   // 6 px, hand-written
    pane_grid(files | tree),                     // still a ratio, rightly
]
```

**The window's height does not appear in it, and that is the fix.** The first rewrite still
used it for one thing — deciding when to compact — and that was enough to keep wobbling,
because the ceiling binds exactly when the panel is tall relative to the window, which is
exactly when someone is dragging the edge. A stale number used on one frame in ten is still
a wobble. So the layout stopped reading the window at all.

What compacts a panel taller than its window is now iced itself. `Limits::height` clamps a
`Fixed` with `amount.min(self.max.height)`, and that maximum is the room the layout actually
has, measured during the layout that uses it — never a frame behind. Measured, not assumed:
a probe widget printed the pane at 700 in a 1013-pixel window, 620 in a 660-pixel one and
520 in a 560-pixel one, each exactly the room left, with `decks_height` still holding 700
throughout. Pulling the window open gives it all back.

A literal height read at the moment of layout cannot be stale, so the wobble is gone by
construction rather than by tuning. What it costs is the divider, and the cost is the reason
the ban above is not really broken: it is a **6-pixel gap with a resize cursor**, not a
splitter implementation. `PANE_SPACING` does double duty as the grid's gap and the divider's
height, so the two halves of the window cannot space differently; the press arms a `bool`;
the pointer arrives on a subscription that **exists only while the divider is held**, on the
same rule as the tick, the autosave and the sweep (§4, §11, §14a), because a cursor-move
message rebuilds every row of the files pane (§9); and the release that ends it is the one
`gestures` already publishes for the file drag (§10). Forty lines, against the ninety of
ratio arithmetic and the four tests they replaced. The tree's splitter is untouched — a
*width* as a share of the window is exactly what `pane_grid` is good at.

`CHROME` survives the rewrite, and the status bar keeps its pinned height, for one caller
only: `dragged_height`, which stops a *drag* before it pushes the browser off the bottom of
the window. That is the one place the window's height is still read and the one place it is
safe to, because a divider drag is not a window resize — nothing is moving but the pointer,
so `self.window` is current rather than a frame behind. Without it the divider could be
dragged past the window's own edge and become ungrabbable, which is the one way a user could
get stuck.

Two details that are the whole behaviour:

- **The wanted height is never overwritten when it is compacted.** Squashing the window
  clamps what is *drawn* — iced's clamp, on the frame that draws it — while `decks_height`
  still holds what the user chose, so pulling the window open again restores the panel
  rather than keeping whatever the squashed window happened to fit.
- **In a window too short for both, the players win and the browser goes to nothing.** The
  reverse of the first attempt, and deliberate: guaranteeing the browser a minimum is
  exactly the calculation that needed the window's height every frame, and it bought a
  wobble. The browser is the pane that scrolls and the one that comes straight back when
  the window grows, so it is the one that can afford to lose. A drag cannot get you there —
  only a window shrunk below the panel's own height can, and growing it undoes that.

`ponytail:` if a pane ever needs to keep a *width* the same way, this generalizes; nothing
here is written twice yet, so nothing is abstracted.

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

- **Stop means reset**, as asked: `pause()` **then** `try_seek(Duration::ZERO)`. It is
  *not* rodio's `Player::stop()`, which drops the queued source and would force a
  re-decode.
  **The order matters, and the spike is what proved it.** rodio applies control changes on
  a 5 ms `periodic_access` tick, and `try_seek` blocks until the audio thread has actually
  seeked. Seek first and the callback plays on from 0 until the pause catches up a tick
  later, so the playhead settles at **5 ms, not 0** — non-deterministically, since it
  depends on where in the tick the call lands (one machine printed `0ns`, another `5ms`,
  same binary). Pausing first puts both on the same tick. Safe because rodio's `pausable`
  sits *inside* `periodic_access`, so the control tick keeps running while paused —
  it has to, or unpausing would be impossible.
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
- **A finished track has to be put back.** `empty()` means rodio has *consumed* the source,
  not that it is sitting at the end of it: `play()` afterwards is silence, and `try_seek`
  has nothing to rewind. Measured — after the last sample, `empty()` is `true` and a bare
  `play()` leaves it `true` with the playhead frozen at the end. So `Stopped` after a track
  ends is only half true until the app re-appends the file, which it does on the same tick,
  before the handover (§7a) has a chance to replace it. Without that, a track that ends with
  nothing queued behind it leaves a player that looks ready at 0:00 and cannot be started —
  which is exactly how the bug showed up.
- **The playhead is `get_pos()`**, read on the same tick. It is the *decoder's* position,
  which leads the speaker by the device buffer. Measured in the spike: a seek to `30s`
  read back as `30.155s` immediately after. Irrelevant for a readout; it would matter for
  beat-matching, which is not v1 (§14).

**Verified by the spike** (`cargo run -- a.mp3 b.mp3`, two mp3s on a 44.1 kHz stereo
device): every claim above holds against rodio 0.22.2 — `Player::connect_new(sink.mixer())`,
the seekable builder, `total_duration()` answering (184.8s / 217.4s) only when read before
`append`, and stop leaving the position at `0ns` with `empty()` still false, replaying from
the top.

Two things the docs did not mention, both found by running it rather than reading it:

- `MixerDeviceSink` **logs a warning when dropped**, so the real `audio.rs` calls
  `log_on_drop(false)` on it at construction.
- **rodio's control tick is 5 ms** (`periodic_access`), which is the unit every
  pause/seek/volume change is quantised to. That is what makes the stop ordering above
  matter, and it is the floor on how tightly any two transport calls can be sequenced.

---

## 7a. The queues (`queue.rs`, `ui/queue.rs`)

Three queues, drawn as a second row inside the players panel: **Cue 1** under Player 1, **Cue
2** under Player 2, and **Next up** under the mixer, shared between them.

### rodio's queue is the wrong queue

§14 deferred this with a one-line plan: *"rodio's `Player` is already a queue; v1 just never
appends more than one source."* That line is wrong, and it is worth saying why before
anything else, because it is the kind of wrong that would have been discovered three days in.

Appending a second source to rodio's `Player` breaks four things the app already has:

- `Track::duration` and the waveform describe **one file**. A queued second source is not in
  either.
- Seeking would seek within the concatenation, so the strip and the playhead would stop
  agreeing with the audio.
- `Ended` is `Player::empty()` going true on the tick (§4, §7). It does **not** go true
  *between* queued sources, so the app would never learn that the track changed — the title,
  the waveform and the time readout would all still be describing the track that finished.
- The transport state machine has no state for "playing, but a different track than the one
  loaded".

So the queue is **app-managed**: a list of paths, and on `Ended` the app loads the next one
through the same `load` every other door uses. rodio keeps playing exactly one source at a
time, which is the arrangement everything else in §7 was built on.

### One rule for when a queue is read

**A track ending is the only event that pulls from a queue.** Not a player sitting empty at
startup, not a file being added to a queue, not a load that failed. That is what makes every
automatic load traceable to a track the user heard end, and it means adding files to a queue
never causes a sound.

When a player's track ends, `next_source` decides where the replacement comes from: **its own
cue first, the shared queue second.** A track deliberately cued to Player 1 outranks the pool,
which is what makes the pool "whatever is free" rather than a third queue with rules of its
own. Both empty, and the player just stops, as it always did — and a queue whose **Auto-load**
is switched off counts as having nothing to offer, however full it is (below).

The new track **lands on `Stopped`**, like every other load (§7). It is ready at 0:00 with its
waveform scanning, and audible only when someone presses Play. On a mixer an unrequested
fade-in is a mistake that cannot be taken back, and §7's "a successful load always lands on
Stopped" did not need an exception carved into it — a queue told to start its tracks presses
Play *afterwards*, which is a second event and not a fourth kind of load.

The track is *taken* out of the queue rather than marked as played: a queue is what is still to
come, so the row leaving as it reaches the player is what makes the queue mean that.

### The two switches, and why they are per queue

Both paragraphs above describe a policy, and a policy the whole app is in is a policy that is
wrong for someone. So each queue carries **Auto-load** and **Auto-play**: whether it hands its
top track over when a player runs out, and whether that track then starts by itself. They ship
on and off respectively, which is exactly what the app did before they existed.

Two switches rather than one three-way setting, because they answer two questions and the
**middle position is the useful one**: load without playing is the default above, and a single
toggle would offer only the two ends of the range.

Per *queue* rather than per player, which is what makes them worth having. Cue 1 can run the
evening by itself while **Next up** sits there as a shelf someone takes from by hand — one
setting each, rather than a mode the whole app is in. It also means the switches that decide
whether a track plays belong to **the queue that gave it**, not to the player that received it,
which is the rule `advance` reads them by.

A queue with **Auto-load** off is *skipped*, not blocking: `next_source` passes over it and asks
the next one, so switching a cue off still lets the shared queue feed that player. The switch
belongs to one queue and says nothing about the other. Switch every queue off and the player
simply stops with full queues in front of it, which is what switching every queue off asks for.

Two rules keep the automatic start honest:

- **Auto-play goes dead while Auto-load is off**, because nothing is handed over for it to
  start. Dead rather than silently ignored — the same "dead rather than absent, and dead for a
  reason" rule the footer's buttons follow. A ticked box that does nothing is a lie about what
  the app will do at the end of the track.
- **Play is pressed only if the file actually arrived.** A load that fails leaves the previous
  track in the player and says so in the notice (§7) — pressing Play on *that* would be the app
  restarting a track nobody queued, which is the one way an automatic start could play the
  wrong thing. `advance` checks that the loaded track is the one it just took.

The switches are a settings change and not an edit: they mark the file dirty without going
through `queued`, because nothing was added and there is nothing new to measure.

### The selection is the hard part

Everything in `queue.rs` is `Vec` arithmetic, and all of it exists to answer one question:
**what happens to the highlighted row when the queue moves under it?** A row that stays
highlighted while a different track slides beneath it is worse than no highlight at all — the
next button press then acts on something the user did not point at.

So the selection is an **index**, not a path — the opposite of the files pane (§9), because a
queue may hold the same track twice and a path does not name a row — and every edit carries it:

- an insert **above** the selection pushes it down, so the highlight stays on its track;
- removing a row above it pulls it up;
- removing the selected row leaves the highlight on **the row that slid into its place**, so
  pressing remove three times removes three consecutive rows rather than needing a re-aim
  after each;
- removing the last row falls back to the new last row, and an empty queue selects nothing;
- `shift` swaps two rows **and follows whichever of them was selected** — the one a
  swap-only implementation gets wrong.

Each of those is a test, and one of them earned its place immediately: the first `remove`
used a `?` inside a `match` arm to find the fallback index, which returns from `remove`
*itself* — so the row was deleted, the function reported that nothing had been removed, and
the selection was left pointing at a track that was gone. Written out longhand instead.

### What the buttons are

Per queue: **⤒** and **⤓** add the files pane's selection to the top or the end. They live on
each queue rather than in the browser's header because there are three queues and two ways in:
six buttons in one header would each need a label saying which queue they meant, where a button
sitting *on* a queue needs none.

**✕** removes the selected row, **▲ ▼** move it, and **← →** send it to the neighbouring queue,
appended. The arrows sit at the outer edges of each queue's footer, facing the queue they send
to, so the pair either side of a gap reads as one control *between* two queues. Neighbours
only: Cue 1 and Cue 2 are not adjacent, so a track cannot be thrown across the shared queue
without stopping there — which is the point of the middle queue being in the middle.

Every button is **dead rather than absent** when it cannot act, and dead for a specific
reason each time: nothing selected, already at the top, already at the bottom, no neighbour in
that direction, nothing addable selected in the browser.

### Queueing a track that is already queued

Adding a track that is already in a queue **asks**, with a native OK / Cancel dialog naming
where it already is. Not a refusal: playing something twice in a set is a thing people do, and
so is adding it twice by accident, and nothing in the app can tell those apart. A silent
refusal would be the app deciding on the user's behalf; the silent accept it replaces is what
left the mistake possible in the first place. Asking is the only honest option, and it costs
one click in the case that was already deliberate.

**All three queues are searched, not just the one being added to.** The mistake worth catching
is a track that plays twice in an evening, and Cue 1 and Cue 2 each holding it does that
exactly as surely as one queue holding it twice — the duplicate is in the *set*, not in a queue.
That is also why the message names where it already is rather than only saying that it is.

Four ways in are checked and two are not, and the two say what the rule is: `⤒` / `⤓`, a drop
from the files pane, a drop from another queue, and `← →` all put a track somewhere it was not.
A **reorder** inside one queue and a **drag onto a player** cannot produce a duplicate, so
neither asks.

Two details are load-bearing:

- **A row on its way out is not its own duplicate.** A cross-queue move — dragged, or sent with
  `← →` — finds the row in the queue it is leaving. `already_queued` takes the row being moved
  and skips it, or every single cross-queue move would ask a question with one honest answer.
  The exception is the *row*, not the track: a second copy somewhere else still counts.
- **Nothing is touched before the answer.** `← →` used to take the row out and then append it;
  it now looks at the selected row, asks, and only then takes it, because a cancelled warning
  has to leave the queue exactly as it was — and putting a row back afterwards would mean
  rebuilding the selection that came out with it.

`ponytail:` a native modal blocks the GUI thread while it is open, so the playhead stops with
it — exactly as it does for the **Load…** dialog (§10), and exactly what a modal *is*. An
in-app confirmation bar would cost its own state and its own two messages, and is worth
building the day this has to say more than yes or no.

macOS says so out loud, once per dialog, and the line is worth writing down because it reads
like a defect and is not one:

```
CFUserNotificationDisplayAlert: called from main application thread, will block waiting for a
response.
```

That is `rfd` taking its **parentless** path. Given a parent window it builds an `NSAlert`
owned by the app; given none it asks `CFUserNotification`, which is another process drawing
the alert while this one waits — hence both halves of the warning, the blocking one being the
shortcut above, already taken deliberately. iced lends a window handle inside `window::run`
alone, which is a `Task`: **Clear cache** could take one, but `admits` is asked in the middle
of three queue edits and hands its answer back to the code that asked, so parenting *it* means
three continuations and a queue that can change while the question is open. Parenting one and
not the other would trade a log line for two dialogs that do not look alike, so both stay as
they are (Q38).

### Playing a queued track now

A **double click** on a queued row loads it into a player immediately, taking it out of the
queue as it goes. It is the third way a track leaves a queue, and the three agree: a drag onto
a player, a double click, and the handover at the end of a track all *take* the row, because a
queue is what is still to come and a track that has reached a player is no longer that.

Which player is the only decision in it, and it is already made elsewhere: a **cue** plays on
the player it sits under, because that is what a cue means. The **shared** queue has no player
of its own, so it uses `deck::idle_target` — the same "whichever is free" rule an unaimed OS
drop uses (§10), and the same rule the automatic handover would have applied a minute later.

Double click rather than a fifth button in the footer, for two reasons and neither is space:
the files pane already loads on a double click (§9), so the gesture is the one the app has
already taught; and a footer button would need the same five-way disabled logic as the rest,
where a double click on an empty queue is a double click on nothing.

The press that opens a double click has already armed a drag (§10), and the row it is carrying
is about to be removed — so the load **disarms it explicitly** rather than letting the release
find a row that is no longer there.

### Dragging

A drag can now start in the files pane **or in any queue**, and land on a player **or between
two rows of any queue**. Four gestures, one mechanism, and the mechanism is the one §10 already
had — generalised in two places rather than duplicated.

**What is carried** is `Drag { item, from }`, and `from` is the whole difference between a
copy and a move: `None` means the files pane, so the folder keeps its file; `Some((queue,
index))` means a row, which leaves that queue when it lands. Dropping a queued row onto a
player therefore takes it out of the queue — it is jumping the queue, which is the same thing
the queue would have done for it later.

**Where it would land** is `DropTarget`: a player, or a queue *and a row index*. The index names
the caret **above** that row, `len` meaning past the last one — a caret sits between rows, not
on them, which is what makes "drop at the end" expressible at all.

Three things had to be got right, and each is a rule rather than a detail:

- **Entering and leaving are different shapes.** A queue has as many targets as it has rows,
  but leaving it is one event, so `DragOut` carries a `Zone` — a player or a whole queue — and
  clears the hover *only if the hover is still in that zone*. Nothing orders an enter against
  the leave it replaces, and a leave that cleared unconditionally would wipe a target the
  pointer is genuinely over. The old code already had this guard for two players; it now has
  to hold across three queues and dozens of rows, which is why the leave got its own type.
- **The caret is reserved, not inserted.** Two pixels between every pair of rows, always
  present and merely *coloured* when it is the target. A caret that appeared would move the
  row under the pointer — changing the target as a side effect of showing it, which is a
  feedback loop and not a hint.
- **The end of a queue is a real widget.** Empty space below the rows inside a `scrollable` is
  not a widget and cannot be entered, so appending had nowhere to aim. There is a twelve-pixel
  tail strip after the last caret whose whole job is to mean "append" — and it is also the
  only target an *empty* queue has.

Exactly one indicator is ever lit: a drag headed for a queue lights no player ring, because two
indicators at once would each be half a lie.

The one piece of arithmetic is `relocate(from, to)`, for a row dragged **within its own queue**.
Once the row is lifted out, everything below it has shifted up, so a caret that was below the
row lands one place earlier than its index said. It is a function rather than two lines at the
call site precisely so that off-by-one has somewhere to be tested — including exhaustively,
because a reorder that loses or duplicates a row is the one failure a queue cannot survive.

### Scrolling while dragging

A drag can only land on a row that is on screen, and a hand holding a mouse button cannot
reach for a wheel. So resting the pointer on a queue's **header scrolls it up** and on its
**footer scrolls it down**, eight pixels every thirty milliseconds, for as long as the pointer
stays there.

The header and the footer *are* the edges — that is the whole trick, and it is what makes this
cost no layout at all. The obvious design is two strips that appear when a drag begins, and it
is wrong twice over: a strip that appears would push every row down the moment the drag
started, which is the same feedback loop the reserved caret exists to avoid and worse, because
it moves the rows before the user has aimed at anything; and a strip reserved for ever would
spend twenty pixels of every queue on something useful for a second at a time. The header and
the footer are already exactly the top and bottom of the rows, and during a drag they have
nothing else to do — the buttons on them cannot be pressed by a button that is already held.
They are wrapped in their container *whether or not* a drag is in flight, so arming an edge
changes its colour and nothing else.

Arming follows the same rule as the drop target and for the same reason: entering one edge and
leaving another arrive in an order nothing guarantees, so a leave clears only the edge it is
actually about. And the release clears it **unconditionally**, because the edges are only
`mouse_area`s while a drag is in flight: letting go *on* one destroys the widget that would
have reported the pointer leaving it, and a queue that kept scrolling after the drag ended would
be a bug with no way out but another drag.

The scroll itself is `operation::scroll_by` rather than an offset the app works out. iced
clamps it against the pane's real bounds, which the app does not know — the panel's height is
whatever is left after the players took theirs, and any number derived from `self.window` is a
frame stale (§6). What the app *does* know is where the pane ended up, because a `scrollable`
republishes its viewport on the next redraw whenever it has moved, `on_scroll` included. That
is what keeps the virtualized rows following a scroll the pointer never asked for.

### Layout, and what a divider drag now grows

The three queues are a second row *inside* the fixed-height players panel (§6), each under its
own column. The controls above them — title, time, waveform, transport, mixer — are all
fixed-size rows, so the panel's player half **shrinks to its content** and the queue takes
everything left over. That is what makes dragging the one divider grow the *queues*, which is
the thing whose useful size varies, rather than padding the players with empty space.

The default `decks_height` grows 300 → 480 to fit them. An existing `settings.json` keeps
whatever height it had, so an upgrade opens with short queues until the divider is dragged —
which is the right trade against overriding a value the user chose.

### Persistence

All three queues are in `settings.json` (§11), and they are the **only unbounded thing in that
file** — worth saying out loud, because everything else there is one number. A cue built
over an evening and lost to a quit is worse than no cue.

Stored as plain paths, and sanitized on the way in like every other field: a queued track that
has been deleted, renamed or unmounted is **dropped**, and so is one whose extension is not
media. A queue is a promise about what plays next, and the worst possible moment to discover a
broken row is when a track ends and the next one is due. One bad path does not empty the queue.

The two switches go in beside them, as `auto_load` and `auto_play`: three of each, in **draw
order** — Cue 1, Next up, Cue 2 — which is deliberately not the order `cues` is in, since that
pair is per player and has no slot for the shared queue. Nothing sanitizes them, because a
`bool` has no wrong value that serde would let through, and a file written before they existed
reads as "hand over, do not start" — which is what the app did then, and is what §11's
`#[serde(default)]` rule is for.

### How long the queue runs for

Each footer shows `4 · 18:22`, and the number is what a queue is *for*: a list of names says
what is coming, a running time says whether it fits.

Getting it needs the one thing a path does not carry. `audio::duration` asks the same question
`load` answers — build the decoder, read `total_duration()` — of a file nobody has loaded.
Building the decoder parses the container's header and stops; it is an open and a parse, not a
decode, which is what makes this affordable for a whole queue where a waveform scan is seconds
per file (§14a).

It still touches the disk, so it obeys §4's rule without needing to be argued about: **if it
blocks, it gets a thread.** One `off_thread` job for the whole batch rather than one per file
— they are wanted together, and a restored queue would otherwise be dozens of threads — and
the answers come back as one message.

**Nothing is asked about twice**, and the reason that needs saying is the timing: a row counts
as unmeasured until its answer *lands*, which is long after the job that will produce it
started. So a second edit arriving a few milliseconds later would find every file the first
job is holding still looking unmeasured and send them all off again — twenty quick edits would
open the first file twenty times, an O(n²) that nothing in the code looks like. `measuring` is
what a job takes with it on the way out and gives back on the way in, and `to_measure`
subtracts it. One invariant makes the giving-back safe: **the arm is handed exactly the batch
that was asked about**, whatever happened to the job, so a path cannot be stranded in the set
and leave a file nothing will ever look at again.

A job can only fail to answer by panicking, and its batch is then recorded as
measured-and-no-length rather than left alone. That is the safer of the two wrongs: forgetting
it strands the files, and re-queueing them retries a panic that is probably deterministic —
on every edit, for the rest of the run. The footer already has a way of saying what this looks
like, which is that the running time keeps its `+`.

`Item::duration` is an `Option<Option<Duration>>`, and both layers earn their place. The outer
one is *has anyone asked*, the inner one is *did the file answer*. Collapsing them would make
the app re-open an unreadable file every time anything else was added, for ever. Measurements
are applied **by path across all three queues**, not by index into one, because the queues can be
edited while the lookup runs — and a queue may hold the same track twice, so one answer settles
both rows.

`Queue::total` returns the sum **and whether it is the whole truth**, and the footer prints
a `+` when it is not. A row still being measured and a row nothing can measure both leave the
`+` on. The alternative is arithmetic that quietly counts a missing track as zero, and a number
that exists to be planned against must not do that.

---

## 7b. The handover, whole or trimmed (`queue.rs`, `app.rs`)

A track ends twice. The file runs out, which is what §7a's handover waits for — and before
that, somewhere earlier, the *music* stops. In between sits whatever the encoder padded, the
engineer faded into, or nobody trimmed off the master: two seconds of room tone, eight seconds
of run-out groove, the silence an MP3 encoder adds because its frames do not divide evenly.
Played back to back, that gap is the difference between a set and a sequence of files.

So each queue gets a third setting beside **Auto-load** and **Auto-play**:

| | **Whole track** | **Skip blanks** |
|---|---|---|
| when the next track takes over | when the file runs out | when the music stops |
| where the next track starts | 0:00 | where *its* music starts |

**Per queue, and read from the queue that supplies the next track**, which is Q26's rule
unchanged: the queue waiting behind a player is what says how it wants to take over, so Cue 1
can run an evening back to back while **Next up** stays a shelf that plays what it is handed,
whole. A `pick_list` rather than a third checkbox, for the same reason the crossfader's curve
is one — the two positions are not on and off but two behaviours, and both want naming.

`Whole` is the default and has to be: cutting a track short is not a thing to start doing
unasked, the same reasoning that keeps every load on `Stopped` (§7).

### Two ends, one handover

The tick already reads the playhead 20 times a second (§4), which means the second end costs
no new machinery at all — only a comparison next to the one already there:

```rust
if engine.finished(id) { /* the file ran out (§7) */ }
else if self.cuts_early(id, position) { /* the music stopped */ }
```

Both push the player onto the same `ended` list, so `advance` is reached by two roads and
knows about neither. What the two branches must *not* share is what they do to the player, and
the difference is the one the last fix in §7 turned up: `empty()` means rodio has consumed the
source, so a finished track has to be re-appended before anything can play it again. An early
cut has consumed nothing — the file is still in the player and still sounding — so it is
`stop`ped instead, rewound and paused. Re-appending there would be a wasted decode, and doing
nothing at all would leave the tail audible under the next track if the load failed.

Three conditions, and each of them is a reason not to cut:

1. **The queue waiting behind asked for it.** Off by default, per queue.
2. **This track's edges are known** (§14c). A file nobody has scanned plays whole, silently —
   a notice every four minutes about a setting the user turned on themselves is noise.
3. **There is something to hand over to.** `next_source` is asked first, so the last track of
   the evening plays its run-out: stopping a player early with nothing to follow is worse than
   the silence it saves.

The third is why `cuts_early` lives in `app.rs` and `hands_over_early` — the pure part, and the
tested one — lives in `queue.rs`. The rule is arithmetic; knowing whether a queue has
anything left is the app's business.

The 50 ms tick puts the cut up to a tick late, which is a twentieth of a second of run-out that
still plays. Nobody has ever heard that. Making it exact would mean a callback rodio does not
offer, which is the same wall §7 hits at the other end of the track.

### The other half of the same setting

A queue that skips the blanks at the end of one track also skips them at the start of the next:
`advance` seeks the freshly loaded track to where *its* music starts before pressing Play. Same
setting, same queue, both ends — a handover that trimmed one end and not the other would leave
the gap it just removed.

That seek is the only thing that has to know the *incoming* track's edges, and it needs them
**at load time**, not two seconds later when a scan lands: a jump three seconds into a track
that is already playing is exactly the artefact this feature exists to remove. So it uses what
is already known and nothing else (§14c), and a track nobody has prepared starts at 0:00. That
is the whole reason the folder scan of §11b has a button.

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

### The preset buttons

Every slider in the strip has its ends on buttons, and the crossfader has its centre on
one too: **◄ 1 / centre / 2 ►** under the crossfader, **0** and **max** either side of each
volume fader. Added after the smoke test, because the strip was usable and still annoying —
a slider is the right control for *searching* for a value and the wrong one for *returning*
to a value you can already name.

The centre button is the one that is not merely a convenience. `0.0` and `1.0` are the ends
of the travel and a drag lands on them by shoving the knob into the wall; **`0.5` exactly is
a value a mouse hits by luck**, and a crossfader parked at 0.49 sounds like a crossfader
parked at 0.50 while not being it. A button is the only way to be certain.

What makes this three lines rather than a feature is that the buttons emit the **same
messages the sliders emit** — `FaderChanged` and `CrossfaderChanged`, with a literal instead
of a drag position. No new message, no new state, no new arm in `update`, and nothing added
to §12: every value a button can produce is one `gains` is already tested at. That is worth
saying out loud because the tempting shape — a `Preset` message with its own handler — would
have been a second path into the same state, and second paths are what drift apart.

`ponytail:` no keyboard shortcuts and no double-click-to-centre, which is what a hardware
mixer's detent would be. Three buttons cover it; add the gestures if the buttons turn out to
be the thing being hunted for.

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
- **Rows, not icons** — name, tempo, playing time, size, modified date, and a leading glyph
  marking *audio* / *video* / *other*. A media browser is scanned by name and length, not by
  thumbnail. The tempo (§14d) and the playing time (§14c) sit *before* the size because they are
  the columns that say what a file is for rather than what it is, and the tempo leads because a
  set is grouped by it before it is timed. Both are blank until something has scanned the file,
  which is the same moment its `✓` appears (§11c).
- **Not `widget::table` — `scrollable(column(rows))`.** iced 0.14 does ship a `table`, and
  three aligned columns with a header looked like exactly its job. The spike
  (`src/bin/ui_spike.rs`) says no: **`table` has no row.** `table::Column::view` produces
  one *cell* per row, and `Table` flattens every column's cells into one flat list. Two
  consequences, one fatal:
  - A click has to be attached per **cell**, not per row — four `mouse_area`s a row here,
    with the inter-column padding and separator left dead between them. Awkward, and it
    makes the §10 drag gesture originate from a cell rather than a row, but survivable.
  - `table::Style` carries **only** `separator_x` and `separator_y` colours. There is no
    row background, so a selected or hovered row **cannot be drawn at all**. That is what
    settles it: §9 rows need a selected state.

  So each row is a `mouse_area(container(row![...]))` — one hitbox, one background, one
  message. The cost is that column alignment becomes manual `width()` on the cells, which
  is nothing for four fixed columns. cmote built its rows by hand too, though for a
  different reason (an icon *grid* with rubber-band selection).
- **Everything is listed, media is distinguished.** Hiding non-media would hide the
  `.cue` and the artwork that tell the user they are in the right folder. Only a media
  row is loadable; the rest are inert.
- **Sort: folders never appear here** (that is the tree) — files sort by name,
  case-insensitively, natural-numeric so `track2` precedes `track10`.
- **Choosing a folder**: click it in the tree, or the **Open folder…** button (`rfd`).
  Both funnel through one `select_folder(path)`.
- **The shown folder watches itself** — `notify`, and the subsection below.
- **Virtualization, hand-rolled** — the ceiling this file used to carry a `ponytail:` note
  about, now measured and lifted. See the subsection below.

### Only the visible rows are built

This started as a `ponytail:` note: a `scrollable` builds every row on every `view()`, so a
5 000-entry folder builds 5 000 widgets per frame, and at the 20 Hz playing tick (§4) that
is 100 000 widgets a second for rows nobody can see. The note said a music folder is tens to
hundreds of files, so the ceiling was in the right place. **Then it was measured, and it was
not.**

A release build, forced to redraw at the playing tick, on a folder of 5 000 files:

| | 50 files | 5 000 files |
|---|---|---|
| whole process, at 20 Hz | 7.3 % of a core | **70.1 %** |
| `view()` alone, per frame | 0.13 ms | **16.7 ms** |

Sixty per cent of a core to draw a hundred rows and skip four thousand nine hundred. A
20 000-file folder spent **77 ms** per frame in `view()` alone, against a 50 ms frame budget.

The note's own upgrade order was `widget::lazy` first, then hand-rolled. That order was
**skipped, on the numbers**. `lazy` caches the built element between changes, so it would
have removed the 16.7 ms of *building* — but the process cost 63 % over baseline and building
is only 33 % of it. The rest is iced laying out five thousand rows every frame, which `lazy`
does not touch, because `UserInterface::build` lays out the whole tree each frame whether the
elements were cached or not. It would also have cost a dependency (`ouroboros`, and a
licence line in `deny.toml`) and a version counter to hash cheaply — state that is silently
wrong the day someone forgets to bump it. Hand-rolled costs one `f32`.

So: **`browser.scroll` is the only new state, and one number of it.** The view builds a
fixed `ROWS_BUILT = 200` rows starting at the row under the top edge, and puts the rest
above and below as two `Space`s of exactly the height they would have taken. The scrollbar
is therefore the size it would have been, and every row is where it would have been.

Three things make that honest rather than approximate:

- **The row height is pinned, not measured.** `container(...).height(ROW_HEIGHT)` with the
  contents centred. Virtualization has to know where a row *would* be without laying it out,
  and iced offers no way to ask a widget how tall it turned out (§6 met the same wall from
  the other side). Choosing the number instead of measuring it makes the arithmetic exact by
  construction rather than exact if the font co-operates.
- **A fixed count, not the pane's measured height.** A window is clamped to 4096 points
  (`settings.rs`), so a pane can never show more than 171 rows of 24; 200 covers that with
  room for a scroll offset that is a frame stale. Measuring the pane would mean tracking a
  second number, and being wrong about it on the first frame — before any scroll event has
  ever arrived — which is exactly when the pane must not be blank.
- **The window stops at the last full pane.** `visible_rows` clamps its start to
  `total - ROWS_BUILT`, so an offset left over from a longer listing shows the *end* of the
  new one rather than nothing, which is what the `scrollable` itself does when its content
  shrinks under it.

Choosing a folder resets both halves — the field *and* the widget, via a `scroll_to`
operation, because the `scrollable` keeps its own offset and would otherwise open the new
folder scrolled to wherever the old one was left. A **refresh** deliberately resets neither:
re-reading a folder should leave you where you were reading.

Result, same measurement: **70.1 % → 9.3 %** against a 7.2 % baseline, and `view()` flat at
0.5 ms for 500, 5 000 or 20 000 files. §16's framework question is no longer waiting on this.

The three queues use the same arithmetic, from the same function — `visible_rows` moved to
`ui/mod.rs` and took a **row pitch** as an argument when the second caller arrived. That
parameter is the whole difference between the two: a queue reserves a two-pixel caret above
every row (§7a), so its rows are 22 pixels apart where the files pane's are 24. A shared
helper with a hard-coded constant would have been the same code being quietly wrong in one of
its two homes, which is worse than two copies.

A queue is normally tens of rows where a folder is thousands, so this buys nothing today. It
is here because "normally" is not a bound and the arithmetic was already written and already
tested — the honest reason to reuse something rather than the flattering one.

**The bar is drawn over the rows unless it is asked not to be.** iced lays a `scrollable`'s
content out at the full width and paints the vertical bar on top of it; given a `spacing` it
reserves `width + 2 × margin + spacing` instead, and only while the bar is actually showing, so
a list too short to scroll keeps every pixel. Left alone, the right-hand end of every row sits
under the bar — invisible until a column is flush with that edge, which is why it showed up in
a queue first: the running time is right-aligned against it and was being cut in half. All
three queues take the same two-pixel gap from one constant (`ui::SCROLLBAR_GAP`), because the
defect is the layout's and not the queue's, and the two that do not show it today show it the
moment a name or a date grows long enough to reach the edge.

### The shown folder watches itself

`notify` was the third thing §14 deferred and the first that came back cheaply, because the
work of the last two rounds had already been done: re-listing a folder is a thread now (§4)
and re-drawing one is flat (above), so "just re-list it" is an answer the app can afford to
give often. Deferred features get cheaper when the things under them get fixed, and that is
the argument for fixing the things under them.

**One folder, non-recursively: the one the pane is showing.** The tree lists its own and is
not watched — a ceiling, not an oversight, and the cost of lifting it is one watcher per
expanded folder rather than one for the app.

Three decisions worth the words:

- **`Subscription::run_with(folder, watch)`, keyed on the path.** The subscription's identity
  *is* the folder, so choosing another one tears the old watcher down and builds a new one
  with no code that says so. This is the one place iced's "subscriptions are declarative"
  claim pays a real dividend: the alternative is a watcher owned by the app struct and a
  `Drop` dance in `select_folder`.
- **What changed is deliberately not read.** Every event means "the listing might be stale",
  and the answer is a whole re-list either way, so the event's paths and kinds are thrown
  away unread. That makes the four-slot channel a *feature*: a burst of a hundred events
  fills it, the rest are dropped, and dropping them costs nothing because a re-list reads the
  folder as it is now rather than replaying a diff. A debouncer crate would have bought
  ordering and coalescing that this design does not need.
- **The settle timer is `RefreshPressed`.** A file event sets `stale`, and while `stale` a
  500 ms timer fires the *same message the Refresh button sends*. So the watcher needed one
  new message and no new path through `update` — and clearing `stale` in that arm means a
  manual refresh also satisfies a pending automatic one, which is free and correct. Same
  shape as the autosave throttle (§11): the timer exists only while there is something
  waiting, so a folder nothing is happening in costs nothing.

A watcher that cannot be created says so on stderr and gives up, like `settings.rs` with a
file it cannot write. Refresh and the refresh key still work, so the failure costs a
convenience rather than a capability — and a status line about it would be noise for
something the user never asked for.

### Refreshing by hand, which is still a feature

Watching does not retire the **Refresh** button or the refresh key. A permission, a network
mount or a watcher that would not start can all take the watching away, and the manual door
has to work when they do.

The key is **two** keys, and that is the whole decision: F5 is *the* refresh key on Windows,
and on a Mac laptop it is a system key the app is never sent unless the function-key
preference is flipped, so F5 alone would have shipped a shortcut that does not exist on the
machine this is developed on. ⌘R is what a Mac reaches for and means nothing on Windows. One
arm covers both, because `Modifiers::command()` is already Cmd on macOS and Ctrl everywhere
else — the `cfg` is inside iced, not in clecta.

`event::listen_with` rather than a widget, for the same reason the drop gestures use it: a
key press belongs to the window, not to anything that has focus. It is always on and costs
nothing at rest, unlike the divider's pointer listener — a key press is rare where a cursor
move is constant, and this one publishes no message at all unless the key was the refresh
key.

Measured end to end, with a print in the listing arm: boot listed 1 entry, one file added
gave 2, **three files added at once gave one re-list at 5** rather than three, one deleted
gave 4, and three seconds of an idle folder gave nothing at all.

---

## 9a. Selecting more than one row (`select.rs`, `browser.rs`, `queue.rs`)

Every door into a player and every door into a queue used to take one file, because the pane
could only hold one row selected. Loosening that is one change to a field and a long tail of
questions about what the *existing* actions then mean — which is the whole of this section.

### One rule, two storages

The gesture is the same everywhere and it has three cases: a plain press **replaces**, a
command press **toggles**, a shift press takes **everything between** the anchor and the row.
Both panes obey it, so `select.rs` holds the rule — an enum, the modifier mapping and the
inclusive range — with no iced in sight so it can be checked with no window.

What the two panes cannot share is the *storage*, and the reason is already in this document.
The files pane is keyed by **path**, because a refresh renumbers its rows underneath the user
(§9). A queue is keyed by **index**, because a queue may hold the same track twice and a path
therefore names no row (§7a). So one rule, two sets.

Both hand their selection back **top to bottom**, and that is not a convenience: it is the
order the actions run in. The pane reads its order back off the listing rather than out of the
set, so it is the natural-numeric sort the user is looking at and it cannot drift after a
re-sort; the queue uses a `BTreeSet`, whose order *is* the row order.

The anchor moves on a plain or command press and **stays put for a range**, which is what lets
a shift-click be corrected: clicking again re-measures from the same start rather than from
wherever the last one landed.

### A press has to do two jobs

There is no separate gesture to start a drag with, so a press both moves the selection and arms
the drag (§10). That collides with multi-select immediately: collapsing to one row on a plain
press would destroy the selection the drag is about to carry. So **a plain press on a row that
is already selected leaves the selection alone**, and the drag picks up all of it.

The other half of that trade is the release: in a file manager, a plain press on a selected
row collapses the selection once it is clear no drag happened — and now it does here too
(Q50). The press remembers the row it deferred on (`Click::defers`, the same test both press
arms already made inline), and the release turns that memory into a plain click. Two things
claim the click first and cancel it. A drag that lands on a target takes the collapse with
it, because releasing a carried selection must not also narrow it. And a **double click**
acts on the whole selection (the table below) but arrives *after* its first click's release —
so the release does not collapse either; it leaves the job pending, and a timer fires it once
the double-click window (300 ms and six pixels, in iced's `mouse::Click`) has passed with
nothing else claiming the press. Anything that acts on the selection in the meantime — a new
press, the double click's load, ⌘A, Escape — cancels the pending collapse rather than letting
a stale click fire under a newer gesture. The timer is the price every file manager pays for
the same promise: the visible narrowing lags the click by the double-click interval. It is
also why Q33's estimate of "one remembered path and a branch" was short by exactly one
timer — the branch was free, the double click was not.

### What each door now means

| door | one file | several |
|---|---|---|
| **→ Player 1** | loads it | first loads, **rest go to the top of that player's cue** |
| double-click a row | loads it into the idle player | the same, for the whole selection |
| drag onto a player | loads it | the same |
| **⤒ ⤓** | adds it | adds all of them, in pane order |
| drag into a queue | inserts it at the caret | inserts all of them at the caret, in order |
| `✕ ▲ ▼ ← →` | acts on the row | acts on every selected row |

The first line is the only one that needed inventing, because a player holds one track and five
files have no obvious single meaning. The answer was already in the app: the cue in front of a
player is what plays next, so "load these five" is *load one and queue four*, and the handover
of §7a does the rest. The **top** of the cue rather than the end, which is this section reading
the intent over the word: the promise is that the five play back to back, and appending would
let whatever was already queued play in the middle of them.

A double click and a drag act on the selection when the row they name is part of it, and on
that row alone when it is not — both are reachable, since a command-click can deselect the very
row it lands on, and a gesture that names a file should still do something with that file.

`▲` and `▼` move every selected row one place, blocked when the selection already touches the
end it is moving towards. Blocked **entirely**, scattered rows included: moving some of what
was asked for and not the rest is worse than moving none, because pressing the other button
does not undo it.

### The duplicate warning grew a third answer

Asking per file (§7a) does not survive a batch: twenty files with three repeats would be three
modal dialogs in a row for one button press. One dialog, then — and the moment it names a count
rather than a file, the old two answers are not enough. Nineteen good tracks and one repeat has
an answer that is neither *all* nor *none*, and it is the one most people want.

So **Yes** queues them all again, **No** queues only the ones that are not already somewhere,
and **Cancel** does nothing at all. Cancel keeps meaning what Cancel means; the middle answer
is a button of its own rather than a Cancel that quietly does something.

The three buttons are the platform's Yes / No / Cancel with their meanings spelled out in the
text, rather than rfd's custom labels: those need a Cargo feature that is off by default and,
by rfd's own documentation, work on Windows only with it — and a dialog whose buttons might be
unlabelled on the one target nobody has run is not worth three words.

Which is why the answer is an `Admission` and not a filtered list: the `←` / `→` buttons have
to filter the **rows** and the **tracks** by the same test, so what leaves one queue is exactly
what arrives in the other. `Admission::keeps(position)` is that test, and `queue::duplicates`
is the pure half that finds them.

### Two more keys, and what they are not

**⌘A / Ctrl+A** selects every row the pane is *showing*, so the hidden filter decides what
"all" means — the same rule the selection itself follows. **Escape** clears it.

Both act on the files pane and not on the queues, and that is a line rather than an oversight:
the app has no focus model, so "select all" in a window with four selectable panels would have
to guess which one was meant. The pane is the one with thousands of rows, which is where
selecting them all by hand is not an option.

### And one thing that did not change

An **OS drop** of several files still takes the first and declines the rest by name (§10).
Nothing was learned that changes it: a Finder drop arrives as one event per file with no
position and no end-of-drop event, so the app cannot know it has them all — where a selection
in the pane is already a list before anything is dragged.

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

**Built as `deck::idle_target`, not as `os_drop_target`** — the double-click in the files
pane needs the identical rule, and giving one function two names to suit two callers is
how two implementations start. The name says what it computes rather than which gesture
asked (§9's double-click was written against it before the drop existed).

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

### What the implementation added to the plan

- **The in-app drag needs no drag state of its own beyond the file.** A press on a media
  row already sends `RowSelected`, so that message arms the drag; the release disarms it
  wherever it lands. **A plain click is a drag that landed on nothing** — same code path,
  no gesture recogniser, no threshold, no timer.
- **The release has to come from a raw event, not from the target.** `mouse_area` has
  `on_release`, but a drag let go over a *button* is captured and the target never hears
  it — which would leave the drag armed for ever. So `event::listen_with` takes every
  left release regardless of status, and the player panels report only enter and exit.
- **Enter/exit must carry which player, and be compared rather than merely cleared.** Both
  panels see the same cursor move in one pass, in view order, so moving right-to-left
  fires *enter* on Player 1 before *exit* on Player 2. A bare "clear the hover" on exit
  would erase the enter that had just happened.
- **The panels become drop targets only while a drag is in flight.** Outside one,
  `mouse_area` would publish a message every time the pointer crossed a player, for
  nothing.
- **The burst boundary for a multi-file drop is the hover flag.** There is no
  end-of-drop event: winit reports one `DroppedFile` per file and nothing after them. So
  the first drop *takes* the hover flag and every later file in the burst sees it already
  false. Verified against both backends — macOS `performDragOperation:` and Windows
  `IDropTarget::Drop` each emit only `DroppedFile`, neither cancels the hover first.
- **`drop_outcome` is not free of the filesystem, and cannot be.** `is_dir` is the only
  way to tell a dropped folder from an extensionless file. It is free of `self`, which is
  what the testability actually rests on; the folder case is tested against
  `CARGO_MANIFEST_DIR`, a directory that is certain to exist, so no fixture is created.

### What the queues did to this section

§7a added three queues, and with them three more places a drag can start and a great many more
places it can land. The mechanism above did not change shape: `drag` still arms on a press and
disarms on the release `gestures` already takes, and the release still asks one question about
where the pointer is. Two things grew:

- **`drag` carries a `Drag`, not a `PathBuf`** — the item plus where it came from, which is
  what tells a drop whether to copy (from the files pane) or move (from a queue).
- **`hover` is a `DropTarget`, not a `DeckId`**, and the leave message carries a `Zone`
  instead. The old "only clear if it is still mine" guard was already the right idea for two
  player panels; it is *load-bearing* now that the panels number three queues of rows.

An **OS** drop is unchanged and still lands on the idle player. It carries no position, so it
cannot aim at a queue any more than it could aim at a player — the same upstream limitation,
now with more targets it cannot reach.

---

## 11. Portability — a hard requirement (`paths.rs`)

**Copy the app anywhere, run it, leave no trace outside its own folder.** Not an
aesthetic: it is the requirement.

- **Every file clecta writes goes in `clecta-data/` beside the app.** One file in v1 —
  `settings.json` (curve, faders, crossfader, last folder, window size, the height of the
  players-and-mixer section). No registry
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
  refusal to start. A settings file must never be able to brick the app. Neither `load`
  nor `save` returns a `Result`, because there is nothing a caller could usefully do with
  one. The file is plain text a user can edit, so it is a **trust boundary**, not mere
  deserialization: a value outside the range the UI can produce — a fader of 1.5, a
  window ten pixels wide, a folder that has since been deleted — falls back to its
  default *field by field*, so one hand-edited number does not discard the whole file.
  **The window ceiling is a renderer limit, not taste**, and it took a real crash to find
  out: 16000 was the original cap, and a file asking for a 15000×15000 window panicked
  inside `Surface::configure` before the first frame — `maximum extent for either
  dimension is 8192`. A settings file killed the app at launch, which is the one thing
  this module promises cannot happen. wgpu only guarantees 8192, and a surface is
  measured in *physical* pixels, so a 2× display doubles whatever the file asks for. The
  cap is now **4096** — still larger than any real display in logical points, with the
  margin the HiDPI factor needs.
- **Written on a short throttle, and at exit.** The original design was one write, at
  exit: `exit_on_close_request(false)` routes the close through `update`, which writes the
  file and then calls `iced::exit()` — unconditionally, or the window would refuse to
  close. **That turned out to be reachable only by the close button.** macOS ⌘Q and the
  app menu's **Quit** run `applicationWillTerminate`, which winit converts to
  `LoopExiting`; iced 0.14 does not implement winit's `ApplicationHandler::exiting`, so
  the event is dropped before any clecta code sees it. There is no hook to add — the
  design had to change, not the wiring. So a `dirty` flag is set by the six things worth
  keeping, and a `time::every(2s)` subscription **exists only while `dirty` is true**,
  saving and clearing it. Nothing ticks at rest, and quitting any way at all costs at most
  the last two seconds. Strictly a *throttle*, not a debounce: the write lands two seconds
  after the **first** change of a burst rather than being postponed for as long as a fader
  keeps moving, which caps the exposure instead of extending it. Two things this earned:
  marking the window dirty unconditionally made **every launch rewrite the file it had
  just read**, because creating the window emits a resize event carrying the size that was
  just asked for — so the resize arm compares before it marks; and `boot` clears the flag
  after restoring the last folder, since restoring is not a change. `ponytail:` still a
  plain `fs::write`, not write-to-temp-then-rename: losing the fader positions to a crash
  *mid-write* costs one run of defaults. **The remaining gap is a kill or a crash within
  two seconds of a change**, which is the honest cost of not writing per slider frame.
- **The folder does not wait for the throttle.** A successful directory listing writes the
  file there and then. The two-second window is the right price for a fader, which changes
  sixty times a second and whose exact value nobody can name; it is the wrong price for the
  folder, which changes once and is the setting a user most expects to survive quitting
  right after they set it. Navigating and quitting inside two seconds is not a corner case —
  it is what "open the folder and go" looks like.

  It cost four lines and no new state, because `dirty` already says exactly the right thing:
  a listing flushes **only when something is actually unsaved**. That guard is what keeps a
  refresh, and the listing that opens the app, from writing a file they did not change. A
  listing that *fails* does not flush — the folder is still shown, so the throttle will
  store it, but a folder the app could not read is not this run's last word.

  Verified by making the branch unreachable-by-accident and then reaching it. The throttle
  was temporarily set to sixty seconds so that any write at all had to come from the flush;
  the file was written 1.9 s after launch, which is a debug build creating a wgpu surface,
  not the flush taking its time. With the throttle back at two seconds, a boot that changes
  nothing still leaves the file untouched — the regression this guard exists to prevent.
- **One binary, no installer.** Same release profile as cmote (`lto`,
  `codegen-units = 1`, `strip`, `panic = "abort"`). `#![windows_subsystem = "windows"]`
  in `main.rs` so no console window pops on Windows (inert on macOS).
- **`bundle-macos.sh`**, cmote's script adapted: wrap the release binary in a minimal
  `Clecta.app` (`Contents/MacOS/` + `Info.plist`) so Finder launches it as a GUI app
  instead of through Terminal. It takes an optional binary path, because the shipped
  artifact is the Intel cross-build and `target/release/` is the wrong one on an Apple
  Silicon machine. **The bundle is also the only way to run the walk-up above**, and
  doing so turned the `.app` rule from three unit tests on strings into a fact: a bundle
  copied to an empty folder and launched creates `clecta-data/` *beside* `Clecta.app`,
  and `~/Library/Application Support/clecta` still does not exist afterwards. Killing
  that process left the folder empty, which was the save-at-exit gap above, seen — and is
  what the throttle now closes.
- **No C toolchain**, and this too is real rather than aesthetic: cpal binds CoreAudio /
  WASAPI through `objc2` / `windows-sys`, both pure Rust, and symphonia is pure Rust.
  Nothing needs NASM or a vendored C library — the property cmote had to fight for with
  `ring` (§2 there) comes free here. **Re-confirmed at each step**: 1.9 MB for the
  audio-only spike, 5.8 MB once iced and wgpu were in the tree — the one dependency that
  could have changed the answer — 7.6 MB for the app itself with rfd and smol added, and
  **7.7 MB with serde and serde_json**. `otool -L` lists only OS frameworks at every step: CoreAudio, AudioToolbox,
  AppKit, Metal, QuartzCore, CoreGraphics and friends. Not one third-party dylib to ship
  alongside, which is the property that makes "copy it anywhere and run it" true.
- **Building the shipped Intel binary on an Apple Silicon Mac**: add
  `--target x86_64-apple-darwin` (the Xcode CLT SDK carries both slices; Rosetta 2 only to
  *run* it locally). Same split CI uses.
- **Audio device changes are a real failure mode.** Unplugging the interface mid-set
  kills the cpal stream. v1 surfaces it as a notice line and a **Reconnect audio**
  button rather than pretending; auto-recovery is §14.

---

## 11a. The cache (`cache.rs`)

`clecta-data/cache.redb`, beside `settings.json` and nothing like it: **what has already been
worked out about other people's files**, so it is not worked out again. A waveform costs a
third of a second of decoding per track (§14a) and was being paid on every launch, for the
same files, for ever.

### It is a cache, and that word does the work

Everything below follows from one sentence: **deleting this file loses nothing but time.**
Nothing reads it to decide what is *true* — only to avoid recomputing what is already known.
So:

- every failure is swallowed, because there is no caller that could do anything useful with a
  `Result`. A database that will not open leaves the app doing exactly what it did before this
  file existed;
- a file that will not open is **deleted and recreated** rather than repaired, which turns
  "corrupt once" into "cold once" instead of "no cache for ever";
- **a miss is indistinguishable from no cache at all** — a missing entry, a stale one, a record
  written by an older format and a database that is not there all return `None` down the same
  path.

That is also why the growth policy needs no number in it. At startup, entries whose file is
gone are dropped, so the cache is bounded by the library it describes: 4–8 KB a track, under
80 MB for ten thousand of them. No cap to tune, no eviction that can throw away the track you
were about to want.

### Not SQLite, and the reason is a ban we wrote on purpose

The obvious answer is SQLite, and it is the wrong one *here*. `rusqlite` pulls
`libsqlite3-sys`, which compiles C — and `deny.toml` bans `cc` outright, because the
no-C-toolchain property (§11) is the one thing that makes "copy it anywhere and run it" true.
§12 calls that ban a tripwire for exactly this moment, so it did its job: the choice became
visible instead of quietly dying.

**redb** is what SQLite would have been without the C: pure Rust, one dependency of its own
(`libc`, which is declarations rather than a compilation), MIT OR Apache-2.0 — both already on
the allow-list — a single file, and ACID. Checked rather than assumed: no `cc` in either
target's tree, and `cargo deny` passes with no new allow-list entry.

What is given up is SQL. There is no `SELECT … WHERE bpm > 120` here, and the day something
wants one is the day this is revisited. What is *not* given up is the shape the later data
needs: a table per kind of fact, keyed by the same file.

### Two tables, not one record

`waveforms` and `durations` are separate because they are worked out at different moments by
different jobs — a waveform when a track is loaded, a length when it is queued. A table each
means a write touches only what it knows, and it is where the next kind of fact goes: **a new
table, not a migration**.

That promise has now been kept twice — `trims` (§14c) and `tempos` (§14d) — and the second one
shows what it is worth beyond the migration it saves: a corrected tempo will need a store that
can be emptied on its own without touching a waveform (Q41), and a table is already that.

**But four tables are not four questions** (Q44). Three of them — the waveform, the edges and
the tempo — are what *one decode* works out, so a caller wanting any of them wants all of them,
and the rule that a hit means all three was being enforced in two places at once: once in the
app's `cached_scan`, chaining three lookups, and once here in `prepared`, doing it again for the
files pane's marks. The comment between them said the two had to be kept in step by hand, and
adding `tempos` proved it by making both wrong at the same time until both were edited.

So the store now answers `scan` and `store_scan`, and `ready` for the callers that want
everything except the eight kilobytes. The tables are private, `FULL` names the three in one
array, and one function reads them in **one transaction** rather than three. Nothing outside
this file knows how many tables there are, what order they are in, or that the stamp is checked
per table — which is what "a table, not a migration" was supposed to mean all along: the *store*
absorbs a new fact, not everything that reads from it.

One write transaction is not only tidier. Three separate writes could leave a waveform on disk
with no tempo beside it, and `scan` would read that for ever as a miss and re-store it
identically every time — a file that is scanned on every launch and never says why.

`durations` stays its own pair, and that is the asymmetry worth naming rather than smoothing: a
length is a **header parse**, not a decode. Folding it into `scan` would mean a queue asking for
a running time triggering a full decode of every row, which is the thing §7a is built to avoid.

Each value is a format byte, then the stamp, then the payload. The format byte is one byte and
buys the only migration story a cache needs — bump it, and every old record reads as a miss
and is overwritten the next time it is wanted. Without it a changed layout reads old bytes as
new ones, which for an array of floats means a waveform of *noise* rather than an error.

Peaks are stored as little-endian `f32`, four bytes a column, not quantised to a byte. The
strip is a few hundred pixels wide and would never show the difference, but four times nothing
is still nothing (8 KB at most), and an array that is **bit-identical to a fresh scan** means a
cached waveform can never be the suspect when something looks wrong. A length is eight bytes
of nanoseconds, or **no bytes at all** for a file that had none — the empty payload is how
"asked, and there is no length" is told apart from "never asked", which is the same distinction
`queue::Item::duration` draws in memory and for the same reason.

### The stamp, and what it is deliberately not

An entry is good only for the file it was written for, and the test is **size plus modified
time** — one `stat`, which the browser already does for every row it draws.

Not a content hash. Hashing every byte is roughly what the scan it is avoiding costs, and
hashing a sample of the bytes buys the rename case at the price of a read and a collision
nobody can rule out. The two cases the stamp gets wrong cost exactly **one re-scan each**: a
file edited without changing its length inside its filesystem's timestamp granularity — two
seconds on FAT32, which a portable install on a USB stick may well be sitting on — and a file
renamed or moved, which loses its entry and earns a new one. A cache that is occasionally cold
is a cache; a cache that is occasionally *wrong* is a bug that looks like a corrupt file.

A path that is not UTF-8 is simply never cached. Both shipped platforms produce UTF-8 for
anything a person typed, and the alternative is `OsStr::as_encoded_bytes` and an `unsafe` to
get back — a real cost whose only symptom is a track that re-scans.

### Where it is asked, and where it is not

**Inside the jobs, never on the GUI thread.** A commit is an `fsync`, which is §4's rule
verbatim: if it blocks, it gets a thread. So `cached_scan` and `cached_duration` sit on the
far side of `off_thread`, replacing the decode they wrap, and the app above them is unchanged —
a hit and a scan produce the same message. That is what kept this feature from touching the
transport, the widget or the queues at all.

`cached_facts` — what a queue edit learns about a track without decoding it — now asks the same
`ready` the files pane's marks are built from, which fixed a disagreement nobody had noticed
(Q44). It used to read the trims table and the tempos table one at a time, so a track scanned by
a build that knew nothing about tempi showed a playing time in a queue while its row in the pane
showed no `✓` at all. Two answers to "has this been scanned?" in two panes about one file.

The pruning pass is the one job in the app that gets a **bare `std::thread::spawn`** rather
than `off_thread`: nothing waits for the answer and nothing on screen changes, so there is no
message to send and no `Task` to carry it.

Two asymmetries are deliberate. A **failed scan is not stored** — the cache holds answers about
files, not the fact that one would not open, which is a condition that can change under it. A
**length that came back empty is** stored, because `audio::duration` has no error to report,
only the absence of a length, and that is exactly what stops the queues re-opening an
unreadable file on every edit for the rest of the run.

### What it actually bought, measured

| | first time | from the cache |
|---|---|---|
| a 3½-minute WAV, `--release` | **73 ms** | **54 µs** |
| a 3-second WAV, `--release` | 9.7 ms | 35 µs |

The read is flat because a stored array is at most 2048 columns whatever the track's length,
so the ratio grows with the file: **1 350×** for the WAV above, and an MP3 — 325 ms of
symphonia rather than 73 ms of PCM passthrough (§14a) — lands nearer six thousand. Both arrays
came back bit-identical to the scan that produced them, which is the property the `f32` storage
was chosen for.

---

## 11b. Preparing a folder (`fsio.rs`, `app.rs`, `ui/browser.rs`)

The cache of §11a fills itself as the app is used: a track is loaded, it is scanned, the answer
is kept. That is enough while the only thing waiting on it is a picture — a waveform arriving a
second late is a waveform arriving.

§7b broke that. Where a track's music starts has to be known **before** the track is loaded,
because the seek that uses it happens at the handover; and a queue measurement deliberately
does not work it out, because working it out means decoding the whole file and a queue edit
that decoded fifty tracks would freeze four threads for half a minute. So there has to be a way
to say *do all of this now*, and it is two buttons under the files pane:

- **Prepare folder** — every media file in the shown folder and everything under it, waveform,
  length and music edges, into the cache.
- **Clear cache** — throw all of it away, for every file, and start again.

### The walk

`fsio::media_tree` is the app's one recursive read, and it is deliberately not what the pane
does: the pane shows one folder because that is where the user is, and this walks the tree
because "prepare this folder" means an evening's music, which lives in a folder of albums.

**Symbolic links to folders are not followed**, which is the whole termination argument: a link
pointing at one of its own ancestors is an infinite tree, and `DirEntry::file_type` reports a
link as a link rather than as the folder behind it. A link to a *file* is still collected,
because the pane already lists those and playing one works.

The walk runs to completion before any decoding starts. That costs a second on a large tree and
buys a **total** — a count that grew as folders were discovered would show progress running
backwards, which is the one thing a progress display must not do.

### Four at a time, and no timer

A scan is a decode of an entire file — a third of a second for a typical MP3 (§14a). One at a
time leaves a folder of two thousand tracks running for ten minutes; one per core leaves
nothing for the audio callback of somebody who is playing a set while it works. Four is the
compromise, and it is a constant with the reasoning next to it.

The driver is a **chain of messages that refills itself**, not a subscription and not a job
queue: `scan_step` hands out as many files as the fan-out has room for, each answer calls it
again, and it clears itself when the last thread reports. Nothing ticks, nothing polls, and a
scan that is not running costs exactly nothing — the same rule every subscription in §4 follows,
reached without needing one.

Three counters, and they are separate because the files go out ahead of the answers: `next` is
what has been handed to a thread, `done` is what has come back — which is the number on screen,
because a count that jumped four ahead of the work would be a lie — and `running` is how many
are out, which is what the fan-out is capped against and what says when the scan is over.

**Stop cuts the list down to what has already gone out** rather than dropping the scan where it
stands. There is no way to interrupt a decode that does not mean checking a flag inside the
sample loop, so the four files in flight are going to finish either way; the counters stay
honest, their answers are kept, and the scan clears itself as the last of them lands. Dropping
the state instead would leave four threads reporting into a scan that no longer exists — and
starting another one before they landed would count their files twice and run `running` past
zero, which is a panic in a debug build and a very long scan in a release one.

### Clearing it

The mirror image, and the only button in the app that throws work away. It is one write
transaction that drops all three tables — asked about first with the same modal the duplicate
warning uses (§7a), and run off the GUI thread like every other touch of the store, because a
commit is an `fsync`.

What it does **not** clear is what the app has already learned this run. Those answers are
still true, and blanking them would wipe the waveform of a track that is playing. The disk is
what a clean start means: the next launch works everything out again.

---

## 11c. Saying which files are ready (`cache.rs`, `browser.rs`, `ui/browser.rs`)

§11b built the work and a count of it. What it did not build was an answer to the question the
count makes people ask: *which* files. "Prepared 412 files" is a receipt for a folder, not for a
track — and a fortnight later, after a relaunch, there is no receipt at all. The store knows,
and nothing on screen said so.

So the files pane grew one column, on the left, before the `♪`:

| Column | Meaning |
| --- | --- |
| `◐ ◓ ◑ ◒`, turning | a thread is decoding this file **now** |
| `✓`, green | the store holds a full scan of **this version** of the file |
| blank | neither |

One column for both, because they are the two ends of one sentence — this file is being worked
out, this file has been — and a row is never in both states at once. A leading column rather
than a trailing one so the marks line up at the pane's edge, which is what makes a prepared
folder readable without reading any of it.

### "Prepared" is every table or none of them

The mark tests exactly what `cached_scan` tests when it decides it has a hit: the waveform, the
music's edges *and* the tempo, for this exact stamp (§11a). Not "there is an entry for this
file" — a track the queues merely measured the length of has a row in `durations` and is still a
third of a second of decoding away. A mark that included it would be true about the database and
a lie about the thing the user cares about, which is whether loading this track will be instant.

That also makes the column a picture of §11a's staleness rule: edit a file and its mark goes,
because the stamp moved and the store no longer answers for what is on disk.

**And it is what a new fact costs.** Adding the tempo table (§14d) took every `✓` in every
already-prepared folder away until one more **Prepare folder** run put them back, because a scan
from a build that knew nothing about tempi is exactly the "still a whole decode away" case above.
That is the honest price and the right way round to pay it: the alternative was a `✓` that means
two different things depending on which build wrote the record, and a tempo column with permanent
holes in it that nothing but **Clear cache** would ever fill.

### Asked once per listing, off the thread, for no `stat` at all

`view` runs every frame; the store is a file. So the pane holds a **copy** of the answer, and
the answer is asked for once — when a listing lands, on a thread, like every other touch of the
cache.

The nice part is that it costs no filesystem work. A stamp is a size and a modified time, and
every row in the pane is *showing* both, because that is what the listing read them for. So
`cache::stamp_of` builds the same stamp `cache::stamp` would have stat'd for, and a folder of
four hundred files is asked about with zero `stat` calls. `stamp` is now written in terms of
`stamp_of` rather than beside it, because two functions computing "the same" stamp differently
would be a bug that looks like a cold cache.

The whole listing is asked about, not the visible rows: the hidden filter costs no filesystem
work by design (§9), so revealing a dotfile has to reveal its mark with it.

The same query now brings back **how long each file's music runs** (§14c), because it was
already reading the edges to decide the mark and the playing time is arithmetic on them. One
map rather than a set beside a map: the tick and the number are the same fact out of the same
two tables, and two containers would be two chances to hold one without the other.

### Optimistic between two listings, never stale the other way

Marks are added as work lands — a folder scan reports a file, or a track is loaded into a player
and scanned, which is the same work and gets the same mark. That is one rule for the whole
window: *a file with a thread on it spins, a file the store holds spins no more and gets a tick,
whoever asked for the decode.*

Adding on the way is one `ponytail:`-marked approximation: a successful scan is taken to be a
**stored** scan, and a file that cannot be stat'd is deliberately never cached, so its row is
marked and should not be. The alternative was a second store lookup per file to learn what the
decode had already established. It is self-correcting, because the listing's own question
*replaces* the set rather than adding to it — so the next refresh takes a wrong mark straight
back off, and a job that dies answers "nothing is prepared", which is the right way round to be
wrong. **Clear cache** empties the column for the same reason: it is a report of what is on
disk, and there is nothing on disk.

A refresh keeps the marks of the rows that survive it, rather than blinking the whole column off
while the store is asked again — the same `retain` the selection uses (§9a), for the same
reason. A row that the new listing lost takes its mark with it, or a folder of new files would
inherit the last one's answers.

### The spinner is one counter, already there

The turning glyph is driven by the same counter that sweeps the band across a player's strip
(§14a) — one phase for the whole window, so two scans running at once turn together instead of
drifting apart into something that looks like a rendering fault. Its subscription grew one
clause and no timer.

Four frames rather than a braille spinner's eight, because that counter takes 1.2 s to go round:
eight frames of 150 ms read as a flicker where four of 300 ms read as turning.

Which files are turning has to be **kept**, not derived. The four in the air are never
`files[done..next]` — answers come back out of order — so `Scanning` carries the paths as well
as the counts, and a row keeps spinning until its own file reports rather than until the next
one goes out.

### What did not get marks

The three queues. It was considered and declined: a queue row is a name and a length in a narrow
queue, the readiness of a queued track is already visible on the player it is waiting behind, and
it would be a fourth place the set has to be kept right. The files pane is where files are
chosen, so the files pane is where they are marked.

---

## 12. Testing

Rust's built-in `#[test]` / `#[cfg(test)]`, AAA pattern, no framework — same as cmote.
Pure logic is tested — including the pure arithmetic that lives inside a module which is not
otherwise pure; anything needing a device or a real folder is manual.

- **`mixer.rs`** — the required one. Both curves at both ends and the centre, each curve's
  defining identity at the midpoint (`g1² + g2² = 1` / `g1 + g2 = 1`), and the
  fader-at-zero invariant under either curve.
- **`paths.rs`** — the `.app/Contents/MacOS` walk-up returns the directory beside the
  bundle, and an ordinary exe path returns the directory beside the binary. Pure string /
  path arithmetic, so no bundle needs to exist to test it.
- **`settings.rs`** — a round trip, and every broken input (empty, truncated JSON, wrong
  types, not an object) reading as defaults rather than an error. Two more the
  implementation earned: a *missing* field keeps its default rather than failing the
  parse, so adding a field cannot invalidate a file someone already has; and an
  out-of-range value falls back **alone**, with the good values around it kept. The
  missing-field test names the newest fields specifically: a file written before the queue
  switches existed must read as "hand over, do not start", and one written before the
  handover choice existed as "play the file whole" — in both cases what the app did at the time
  (§7b), and one written before tempos could be corrected has corrected none (§14d). The
  corrections get a test of their own, because that map is the part of this file somebody really
  might edit by hand: a correction on a file that has been deleted or is not media at all is
  dropped, and so is a `0`, a negative and a `NaN` — while the good one beside them stays, which
  is the same "one bad value does not take the good ones with it" rule the faders follow.
- **`tree.rs`** — collapse takes the subtree and keeps the listings; `None` vs
  `Some(vec![])` survives a collapse/expand round trip. On what `expand` returns, the
  implementation split the case in two, and the split is the interesting part: **`expand`
  always asks for a re-list**, because a folder the user deliberately opens should show
  what is there *now*, while **`reveal`** — opening the ancestors of a folder chosen
  elsewhere — returns *exactly* the ones never listed, because nobody asked for that
  filesystem work. One test each.
- **`select.rs`** — the click rule (§9a), which is four lines and two of them are wrong in a
  way nobody notices until a set is half selected: which of the three a press is, including
  **shift winning over the command key** when both are held, and a range that is inclusive at
  both ends and does not care which way it was dragged. `defers` gets its own: exactly the
  press that would destroy what a drag is about to carry — plain, on a selected row — and
  nothing else, because both press arms and the release ask it (Q50) and an inline condition
  asked four times is the pair that drifts.
- **`browser.rs`** — extension → category (audio / video / other), the natural-numeric
  sort, the hidden filter. Then the selection (§9a): a plain click replacing, a command click
  adding *and* taking away, a shift-click that can be adjusted by shift-clicking again, and a
  range with no anchor falling back to a plain click. Two more say what the selection is *for*
  — it comes back in **row order** however it was clicked, since that is the order the actions
  run in, and a `.txt` can be selected but is never handed to a player. And the refresh case,
  rewritten: a listing that lost one of two selected rows keeps the other highlighted rather
  than clearing both. The prepared marks (§11c) follow the same rule and get the same test in
  one: a refresh keeps the mark of a row it still lists and drops the one it lost, a file worked
  out while the pane shows it is marked, a path the pane has never heard of is *not* — so a scan
  of a folder the user navigated away from cannot mark rows by name in the folder they are
  looking at now — and emptying the store empties the column. The playing time and the tempo
  ride in the same test (§14c, §14d), because they ride in the same map: a scanned file's seconds
  and beats come back together, a scanned *silent* one comes back marked with neither, and a file
  nobody has scanned is told apart from both.
- **`deck.rs`** — the transport state machine as a pure `transition(state, event)`, so
  every edge is checked with no audio device in the room. **`Seeked` is now one of those
  edges** (Q48): a seek leaves a playing player playing and a paused one paused, which is Q14's
  rule, and turns a *stopped* one into a paused one, which is not — `Stopped` in this app means
  at the top of the track, so a player labelled stopped and sitting at 1:30 is the label lying
  about where Play would start. That edge existed before this and lived in `app.rs`, where the
  state machine had never heard of it and nothing could check it.

  Four more test the *pairing* rather than the decision, which is what the `Option<Engine>`
  bought: pass `None` and what is left is the model. Pause keeps the playhead where the listener
  last heard it while Stop rewinds; the file running out and the Stop button agree about where
  the player ends up, which they did not quite before — the playhead was being zeroed in three
  places and one of the three did not do it; a seek on a stopped player moves the position *and*
  the label together, since the tick that would otherwise redraw the playhead does not run for a
  player that is not playing; and an empty player cannot be moved by anything, including a seek,
  which is the one that could have slipped through — its own rule is about `Stopped`, and it
  moves the playhead before it looks at the transport at all.
- **Drop policy** (`deck.rs`) — `drop_outcome(...) -> DropOutcome` pulled free of `self`,
  exactly as cmote pulled `drop_outcome` and `plan_uploads`, and tested for the folder /
  non-media / multi-file cases, plus the ordinary one that loads. **`idle_target`** — the
  `os_drop_target` of §10 — gets its own table: empty+empty → 1, loaded+empty → 2,
  playing+paused → the paused one, both playing → 1, and the invariant that it never
  names a playing player while an idle one exists.
- **`waveform.rs`** — the folding, the pixel fit and the scanning band (§14a), which between
  them are all the arithmetic the display has. That a scan stays inside its bounds however
  long the file is,
  that a halving keeps the loudest sample rather than smearing it, that a `NaN` from a
  decoder does not blank its column, and — the one that matters most, because its caller is
  a `draw` and a slice out of range there is a panic mid-frame — that **every column of
  every width is in range**, including widgets wider and narrower than the scan and a
  column past the end of its own width — and the same guard for the scanning band, which
  slides in from off one edge and out past the other and must never be drawn outside the
  strip on the way. `seek_fraction` is here too, and it is the test that **paid for itself
  immediately**: it caught that `f32::clamp` passes a `NaN` through unchanged, so the
  first version's clamp was decoration and a mis-measured strip would have panicked
  `Duration::mul_f32` on the click (§14b).

  `Edges` is here too, and it is the arithmetic §7b trusts with a cut (§14c). A leader and a
  run-out are found to the sample; a `NaN` and everything under the threshold read as silence,
  so a file of them has *no* edges rather than edges at its two ends; and one loud sample is
  enough to be one, which is the no-hold-time ceiling pinned rather than merely admitted. The
  fourth is the one that would ship silently wrong: **a channel is not a second**. The decoder
  interleaves, so a stereo file holds twice the sample rate per second, and getting that
  backwards puts every trim at twice its real depth — a handover that starts the next track
  halfway through its first verse, on stereo files only, which is all of them. A fifth reads
  the answer back the way the two panes do: `Trim::music()` on a file with four seconds of
  leader and two of run-out, plus edges the wrong way round giving `0:00` rather than the panic
  a `Duration` subtraction would otherwise be (§14c).

  `Tempo` is the third accumulator and gets three (§14d). The first builds a click track at 100,
  128 and 174 BPM and reads each back **to a fiftieth of a BPM**, which is the whole claim the
  second decimal makes — and 174 is in there on purpose, because 87 fits every other click exactly
  as well and the faster reading has to win. The second is the same trap `Edges` has: **a channel
  is not a beat either**, and reading a stereo file as mono is a tempo out by a factor of two,
  which is the one mistake a detector is not allowed to make quietly. The third is the two kinds
  of nothing — silence, a clip too short to hold two of the slowest beats it is asked about, a rate
  the decoder would not answer for, and a stream of `NaN`s, all of which have to be *no tempo*
  rather than a number somebody would act on.
- **`queue.rs`** — the queues (§7a), and almost every test is about the *selection* rather
  than the queue: an insert above it carries it down, a remove above it pulls it up, removing
  the selected row lands on whatever slid into its place, removing the last row falls back to
  the new last, and a shift takes the highlight with the track it moved. Plus the two rules
  around them — the arrows reach a neighbour and only a neighbour, and `next_source` prefers a
  player's own cue over the shared queue. A second `next_source` test pins the word *skipped* in
  the switches' rule: a queue with **Auto-load** off is passed over rather than treated as an
  empty queue that ends the handover, so a cue switched off still lets the shared queue feed that
  player. The remove test paid for itself on the first run: a
  `?` in a `match` arm was returning from `remove` itself, so the row was deleted, the
  function said nothing had been, and the selection pointed at a track that was gone.

  `relocate` — rows dragged within their own queue — gets four cases and then an exhaustive
  one. The four are the off-by-one in both directions, the drop past the last row, and the two
  carets that touch the dragged row itself (its own top and bottom edge, both meaning "leave
  it alone", and both easy to reach with a twitchy hand). The exhaustive one runs every
  `from` × `to` on a four-row queue and asserts the *contents* are unchanged as a set: a
  reorder that loses or duplicates a track is the one failure a queue cannot survive, and
  twenty-five cases is cheaper to run than to reason about.

  Multi-row adds a case and then thirty more (§9a). The case is a **block** dragged past a
  caret with rows of its own above it, which is the same off-by-one as before with a count
  instead of a one — and it checks the block keeps its own order, which lifting the rows out in
  the wrong direction would quietly reverse. The thirty are every *pair* of rows to every
  caret, asserting the contents again and that both rows come out highlighted where they
  landed — or that the selection is untouched when the move was a no-op, which is the
  distinction the first version of that assertion got wrong and the test caught.

  The selection itself gets one test per gesture, mirroring the pane's (§9a): a press with
  each of the three modifiers, and a row that is not there selecting nothing. `shift_selected`
  gets three — a single row carrying its highlight, a block of two moving as a block, and a
  *scattered* pair each moving past the row below it — plus the blocking rule, which is that a
  selection touching the end it is moving towards blocks the whole move rather than part of it.
  `take_rows` gets the case the duplicate warning makes real: taking **some** of a selection,
  where the rows left behind keep their highlight and an index handed in twice takes one row
  rather than whatever slid into its place.

  `duplicates` gets the batch half of the search: the positions of the tracks already queued,
  in a batch where two of four are, so a caller can filter rows and tracks by the same test.

  The running time gets two more (§7a). One walks a queue from nothing measured to fully
  measured and checks the *flag* at every step, including the state that is easy to get wrong:
  a row that has been measured and has no length keeps the `+` on for ever, because "asked and
  answered nothing" is not "counted". The other queues the same track twice and confirms one
  answer settles both rows and nothing else — which is the reason `measured` works by path
  where everything above it works by index.

  A third pins the two halves of `measured` apart (§14c, §14d). A row is measured the moment it
  is queued and scanned much later, if ever, so the length is settled once and kept while the
  scan has to be allowed to land on a row that already has one — the order it actually happens
  in, and the one a single `duration.is_none()` filter would have made impossible. The same test
  checks a later queue edit, which only reads the store, cannot take it away again. It is now
  *one* fact rather than two (Q45): the playing time and the tempo were separate fields with the
  same rule applied to each, and a row holding one without the other was never a state anything
  could produce.

  `hands_over_early` gets one, and it is three conditions rather than a number (§7b): the music
  has stopped *and* this queue asked to skip the blanks *and* somebody has scanned the track.
  It pins the two silent cases especially — a queue set to **Whole track** never cuts however
  far past the music the playhead is, and a track nothing has scanned plays whole for ever
  rather than being cut at zero, which is what a missing trim read as "the music ends at the
  start" would do.
- **`queues.rs`** — everything that is true of the three queues together rather than of any one
  of them (Q47), which is where most of §7a's rules turned out to live.

  The arrows reach a neighbour and only a neighbour, so a track cannot jump from one player's
  cue to the other's in one press. `next_source` prefers a player's own cue over the shared
  queue, falls through to the shared queue when the cue is empty, stops the player when both are,
  and never offers the *other* player's cue. A second test pins the word *skipped* in the
  switches' rule: a queue with **Auto-load** off is passed over rather than treated as an empty
  queue that ends the handover, so a cue switched off still lets the shared queue feed that
  player, while both switched off is a player stopping with full queues in front of it.

  `already_queued` gets two, and the second is the one that matters. The first is the ordinary
  search: a track is found in whichever of the three queues actually holds it, not merely in the
  one being added to, and a track nothing holds is found nowhere. The second is the exception —
  a row on its way out of a queue must not warn about colliding with *itself*, or every
  cross-queue move would ask a question with one honest answer — and it pins that the exception
  is the **row** and not the track, by leaving a second copy elsewhere and checking it is still
  found. `duplicates` gets the batch form: positions into the batch, so a caller can filter rows
  and tracks by the same answer.

  `take_unmeasured` gets the two halves of "ask about each file once", and the rename is part of
  the test: a track sitting in two queues produces one entry rather than two, and **asking twice
  in a row gives the batch and then nothing**, because recording what went out is part of asking
  rather than a second line beside it. A third checks that a row measured and answered *nothing*
  is never asked about again either, which is the other thing the two-layer
  `Option<Option<Duration>>` is for. And one answer settles every row holding that track,
  including the same track queued twice in one queue — the reason `measured` works by path where
  everything above it works by index. The scroll offsets get one: three panels, three offsets,
  or scrolling one would scroll all of them.
- **`fsio.rs`** — one test, for the one rule in the module (§11b): the recursive walk finds
  media at every depth, in the pane's own order, and nothing that is not media at any depth.
  Plus the boundary it draws around failure — an unreadable **root** is an error, because a
  scan that silently found nothing there is indistinguishable from a folder with no music in
  it, while a folder deeper down that cannot be read is skipped in silence.
- **`ui/mod.rs`** — `visible_rows(scroll, total, row_height, built)`, the virtualization's
  whole arithmetic, shared by the files pane and the three queues (§9). A range wrong by one
  row leaves a blank strip where a row should be; wrong by a lot shows an empty pane over a
  full folder. So: a list shorter than the cap is built whole, scrolling moves the window by
  whole rows and a partly visible row counts as visible, the end of a long list still fills
  the pane rather than running off it, and an impossible offset — negative, `NaN`, infinite —
  still names real rows, which is the `as usize` saturation this leans on being pinned rather
  than assumed. One case exists only because the function is shared: the same offset names a
  different row at a pitch of 22 than at 24, which is what a hard-coded constant in a shared
  helper would have got quietly wrong. `spinner(phase)` is the other pure thing here (§11c):
  one whole turn at the rate the sweep counter actually ticks shows **all four frames, in
  order** — a spinner that skipped one would still animate, which is why this counts them —
  and the values no caller sends today are pinned too, because this indexes an array and a
  phase of exactly 1 would be a panic inside a `draw`. `format_lengths(music, duration)` is the
  third (§14c), and it is here rather than in either pane because it is what makes the two agree:
  the six states a row can be in, from blank through `--:--` and a plain length to
  `2:58 / 3:15`, including the one order that would otherwise print a separator with nothing
  after it — scanned before it was measured. `format_tempo` gets the same treatment (§14d): the
  two kinds of nothing, and a number that is always two decimals — including a round 128, a
  127.999 that rounds up to it, and a hundredth landing exactly on the boundary. `corrected_tempo` is
  the third, and it is the whole of how a correction reaches a row: it replaces whatever the
  detector said, including the `--` of a file scanned and found to beat at nothing, and it shows
  on a file nothing has scanned at all — while a file nobody corrected comes back in exactly the
  state it went in, in all three of its states.
- **`cache.rs`** — two halves, and both are needed (§11a). The encoding is pure and is checked
  without a database: a record that survives a round trip *exactly*, since a cached waveform is
  supposed to be the same array a scan produced; a stamp that does not match reading as a miss,
  for both a changed length and a changed timestamp; a record carrying another format byte
  read as a miss rather than as an array of plausible noise; a length and the *absence* of one
  told apart; and a payload that is not a whole number of `f32`s thrown away rather than
  truncated, because a waveform missing its last column would draw without complaining.

  Then one pass over a **real database in a temporary folder**, because "the bytes are right"
  and "redb was asked the right question" are different claims: store a whole `Scan` and get the
  same one back, a file that was never cached, then rewrite the fixture and watch its entry stop
  answering, then delete a second fixture and watch `prune` drop exactly its entries — **three
  of them, one per table**, which is the assertion that says `store_scan` really wrote all three
  and not just the first — and `clear` the lot, with the store still usable afterwards, since a
  cleared cache is a cache and not a corpse (§11b). One case in that pass is written through the
  private `write` rather than the public interface, on purpose (Q44): a file with a waveform and
  a tempo and *no edges* is the one state `store_scan` can no longer produce, and it is exactly
  the state a store written by an older build is in — so the test reaches past the door to build
  it, and then checks the door refuses it. The trim
  encoding gets its own pure test for the halves that matter: "scanned, and this file is
  silent" told apart from "never scanned", and **half a trim** — eight bytes where sixteen
  belong — thrown away rather than read as a start with no end. The tempo encoding gets the same
  (§14d), plus the two values that would print as a tempo nobody could act on: a stored `NaN` and
  a stored negative are misses, so the record is thrown away and the file is scanned again. The same pass now also asks
  what the files pane asks (§11c): `prepared` names the file that has every table and not the
  one that has only a waveform and a tempo, and a listing's own size and modified time build the
  *same* stamp a `stat` does — the equality the whole no-filesystem-work claim rests on, and a
  silent drift there would show as a folder that is never marked and never explains why. And it
  reads the playing time and the tempo back out of the same answer (§14c, §14d): a scanned file
  gives the seconds between its edges and the beats it runs at, one scanned and found silent gives
  a mark with neither, and the one still missing a table gives nothing at all.
- **`ui/queue.rs`** — `running_time`: how many tracks, how long they run, and the `+` that
  says the total is a floor rather than a figure. An empty queue says *nothing at all* rather
  than `0 · 0:00`, because three empty panels each announcing their emptiness is furniture.
  `arrows(first, last, count)` is the second (§9a), and it is two off-by-ones against opposite
  ends of the same queue — `> 0` at the top and `+ 1 < count` at the bottom. One test covers the
  ends, the middle, the blocks that touch each end, the whole queue selected, and the single row
  of a single-row queue, which is where an off-by-one shows first. An arrow live one row too far
  is a block moved off the end of the queue it is in.
- **`ui/browser.rs`, `ui/deck.rs`, `ui/tempo.rs`** — the decisions that were sitting inside a
  `view`, lifted out and given the tests §12 has asked for all along (Q42). Four rules, and each
  one had a way of being wrong that no glance at the screen would catch.

  `mark_of(working, prepared)` is the leading column's three-way (§11c), and the case worth
  writing down is `(true, true)`: **Prepare folder** over a folder already prepared re-reads
  every file, so a row is both at once, and the one that says `✓` through all of it is lying
  about what is happening to it. Working wins. It also stopped being a string: the green that
  means *ready* is now chosen by asking which state the row is in rather than by comparing the
  glyph against `"✓"`.

  `load_label(id, count)` is §9a's rule that the count appears only once there is more than one
  — a `(1)` on the commonest click there is would be noise, and a dead button reads as the plain
  promise it will make when it wakes up.

  `music_span(length, trim)` and `progress(position, length)` are `ui/deck.rs`'s two divides
  (§14c, §7). They now take the two numbers rather than the `Deck` they came off, which is what
  lets the interesting cases be written at all: a stream with no length, a zero-length track, a
  stale trim whose edges sit past the end of the file — clamped to the strip, because the caller
  is a `draw` — and a playhead a tick past its own total, which the app really does produce
  twenty times a second at the end of every track. `music_span`'s guard changed while it was
  being written: `total <= 0.0` lets a `NaN` through and `f32::clamp` passes one straight on,
  which is the exact trap `seek_fraction` fell into in §14b, so it is `(total > 0.0).then(…)`
  like its neighbour.

  `detected_line(value, detected)` is the quiet line under the tempo editor (§14d), and writing
  the test changed the code: it compared with `f32::EPSILON`, about 1.2e-7, where the line is
  about what the panel *shows* to two decimals. So it compares the two rendered strings, and
  "detected 128.00" can no longer appear under a 128.00.
- **`audio.rs`** — a second test that needs no output device, beside the scan (§14a): a
  generated three-second WAV is measured through `duration`, and a path that does not exist
  answers `None` rather than failing. The queues' whole running time is built out of that one
  answer, so a `None` from a perfectly readable file would leave every total with a `+` on it
  and nothing to say why.
- **`ui/waveform.rs`** — the other test under `ui/`, and the pair of them show the rule:
  everything else in that folder is composition, but these two files hold arithmetic and a
  *gesture*, and both have rules. `scrub(event, over, scrubbing)` is pure, so all three are
  checked
  without a window — a press arms only over the strip, a move seeks only while the button is
  held (and follows it *outside* the strip, which is what lets a drag run past either end),
  a release disarms wherever it happens, and the events that pass through in quantity — the
  right button, the cursor leaving — do nothing even mid-scrub (§14b). Those four passed
  while every click seeked twice, which is the lesson the fifth test pins: the four events
  macOS really sends for one click, replayed **in order**, because each of them was already
  handled correctly on its own and only the sequence was wrong (§14b).
- **`app.rs`** — what a *divider drag* is allowed to store (§6), and which key means
  refresh (§9). Nothing else here is arithmetic any more: the players' height reaches the
  widget as a literal and iced does the compacting. So the tests cover the one calculation
  left, and the one branch that came after it. That a
  drag inside the bounds is stored untouched — `assert_eq!`, not a tolerance, or the panel
  would creep a pixel every time it was grabbed; that a drag far below the window still
  leaves the browser `MIN_PANE`, at every window height from 400 to 2000, which is what
  keeps the divider on screen and grabbable; that a drag above the window's top reads as the
  floor rather than as a panel of nothing; and that an impossible window — zero, shorter than
  its own chrome, or `NaN` — stores a finite height instead of a `NaN`. The keyboard
  gets a table: **F5** with and without modifiers, **⌘R**, **⌘A** and **Escape** (§9a), and
  the near-misses that must *not* fire — a bare `r` or `a` above all, since a plain letter
  that re-listed the folder or selected every row would go off on any stray key press. The
  table is written in `Modifiers::COMMAND` rather than in `LOGO` or `CTRL`, so the same test
  asserts Cmd on macOS and Ctrl on Windows.

  The folder scan's bookkeeping is the third (§11b), and it is three counters with no timer:
  the `Task`s the driver returns need a window, but the arithmetic deciding how many there are
  does not. One test runs a whole ten-file scan an answer at a time and asserts the fan-out is
  never exceeded, every file goes out exactly once, and no thread is left unaccounted for. The
  other is **Stop**: nothing more goes out, the scan is not over until the four already
  decoding report, and each of them is counted once — the case that would otherwise underflow
  `running` the moment a second scan was started on top of a cancelled one. That second test
  now reports its four **out of order**, which is the case the busy list exists for (§11c): the
  files in the air are never `files[done..next]`, so the assertion is that exactly one row stops
  spinning per answer and none is left turning at the end.
- **`audio.rs`** — one test, and the only one this module can have: everything else here
  needs an output device. A *scan* does not, so the decode path is checked for real, from a
  file on disk to the array the widget draws. The fixture is generated rather than
  committed — one second of digital silence then one second at full scale, written as
  sixteen-bit PCM by twelve lines of header — because a binary in the repo is a fixture
  nobody can read a diff of. The same fixture now checks the *other* half of a scan (§14c):
  the join the array can only place "somewhere in this column" is asserted as a time, exactly
  — the music starts at one second and runs to two — which is the whole reason the edges are
  counted per sample.
- **Manual smoke test**, documented in the README: load both players, play both, sweep
  the crossfader on **both curves**, stop and re-play, load a `.mp4`, fold the tree, drag a
  row to a player, drag a file in from Finder, pull the audio device — and the
  **portability check**: copy the binary to an empty folder, run it, confirm
  `clecta-data/settings.json` appears *there* and nothing appears in `~/Library` or the
  registry. **The folder half is confirmed on macOS, both ways**: a bare release binary
  copied to an empty folder creates `clecta-data/` beside itself, and a `Clecta.app` from
  `bundle-macos.sh` creates it beside the *bundle* — the walk-up of §11, run rather than
  argued. `~/Library/Application Support/clecta` does not exist after either. **⌘Q came
  off this list by failing**: the app menu's **Quit** wrote nothing at all, which is what
  forced the throttle in §11. The *file* half is confirmed too, and without a click —
  launch with a settings file asking for a window larger than the display, let the OS
  resize it, and the app writes `2005×1227` along with the faders and the curve **while
  still running**; killing rather than closing it proves only the throttle can have
  written that. Asking for 15000×15000 instead is how the wgpu ceiling in §11 was found.
  The close *button* path and the drop gestures needed a person, and a person has now run
  them on macOS: the button writes, the ring lights one player and never both, a drag let
  go over nothing disarms, and a release over a button does not leave it armed.
  **The waveform came off this list the way ⌘Q did — by failing.** Its numbers were checked
  three ways before anyone looked: the unit tests above, the generated-WAV decode, and a
  scratch build that auto-loaded a file with a known envelope and printed what the running
  app scanned (1723 columns, peaks of 0.928 and 0.586 exactly where the envelope put them,
  laid out at 424×56, drawn at 20 Hz for forty seconds with no panic). All three agreed,
  all three were right, and the strip was blank — a contrast mistake, told in §14a.
  `screencapture` from a script returns a black frame, the same permission wall as the
  scripted-click attempt above, so there was no way to close that gap without a person.
  Two attempts is what it cost, and it is the second time on this list that the thing the
  automation could not reach is the thing that was broken. It is also the shape of the whole
  list: everything a script could check was right, and both defects that shipped were in the
  part only an eye could see.
  **The splitter made it three.** The seek gesture and the mixer's preset buttons passed on
  the next pass, and the players' section did not: it held its height across a resize and
  *wobbled* getting there, scaling with the window and snapping back on every frame. Its
  arithmetic had been probed twice and was exact both times — the defect was a frame of
  latency between the two, which no unit test can have an opinion about (§6, Q16).

  **Every macOS item on this list now passes.** What no macOS run can say anything about is
  **Windows** — the first-class target of §1, type-checked by CI on every push and never
  once executed. Three sites exist only for it: the `windows_subsystem` attribute in
  `main.rs`, `paths::per_user()`, and the drive-letter walk in `fsio::roots()`. A compiler
  that accepts them is not a machine that has run them.

**CI** mirrors cmote's `.github/workflows/ci.yml` — four jobs: `rustfmt` on Linux,
clippy + test on Windows natively, clippy against **`x86_64-apple-darwin`** plus a native
test run on the Apple Silicon macOS runner, and `cargo deny` + `cargo audit` on Linux.
`defaults.run.working-directory: desktop` covers the `run:` steps; the two actions that
do not use a shell — `rust-cache` and `cargo-deny-action` — are given the path
explicitly, which is the one thing that does not carry over from a repo whose crate is at
the root.

**What the supply-chain gate actually guards** (`deny.toml`), beyond copying cmote's:

- **`MPL-2.0` in the allow-list** — symphonia, all thirteen crates of it. Not a
  formality: it is the one non-permissive licence clecta ships, and while file-level
  copyleft means linking it in does not make clecta MPL, §3.2 does mean whoever gets the
  binary must be able to get symphonia's source. Unmodified crates.io releases, so
  upstream answers it — the obligation only grows teeth if a decoder is ever patched.
  The rest of the list is the *minimal* set: each entry is demanded outright by some
  crate, not merely as one arm of an `OR`.
- **`cc` and `cmake` banned.** The no-C-toolchain property (§11) is the one thing that
  makes "copy it anywhere and run it" true, and it holds today partly by luck. `cc` is
  the tell — it is the crate that shells out to a C compiler — and it is in the lock file
  *already*, under `android-activity` and `wayland-backend`, neither of which is a
  shipped target. The `[graph] targets` prune is what keeps it out, so the ban is a
  tripwire: the day a dependency needs a C compiler on Windows or macOS, CI says so
  instead of the property quietly dying.

  **That day came, and the tripwire worked** (§11a). The obvious store for a waveform cache
  is SQLite, and `rusqlite` pulls `libsqlite3-sys`, which compiles C on both shipped targets.
  The ban turned "a dependency that quietly ends portability" into a decision someone had to
  make out loud, and the answer was `redb` — the same shape without the compiler. Worth
  recording because a tripwire nobody ever trips is indistinguishable from one that does not
  work.
- **No release build in CI.** The README's fourth local check is deliberately not a job:
  `clippy --all-targets` already type-checks everything, and `lto` + `codegen-units = 1`
  costs minutes per run to re-prove a thing that only matters at release time.

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
- **Cue points, loops, tempo / pitch *adjustment*.** Real DJ features. Changing a tempo needs a
  time-stretch stage rodio does not have (`rubato` or a phase vocoder). **BPM detection came off
  this list** — §14d, because the decode that finds it was already running for the waveform;
  *playing* a track at a different one is still the hard half.
- **Typing a tempo in.** Ruled out rather than deferred, strictly (§14d): an octave error is
  the only mistake the detector makes that a person can spot at a glance, and `/2` / `×2`
  reverse it exactly. Reopens the day a real track comes back wrong by something that is not
  a power of two — a 3/4 heard as 4/4 is ×1.333, which two buttons cannot reach.
- **Headphone cue / pre-listen.** Needs a *second* output device and a second mixer —
  the point at which the "one shared stream" decision in §4 has to be revisited.
- **Watching the tree.** The files pane watches the folder it shows (§9); the tree does not
  watch the folders it lists. One watcher per expanded folder against one for the app, for a
  pane whose contents change far less often.
- **Recursive / multi-file drops**, and drag-*out* to the desktop (iced cannot originate
  an OS drag at all — cmote §29 there).
- **Positional OS drops** — blocked upstream in winit, not in clecta (§10). If winit ever
  surfaces `draggingLocation` / `IDropTarget`'s `pt`, `os_drop_target` demotes to the
  fallback for "released somewhere that is not a player" and the aiming starts working
  with no other change.
- **Auto-recovery from a device change**, rather than the notice line in §11.

---

## 14a. The waveform (`waveform.rs`, `ui/waveform.rs`)

The first thing off §14's deferred list, and the reason it was picked first: it is the only
part of the app that iced does not already have a widget for, so it is where the framework
stops being a catalogue and starts being a library.

### The scan does not know how long the file is

A waveform is a peak array, and the obvious way to build one is *samples ÷ columns*. That
division needs the sample count, and there is not always one: §7 already records that
`total_duration()` answers `None` for a stream, and even when it answers, trusting it to
predict how many samples a decoder will actually emit is a bet with no upside.

So `waveform::Fold` never divides. Every sample starts as its own column, and when the
array fills to `MAX_COLUMNS` it is **halved** — pairs folded to their maximum — doubling
what one column stands for. That is a mipmap built in one pass, and it costs almost
nothing: each halving touches half as many elements as the one before, so all of them
together cost less than one more pass over the finished array. The result is between half
and all of `MAX_COLUMNS` for a file of any length, from a two-second sample to an hour-long
set, with no branch anywhere for "unknown duration".

**Maximum, never average**, at both ends — folding a pair and squeezing the array into a
narrow panel. Averaging a waveform down to a few hundred pixels flattens exactly the
transients it exists to show; a kick drum becomes a bump.

### Where the seconds go

`audio::peaks` is a *second, independent* decode of a file that is already loaded. It has
to be: the playing decoder cannot be read twice, and reading it would move the playhead.
It decodes every sample and throws them all away as it goes, so the memory cost is the
array and nothing else — but the time cost is real, which is why it is the third thing in
§4's diagram to run off the GUI thread.

How real, measured on one 3½-minute MP3:

| build | scan |
|---|---|
| `cargo run`, as first written | **16 745 ms** |
| `cargo run`, with `[profile.dev.package."*"] opt-level = 3` | **509 ms** |
| `--release` | **325 ms** |

Those three lines of `Cargo.toml` are the single largest change in this section, and they
are worth understanding rather than copying: symphonia is arithmetic in a tight loop, and
unoptimized arithmetic is *fifty times* arithmetic. Optimizing only the **dependencies**
keeps clecta's own code at `opt-level = 0`, so it stays debuggable and still rebuilds in
seconds; the dependencies compile once and cache. Any project whose real work happens
inside a dependency wants this, and a learning project is exactly where "debug builds are
slow" gets mistaken for "my code is slow".

### The single-threaded executor

`Task::perform` was the wrong tool, and the way that showed up is worth recording: pressing
Play during a scan produced *audio with a frozen clock*. The transport said "playing", the
sound came out, and `0:00 / 3:42` sat there.

iced's smol backend spawns `SMOL_THREADS` worker threads and **defaults to one**. So a scan
inside an async block did not merely queue the next directory listing, which is what the
first `ponytail:` note here guessed — it stopped the whole runtime. The 20 Hz playhead tick
is a subscription; so is the autosave. Both stopped dead. Measured from the moment a scan
starts:

| scan runs on | ticks during the scan |
|---|---|
| the executor (`Task::perform`) | **641 ms of nothing**, then a dozen delivered in the same millisecond |
| its own `std::thread` | a steady **49–51 ms**, throughout |

So the decode gets a real thread and the executor gets a `oneshot` to await, which is the
one thing it is good at. Two threads at most, one per player, each living exactly as long
as the scan that spawned it.

The general lesson is not "smol is bad" — it is that **`Task::perform` is for `await`, not
for work.** Anything that occupies a CPU belongs on a thread, and an async runtime with one
worker turns "this is a bit slow" into "the app is frozen".

That lesson was applied by halves at first: the scan moved, and the two directory reads
stayed behind under a `ponytail:` note betting they were short enough. They were not — 25 ms
for a 5 000-file folder, 95 ms for 20 000 — so the pattern is now a shared `off_thread` in
`app.rs` and all three jobs use it. The rule is easier to keep than the bet was: **if it
blocks, it gets a thread**, and there is no per-call judgement left to get wrong.

### Saying that a scan is running

Half a second of nothing is long enough to look broken, so the strip animates: a band
travelling along the flat centre line, `sweep_band` in `waveform.rs` and one more
`fill_quad` in the widget. It is drawn *in the strip* rather than written in the status bar
because the strip is the thing being waited for, and it needs no words in any language.

Two details are deliberate. There is **no threshold** — the band appears the moment a scan
starts rather than after 250 ms, because the gate costs a timestamp per player to spare a
one-frame flash on a file short enough not to matter. And `Deck::scanning` is a real field
rather than `peaks.is_empty()`, because an empty player and a *failed* scan both have no
peaks and neither of them is working on anything.

The animation is a plain integer counter advanced by a `Sweep` message, so nothing in
`view` reads a clock, and its subscription follows the same rule as the tick and the
autosave: it exists only while it is needed, and nothing animates at rest.

It is a free function rather than an `Engine` method, and that is not tidiness: a scan
needs no output device, so the waveform still appears while the app is saying "no audio"
(§11).

Two traps came with running it in the background, both of which are the same trap:

- **A scan can outlive the track that started it.** A player can be given a second file
  inside the seconds the first takes to scan. The message carries the path it was started
  for, and an array that no longer matches what is loaded is dropped — the pattern §9
  already uses for a directory listing that arrives after the user has navigated away.
- **The old array must go at load time, not at scan time.** Clearing `peaks` when the new
  scan *lands* would leave the outgoing track's shape on screen under the incoming track's
  playhead for a few seconds, which reads as a bug rather than as waiting.

### The widget is three methods — then five, then seven

`ui/waveform.rs` first implemented `advanced::Widget` with `size`, `layout` and `draw`.
Everything else the trait asks for has a default that is already right for a widget with
no children, no state and no events — which is worth saying out loud, because the trait
looks like nine methods of work and is not. Making it clickable (§14b) added exactly two
more and no state; making it *draggable* added `tag` and `state` for a single `bool`. Three,
five, seven — and the shape of that sequence is the point: each capability paid for itself
separately, and none of them made the previous one more complicated.

`layout` is `layout::atomic`: no children to place, no intrinsic size to negotiate, take
the width offered and the fixed height asked for. `draw` is `fill_quad` and nothing else —
the same primitive every built-in widget's background is made of, so this is not reaching
past iced into wgpu. One quad for the bed, one per pixel column, one for the playhead.

Two deliberate consequences:

- **Bars are drawn per *pixel*, not per scan column**, which is why `column_peak` exists
  and takes the width as an argument. A panel is a few hundred pixels wide and the scan is
  a couple of thousand columns; drawing one quad per column would be three times the work
  and would alias into a grey smear.
- **It is implemented for the concrete `Theme`**, not a generic one, because the colours
  are read from its palette. Naming roles rather than colours is what keeps the strip
  legible if the theme ever stops being `Dark`.

`advanced` is a *feature toggle* on crates already in the tree — turning it on adds nothing
to `Cargo.lock`, which is checked. The waveform costs no supply chain (§12).

### A role is not a contrast

The first version of this widget shipped invisible. Every number was right — 424×56 of
layout, 1723 columns of scan, a playhead at the correct fraction — and what appeared on
screen was a red vertical line on an empty panel. Nothing else.

The cause was the palette, chosen by reading role names and never by comparing values. The
bed was `background.weak`, which is *exactly* what `container::rounded_box` paints the
panel behind it, so the bed did not exist. The bars were `background.strong`, which in the
Dark theme is `#50545d` against that panel's `#43464e` — thirteen levels of grey, in a bar
one pixel wide.

Printing the palette is what settled it, and the four roles are now picked by comparing:

| part | role | dark theme |
|---|---|---|
| the bed | `background.weakest` | `#323439`, darker than the panel's `#43464e` |
| not yet played | `secondary.base` | `#878a90` |
| already played | `primary.base` | `#5865f2` |
| the playhead | `danger.base` | `#c3423f` |

The empty strip gained a flat centre line at the same time, for the same reason: a bare
rectangle reads as broken rather than as waiting, and "waiting" is the honest state for
the seconds a long track spends being scanned.

Worth saying plainly, because it is the transferable part: **§12's verification was
thorough and could not have caught this.** Seven unit tests, a real-file decode, and a
printout from the running app all agreed, and all of them were measuring numbers that were
already correct. The defect was entirely in the mapping from correct numbers to visible
pixels, and the only instrument for that is an eye. It took one glance to find what none of
the automation could.

---

## 14b. Scrubbing (`ui/waveform.rs`, `audio.rs`)

§14 deferred this and predicted what it would cost: "a `Widget` that handles events needs
`update` and a `Shell`". That was right, and for the *click* it was the whole bill — two
methods, no state, and a strip that was a picture became a control. **Dragging** came
after, and cost exactly what §14 said it would and nothing more: `tag` and `state` for one
`bool`. The section keeps both halves in the order they happened, because the second one is
the cheaper story only because the first one had already decided everything hard.

### Seeking is the one thing that does not touch the transport

The rule the user asked for, and the one worth writing down: **a click moves the playhead
and nothing else.** Playing keeps playing from the new place; paused stays paused there;
stopped stays stopped there. So `seek` is deliberately *not* a `deck::Event` and never
reaches `transition`. §7's state machine has no edge for it because there is no edge to
have — the transport is unchanged by definition, and adding a self-edge to every state
would be writing down "nothing happens" four times.

That also decides the rodio call. `stop` pauses *before* it seeks, for a reason the spike
paid for: control changes land on a 5 ms tick and seeking first lets the callback play on
from zero until the pause catches up. `seek` must not borrow that trick. A pause would be
audible — a playing player would gap, and a paused one would need a `play` afterwards that
the user never asked for. The price is that the landing point can be a tick out, which is
1/200th of a second in a track and a fifth of a pixel in the strip.

### The playhead has to be set by hand

`deck.position` is normally whatever the last tick polled, and **the tick only runs while
something plays** (§4). So a seek on a paused player would move the audio and leave the red
line exactly where it was until someone pressed Play — a click that visibly does nothing,
which is worse than a strip that is not clickable at all. `seek` writes the position itself.

### `clamp` is not a range guarantee

`seek_fraction` lives in `waveform.rs` with the rest of the arithmetic, and it exists for
one reason: the caller multiplies a `Duration` by its result, and `Duration::mul_f32`
**panics** on a `NaN` rather than saturating. A zero-width strip divides to `NaN`, so the
first version guarded the width and clamped the result.

The test found that this was not enough, which is the point of writing the test:

```
width 400, x NaN gave Some(NaN)
```

**`f32::clamp` passes a `NaN` straight through.** It is `if self < min … else if self > max
… else self`, and every comparison against `NaN` is false, so `NaN` falls out of the `else`.
A clamp reads like a range guarantee and is not one. The guard is now a range *test* on the
way out — the same `!(0.0..=1.0).contains(…)` shape `settings.rs` already uses on a
hand-edited file, and for exactly the same reason: this is a trust boundary, not a
formality. The app-side handler repeats the test rather than trusting the widget, because
it is the code that does the multiplying.

### What the methods actually do

- **`update`** acts on `ButtonPressed`, not `ButtonReleased`. A transport control should
  answer as the button goes down; waiting for the release makes a click that drifted three
  pixels feel like it landed somewhere else. It publishes a *fraction*, not a time: the
  widget does not know what a second is, and the track's length lives in the `Deck`. It
  captures the event afterwards — nothing under the strip handles a left press today, but
  a widget that acted on a click saying so is the contract every built-in control keeps.
- **`mouse_interaction`** returns `Pointer` over a seekable strip and `None` otherwise,
  where "seekable" is `progress.is_some()` — the same test that decides whether a playhead
  is drawn, and not a coincidence: a strip with no total to place a playhead against has no
  total to seek within. An empty player therefore reads as not-a-control without needing a
  greyed-out look. It also stays `Pointer` for as long as a scrub is held, wherever the
  cursor has wandered to: the gesture still belongs to this strip, and a cursor that changed
  shape halfway through a drag would say it had been dropped.
- **`tag` / `state`** are the whole cost of dragging: one `bool`, `scrubbing`, in the
  widget's `Tree` state. Not a field on the struct — the struct is rebuilt from scratch
  every frame by `view`, so anything written into it is gone before the next event arrives.
  `Tree` state is where iced keeps the thing a widget has to remember *between* frames, and
  a held mouse button is the definition of that.

### A drag is the same seek, more often

This is the part worth writing down, because it is why the diff was small. A scrub needed
**no new message and no new arm in the app**. The widget already published a fraction on a
press; dragging publishes the same fraction on each move, and `Clecta::seek` cannot tell the
difference. Everything the click had already settled — the fraction rather than a time, the
range test on the way out, the playhead written by hand because the tick is not running —
is settled for the drag too. The lesson generalises: a gesture that produces the *same*
message its click already produced is nearly free; one that needs a new message is not.

The rules of the gesture are pure and tested, in a `scrub` function beside the widget, for
the same reason `deck::transition` sits beside the app. There are only three of them and
each is wrong in a way only a window would show:

- **A press arms only when the pointer is over the strip.** A press anywhere else in the
  window belongs to something else; arming on it would make the next mouse move seek a
  track nobody touched.
- **A move follows wherever it goes, once armed** — over the panel, past either end, out of
  the window. `seek_fraction` clamps, so leaving the strip parks the playhead at the edge it
  left by, which is what makes a scrub forgiving of a hand that wanders. The clamp was
  already tested for this: the test's own comment said "reachable while a button is held and
  dragged", months before anything could hold and drag.
- **A release disarms wherever it happens.** Over the mixer, over the browser, off the
  window — a button let go is let go, and a strip left armed would scrub on the next stray
  move. This is the rule that cannot be tested by trying it once and it working.

### Every click seeked twice

Three correct rules and a fourth event nobody knew about. Clicking a strip while a track
played replayed a tenth of a second from the click target — a short repeat, over the audio
that was already right, which is the sound of a player being sent to the same place twice.

The cause is not in this file at all. winit's macOS backend emits a `CursorMoved` *before*
every `MouseInput`, on the down and on the up alike:

```rust
#[method(mouseUp:)]
fn mouse_up(&self, event: &NSEvent) {
    self.mouse_motion(event);              // ← a CursorMoved, every time
    self.mouse_click(event, ElementState::Released);
}
```

It has a reason — a window entered from another window used not to receive `mouseMoved:`, so
the click carried the position instead (winit #1490) — and it means a single click arrives as
**four** events, not two. The third is a move at the position the press already handled, and
the strip was armed by then, so it followed: seek, hold the button for a tenth of a second,
seek back to where the press had already gone. Every rule the gesture has was obeyed.

The fix is a second thing to remember beside `scrubbing`: the fraction the gesture last
published. A fraction it has already been to is not published again, so the phantom move
before the release is silent, and so is a hand held still mid-scrub. The memory belongs to
the **gesture**, not the widget — a release clears it — because clicking the same spot twice
should seek twice, which is exactly what someone asking to hear that moment again wants.

Two lessons, and the second is the one worth keeping. The first: a widget's event stream is
the *platform's*, not the toolkit's idea of the gesture, and the platform sends more than a
gesture needs. The second: this was invisible to every test in the file, because each of the
four events is handled correctly on its own — the bug lived only in their order. The
regression test replays all four, in that order, rather than adding a fifth rule to `scrub`.

`ponytail:` one seek per pointer move, and `Engine::seek` blocks the GUI thread until the
audio thread has performed it. Fine for a local file, where a seek is a format-level jump
rather than a decode, and a stutter would be audible immediately rather than lurking. If a
slow source ever makes a scrub stutter, the fix is to coalesce the moves within a frame,
not to make the widget cleverer.

---

## 14c. Where the music is (`waveform.rs`, `audio.rs`, `cache.rs`, `ui/deck.rs`)

§7b needs one number per track and its mirror: where the music starts, and where it stops. A
`Trim` is those two `Duration`s measured from the top of the file, and everything below is
about how they are found, kept and shown.

### Found in the pass that was already running

`audio::scan` decodes every sample of a file exactly once and throws them all away as it goes
(§14a). Finding the edges is one comparison per sample bolted onto that loop — a second
accumulator beside `Fold`, not a second pass and certainly not a second decode.

**Sample-exact, not read off the finished waveform.** The array holds at most 2048 columns
however long the file is, so one column of a five-minute track is a sixth of a second: trimming
to a column would either clip the first transient or leave a sixth of a second of leader, and
both are audible in exactly the moment this feature exists to smooth. The peaks are for the
eye; the edges are for the transport, and they are worth their own arithmetic.

A file with nothing above the threshold in it has **no** edges — `None`, which is an answer and
not a failure. It is what stops a track of pure silence from being trimmed away to nothing, and
it is worth storing, or every launch would decode it again to be told the same thing.

### The threshold, which is a knob

−50 dBFS. Digital silence is 0 and a mastered track sits within a few dB of 1, so anything in
between is a judgement: low enough not to clip a fade or mistake a quiet intro for the leader,
high enough to sit above the dither and the tape hiss a rip carries — which are the two things
that would otherwise make every file's music start at sample zero and the whole feature do
nothing.

`ponytail:` one threshold for every file, and no hold time. A lone click in the leader of a
scratched record is therefore where the music starts, and a vinyl rip with a loud floor reads as
music throughout and gets no useful trim at all — the same answer as not having scanned it,
which is the failure mode to prefer. The upgrade is a percentile of the file's own amplitudes,
which needs the whole array kept rather than a running pair.

### Kept in a table, not in a version bump

The edges are worked out by the *same* pass as the waveform, so they could have been two more
fields on that record. A table of their own (§11a) costs one lookup and buys two things: every
waveform already on disk stays readable, where a changed record layout would have meant bumping
`FORMAT` and rescanning all of them to add a field; and a handover that wants a track's start
reads sixteen bytes rather than eight kilobytes of amplitudes it has no use for. §11a promised
that the next kind of fact would be a table rather than a migration. This is the promise being
kept, and the first chance to break it.

Both tables have to answer for a scan to count as cached. A file scanned by a build that knew
nothing about edges has its waveform and no trim, so it is decoded once more and both are
stored — which is the whole cost of adding this to an existing cache.

### In the app: one map, three readers

`Clecta::trims` is a `HashMap<PathBuf, Trim>` of what this run has been told, filled by every
job that finds out — a track's own scan, a queue measurement reading the cache, a folder scan
working it out — and read by the three places that need it: the early cut, the track it starts
next, and the button above the strip.

A map rather than a field on `Deck` and another on `queue::Item`, because the same answer
serves a loaded track, a queued one, and one that is neither yet. **A miss is the ordinary
state** and means *play this whole*: nothing here is required for the app to work, which is
what makes §11b's button an optimization rather than a step.

A `None` from a job is not stored as "there is no trim", and the distinction matters: it means
*that* job could not say, and another one still might — a queue measurement only reads the
cache where a folder scan decodes. Overwriting a known answer with silence would make queueing
a track un-learn what scanning it had taught.

### Two buttons and two marks

Above every strip: **⇤ 0:00** and **⇥ music**. They send the playhead to the top of the file
and to the top of the music, and each is dead when it has nowhere to go — dead rather than
absent, or the row would jump under the pointer as scans landed.

On the strip itself, the two edges are drawn as hairlines in the same green the drop ring and
the drag caret use, because they answer the same kind of question: *this is the place the
control is talking about*. Without them, **⇥ music** jumps to a spot the user has to take on
trust.

### The number the edges were actually for: how long the music runs

`Trim::music()` is `end - start`, and that is the whole of it — **derived, never stored**. The
two edges are already in the store, a third number written beside them is a third number that
can disagree with them, and the one that would be wrong is the one on screen. It costs a
subtraction; the ceiling it avoids is a table that has to be kept in step with another table.

It appears in the two places files are chosen and ordered:

- **The files pane**, in a column before the size (§9). A folder listing shows what a file *is*
  — its size, its date — and this is the first column that says what it is *for*. It also gives
  the `✓` beside it something to be about: a mark that only says "ready" is a mark you check
  once, where a mark that brings a number with it is one you read.
- **A queue row**, in front of the file's own length: `2:58 / 3:12`. The music first because
  that is what the evening is planned against, the file's length behind it because that is the
  number every other program on the machine agrees with. The same `format_lengths` builds both,
  so a row cannot say one thing in one pane and another in the other.

**It costs no extra work anywhere.** The pane's answer rides along with the query §11c already
makes once per listing — the edges were being read to decide the mark, and the playing time is
arithmetic on what came back. A queue row's rides along with `cached_facts`, which was already
reading the trim so the handover could skip the blanks.

The two panes need different rules for *not knowing*, and both are in `format_lengths`. A
length is a header parse the queues pay for on every edit, so **not measured** (blank) and
**measured, no length** (`--:--`) are worth telling apart. A playing time is a full decode and
is only ever read, so a missing one means nothing more than nobody has scanned this yet — the
same rule the trims map follows (above), and the reason `Queue::measured` applies its two
halves under two different tests. Filtering both on `duration.is_none()` would have meant that
a row measured when it was queued could never learn its playing time afterwards, which is
precisely the order it happens in.

A prepared file with no music at all shows `--:--`: it was scanned, and the answer is that there
is nothing in it. That is the same shape of answer a file with no length gives, and it reads the
same way — the store has stopped asking.

The running time in a queue's footer still adds up the **file** lengths, deliberately. It is the
one number that has to match the clock on the wall whatever the handover setting says, and a
total that changed meaning when a switch was flipped would be a worse number than one that is
occasionally longer than the set.

### A stopped player that is asked to move becomes paused

Q14 said a seek changes nothing about the transport, and that is still right for `Playing` and
`Paused`. `Stopped` is the one that was lying. In this app it means *at the top of the track* —
it is what **⏹** rewinds to and what every load lands on — so a player labelled "stopped"
sitting at 1:30 is the label promising something Play will not do.

That was already reachable by clicking the strip of a stopped player; the two buttons only made
it obvious. So the rule lives in `seek_to`, where **every** seek passes through — the click, the
scrub, both buttons, and the handover's own trim — rather than on the two controls that
prompted it. One line, and the state machine of §7 is untouched: this is the same "seeking is
not a transport event" boundary Q14 drew, on the other side of the transport it was drawn
around.

---

## 14d. The tempo (`waveform.rs`, `audio.rs`, `cache.rs`, `ui/browser.rs`, `ui/queue.rs`)

The third thing one decode can say about a file, after its shape (§14a) and its edges (§14c):
how fast it beats. It is the number a set is *grouped* by before it is timed, which is why it
leads both columns it appears in.

### A third accumulator, not a third pass and not a dependency

`Tempo` sits beside `Fold` and `Edges` in the same `for sample in source` loop. Everything
expensive about this feature was already being paid for by the decode; what is added is one
branch and one addition per sample, and then some arithmetic on an array a thousandth the size
of the file.

**Written here rather than pulled in.** The obvious crate for beat detection is `aubio`, which is
C — and `deny.toml` bans a C toolchain on purpose (§11), because that ban is the whole of the
"unzip and run" portability promise. So the detector is about a hundred lines of arithmetic in a
module that already had no dependencies, which is also what makes it testable with no audio
device and no window (§12).

### How it works, in the order the numbers appear

1. **A loudness envelope.** One number per 512 interleaved samples — the sum of the sample
   magnitudes in it. That is 11.6 ms of a mono 44.1 kHz file and 5.8 ms of a stereo one: short
   enough to put a kick drum in a bin of its own, long enough that a five-minute track is a few
   tens of thousands of bins rather than millions.
2. **Onsets, not loudness.** A tempo is not in how loud the music is but in *when it gets
   suddenly louder*, so the envelope becomes the rise in its logarithm from one bin to the next,
   negatives dropped. The logarithm is what makes a beat in a quiet passage weigh the same as one
   in a loud passage — an ear hears ratios, and an amplitude difference would let the loudest
   thirty seconds of a track decide its tempo. The mean is taken back off, leaving a train of
   spikes around zero, so the search below is about *where* the spikes are and not about the
   constant they sit on.
3. **The coarse pass: how well the track agrees with itself.** For every whole-bin lag in the
   allowed range, multiply the onset track by a copy of itself shifted that far and add it up.
   The lag that scores highest is the beat. The sum is deliberately **not** divided by the number
   of terms in it, which is what settles the tie between a lag and its double in favour of the
   faster reading rather than leaving it to the last bit of a float.
4. **The fine pass: one bin of a Fourier transform.** This is where the second decimal comes
   from, and it is the part that took a rewrite. Refining the correlation at a *fractional* lag
   means reading between two bins, and a straight line between two samples has its maximum at one
   end or the other — so the refined answer snapped back to whole bins, which at 128 BPM is three
   BPM wide. A rotating phasor has no such steps: `strength(period)` is smooth in the period, so
   the peak can sit anywhere between two bins and every beat in the track pulls on where it sits.
   Two narrowing sweeps of sixty-five candidates reach a thousandth of a bin, and a click track
   built at 100, 128 and 174 BPM reads back to within a fiftieth of a BPM.

### The range is not folded, and that is a decision about who fixes an octave

65–200 BPM, reported as found. A detector that quietly doubles a 70 and halves a 174 is one that
cannot be argued with — and it will be wrong sometimes, because half-time and double-time are
genuinely both true about a lot of music. So the app reports what it measured and the *person*
overrules it. **Correct tempo** is the other half of that decision, and the rest of this section.

### The editor: two buttons, because that is what a wrong tempo is wrong by

Right-click a row — in the files pane or in a queue, since a wrong number is usually noticed in
the queue it is about to be played from — and a menu opens with one entry. The entry is dead on a
row nothing has scanned: `/2` and `×2` need a number to start from, and there is nothing here to
type one with.

The editor is `/2`, the number, `×2`, and a footer of **Cancel** and **Apply**. No text field, and
that is not laziness twice over: an octave error is the *only* error this detector makes that a
person can see at a glance, and halving and doubling are exactly reversible — `×2` after `/2`
gives back the bits that were there, since both are powers of two. So there is no undo button and
no need to remember what the detector said. It is said under the number anyway, quietly, because
"what was it before I started" is the one question two buttons cannot answer.

Nothing is written until **Apply**. Cancel, a click outside the panel and Escape are the same
thing, and Escape now closes the panel *before* it clears the selection — "never mind" means the
most recent thing.

### The app's first panel drawn over the window

Every other dialog in clecta is native (`rfd`), because the OS draws those better than we can.
This one cannot be: `rfd` offers buttons and a message, and this holds a value that changes while
it is open. So it is `stack![window, shade, panel]` — the same widgets as everything else, laid
over the top, with the dimmed rest of the window as the hit target that dismisses it. A panel that
can only be closed by its own buttons traps the app the day one of them is missed.

`ponytail:` it is **centred, not at the pointer**. iced's press messages carry no position and a
pane cannot ask how big it is, so the only way to a cursor position is a subscription publishing a
message on every mouse move in the app — which §6 already refuses to leave switched on, because
each one rebuilds every row of the files pane. The upgrade is that subscription, armed by the
right-press and disarmed by the menu.

### A correction is not a cache fact, and that decides where it lives

`settings.json`, not `cache.redb`. The cache's first sentence is that deleting it loses nothing
but time (§11a) — a detected tempo obeys that, a corrected one does not, because nothing can work
it out again. Putting it in the cache would have made **Clear cache** a button that destroys
answers, which is a different button from the one that is there now.

So the two are cleared apart. **Clear cache** takes the detected tempos with the waveforms;
**Clear tempo corrections**, beside it and dead when there are none, takes only the corrections and asks
first. The wording of the two warnings is deliberately different: one costs the time to work
things out again, the other costs decisions.

### Applied where the row is drawn, not written into the model

The correction reaches the screen through one function, `ui::corrected_tempo`, called as each row is
built. The alternative — writing the new value into the files pane's map and into every queue
holding that file — is four places to keep in step, and it has no answer at all to **Clear BPM
edits**: putting the detected numbers back would mean re-reading the store for the pane and
re-measuring every queue, for a change that is supposed to be instant.

This way the model holds what was *measured*, `settings.json` holds what was *decided*, and
neither can drift. It also gives a correction on an unscanned file somewhere sensible to show: a
folder dropped from the cache keeps the numbers a person put there, which is the whole reason they
are in the other file.

### Two decimals, and what they are honestly worth

Always two, including on a round 128, because a column of numbers that sometimes has a fractional
part and sometimes does not has to be read rather than scanned. No `BPM` suffix anywhere: position
says what it is, and three characters on every row of two panes would cost more width than the
number does.

`ponytail:` the second decimal is earned on a track with **one** tempo. The phasor's precision
comes from turning over the whole file, so a live recording that drifts, or a set with a tempo
change in it, gets the average of the whole thing to two decimals of false precision. The upgrade
is a tempo per section rather than per file, which is a different feature; the manual editor above
is the cheaper answer.

`ponytail:` a positive score is the whole confidence test. A recording with no beat in it at all —
speech, an ambient wash — will still have *some* peak, so it gets a number rather than the `--`
it deserves. Silence and anything under a few seconds are rejected; the rest is what the editor is
for.

### Where it appears, and what nothing-known looks like

The same two places §14c chose, for the same reason, and in front of the playing time in both: a
column of its own before the size in the files pane, and leading a queue row's `128.00  2:58 /
3:12`. `format_tempo` builds both, so the two panes cannot answer differently.

The two kinds of nothing are the ones §14c already drew. **Blank** is *nobody has scanned this*,
because a column of placeholders turning into numbers one by one is worse than a column that
fills in. **`--`** is *scanned, and there is no tempo in it* — an answer, and one worth storing,
or a spoken word file would be decoded again on every launch to be told the same thing. A queue
row draws both as blank, because a queue only ever *reads* what a scan left behind and has no way
to tell the two apart — the same one-layer rule its playing time follows.

### A table of its own, again

`tempos` beside `waveforms`, `durations` and `trims` (§11a). The same argument as the trims table,
and now with a second use to point at: a build that knows about tempi reads every waveform and
every trim already on disk, where a fourth field on the waveform record would have needed `FORMAT`
bumped and the whole library rescanned to add four bytes.

It also pays for the feature that comes next. A corrected tempo is *not* a cache fact — deleting
it loses something no decode can work out again — so it needs a store it can be cleared from on
its own, with a confirmation, without touching a waveform. A table each is what makes that a small
change rather than an argument with §11a's first sentence.

---

## 15. The decision log

| # | Question | Decision | Landed in |
|---|---|---|---|
| Q1 | Audio engine | **rodio 0.22** — cpal + symphonia with the mixing already written; the real-time layer is a `Source` away if it later becomes the lesson | §1, §2, §3 |
| Q2 | Crossfader curve | **Switchable**, `Power` (constant-power) default, `Linear` for the same beat-matched track on both players. One `match`, one state field, persisted | §1, §8, §12 |
| Q3 | OS drop targeting | **The idle player wins** — no track, else not playing, else Player 1 — with the hover ring showing which. Derived from state: no armed flag, no dialog. In-app drags are aimed normally | §1, §10, §12 |
| Q4 | Targets + portability | **Windows 11 + macOS Sequoia Intel**, dual CI, and **portability as a hard requirement**: everything written goes to `clecta-data/` beside the app, including beside the `.app` rather than inside it | §1, §9, §11, §12 |
| Q5 | Splitters | **`widget::pane_grid`**, not a hand-rolled third implementation. Spiked: the fixed layout fits, reordering is opt-in, the fold costs ~14 lines | §6 |
| Q6 | Files pane rows | **`scrollable(column(rows))`**, not `widget::table`. Spiked: `table` has no row element, so a row cannot carry a selected state | §9 |
| Q7 | When to save | **A `dirty` flag and a 2s throttle**, alongside the write at close. Not a design preference — the smoke test showed ⌘Q never reaches the app, so saving only at exit lost the settings for the ordinary way of quitting a Mac app | §11, §12 |
| Q8 | Sizing the peak array | **Halve as it fills**, never divide. The sample count is not knowable up front — a stream has no duration at all — so the mipmap replaces the branch instead of guarding it, in one pass and for a file of any length | §14a |
| Q9 | Drawing the waveform | **A custom `advanced::Widget`**, not `canvas`. The plan called for the `Widget` as the lesson (§16), and laziness agreed for once: `advanced` is a feature toggle that adds nothing to `Cargo.lock`, while `canvas` would pull `lyon` in to tessellate rectangles | §14a, §12 |
| Q10 | Picking colours from a palette | **Compare the values, never trust the role name.** Not a preference either: `background.weak` is what `rounded_box` paints a panel, so a strip using it for its bed drew nothing at all | §14a |
| Q11 | Where long work runs | **A `std::thread` and a `oneshot`, not `Task::perform`.** iced's smol executor defaults to *one* worker, so CPU work in an async block stops every subscription in the app — the playhead clock froze while a track was being scanned | §4, §14a |
| Q12 | Saying a scan is running | **A band sweeping the strip, from the first frame.** In the strip rather than the status bar, because the strip is what is being waited for; no 250 ms threshold, because the gate costs more than the flash it prevents | §14a |
| Q13 | When a folder is saved | **Immediately on a successful listing**, not on the 2 s throttle. The throttle's price is right for a fader that moves sixty times a second and wrong for a folder that moves once; navigating and quitting straight after is ordinary use, not a corner case | §11 |
| Q14 | What a seek does to the transport | **Nothing.** Playing keeps playing from the new place, paused stays paused there. Not a `deck::Event` and never through `transition`, because a self-edge on all four states is "nothing happens" written four times — and no pause around `try_seek`, which would gap a playing track | §14b, §7 |
| Q15 | How the players' section is sized | **A pixel height, persisted, compacted only when the window is too short.** Its rows are all fixed, so a ratio hands the extra space to the one pane that cannot use it. And since iced 0.14 cannot report a laid-out size, the chrome around the body is made a *constant* — status bar pinned to 24 px — rather than measured or guessed | §6, §11 |
| Q16 | Which widget holds that height | **Not `pane_grid` — a plain column and a 6 px hand-written divider.** Converting pixels to `pane_grid`'s ratio was arithmetically exact and visibly wrong: iced redraws at the new window size a frame *before* the resize message reaches `update`, so the panel scaled and snapped back on every frame of a live drag. A literal height cannot be stale. Partially reverses Q5, which still stands for the tree | §6 |
| Q17 | Who compacts a panel too tall for its window | **iced does.** Keeping the window's height for the compaction ceiling alone was still enough to wobble, because the ceiling binds exactly when the edge is being dragged. `Limits::height` clamps a `Fixed` to the room the layout actually has, measured on the frame that uses it, so the app hands over a literal and reads the window's height nowhere in `view`. The price is that a window shorter than the panel takes it out of the browser rather than the players | §6 |

| Q18 | Whether a deferred ceiling is really where the note says it is | **Measure before believing your own `ponytail:` note.** Two of them claimed a local disk and a music-sized folder made the cost irrelevant. Measured: a 5 000-file folder cost **70 % of a core** at the playing tick and **25 ms** of frozen executor to read. Both notes were written by the same hand that wrote the code they excused, which is why neither had a number in it | §4, §9 |
| Q19 | `widget::lazy` or hand-rolled virtualization | **Hand-rolled, skipping the upgrade order §9 had written down.** `lazy` caches building, and building was only a third of the cost — iced lays out the whole tree every frame whether the elements were cached or not. It also wanted a dependency and a version counter that is silently wrong the day someone forgets to bump it. Hand-rolled wanted one `f32`, and it needed the row height to be *pinned* rather than measured, which is the same answer Q16 reached from the other end | §9 |
| Q20 | Where a queue's scroll edges live, for a drag that has to reach an off-screen row | **The header and the footer *are* the edges.** Two strips that appeared with the drag would push every row down as it began — the caret's feedback loop, arriving before the user has aimed at anything — and two reserved for ever would spend twenty pixels of every queue on something useful for a second at a time. The header and footer are already the top and bottom of the rows, and a button that is already held cannot press the buttons on them | §7a |
| Q21 | Whether the app can work out where a queue is scrolled to | **No, and it does not have to.** The pane's height is whatever the players left over, and any number derived from `self.window` is a frame stale (Q16). So the scroll is `scroll_by`, which iced clamps against the real bounds, and the app learns the result rather than deciding it: a `scrollable` republishes its viewport on the next redraw whenever it has moved. That is also what keeps the virtualized rows following a scroll no pointer asked for | §7a, §9 |
| Q22 | What to do about a track being queued twice | **Ask, across all three queues, with a native modal.** Refusing would be the app deciding something it cannot know — playing a track twice in a set is deliberate as often as it is a slip. The scope is the *set* rather than the destination queue, because Cue 1 and Cue 2 each holding a track plays it twice just as surely as one queue holding it twice. The modal is `rfd`, which the app already carries for **Load…**, against an in-app confirmation bar with its own state and two messages: the same trade §10 made, and reversible the day the question needs more than two answers | §7a |
| Q23 | SQLite for the file cache | **No — `redb`, and the reason is a ban we wrote ourselves.** `rusqlite` pulls `libsqlite3-sys`, which compiles C on both shipped targets, and `deny.toml` bans `cc` because the no-C-toolchain property is what makes "copy it anywhere and run it" true (§11). §12 called that ban a tripwire for exactly this moment; this is the moment, and it worked. redb is the same shape without the compiler — pure Rust, ACID, one file, MIT OR Apache-2.0 already on the allow-list. What is given up is SQL, and the day something wants a `WHERE` clause is the day to revisit it | §11a, §12 |
| Q24 | What makes a cached entry stale | **Size plus modified time — one `stat`, not a hash.** Hashing every byte costs about what the scan it avoids costs; hashing a sample buys the rename case for a read and a collision nobody can rule out. The two cases the stamp gets wrong cost exactly one re-scan each: an in-place edit inside FAT32's two-second granularity, and a rename. A cache that is occasionally cold is a cache; one that is occasionally *wrong* is a bug that looks like a corrupt file | §11a |
| Q25 | Whether a gesture may act on the same place twice | **No — a fraction already published is not published again, and the memory dies with the gesture.** winit's macOS backend emits a `CursorMoved` before *every* `MouseInput`, so one click reaches a widget as four events and the phantom move before the release re-seeked to where the press had already gone: a tenth of a second of the track audibly replayed. The alternative — a movement threshold in pixels — is a number to tune that still fires on a stationary click; comparing the fraction is the quantity that actually matters, and it covers a hand held still mid-scrub for free. Clearing it on release keeps a second click on the same spot a second seek, which is deliberate | §14b |
| Q26 | Where the handover's two switches belong | **On each queue, as two checkboxes, and read from the queue that gave the track.** Two rather than one three-way setting, because the middle position — load without playing — is the app's own default and a single toggle offers only the ends. Per queue rather than per player, because that is what lets Cue 1 run the evening by itself while **Next up** stays a shelf; a per-player setting could not say that, and a global one would be a mode. A queue switched off is *skipped* rather than blocking, so a cue that is off still lets the shared queue feed that player. **Auto-play** is drawn dead while **Auto-load** is off, and `advance` presses Play only if the file it just took actually arrived — a load that failed leaves the *previous* track in the player, and starting that is the one way an automatic start could play the wrong thing | §7a |

| Q27 | What counts as the start and the end of the music | **A fixed −50 dBFS threshold, measured per sample in the pass that was already decoding the file.** Not read off the peak array, which holds 2048 columns however long the track: one column of a five-minute file is a sixth of a second, so a column-accurate trim clips the first transient or leaves a sixth of a second of leader — audible in exactly the moment the feature exists to smooth. The threshold is the knob the physical world needs: below it sit dither and tape hiss, which without it would make every rip's music start at sample zero. No hold time, so a click in the leader wins — the ceiling is named, and the upgrade is a percentile of the file's own amplitudes | §14c, §7b |
| Q28 | Whether a seek may touch the transport after all | **Yes, in one direction: `Stopped` becomes `Paused`.** Q14 is still right for `Playing` and `Paused`, and was wrong about `Stopped` — which in this app means *at the top of the track*, so a player labelled "stopped" at 1:30 is the label promising something Play will not do. Already reachable by clicking the strip; the two jump buttons only made it obvious. The rule lives in `seek_to`, where every seek passes, rather than on the controls that prompted it — and still outside `deck::transition`, which is the boundary Q14 actually drew | §14c, §7 |
| Q29 | How a folder scan is driven | **A chain of messages that refills itself, four files at a time.** No subscription, no timer, no job queue: `scan_step` hands out what the fan-out has room for, each answer calls it again, and it clears itself when the last thread reports — so a scan that is not running costs nothing, which is §4's rule reached without one. Four because a decode is a third of a second a file: one at a time is ten minutes for two thousand tracks, one per core starves the audio callback of whoever is playing a set while it runs. **Stop cuts the list down to what has gone out** rather than dropping the state, because the files in flight are going to finish anyway and a scan dropped mid-air would have them reporting into nothing — and a second scan started before they landed would count them twice and run `running` past zero | §11b |
| Q30 | How a new kind of cached fact arrives | **As a table, which is what §11a promised and this is the first chance to break.** The music's edges are worked out by the same pass as the waveform, so they could have been two fields on that record — at the price of bumping `FORMAT`, which is a re-scan of every waveform on disk to add a field. A table costs one lookup and keeps them all, and it means a handover asking where a track starts reads sixteen bytes rather than eight kilobytes of amplitudes it has no use for | §11a, §14c |

| Q31 | What "→ Player 1" means with five rows selected | **The first plays, the rest go to the top of that player's cue.** A player holds one track, so the only question was where the other four go — and the app already had the answer in front of it: a cue is what plays next, and the handover of §7a turns four queued rows into four tracks that play back to back. The *top* rather than the end, which is reading the intent over the word: appending would let whatever was already queued play in the middle of the batch. Every aimed door shares it — the buttons, the double click, the drag — so five files mean the same thing however they were sent | §9a, §7a |
| Q32 | How a duplicate warning survives a batch | **One dialog, three answers.** Per-file asking would be three modals for one button press; and once the question names a count, *all* and *none* are not enough — nineteen good tracks and one repeat wants the middle answer. Yes queues everything, No queues only what is not already somewhere, Cancel does nothing, so Cancel still means Cancel. The answer is an `Admission` rather than a filtered list because the `←` / `→` buttons must filter rows and tracks by the same test. Platform Yes/No/Cancel with the meanings in the text, not rfd's custom labels, which need a Cargo feature that is off by default and work on Windows only with it | §9a, §7a |
| Q33 | Whether a press may still collapse a selection | **Not on the press, and for now not at all.** A press has to arm the drag as well as select (§10), so collapsing to one row on a plain press would destroy the selection the drag was about to carry. A file manager collapses on *release* instead, once no drag has happened; clecta does not, and the gap is marked rather than filled — narrowing five rows to one means clicking elsewhere or pressing Escape, and the fix is one remembered path and a branch | §9a, §10 |

| Q34 | What a row's `✓` promises | **A full scan on disk for this exact version of the file — the same test a load uses to skip decoding.** Not "there is an entry for this path": a track the queues measured the length of has a row in `durations` and is still a third of a second of decoding away, so counting it would make the mark true about the database and false about the only thing anyone reads it for. Both tables or neither, which also makes the column a live picture of §11a's staleness rule — edit a file and its mark goes, because the stamp moved | §11c, §11a |
| Q35 | How the pane learns what the store holds, without touching disk on every frame | **Asked once per listing, on a thread, for zero `stat` calls.** A copy of the answer rather than the store itself, because `view` runs every frame and the store is a file. The nice part is free: a stamp is a size and a modified time, and every row is already *showing* both — so `stamp_of` builds from what the listing read, and `stamp` is now written in terms of it rather than beside it, since two functions computing "the same" stamp differently would be a bug that looks like a cold cache. The whole listing is asked about, not the visible rows, because §9's hidden filter costs no filesystem work and revealing a dotfile has to reveal its mark | §11c, §11a, §9 |
| Q36 | Whether a mark may be added without asking the store | **Yes, optimistically, because the listing's answer replaces it.** A scan that succeeded is taken to be a scan that was stored, which is one case out — a file that cannot be stat'd is deliberately never cached — against a second lookup per file to learn what the decode already established. It is safe only because the set is *replaced* by the next listing rather than added to, so a wrong mark lives until the next refresh and no longer; a dead job answers "nothing prepared", which is the right way round to be wrong; and **Clear cache** empties the column, since it is a report of what is on disk. A `ponytail:` names it | §11c |
| Q37 | Which files spin, and what drives them | **Any file with a thread on it, off the counter that was already turning.** One rule for the whole window: a folder scan and a player's own waveform scan are the same work, so a row spins for either and gets the same `✓` after. The phase is §14a's sweep counter — one for everything that turns, or two scans at once drift apart into what looks like a rendering fault — and its subscription grew one clause and no timer. *Which* files are turning has to be kept rather than derived: answers come back out of order, so the four in the air are never `files[done..next]` | §11c, §11b, §14a |
| Q38 | What to do about `CFUserNotificationDisplayAlert` in the macOS log | **Nothing, and write down why.** It is `rfd`'s parentless path: no parent window means another process draws the alert while this one waits, which is the modal shortcut §7a already took, said out loud by the OS. iced lends a window handle inside `window::run` alone, so **Clear cache** could be parented but `admits` — asked in the middle of three queue edits, answering the code that asked — could not without three continuations and a queue that may change while the question is open. Parenting one of the two would trade a log line for two dialogs that do not look alike | §7a, §11a |
| Q39 | Where the music's *length* comes from, and what it displaces | **Derived from the edges already stored, and it displaces nothing.** `Trim::music()` is `end - start`: a fourth table would be a number that can disagree with the two it was computed from, and the wrong one would be the one on screen. It reaches the files pane on the query §11c already makes once per listing — the edges were being read for the mark, so the number is free — and a queue row on the trim `cached_facts` was already reading for the handover. In the pane it is a column of its own before the size, because a listing says what a file *is* and this is the first thing that says what it is *for*; in a queue it goes in front of the file's length rather than instead of it, since the music is what the evening is planned against and the file's length is what every other program on the machine agrees with. The footer's running time deliberately stays a sum of *file* lengths: a total that changed meaning when a handover switch was flipped would be worse than one that is occasionally long | §14c, §11c, §9, §7a |
| Q40 | How a tempo is found, and how far it is trusted | **A third accumulator on the decode that was already happening, reported raw between 65 and 200 BPM.** An onset track from a 512-sample loudness envelope, a whole-bin autocorrelation for the beat, then one bin of a Fourier transform to refine it — the phasor because a correlation read between two bins snaps back to whole bins, which at 128 BPM is three BPM wide, and the column claims two decimals. Written rather than pulled in, because the obvious crate is C and `deny.toml` bans a C toolchain (§11). **Not** folded into a narrower window: half-time and double-time are both genuinely true about a lot of music, so the app reports what it measured and a person overrules it (Q41). The `✓` now means three tables rather than two, which cost every prepared folder one more **Prepare folder** run — the honest price of a mark that means one thing | §14d, §11c, §9, §7a |
| Q41 | Where a *hand-edited* tempo lives, and how it reaches the screen | **In `settings.json`, cleared on its own, and applied as the row is drawn.** A detected tempo is a cache fact — deleting it costs time. A corrected one is a person's answer about a track and nothing can work it out again, so it cannot live in a file whose first sentence is that losing it costs nothing (§11a): **Clear cache** takes the detected numbers, **Clear BPM edits** takes the corrections, and each asks first. It is applied by one function at draw time rather than written into the pane's map and every queue holding the file, which is four places to keep in step and no way at all to put the detected numbers back when the corrections are dropped. The editor is `/2` and `×2` and nothing else: an octave is the only error visible at a glance, and the two are exactly reversible, so no undo button is needed. It is the app's first panel drawn *over* the window, since `rfd` cannot hold a value that changes while it is open — centred rather than at the pointer, because the only route to a cursor position is a subscription that fires on every mouse move | §14d, §11, §11a |
| Q42 | Where a decision inside a `view` belongs | **Beside the view, as a named function with a test — not moved to another module, and not left in the closure.** §12's rule was already "pure logic is tested, including the arithmetic inside a module that is not otherwise pure", and five places under `ui/` were quietly exempt from it because a decision written inside a `.map()` has no name to call and no interface to cross. Lifted rather than *relocated*: `mark_of` belongs with the column it marks and `music_span` with the strip it measures, so a rule and the widget it is about stay in one file — `ui/mod.rs` is for what two panes share, which none of these are. Writing the tests changed two of the four: `detected_line` was comparing floats where the question is about two rendered decimals, and `music_span`'s `<= 0.0` guard let a `NaN` through into a `clamp` that passes them on. Both were reachable only from inputs nothing produces today, which is the argument for the rule rather than against it | §12, §11c, §14c, §14d, §9a |
| Q43 | What to do about a rule the app had written down more than once | **Delete the copies, and put the survivor where the third caller can reach it.** Two of these had grown quietly: the selected-row fill existed twice, byte for byte, in `ui/browser.rs` and `ui/tree.rs` — with the queues importing the browser's, which is a queue asking the files pane what colour a queue row is — and "the last component of a path" existed *three* times, in `fsio`, in `tree` and inline in `Entry::new`. Neither is an abstraction being introduced: both are one implementation being kept and two being removed, which is the only kind of sharing that needs no justification beyond the count. The fill moved to `ui/mod.rs`, which is exactly the module for what more than one pane shares; the name stayed in `fsio`, which is where the names are read off the filesystem to begin with. `Entry::new`'s copy differed by one line — it fell back to an empty string where the other two fall back to the whole path — and the survivor's behaviour won, since a blank row is never the better answer | §5, §9 |
| Q44 | Whether the store answers per table or per question | **Per question. `scan`, `store_scan`, `ready` — and the four tables go private.** Eight of `Cache`'s twelve methods were a getter and a setter each, every one a one-line forward to the same two private functions, and the real rule — that a hit means every table a decode fills — lived *outside* them: once in the app's `cached_scan` and once in `prepared`, with a comment admitting the two were kept in step by hand. Adding the tempo proved the comment right by breaking both at once. The rule is now one array and one function; the caller asks whether a file has been scanned and is told. Three consequences fell out rather than being aimed at: **one write transaction**, so a half-stored scan is no longer possible (three writes could leave a waveform with no tempo, which reads as a miss for ever and re-stores identically every launch); **one read transaction**, which was the `ponytail:` note on `prepared`; and the queues and the files pane finally agreeing about one file, since `cached_facts` was reading tables one at a time and would show a queue row's playing time for a track the pane refused to tick. `durations` keeps its own pair on purpose — a length is a header parse, and folding it in would make asking for a running time decode the whole file | §11a, §11c, §7a, §14c, §14d |
| Q45 | Whether a row keeps a scan's answers as one thing or as loose fields | **As one thing, and the same one everywhere.** Three facts about a file were being carried by five types with overlapping fields — `Scan`, `Facts`, `Ready`, `Trim`, `Item` — and the seams between them were where the app disagreed with itself. `Item` held `music` and `tempo` as two independent `Option`s that were set by two copies of the same rule, so "a playing time with no tempo beside it" was representable and meaningless; `Facts` held the same pair a second time; and the queue row then wrote `item.tempo.map(Some)` to re-inflate a two-layer answer it had flattened on the way in. All three now hold one `Ready`, so a queue row and a files-pane row are drawn by the *same expression* — which is the real test of whether two panes agree, rather than two comments saying they should. `duration` stays a separate field with its own two layers, for the reason in Q44: a length is a header parse, a scan is a decode, and they arrive at different moments under different rules | §7a, §14c, §14d, §11a |
| Q46 | Whether the three accumulators are three things or one | **One `Scanner`, with the three behind it.** `Fold`, `Edges` and `Tempo` are identical in shape — `Default`, `push`, `finish` — and were never independent: they are three answers to one expensive question, and the expense is the decode they share. Driving them separately meant every caller knowing there were three, that two of the three need the sample rate and the channel count and one does not, and that all three must be fed *every* sample or their answers stop being about the same audio. That loop was written four times: once in `audio::scan` and once in each of the three test modules. It is now written once, and the three stay separate types *behind* the interface, still tested one at a time — an internal seam is still a seam, it is just not part of what the module promises. A fourth fact is now a struct, a field, a `push` and a `finish`, all in one file. The same tidying moved `sweep_band` and `seek_fraction` out to `ui/waveform.rs`: they take a width in pixels and never ask `waveform` anything, so they were in the pure module because it was the pure module and not because they belonged there (Q42's rule, applied the other way round) | §14a, §14b, §12, §5 |
| Q47 | Where the rules about the *set* of three queues live | **In `queues.rs`, with the queues.** Thirteen messages, twelve `update` arms and three `Clecta` fields described one thing that had no module: an array of three queues, an array of three scroll offsets and a set of paths in flight, all indexed by `QueueId::index()` on every queue arm — 42 of those lookups in `app.rs` alone, five of them inside `QueueShift`. The tell was already in `queue.rs`, which had grown three free functions taking `&[Queue; 3]`: a type with no name doing a job with no home. So `Playlist` is now one queue (the type is `Queue` since Q49) and `Queues` is the set, and the split is not arbitrary — a duplicate is a duplicate if *any* of the three holds it, a file is measured once however many rows hold it, and the next track comes from a player's own cue *or* the shared pool. None of those is a question a single queue can answer. `QueueId` moved with them, and `index()` stayed public for one caller: the view, whose three `scrollable` ids are a `[&'static str; 3]` because iced wants a `&'static str`. `update` no longer uses it at all. Two smaller things fell out: asking what needs measuring and recording it as in flight became **one** call (`take_unmeasured`) rather than two lines beside each other, which is the pair that goes wrong the day they stop being beside each other; and the queue scroll offsets left `Clecta` for the module that owns the queues, which is where the files pane has always kept its own | §7a, §9, §5 |
| Q48 | Where the pure transport decision meets the audio | **In `Deck`, once — `moved`, `seek`, `ended` — with the engine passed in as the `Option` it already was.** `transition` was pure and well tested and `Engine` was untestable, and the app was re-pairing the two by hand in four places: `transport`, both legs of `poll_players`, and `seek_to`. Four pairings drifted, as four of anything does. The playhead was zeroed in three of them and not the fourth; the notice line was formatted in five places with one near-miss; and `seek_to` wrote `Stopped → Paused` straight into the field, inventing a transport edge the state machine had never heard of and nothing could test. All of it is now one method each, and `update` calls one line per gesture. The engine is `&Option<Engine>` rather than a trait, because the `Option` is not a testing device — the app really does run with no output device (§11) — and passing `None` leaves exactly the state machine, which is the second adapter the seam needed without inventing one. `deck.rs` names `audio::Engine` and still names no rodio type, so the seam §4 cares about is where it was | §7, §7b, §14b, §11, §12 |
| Q49 | Where the product's words are defined | **A root `CONTEXT.md`, one entry per word, opinionated.** Four words — playlist, list, queue, cue — were naming three objects, and "transition" was naming a transport change, the handover and the handover point all at once; a glossary that picks one word per thing is what turns the next rename from a taste argument into a bug report. The split of authority is in `docs/agents/domain.md`: this plan wins on decisions, the glossary wins on vocabulary, and a term defined in both places is a bug here. The plan's prose was swept to obey it, and the code follows in the renames the glossary demanded — `Playlist` → `Queue`, `Transition` → `Handover`, `edited` → `corrected` — each with its `settings.json` key pinned by serde so no saved setting resets | CONTEXT.md, §7a, §7b, §14d |
| Q50 | Whether a release may collapse a selection after all | **Yes — on a timer, and the timer is the decision.** Q33 declined to collapse on the *press* (it would destroy the selection a drag was about to carry) and priced the release at one remembered path and a branch. Building it found the price wrong: a double click acts on the whole selection (§9a) but its message arrives *after* the first click's release, so a release that collapsed immediately would narrow five rows to one and then load one. No message tells the two apart — only time does, which is how every file manager answers the same question. So the release leaves the collapse *pending* and a subscription fires it after `COLLAPSE_AFTER` (350 ms, just past iced's 300 ms double-click window — a coupling by value, marked in the constant); any gesture that acts on the selection first — a new press, the double click's load, ⌘A, Escape — cancels it — at both stages, because a deferral still held by a pressed button would be promoted by its release and fire anyway. The pane remembers the row by path and a queue by index, for the reasons their selections already differ (§9a), and the queue's delayed click carries the pressed row's path and fires only if the index still holds that very track — an index alone stays in range after a handover takes the top row and would name the row below the one pressed | §9a, §10, §7a |
| Q51 | Whether the glossary's "handover point" survives contact with the code | **No — the glossary was wrong, not the type.** Q49's first draft gave the event and the setting separate words (handover / handover point), and the review flagged `Handover {Whole, Trimmed}` for modelling the setting under the event's name. Reading the call sites settled it the other way round: `queue.handover = Handover::Trimmed` is a property *of the handover* — its manner, whole or trimmed — and §7b's own table derives both the *when* and the *where* from that one choice, so the "point" was never a second concept, only a consequence. One glossary entry now carries both sentences, the type keeps its name, and no exception has to be recorded — unlike Deck/Player (§5), where two real things genuinely collide on one word | CONTEXT.md, §7b, Q49 |

Nothing is open. Q5 and Q6 were the two the plan deliberately left for a compiler to
answer; both were settled by a throwaway spike, which is now deleted — what it proved
lives in `app.rs` and `ui/browser.rs`, and the reasoning is in §6 and §9. **Q7 is the one
the plan got wrong** rather than left open: §11 asserted the close request was the last
chance to write, and running the app is what disproved it. Q8 is the one the plan never
thought to ask: §14 said "the whole file decoded to a peak array" as though the array's
size were the easy part.

**Q16 and Q17 are the ones a *later* decision got wrong**, which is a different failure from
Q7 and worth separating: Q15 was not mistaken about what the app should do, only about what
the widget could be made to do it with — and Q16 then fixed only nine tenths of it, leaving
one use of the window's height that was enough to keep the defect alive. Both were settled
the same way, by looking at a running window. That is now four of the forty-one (Q7, Q10,
Q16, Q17), every one found by an eye and none by the tests that were passing at the time —
and Q17 is the sharpest of them, because the tests written *for Q16* passed too.

**Q18 is a fifth kind of wrong, and the most uncomfortable one.** Q7, Q16 and Q17 were
mistakes; Q18 was a pair of *excuses*, written in the file, in the voice of someone who had
thought about it. Both said the ceiling was in the right place and neither carried a number.
Measuring took twenty minutes and moved one of them by a factor of thirty. The rule that
falls out is narrow enough to be useful: a `ponytail:` note that names a cost is a promise to
measure it, and the note is only honest once it quotes what the measurement said.

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
feature — which the waveform now uses, and which cost three methods and no dependency
(§14a). That was the claim this section was making on credit; it is paid.

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
