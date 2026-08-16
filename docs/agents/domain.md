# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

## Before exploring, read these

- **`desktop/PLAN.md`** — the standing design record. It predates this file, it is written to explain
  *why* every choice was made, and its §15 is the decision log (Q1…). It is the closest thing this
  repo has to a `CONTEXT.md`, and it wins over anything below when they disagree.
- **`desktop/README.md`** — the feature list, the per-module test table and the manual-check list.
- **`CONTEXT.md`** at the repo root, if it ever exists.
- **`docs/adr/`** — read ADRs that touch the area you're about to work in.

If any of these files don't exist, **proceed silently**. Don't flag their absence; don't suggest creating them upfront. The `/domain-modeling` skill (reached via `/grill-with-docs` and `/improve-codebase-architecture`) creates them lazily when terms or decisions actually get resolved.

## Layout

Single-context. One crate, rooted at `desktop/`, not at the repo root:

```
/
├── AGENTS.md
├── docs/
│   ├── agents/                        ← this file, and the tracker + label config
│   └── adr/                           ← if and when a decision needs one
└── desktop/
    ├── PLAN.md                        ← the design record, incl. the §15 decision log
    ├── README.md
    └── src/
```

## Don't duplicate the decision log

A decision that fits `PLAN.md` §15 belongs there, in the numbered log, not in a new ADR — the log is
already the place the maintainer reads and the place every `ponytail:` comment points back to.
Reach for `docs/adr/` only for a decision `PLAN.md` has no section for: repo-level process, tooling,
release policy, or anything that outlives the crate.

## Use the plan's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as `PLAN.md` uses it — *deck*, *queue*, *scan*, *trim*, *prepared*, and so on. Don't drift to synonyms.

If the concept you need isn't in `PLAN.md` yet, that's a signal — either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/domain-modeling`).

## Flag conflicts

If your output contradicts `PLAN.md` or an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts PLAN.md §11a (deleting the cache loses nothing but time) — but worth reopening because…_

Two conventions the whole codebase holds to, worth naming here because a skill can break them
without noticing:

- **Every deliberate shortcut carries a `ponytail:` comment** naming its ceiling and its upgrade path.
- **The crate is binary-only** (no `[lib]`), so every test lives in a `#[cfg(test)]` module inside `src/`.
