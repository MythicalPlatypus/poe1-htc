# Next Push Handoff

This repository is at a reviewed guild-beta baseline as of 2026-07-27. The CLI
works end to end against current RePoE Fork data, and the implemented crafting
mechanics have synthetic unit and integration coverage. This integration
reconciled the pending hardening/review branches, retained the valid Harvest
pool-applicability guard, fixed cost-insensitive semantic deduplication, and
rejected two mechanics regressions in the reviewed branch. Clipboard item-text
import remains the latest end-user feature; current work has established the
Stage A service, method identity, pricing/access, data provenance, full A4
scoring, A6 single-cap budgets, A7 controlled search, and A2 versioned JSON
contracts. See README "Importing an Item" for the importer's modeled-state and
unsupported-status boundary.

A3a is also complete: adapters can search owned base-item summaries and query
the exact normal affix pool for a clean Rare base/item-level pair. The query
reuses engine eligibility, preserves ordered weights, and returns deterministic
owned prefix/suffix summaries. Current-item blocker explanations and the
exhaustive craft-selector catalogs remain A3b.

## Last Completed Full Gate

The complete gate was rerun after the A2/A4/A6/A7 slice on 2026-07-27.

- `cargo fmt -- --check` passes.
- `cargo test` passes (241 unit, 32 application-service integration, 4 saved
  DTO contract, 30 engine integration, and 37 item-text import tests; 344
  total).
- `cargo clippy --all-targets -- -D warnings` passes.
- `cargo build --release` passes.
- Release-mode data loading succeeds with the current local RePoE export:
  40,334 mods, 5,161 base items, 765 bench recipes, 106 essences, and 445
  fossils.
- `goals/finish_fractured_chest.toml` completes with seed 42 and the checked-in
  search defaults.
- The imported-item per-unit goal completes, and the zero-expansion run
  terminates normally at generation zero with resolved seed 42.
- The A3 release catalog probe returns 46 prefixes and 63 suffixes for an
  item-level-86 Astral Plate.

The large, patch-dependent JSON files remain local and ignored. `Cargo.lock` is
tracked so every contributor tests the same dependency resolution.

## Active Desktop UI Track

Stage A from [`docs/ui-project-plan.md`](docs/ui-project-plan.md) is in
progress. A1 is complete: the service boundary accepts owned, file-independent
requests, strict raw clipboard text, resolved search settings, configured
methods, and typed price overrides. Preparation validates and constructs the
starting state, preserves deterministic method order, returns structured
diagnostics, and can be handed to a background worker. Search execution and
goal evaluation live behind `OptimizerService`; `cli::run` is limited to
file/stdin and flag adaptation, legacy price-name resolution, and
human-readable rendering.

The internal `ItemSpec` / `WantSpec` / `MethodSpec` adapters remain convenient
domain inputs, while A2 now exposes a separate strict `schema_version = 1`
saved-request and response contract. Seeds use canonical decimal strings,
requests map both ways through checked conversions, response mapping carries
engine/data identity, effective methods and sampling settings, all item slots,
goal reports, all three cost models, per-metric budget comparisons, actual
runtime limits, termination, diagnostics, and the resolved seed. A checked
request JSON Schema and five response compatibility fixtures cover complete,
incomplete, sampled, impossible, and over-budget cases.

A5 and A6 are complete for the first single-cap release: every concrete craft
has semantic identity and structured registry metadata, path identity and
reporting use `MethodId`, `PriceBook` is ID-keyed, duplicate IDs are rejected,
and callers can use legacy defaults, an exact allowlist, or family selection
with individual additions/exclusions without changing registry order. Budgets
bind first-try, retry-expected, or restart-adjusted expected chaos cost,
default to restart-adjusted, prune during search, and retain a labeled
goal-complete over-budget exemplar when no compliant completion exists.
Wallet/resource-vector accounting remains a later extension. Exhaustive craft
catalogs and exact supported item-class lists remain A3 work.

Immutable data provenance is now complete: the pure loader fingerprints the
exact five-file RePoE bundle with versioned SHA-256 framing, keeps an optional
trusted `repoe-version.txt` label separate, and returns both with `GameData`.
`OptimizerService` binds that pair to prepared work and every result without
weakening exact runtime dataset-identity checks.

A4 is complete: legacy wants default to required; presence, threshold, and
per-unit scoring support higher/lower directions and caps; numeric selectors
aggregate all matches and emit typed first-stat warnings; bounded maximum
scores are optional; and conservative pre-search analysis returns structured
normal `impossible` responses for proven group, capacity, source, or
uncraftable-item failures.

A7 core runtime control is complete: generation-committed progress snapshots,
cooperative cancellation, time and expansion limits, normal typed termination,
and exactly one resolved/replayable seed per run. The CLI exposes timeout and
expansion limits; cancellation and streaming observers remain service APIs
until the desktop adapter exists.

Next:

1. Complete A3b current-item blocker explanations and exhaustive craft
   selector catalogs.
2. Complete A8 benchmarks, realistic RePoE fixtures, and release smoke
   coverage for the new semantics.
3. Start the Tauri adapter only after the remaining Stage A exit criteria pass.
4. Keep wallet/resource-vector budgets as a separately specified later
   extension.

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
- The data layer only reads/parses inputs and computes source provenance; it
  never writes data or performs crafting behavior. Engine and currency code
  receive `&GameData`.
- Exact probabilities are preferred. Sampled values must remain visibly marked
  as estimates.
- Required-goal completion outranks raw preference score everywhere: beam
  ranking, sibling hit probabilities, terminal checks, and path pruning.
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
