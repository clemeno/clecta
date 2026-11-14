#!/bin/sh
# Every gate CI runs (.github/workflows/ci.yml), in CI's order, stopping at the first
# failure, on the Rust `rust-toolchain.toml` pins, as CI's is. CI runs the tests on macOS
# too; here they run on this machine only, and the macOS half is the clippy cross-check.
#
# Needs, once per machine: `cargo install --locked cargo-deny cargo-audit`, and ffmpeg on
# the PATH for the format probe, which is skipped, and says so, without it.
set -ex
cd "$(dirname "$0")"

cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
if command -v ffmpeg >/dev/null; then
	sh scripts/probe-formats.sh
else
	echo 'no ffmpeg on the PATH: the format probe CI runs is skipped here' >&2
fi
cargo clippy --all-targets --target x86_64-apple-darwin -- -D warnings
cargo deny check bans licenses sources
cargo audit
sh scripts/check-vendor.sh
