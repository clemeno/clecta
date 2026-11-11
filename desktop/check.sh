#!/bin/sh
# Every gate CI runs (.github/workflows/ci.yml), in CI's order, stopping at the first
# failure. CI runs the tests on macOS too; here they run on this machine only, and the
# macOS half is the clippy cross-check.
#
# Needs, once per machine:
#	rustup target add x86_64-apple-darwin
#	cargo install --locked cargo-deny cargo-audit
set -ex
cd "$(dirname "$0")"

cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo clippy --all-targets --target x86_64-apple-darwin -- -D warnings
cargo deny check bans licenses sources
cargo audit
