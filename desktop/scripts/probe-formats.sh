#!/bin/sh
# One three-second tone per container and codec the app offers (PLAN §3), made by ffmpeg, plus
# the shapes that have broken one: four whose timestamps start 2 s late, as a file cut from a
# longer stream can; an AAC one in fragments of one packet each, as DASH and OBS write MP4; and
# an MP4 and a WebM with a picture first that ends a tenth of a second before the sound, as a
# downloaded video's can. Then `probe_files` on them all: each must agree with itself on its
# length, its decode and where its seeks land (PLAN Q72). The suite cannot make these itself —
# no pure-Rust Vorbis or AAC encoder exists — and fixtures are generated rather than
# committed. CI runs this, and so does check.sh when ffmpeg is on the PATH.
#
# Paths given as arguments are probed too, and so is each line of `scripts/probe-files.local`
# when it exists: a gitignored list of real files of your own, one path a line, which have
# found bugs no generated tone could (Q69, Q72).
#	desktop/scripts/probe-formats.sh ~/Music/a.webm
set -e
cd "$(dirname "$0")/.."

dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
# Stereo, a different pitch per side, so a swap of the two shows.
tone="-f lavfi -i aevalsrc=sin(440*2*PI*t)|sin(660*2*PI*t):s=48000:d=3"
# A picture of 2.9 s, so a seek 50 ms before the sound's end is past the picture's.
picture="-f lavfi -i testsrc=d=2.9:r=25:s=64x48"
for file in a.wav a.mp3 a.flac vorbis.ogg opus.ogg a.m4a a.mp4 vorbis.mkv opus.webm \
	late-vorbis.ogg late-opus.ogg late-vorbis.mkv late-opus.webm fragmented.m4a \
	picture-first.mp4 picture-first-opus.webm; do
	case $file in
	*vorbis.*) codec="-c:a libvorbis" ;;
	*opus.*) codec="-c:a libopus" ;;
	*.mp3) codec="-c:a libmp3lame" ;;
	*.m4a | *.mp4) codec="-c:a aac" ;;
	*) codec="" ;;
	esac
	case $file in
	late-*) shape="-output_ts_offset 2" ;;
	fragmented.*) shape="-movflags frag_keyframe+empty_moov -frag_duration 1" ;;
	picture-first.mp4) shape="-c:v mpeg4" ;;
	picture-first-*.webm) shape="-c:v libvpx" ;;
	*) shape="" ;;
	esac
	case $file in
	picture-first*) inputs="$picture $tone" ;;
	*) inputs="$tone" ;;
	esac
	# Word splitting wanted: each holds several arguments.
	# shellcheck disable=SC2086
	ffmpeg -v error $inputs $codec $shape "$dir/$file"
done

# `split_paths`'s separator, and on Windows the path form it reads.
case "$(uname -s)" in
MINGW* | MSYS* | CYGWIN*) sep=';' ;;
*) sep=':' ;;
esac
paths=""
add() {
	file=$1
	[ "$sep" = ';' ] && file=$(cygpath -m "$file")
	paths="$paths${paths:+$sep}$file"
}
for file in "$dir"/* "$@"; do
	add "$file"
done
if [ -f scripts/probe-files.local ]; then
	# One path a line; blank lines and `#` comments skipped, and a Windows line ending too.
	while IFS= read -r file || [ -n "$file" ]; do
		file=$(printf '%s' "$file" | tr -d '\r')
		case $file in
		'' | '#'*) ;;
		*) add "$file" ;;
		esac
	done <scripts/probe-files.local
fi

CLECTA_PROBE="$paths" cargo test --quiet probe_files -- --ignored --nocapture
