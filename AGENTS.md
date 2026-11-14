# clecta

A native, portable, two-player desktop audio mixer. The crate root is `desktop/`, not this
directory: `desktop/PLAN.md` is the design record and carries the decision log, and
`desktop/README.md` carries the feature list, the per-module test table and the manual checks.
`GLOSSARY.md` here is the product glossary — it wins on vocabulary, the plan wins on decisions.

## Working here

- **Checks**: `desktop/check.sh` runs every gate CI runs, in CI's order. Run it before committing;
  the `pre-push` hook in `.githooks/` runs it too, once `git config core.hooksPath .githooks` is set.
- **Naming** is idiomatic Rust, and PLAN §13 records why the organization's prefix rules yield
  to `rustc`'s own lints here.
- **Windows**: a running `clecta.exe` locks `target/debug/clecta.exe`, so `cargo build` fails with
  os error 5 until the app is closed; `cargo test` and `cargo clippy` still work. A GUI launch writes
  stderr to `clecta-data/clecta.log`. `desktop/scripts/launch-windows.ps1` starts an isolated copy
  for any look at the real app; `desktop/scripts/smoke-windows.ps1` checks its confirmation boxes appear.

## Agent skills

### Issue tracker

GitHub Issues on `clemeno/clecta`, via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical roles, each label string equal to its name. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `GLOSSARY.md` for the words, `desktop/PLAN.md` for the decisions. See `docs/agents/domain.md`.
