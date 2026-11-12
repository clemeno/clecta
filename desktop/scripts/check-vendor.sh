#!/bin/sh
# The vendored rodio (PLAN Q69, Q71, Q72) is the published 0.23.0 plus one patched file, and
# every change block in that file says why in a `clecta patch` comment. Any other difference
# from the crates.io tarball fails, and so does a change block without the marker, a removal
# included: what replaced the upstream lines is what has to explain itself. The tarball ships
# CRLF and git may store LF, hence --strip-trailing-cr. CI runs this, and so does check.sh.
set -e
cd "$(dirname "$0")/.."

dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
curl -sSfL https://static.crates.io/crates/rodio/rodio-0.23.0.crate | tar -xz -C "$dir"

patched=src/decoder/symphonia.rs
if diff -rq --strip-trailing-cr "$dir/rodio-0.23.0" vendor/rodio | grep -v "/$patched differ\$"; then
	echo "vendor/rodio differs from rodio 0.23.0 beyond $patched" >&2
	exit 1
fi

# In diff's normal format each change block starts with a line like `79,82c86,88`, and its
# added lines start with `> `.
diff --strip-trailing-cr "$dir/rodio-0.23.0/$patched" "vendor/rodio/$patched" | awk '
	/^[0-9]/ { if (block != "" && !marked) print "no clecta patch marker in the change at " block; block = $0; marked = 0 }
	/^> .*clecta patch/ { marked = 1 }
	END { if (block != "" && !marked) print "no clecta patch marker in the change at " block }
' >"$dir/unmarked"
if [ -s "$dir/unmarked" ]; then
	cat "$dir/unmarked" >&2
	exit 1
fi
