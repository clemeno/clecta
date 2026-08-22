# clecta

A two-player desktop audio mixer. This glossary is the product's vocabulary, not the
desktop build's: a second front end would inherit every word here unchanged. Decisions
live in `desktop/PLAN.md`, which wins on *why*; this file wins on *which word*. A term
defined in both places is a bug in the plan.

## The players

**Deck**:
One of the two players — its transport, its loaded track, its playhead and its length.
_Avoid_: player. The UI says "Player 1" / "Player 2" because that is the user's word, but
the type is `Deck`: rodio's playback handle is already called `Player`.

**Transport**:
A deck's playback state — Empty, Stopped, Playing or Paused.
_Avoid_: state, status

**Transition**:
A change of transport state. Nothing else.
_Avoid_: using it for a handover, which is the other end of the app entirely.

**Playhead**:
Where in the track a deck currently is.
_Avoid_: cursor, position marker

## The queues

**Queue**:
An ordered list of files waiting to be played. There are exactly three.
_Avoid_: playlist, list

**Cue**:
A queue belonging to one deck — what that deck plays next, and no other deck's business.
_Avoid_: cue list

**Shared queue**:
The third queue, taken from by whichever deck runs out first. Labelled *Next up* on screen.
_Avoid_: common, common list

**Handover**:
A queue giving its top track to a deck that has run out. Per queue, a handover is
**whole** or **trimmed** — that choice is the setting (*Whole track* / *Skip blanks* on
screen), and it decides both when the handover fires and where the next track starts.
The type is `Handover`.
_Avoid_: transition, transition setting, handover point — the "point" is not a separate
concept, it falls out of whole-vs-trimmed

**Item**:
One row of a queue: a file, plus whatever has been measured about it.

**Caret**:
The insertion line drawn in a queue to show where a dragged row would land.

## The browser

**Browser**:
The files pane's model — one folder's listing, its filter, its sort and its selection.
_Avoid_: file browser. The pane and the tree together have no single name, and do not
need one.

**Files pane**:
Where the browser is drawn.

**Tree**:
The folder tree beside the files pane. It lists folders; it does not list files.

**Anchor**:
The row a Shift-click measures its range from. Moves on a plain or command press, stays
put for a range.

## What is known about a file

**File**:
Something on disk.

**Track**:
The audio a file contains, once something has read it.

**Scan**:
One decode of one file, yielding its peaks, its trim and its tempo together. All three or
none — there is no half-scanned file.

**Prepare**:
Scanning every file in a folder.

**Prepared**:
A file the cache holds a whole scan for. Shown as a ✓ in the files pane.
_Avoid_: ready, cached

**Ready**:
The facts a prepared file yields — its tempo and its trim. A payload, not a synonym for
prepared.

**Measure**:
Learning a file's numbers, whoever asked for the decode.

**Peaks**:
The envelope a scan produces, drawn as the waveform.

**Trim**:
Where a track's music starts and ends inside the file.

**Edges**:
What finds a trim while a scan runs.

**Music**:
How long a track's music runs — the length of its trim. Labelled *⇥ music* on screen.

**Tempo**:
How fast a track beats. BPM is its unit, not another name for it.
_Avoid_: BPM as the quantity

**Correction**:
A tempo a person set by hand. Kept in `settings.json` and not in the cache, because
nothing can work it out again.
_Avoid_: edit, override

**Stamp**:
A file's modified time and its length together — what decides whether a cached scan still
describes it.

## The window

**Zone**:
The panel a pointer can leave. Coarser than a drop target: a queue has one zone and as
many targets as it has rows.

**Drop target**:
What a dragged thing would land on if released now — a deck, or one row of a queue.

**Hover ring**:
The outline drawn on the deck a drop would land in.

**Sweep**:
The band that travels across a waveform or a row while its scan is running.

**Fader**:
A deck's own volume.

**Crossfader**:
The one control that trades the two decks against each other.

**Curve**:
The crossfader's shape — how gain divides between the decks across its travel.

**Preset**:
A small button that jumps a fader or the crossfader to a fixed value.

**Notice**:
The one-line message in the status bar. The app's only way of saying something went wrong.
