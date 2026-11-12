# Upstream drafts: rodio and symphonia, the timing of MP4, Matroska and Ogg

Not posted. Text for issues and a pull request on `RustAudio/rodio`, for the maintainer
to post by hand. When a rodio release carries these fixes, delete this file together with
`vendor/rodio` and the `[patch.crates-io]` table in `Cargo.toml` (PLAN Q69, Q72) — once
Opus is decodable without the `CODECS` hook too (PLAN Q71). Issues 2 and 3, at the end,
have no pull request drafted yet, and issue 3 is for symphonia, whose MP4 reader is vendored
too: delete `vendor/symphonia-format-isomp4` once a symphonia release carries that fix.

---

## Issue

**Title:** Symphonia decoder: duration and seeking use the default track, not the decoded one (wrong length for video-first MP4s)

**Body:**

With rodio 0.23.0, and on `master` as of 2026-11-11, an MP4 whose first track is video
reports the wrong `total_duration()`, and seeks are clamped to that wrong length.

Real file: H.264 video (time base 1/12800) followed by 44.1 kHz AAC, 228.44 s long.

| | expected | rodio |
|---|---|---|
| `total_duration()` | 228.44 s | 66.27 s |
| `try_seek(150 s)`, then counting what is left | lands at 150.000 s | lands at 66.255 s |
| `try_seek(30 s)` | lands at 30.000 s | lands at 29.977 s |

Remuxing the same streams with the audio first (`ffmpeg -i in.mp4 -map 0:1 -map 0:0 -c copy out.mp4`, stream 1 being the AAC)
gives the right length and exact seeks.

In `src/decoder/symphonia.rs`, three places use the container's default track while the
decoder decodes a different one: the first track with a known codec. In an MP4, symphonia's
default track is the first one, so here it is the video.

1. `init` computes `total_duration` from `track.codec_params.time_base` (the decoded track)
   zipped with `stream.codec_params.n_frames`, where `stream = default_track()`. Here that is
   the video's 2 922 496 ticks (228.32 s × 12 800) read as 44.1 kHz frames, which gives 66.27 s.
2. `try_seek` passes `track_id: None`, so the demuxer seeks on the default track.
   `refine_position` then reads that track's `required_ts` and `actual_ts` in the audio
   decoder's time base, which is the one-frame error at 30 s. The clamp to `total_duration`
   above is the error at 150 s.
3. `Iterator::next` passes every packet to the decoder, without the track check that the
   loop in `init` has. With AAC, video packets fail to decode and are skipped. With a PCM
   audio track they decode, and the video bytes play as samples.

## Pull request

**Title:** symphonia: use the decoded track for duration, seeking and packets

**Body:**

Fixes #<issue>. `SymphoniaDecoder` now keeps the id of the track it decodes, and uses that
track:

- for `total_duration`, instead of the default track's `n_frames`;
- for `try_seek`, as `SeekTo::Time { track_id: Some(..) }`, so `refine_position` gets
  timestamps in the time base it uses;
- in `Iterator::next`, skipping other tracks' packets the same way `init` already does.

The default-track check stays as an early `return Ok(None)`, so a file with no tracks
behaves as before.

Tested downstream against a generated MP4 with the video first (a 1 kHz video track, then
8 kHz mono `sowt` PCM whose sample `n` is `n`):

- the length is exactly 1 s, instead of 125 ms;
- the decoder yields exactly 8 000 samples, instead of 8 024;
- a seek to 0.5 s lands on sample 4 000, instead of 1 004.

The builder is about 100 lines and can be ported into rodio's tests if wanted.

```diff
--- a/src/decoder/symphonia.rs
+++ b/src/decoder/symphonia.rs
@@ -24,6 +24,8 @@
     decoder: Box<dyn Decoder>,
     current_span_offset: usize,
     format: Box<dyn FormatReader>,
+    // The track being decoded: the duration, seeks and packets all follow it.
+    track_id: u32,
     total_duration: Option<Duration>,
     buffer: SampleBuffer<Sample>,
     spec: SignalSpec,
@@ -76,10 +78,9 @@
         };
         let mut probed = get_probe().format(&hint, mss, &format_opts, &metadata_opts)?;
 
-        let stream = match probed.format.default_track() {
-            Some(stream) => stream,
-            None => return Ok(None),
-        };
+        if probed.format.default_track().is_none() {
+            return Ok(None);
+        }
 
         // Select the first supported track
         let track_id = probed
@@ -107,7 +108,9 @@
         let total_duration: Option<Duration> = track
             .codec_params
             .time_base
-            .zip(stream.codec_params.n_frames)
+            // The decoded track's frame count, not the default track's. In an
+            // MP4 the default track is the video, counted in its own time base.
+            .zip(track.codec_params.n_frames)
             .map(|(base, spans)| base.calc_time(spans).into())
             .filter(|d: &Duration| !d.is_zero());
 
@@ -142,6 +145,7 @@
             decoder,
             current_span_offset: 0,
             format: probed.format,
+            track_id,
             total_duration,
             buffer,
             spec,
@@ -211,7 +215,9 @@
             self.seek_mode,
             SeekTo::Time {
                 time: target.into(),
-                track_id: None,
+                // Seek on the decoded track. `None` means the default track,
+                // whose timestamps `refine_position` would then read in the wrong time base.
+                track_id: Some(self.track_id),
             },
         ) {
             Err(Error::SeekError(symphonia::core::errors::SeekErrorKind::ForwardOnly)) => {
@@ -301,6 +307,10 @@
         if self.current_span_offset >= self.buffer.len() {
             let decoded = loop {
                 let packet = self.format.next_packet().ok()?;
+                // Skip other tracks' packets, as `init` does.
+                if packet.track_id() != self.track_id {
+                    continue;
+                }
                 let decoded = match self.decoder.decode(&packet) {
                     Ok(decoded) => decoded,
                     Err(Error::DecodeError(_)) => {
```

---

## Issue 2

**Title:** Symphonia decoder: playback runs past the container's stated end, and accurate seeks land one packet late for Vorbis

**Body:**

With rodio 0.23.0, a 3 s 440 Hz tone at 48 kHz, made by ffmpeg in each format, decodes to
these lengths. Gapless is on, as it is by default.

| file | `total_duration()` | samples decoded | an accurate seek a quarter in lands |
|---|---|---|---|
| `.m4a` (AAC) | 3.0213 s | 3.0293 s | exactly |
| `.mkv` (Vorbis) | 3.003 s | 3.020 s | 19 ms late |
| `.ogg` (Vorbis) | 3.000 s | 3.000 s | 21 ms late |

Each is a different cause.

1. **The last packet's padding is played.** In the `.m4a` the last sample is 640 ticks long,
   but the AAC decoder outputs all 1 024 frames. A Matroska file says where the sound ends
   only by its `Duration`. symphonia 0.5 sets no trim for either, and its AAC decoder would
   not apply one anyway. Cutting the packet that crosses `n_frames` makes the decode agree
   with `total_duration()`.
2. **The skip after an accurate seek is counted from the wrong packet.** `refine_position`
   skips `required_ts - actual_ts`. But a Vorbis packet decoded right after `reset()` gives
   no frames (it only primes the overlap), and `next` silently moves on to the following
   packet. Every Vorbis seek therefore lands one block late: 1 024 frames, 21 ms. Counting
   the skip from the timestamp of the first packet that gives frames fixes it. When that
   packet already starts past the target, seek earlier and decode again.
3. **Matroska without block durations lands after the target.** symphonia-mkv's seek returns
   the first block with `timestamp + duration >= ts`. With no duration, that is the first
   block that starts at or after the target, and `required_ts.saturating_sub(actual_ts)` is then 0. The
   seek-earlier step in (2) covers this too.

---

## Issue 3, on `pdeljanov/Symphonia`

**Title:** isomp4: the edit list is parsed but never applied, so AAC encoder priming is played

**Body:**

With symphonia 0.5.5, an `.m4a` that ffmpeg makes from a 3 s tone has one edit: media time
1 024 (the AAC encoder's priming), lasting 3 s. `symphonia-format-isomp4` reads it into
`EdtsAtom` and then never uses it. So `n_frames` is the whole media, 3.0213 s; the priming
is decoded and played; and with `enable_gapless` nothing is trimmed.

The fix carried in this repository reads a single edit at normal rate into the media as a
delay and a length:

- it reads sound tracks' edits only: a picture's edit, as a video cut without re-encoding
  keeps, read as a delay pushed a seek near the end past the picture's last frame, and the
  whole seek failed;
- it stops an edit that runs a few ticks past the media at the media's end, as a muxer
  rounding up to a coarser movie clock (QuickTime's 1/600 s) writes, rather than refusing it;
- it sets them as the track's `delay` and `n_frames`;
- it calls `formats::util::trim_packet` on each packet;
- it adds the delay to seek timestamps going in, and takes it off `SeekedTo` coming out;
- it starts a seek's decode one sample early, so an AAC decoder has the overlap it needs
  before the target, looking back into the previous fragment when the target is a
  fragment's first sample.

That is what the MP3 reader does with a LAME tag. One more part is needed:
`symphonia-codec-aac`, like the PCM decoder, ignores `trim_start` and `trim_end`. rodio now
applies them itself when a decoder returns the whole block. The trims are in ticks of the
track's clock, which need not be the sample rate (HE-AAC's is twice the rate decoded), so they
are converted to frames, to the nearest, before being compared with the decoded block.
