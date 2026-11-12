#!/bin/sh
# Each vendored crate (PLAN Q69, Q71, Q72) is its published release plus the files it patches,
# and every change block in those files says why in a `clecta patch` comment. Any other
# difference from the crates.io tarball fails, and so does a change block without the marker,
# a removal included: what replaced the upstream lines is what has to explain itself. The
# tarballs may ship CRLF and git may store LF, hence --strip-trailing-cr. CI runs this, and so
# does check.sh.
set -e
cd "$(dirname "$0")/.."

dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
: >"$dir/wrong"

# vendored NAME VERSION PATCHED-FILE...
vendored() {
	name=$1
	version=$2
	shift 2
	curl -sSfL "https://static.crates.io/crates/$name/$name-$version.crate" | tar -xz -C "$dir"
	published="$dir/$name-$version"

	diff -rq --strip-trailing-cr "$published" "vendor/$name" >"$dir/differences" || true
	for file in "$@"; do
		grep -v -F "and vendor/$name/$file differ" "$dir/differences" >"$dir/rest" || true
		mv "$dir/rest" "$dir/differences"
	done
	sed "s|^|vendor/$name differs from $name $version: |" "$dir/differences" >>"$dir/wrong"

	# In diff's normal format each change block starts with a line like `79,82c86,88`, and its
	# added lines start with `> `.
	for file in "$@"; do
		diff --strip-trailing-cr "$published/$file" "vendor/$name/$file" | awk -v file="vendor/$name/$file" '
			/^[0-9]/ { if (block != "" && !marked) print file ": no clecta patch marker in the change at " block; block = $0; marked = 0 }
			/^> .*clecta patch/ { marked = 1 }
			END { if (block != "" && !marked) print file ": no clecta patch marker in the change at " block }
		' >>"$dir/wrong"
	done
}

vendored rodio 0.23.0 src/decoder/symphonia.rs
vendored symphonia-format-isomp4 0.5.5 src/demuxer.rs src/atoms/elst.rs

if [ -s "$dir/wrong" ]; then
	cat "$dir/wrong" >&2
	exit 1
fi
