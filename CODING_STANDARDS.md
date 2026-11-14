# Coding standards

Rules a reviewer applies to a diff, for what no linter can see. The naming, formatting and
lint rules are tooling's (`desktop/check.sh`, PLAN §13); the design record is
`desktop/PLAN.md`.

## A seek that fails has not moved

`try_seek` returning `Err` means the audio is where it was before the call. Any path that has
already moved the decode — a demuxer seek that succeeded, a decoder reset, a packet decoded —
ends in `Ok`, keeping whatever landing it has, however late. `desktop/src/stretch.rs` keeps
the playhead where it was on an `Err`, and the app says the stream cannot be sought, so an
`Err` after a move leaves the playhead behind the sound and the user told something false.

Look for it in the vendored decoders (`desktop/vendor/`) and in `stretch.rs`: a `?` or an
early `return Err` after the first step that moves anything. PLAN Q72 records the case that
broke it, a Matroska step back below the first cue.
