#!/bin/sh
# One three-second tone per container and codec the app offers (PLAN §3), made by ffmpeg, plus
# four whose timestamps start 2 s late, as a file cut from a longer stream can; then
# `probe_files` on them all: each must agree with itself on its length, its decode and where
# its seeks land (PLAN Q72). The suite cannot make these itself — no pure-Rust Vorbis or AAC
# encoder exists — and fixtures are generated rather than committed, so this is a check run
# by hand after touching the decode path or the vendored rodio, like the smoke script.
#
# Needs ffmpeg on the PATH. Paths given as arguments are probed too:
#	desktop/scripts/probe-formats.sh ~/Music/a.webm
set -e
cd "$(dirname "$0")/.."

dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
# Stereo, a different pitch per side, so a swap of the two shows.
tone="-f lavfi -i aevalsrc=sin(440*2*PI*t)|sin(660*2*PI*t):s=48000:d=3"
for file in a.wav a.mp3 a.flac vorbis.ogg opus.ogg a.m4a a.mp4 vorbis.mkv opus.webm \
	late-vorbis.ogg late-opus.ogg late-vorbis.mkv late-opus.webm; do
	case $file in
	*vorbis.*) codec="-c:a libvorbis" ;;
	*opus.*) codec="-c:a libopus" ;;
	*.mp3) codec="-c:a libmp3lame" ;;
	*.m4a | *.mp4) codec="-c:a aac" ;;
	*) codec="" ;;
	esac
	case $file in
	late-*) late="-output_ts_offset 2" ;;
	*) late="" ;;
	esac
	# Word splitting wanted: each holds several arguments.
	# shellcheck disable=SC2086
	ffmpeg -v error $tone $codec $late "$dir/$file"
done

# `split_paths`'s separator, and on Windows the path form it reads.
case "$(uname -s)" in
MINGW* | MSYS* | CYGWIN*) sep=';' ;;
*) sep=':' ;;
esac
paths=""
for file in "$dir"/* "$@"; do
	[ "$sep" = ';' ] && file=$(cygpath -m "$file")
	paths="$paths${paths:+$sep}$file"
done

CLECTA_PROBE="$paths" cargo test --quiet probe_files -- --ignored --nocapture
