#!/bin/sh
# One three-second tone per container and codec the app offers (PLAN §3), made by ffmpeg, then
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
tone="-f lavfi -i sine=frequency=440:duration=3:sample_rate=48000"
for file in a.wav a.mp3 a.flac vorbis.ogg opus.ogg a.m4a a.mp4 vorbis.mkv opus.webm; do
	case $file in
	vorbis.*) codec="-c:a libvorbis" ;;
	opus.*) codec="-c:a libopus" ;;
	*.mp3) codec="-c:a libmp3lame" ;;
	*.m4a | *.mp4) codec="-c:a aac" ;;
	*) codec="" ;;
	esac
	# Word splitting wanted: each holds several arguments.
	# shellcheck disable=SC2086
	ffmpeg -v error $tone $codec "$dir/$file"
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
