# Next Push Handoff

This repository is at a reviewed guild-beta baseline as of 2026-07-24. The CLI
works end to end against current RePoE Fork data, and the implemented crafting
mechanics have synthetic unit and integration coverage. This integration
reconciled the pending hardening/review branches, retained the valid Harvest
pool-applicability guard, fixed cost-insensitive semantic deduplication, and
rejected two mechanics regressions in the reviewed branch. Clipboard item-text
import remains the latest major feature; see README "Importing an Item" for its
modeled-state and unsupported-status boundary.

## Baseline

- `cargo fmt -- --check` passes.
- `cargo test` passes (145 unit, 30 integration, and 36 item-text import tests;
  211 total).
- `cargo clippy --all-targets -- -D warnings` passes.
- Release-mode data loading succeeds with the local RePoE `3.28.0.16` export:
  39,292 mods, 5,059 base items, 774 bench recipes, 106 essences, and 445
  fossils.
- `goals/finish_fractured_chest.toml` completes with seed 42 and the checked-in
  search defaults.

The large, patch-dependent JSON files remain local and ignored. `Cargo.lock` is
tracked so every contributor tests the same dependency resolution.

## Read First

1. Read `README.md`, especially **Reading Results** and **Known Limitations**.
2. Read the module documentation in the files you will edit.
3. Run `git status --short --branch` before writing.
4. Use a deterministic seed for search behavior comparisons.

## Non-Negotiable Invariants

- Modifier conflicts use RePoE `groups`, never display names.
- Apply `required_level <= item_level` before probability calculations.
- Ordered spawn and generation weights use first-match semantics.
- Fractured and crafted affixes count toward capacity and group conflicts.
- All currency randomness comes from the injected RNG.
- Construct `GameData` with `GameData::new`; its sorted craftable index is part
  of deterministic behavior.
- The data layer only loads data. Engine and currency code receive `&GameData`.
- Exact probabilities are preferred. Sampled values must remain visibly marked
  as estimates.
- Do not add `unwrap()` to production paths in `engine/` or `currency/`.
- The import path alone may accept existing mods with zero current spawn
  weight (fractured/legacy/Delve state). Random roll pools and TOML
  starting-item validation must keep rejecting them.
- Generic implicits, enchants, quality, sockets, and displayed ES on
  `ItemState` are craft-invariant: crafting actions must not touch them, they
  never consume explicit capacity or join explicit group conflicts, and the
  search state signature must include them.
- Clipboard influence, Synthesised, and Split status lines are currently
  unsupported metadata, not modeled state. Do not claim imported influence or
  synthesis support until those statuses are preserved or rejected fail-closed.

## Candidate Work Tracks

Choose track boundaries before coding; these are candidates, not a commitment
to implement all of them in one pass.

### Search correctness

- Model recovery after failed one-shot crafts as an explicit policy instead of
  only optimistic and full-restart bounds.
- Replace Monte Carlo identity probabilities with exact enumeration where the
  state space is tractable.
- Add stability reporting across seeds and beam widths.
- Benchmark and profile realistic high-end goals before expanding the state
  model.

### Crafting coverage

- Metamods and their interaction with Scour, Annul, rerolls, and Harvest.
- Veiled currency and unveil choice modeling.
- Awakener's Orb transfer semantics.
- Imprints, locks, recombinators, Orb of Conflict, catalysts, and other
  league-specific systems.

Each mechanic needs applicability tests, exact conflict/capacity behavior,
probability tests, CLI configuration, and user-facing reporting.

### Product hardening

- Small checked-in RePoE-derived fixtures for real-schema integration tests.
- A machine-readable, versioned goal-file schema generated from or checked
  against the maintained reference in `docs/writing-goals.md`.
- Optional machine-readable output for downstream tooling.
- League-price ingestion as a separately bounded feature; current prices are
  deliberately user-supplied snapshots.

## Collaboration Protocol

- Give concurrent writers disjoint files or modules.
- Make complete, reviewable commits instead of checkpoint snapshots.
- Before integration, run targeted tests for the touched module.
- After integration, run the full gate:

```bash
cargo fmt -- --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

- Run at least one seeded real-data goal after changes to search, goal parsing,
  data loading, or crafting behavior.
- Update `README.md`, this handoff, and local agent instructions when the
  supported-mechanics boundary changes.

## Definition of Done

A work track is done only when mechanics, search semantics, tests, CLI
configuration, probability labeling, documentation, and the release smoke test
agree. Passing synthetic tests alone is not enough for changes that depend on
the real RePoE schema.
