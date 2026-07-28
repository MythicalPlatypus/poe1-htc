# PoE1 HTC Desktop UI Project Plan

Status: In Progress  
Target: A local desktop application for building a crafting goal, applying
resource and method restrictions, running the optimizer, and comparing several
recommended paths.

## 1. Product Decision

Build the first UI as a **Tauri 2 desktop application** with a React and
TypeScript frontend.

This is the best fit for the current project because:

- The crafting engine already exists as a Rust library.
- Searches are CPU-heavy and use Rayon; they should remain native rather than
  being moved into browser JavaScript or WebAssembly.
- RePoE data is large, patch-dependent, and already loaded from local files.
- A desktop application can call the Rust library directly without operating a
  hosted API, job queue, database, or user account system.
- The same typed application service can support the existing CLI and a future
  hosted web API.

A public web application is not part of the first release. It can be added
later by placing an HTTP adapter in front of the application service.

## 2. Product Goal

The user should be able to:

1. Select a base item and item level, or paste an existing in-game item.
2. Review and edit the item's existing prefixes, suffixes, fractures, crafted
   modifier, influences, and supported implicits.
3. Browse affixes compatible with the selected base and item level.
4. Select desired affixes, tiers, or stat thresholds.
5. Mark goals as required or preferred and assign relative weights.
6. Select the crafting systems and individual crafts they can use.
7. Enter a currency budget and customize currency prices.
8. Run a cancellable, reproducible optimizer search.
9. Compare several distinct crafting paths, their costs, probabilities,
   resulting items, assumptions, and warnings.
10. Save, reopen, and export a craft configuration.

## 3. First-Release Scope

### Included

- Windows desktop application.
- Local RePoE data loading and version display.
- Searchable base-item picker.
- In-game clipboard item import.
- Manual starting-modifier editor.
- Compatible prefix and suffix browser.
- Presence, minimum-roll, and value-scaled goal scoring.
- Required and preferred goals.
- Crafting-method allowlist.
- Custom prices and a hard chaos-equivalent spending cap.
- Top distinct crafting paths.
- Representative final item for each path.
- Expected cost, restart-adjusted cost, and success probability.
- Background search, progress, cancellation, and deterministic seeds.
- Local save/load and TOML/JSON export.
- Clear approximation and unsupported-mechanic warnings.

### Deferred

- Hosted multi-user web service.
- Authentication, cloud storage, or shared public links.
- Live economy-price ingestion.
- Exact stash inventory consumption by every individual currency type.
- Full probability distribution of all outcomes for every recommended path.
- Interactive manual craft emulation.
- Mobile support.
- Crafting mechanics that are not yet implemented by the Rust engine.

The first budget implementation accepts one chaos-equivalent hard cap and a
selected expected-cost policy. Chaos/Divine wallet quantities, conversion
rates, and exact per-currency inventory accounting are later extensions because
stochastic retry paths require a defined "complete before inventory runs out"
probability model.

## 4. UX Outline

The main workflow uses four persistent steps.

### Step 1: Starting Item

- Search bases by display name and filter by item class.
- Select item level and supported influences.
- Start from a clean base, paste an item, or load a saved craft.
- Display prefix, suffix, fractured, crafted, implicit, and enchantment slots.
- Allow manual modifier entry with tier and roll selection.
- Show import diagnostics without hiding crafting-relevant warnings.
- Prevent impossible capacity, group-conflict, item-level, and roll choices.

### Step 2: Desired Affixes

- Show only affixes compatible with the current base and item level.
- Group results by prefix/suffix, modifier family, and tier.
- Search using rendered modifier text and stat names.
- Let the user choose:
  - exact modifier or any tier in a modifier group;
  - minimum tier or minimum numeric roll;
  - required versus preferred;
  - presence/threshold versus value-scaled scoring;
  - relative weighting.
- Explain conflicts between selected desired affixes.
- Display the maximum possible goal score when it is bounded and known, plus
  whether all required goals can coexist.

### Step 3: Resources

- Enter one chaos-equivalent hard cap and select its expected-cost policy.
- Override individual craft prices.
- Enable or disable crafting families.
- Select individual bench recipes, Essences, Fossils/resonators, Harvest
  operations, Bestiary crafts, influence crafts, and Eldritch crafts.
- Display unsupported systems as unavailable rather than silently ignoring
  them.
- Choose search breadth, maximum path length, result count, and seed under an
  advanced section.

### Step 4: Results

- Run the search in a background task.
- Display current search depth, states examined, elapsed time, and warnings.
- Allow cancellation.
- Present one card per distinct pathway with:
  - completion status and goal score;
  - ordered crafting steps;
  - cost per application;
  - hit chance and whether it is sampled;
  - first-try, retry, and restart-adjusted cost;
  - one-shot success probability;
  - representative final item;
  - satisfied and unsatisfied goals;
  - budget and method-access compliance;
  - engine warnings and known approximations.
- Allow paths to be sorted by completion, expected cost, success chance, or
  weighted score.
- Offer "search wider" and "rerun with a different seed" actions.

## 5. Architecture

```text
React + TypeScript UI
        |
        | typed Tauri commands and events
        v
Tauri adapter
        |
        v
Rust application service
  - request validation
  - base/affix/method catalogs
  - starting-item construction/import
  - budget and access policy
  - optimizer orchestration
  - result DTO construction
        |
        v
Existing Rust domain engine
  - GameData
  - ItemState
  - modifier pools
  - CraftingMethod
  - BeamSearch
```

The UI must not generate temporary TOML and scrape CLI output. The CLI and
Tauri adapter must both call the same application service.

Suggested repository layout:

```text
src/
  app/
    catalog.rs
    request.rs
    result.rs
    runner.rs
    validation.rs
  budget/
  cli/
  currency/
  data/
  engine/
  goal/
  import/
  item/
  search/
ui/
  src/
    components/
    features/
    routes/
    state/
    types/
src-tauri/
  src/
```

Do not move the existing Rust modules into a new crate unless the Tauri
integration demonstrates a concrete Cargo-boundary problem. The root package
already exposes a library target.

## 6. Stage A: Prepare the Rust Project

No frontend work begins until milestones A1 through A8 are complete.

### M0 Contract Decisions

The following compatibility-first decisions are accepted for Stage A:

- Saved requests and responses will begin with a versioned
  `schema_version = 1` JSON contract in A2. That saved schema is separate from
  the current internal A1 request model. In particular, A1b currently uses
  provisional `ItemSpec`, `WantSpec`, and `MethodSpec` adapters and is not the
  frozen serialized v1 DTO.
- Clipboard starts cross the application boundary as raw text plus an optional
  fallback item level, never as a file path or stdin marker. File and stdin
  reading remain adapter responsibilities.
- Every legacy TOML `[[wants]]` entry maps to `required = true`, preserving the
  current definition of target completion when preferred goals are added.
- `MethodId` is an opaque, semantic identifier based on the craft operation and
  its behavior-changing parameters, never its display name or price. IDs use
  stable family/operation segments (for example `currency/chaos` and
  `bench/add-explicit/<mod-id>`), with canonical configuration identity for
  composite crafts. Search order remains the current built-in order followed
  by configured-method request order so fixed seeds do not change.
- `PriceBook` is keyed by `MethodId`. The legacy display-name `[prices]` table
  is resolved by the CLI adapter, including its current unmatched-name
  warnings, before the service runs.
- Existing CLI requests use an unbounded budget. The desktop default is a hard
  cap on restart-adjusted expected chaos-equivalent cost; it must be labeled as
  a modeled expectation rather than guaranteed wallet consumption.
- Application errors and warnings use typed diagnostics with stable codes and
  field paths. Machine-readable costs use an explicit `CostValue` for finite,
  unbounded, or unavailable values; raw `NaN` and infinity are never
  serialized.
- Data provenance lives beside immutable `GameData` in `OptimizerService`, not
  inside the domain database. Responses report an optional RePoE version and a
  stable data fingerprint rather than assuming the README's tested version is
  installed.
- Cancellation is supplied through a non-serialized runtime context containing
  an observer and cancellation token. A cancelled v1 response includes its
  last progress snapshot and diagnostics plus any results from the last fully
  completed search generation (see the ratified termination decisions below).

### A2 Semantic Contract Decisions (Ratified 2026-07-27)

These decisions settled the goal-scoring, budget, termination, status, and cost
semantics that blocked the v1 contract freeze. They bind A2, A4, A6, and A7;
the internal implementation and frozen v1 DTOs now encode these semantics.

Goal scoring:

- Scoring modes are `presence` (default), `threshold`, and `per_unit`.
  Completion stays a binary predicate per want; scoring modes shape only the
  preference score, so `required` semantics and completion-first beam ordering
  are unchanged.
- The `per_unit` attained value is the sum of the selected stat's rolled
  values across every matching mod. Contribution is
  `weight * max(0, min(attained, cap))` for higher-is-better and
  `weight * max(0, cap - attained)` for lower-is-better.
- When a higher-is-better `per_unit` want omits `cap`, its contribution is
  `weight * max(0, attained)`; its maximum score is reported as unavailable
  unless a sound data-bound maximum has been computed.
- A `per_unit` want may be `required` only when it also declares a threshold
  bound; the bound alone defines satisfaction.
- `cap` is optional for higher-is-better and mandatory for lower-is-better so
  every contribution stays bounded and non-negative.
- Direction is inferred from the declared bound: `min_value` means
  higher-is-better, `max_value` means lower-is-better, and declaring both on
  one want is an error.
- A preferred `per_unit` want with no threshold defaults to
  higher-is-better. `presence` rejects bounds and `cap`; `threshold` requires
  exactly one bound and rejects `cap`; `per_unit` follows the required/bound
  and directional cap rules above.
- Numeric modes may select by `stat`, `mod_id`, or `group`. When a numeric
  want matches without an explicit `stat`, each matching mod contributes the
  value of its first RePoE-declared stat; matching a multi-stat mod this way
  emits a typed warning recommending an explicit `stat` selector.
- Provably impossible required combinations (modifier-group conflicts,
  affix-slot overflow, zero-weight mods with no deterministic source) are
  detected conservatively before search and returned as a normal response
  with `outcome = impossible` and structured per-want reasons, never as an
  application error. Unproven cases search normally and may return
  incomplete.

Budgets:

- First-try, retry-expected, and restart-adjusted expected cost are all
  defined budget policies; restart-adjusted expected cost is the default
  binding metric because it weights likelihood (a first-try policy would call
  an Alchemy-only mirror-tier craft affordable). Responses report all three
  figures, each with its own comparison and excess against the active cap, so
  policies can be compared per result.
- The comparison value is `under`, `over`, or `not_comparable`.
  `CostValue::Unavailable` always produces `not_comparable`; an unbounded
  value is always `over` when a cap exists. With no active cap every metric is
  `not_comparable`.
- The budget prunes during search: a node whose selected policy metric
  already exceeds the cap is dropped. Pruning must not change unconstrained
  seeded results.
- Complete-but-over-budget paths are returned with per-path status
  `over_budget` plus the exceeded amount, alongside the best under-budget
  incomplete alternatives.
- Response `outcome` describes whether goal completion was found, independently
  of budget compliance. Therefore an `over_budget` path is goal-complete and
  makes the response outcome `complete`; consumers use each path's status to
  decide whether it satisfies the active cap.
- v1 budgets are a single chaos-equivalent number. Wallet inputs and
  conversion rates move to the A6 later extension with resource vectors.

Search termination:

- Cancelled, timed-out, expansion-limited, exhausted, and pre-search
  impossible runs return normal responses, never errors.
  `termination.reason` (`target_reached`, `step_limit`, `expansion_limit`,
  `timed_out`, `cancelled`, `search_exhausted`, `impossible`) is a separate
  field from `outcome`, so a cancelled run that already found a complete path
  reports both facts.
- `target_reached` means a complete result reached the known maximum
  preference score. Merely finding required-goal completion does not stop a
  search that can still improve preferences. An uncapped score may have no
  known maximum and therefore cannot terminate for `target_reached`.
- The termination snapshot reports elapsed time, completed generations versus
  `max_steps`, states generated and retained, best score so far, and whether
  any complete result existed at stop time.
- Cancellation and deadline checks may run at fine granularity inside a
  generation for responsiveness, but returned results always come from the
  last fully completed generation, so every reported path remains
  reproducible by an uncancelled run with the same seed.
- Snapshot counters and `complete result existed` describe that same last
  fully committed generation, not discarded partial work.
- Every run resolves exactly one `u64` seed before expansion. An omitted seed
  requests a newly generated run seed; the resolved value is stored in the
  response and reused for every deterministic substream.

Result statuses:

- Response-level `outcome`: `complete`, `incomplete`, `impossible`.
- Per-path `status`: `complete`, `incomplete`, `over_budget`.
- Both serialize as snake_case strings under `schema_version = 1`; adding a
  variant later requires an explicit schema bump.

Costs:

- `CostValue` (`finite`, `unbounded`, `unavailable`) is adopted by the
  engine's cost accounting, not only the DTO layer. Finite arithmetic keeps a
  fast path, and prepare-time validation still guarantees every method price
  is finite and positive.
- `unbounded` represents diverging expectations (zero success probability
  under the retry model) and is always over budget. `unavailable` marks a
  metric not computed for the run; it never participates in a budget
  comparison and can never silently pass a cap.
- JSON serializes `CostValue` as a tagged union; raw `NaN` and infinity never
  appear.

### Implementation Status (2026-07-27)

- **A1 complete:** `OptimizerService` owns validation, preparation, search, and
  goal evaluation. The CLI is a file/stdin, flag-precedence, legacy-pricing,
  and human-readable rendering adapter.
- **A5 complete for effective instances:** every concrete crafting method has a
  validated semantic `MethodId` and structured family, description, intrinsic
  price, setup/catalog, coarse item-class, and probability metadata. Search
  identity and path steps use IDs; duplicate IDs are rejected; and access can
  combine family-wide switches with individual additions/exclusions without
  changing registry order. Exhaustive selectable craft catalogs and exact
  item-class lists remain A3 work.
- **A6 single-cap core complete:** `PriceBook` is a validated, deterministic
  `MethodId`-keyed map. First-try, retry-expected, and restart-adjusted
  policies have per-metric comparison/excess reporting; restart-adjusted is
  the default; hard caps prune search; and complete over-budget exemplars are
  labeled without being treated as compliant. Wallet/resource-vector inputs
  remain a later extension.
- **M0 data provenance complete:** loading produces a versioned SHA-256
  fingerprint over the exact bytes and presence of the five RePoE JSON inputs.
  An optional trusted `repoe-version.txt` label is reported separately.
  `OptimizerService` binds both to its immutable data set, preparation
  summaries, repricing, and results; matching fingerprints never replace exact
  in-process dataset identity.
- **A4 complete:** required/preferred presence, threshold, and per-unit modes
  support caps and higher/lower directions. Numeric selectors aggregate all
  matching modifiers, implicit first-stat selection emits typed warnings,
  maximum scores are optional when uncapped, and conservative pre-search
  analysis returns structured `impossible` reasons for proven failures.
- **A7 core runtime complete:** observers receive generation-committed
  snapshots; cancellation, timeouts, and expansion limits are normal
  termination states; partial generations are discarded; and every run
  resolves one replayable seed. CLI flags expose time and expansion limits;
  cancellation/streaming remain service APIs until the Tauri adapter.
- **A2 complete:** strict version-1 saved request/response DTOs map around lean
  domain types, reject non-finite and contradictory values, preserve actual
  runtime limits and provenance, and serialize seeds as canonical decimal
  strings. A checked request schema and golden fixtures cover complete,
  incomplete, sampled, impossible, and over-budget responses.
- The complete 2026-07-27 regression gate passes: 241 unit, 32
  application-service integration, 4 saved DTO contract, 30 engine
  integration, and 37 item-text import tests (344 total), plus strict Clippy,
  formatting, and release-build gates. Release-mode imported-item, per-unit,
  zero-expansion, and fractured-chest smoke runs also pass with seed 42.

### A1. Establish the Application-Service Boundary — Complete

Estimate: 4-6 engineering days

Tasks:

- Create `src/app/`.
- Move orchestration out of `cli::run` into a reusable `OptimizerService`.
- Define owned request types that do not depend on Clap or TOML parsing.
- Keep `GoalSpec` as an input-file adapter, not the only programmatic API.
- Move base resolution and default-method construction out of the CLI.
- Make the CLI translate flags/TOML into an application request.
- Preserve existing CLI output and behavior.

Target contract after A2, A6, and A7 (the completed A1 boundary currently uses
staged internal request types):

```rust
pub struct OptimizeRequest {
    pub starting_item: StartingItemRequest,
    pub goals: Vec<GoalRequest>,
    pub configured_methods: Vec<ConfiguredMethodRequest>,
    pub method_access: MethodAccessPolicy,
    pub prices: PriceBook,
    pub budget: BudgetPolicy,
    pub search: SearchRequest,
}

pub struct OptimizerService {
    // Immutable GameData and application catalogs.
}

impl OptimizerService {
    pub fn validate(&self, request: &OptimizeRequest) -> ValidationReport;
    pub fn optimize(
        &self,
        request: OptimizeRequest,
        observer: &dyn SearchObserver,
    ) -> anyhow::Result<OptimizeResponse>;
}
```

Acceptance criteria:

- CLI integration tests produce equivalent paths for fixed seeds.
- The application service can be called without reading or writing files.
- No application-service method prints to stdout.
- Errors contain stable codes plus human-readable messages.

### A2. Add Versioned Machine-Readable Contracts — Complete

Estimate: 3-4 engineering days

Tasks:

- Add serializable request, response, warning, modifier, item, path, and cost
  DTOs.
- Keep domain types lean; map them to DTOs rather than adding UI fields to
  `ItemState`.
- Add `schema_version` to saved requests and responses.
- Represent non-finite costs explicitly instead of attempting to serialize
  `NaN` or infinity.
- Include data version, engine version, seed, sampling settings, and elapsed
  time in every response.
- Generate or check a JSON Schema for saved craft requests.

Acceptance criteria:

- Request and response JSON round-trip tests pass.
- Golden fixtures cover complete, incomplete, sampled, impossible, and
  over-budget results.
- Schema changes require an explicit version or migration.

### A3. Build Read-Only Catalog APIs — In Progress (A3a Complete)

Estimate: 4-6 engineering days

Status: normalized deterministic base search and clean-Rare compatible normal
affix queries are complete. They return owned summaries and delegate pool
membership and effective weights to the crafting engine. Current-item blocker
reasons, tier-family presentation, selector catalogs, and caching remain A3b.

Tasks:

- Add searchable base-item summaries.
- Add modifier summaries with rendered display text, generation type, group,
  tier, required level, stat ranges, domain, and tags.
- Add a compatible-affix query using the existing eligibility rules.
- Support two compatibility views:
  - rollable now on the current item;
  - compatible with the base but blocked by current capacity or conflicts.
- Expose conflict reasons and occupied-slot information.
- Add exhaustive selector catalogs for method families, Essences, Fossils,
  bench recipes, Harvest, Bestiary, influence, and Eldritch operations,
  including concrete compatible item-class lists. A5 metadata describes only
  instances already registered for an optimization.
- Cache normalized search strings and stable sort keys.

Acceptance criteria:

- Catalog results are deterministic.
- Modifier-group conflicts use RePoE groups.
- Required-level and ordered spawn-weight rules match the crafting engine.
- The UI cannot present a modifier as rollable when the engine rejects it.

### A4. Extend Goal and Scoring Semantics — Complete

Estimate: 5-7 engineering days

Status: required/preferred completion, presence/threshold/per-unit scoring,
caps, higher/lower directions, first-stat diagnostics, optional score ceilings,
and conservative impossible-required-goal detection are complete.

Tasks:

- Separate goal completion from preference scoring.
- Add `required: bool`.
- Support scoring modes:
  - `presence`;
  - `threshold`;
  - `per_unit`;
  - optional capped `per_unit` scoring.
- Define multi-stat modifier behavior explicitly.
- Support "higher is better" and "lower is better" directions.
- Continue supporting exact modifier ID, RePoE group, and stat selectors.
- Detect impossible required-goal combinations before search.
- Preserve legacy TOML behavior by mapping existing wants to presence or
  threshold goals.

Acceptance criteria:

- Existing goal files keep their current scores.
- Unit tests cover threshold boundaries, multiple tiers, negative stat ranges,
  lower-is-better scoring, caps, and conflicting goals.
- A Path of Building-style per-point weight can be represented without
  multiplying the weight merely for modifier presence.

### A5. Create a Stable Crafting-Method Registry — Complete for Effective Instances

Estimate: 5-7 engineering days

Status: semantic identity, path-step identity, duplicate-ID validation,
structured metadata, exact allowlists, and family-wide selection with
per-method overrides are complete. Enumerating every possible configured craft
and its exact item classes belongs to the A3 catalog APIs.

Tasks:

- Give every crafting method a stable `MethodId`.
- Move the default orb list out of `cli/mod.rs`.
- Add method metadata:
  - display name;
  - family;
  - description;
  - default price;
  - required catalog or configuration;
  - coarse item-class support classification;
  - probability quality/approximation label.
- Replace "default methods always enabled" with an explicit allowlist policy.
- Support family-wide toggles and individual configured-method selection.
- Validate duplicate or incompatible configured methods before search.

Acceptance criteria:

- Disabling a method guarantees it cannot appear in a path.
- Existing CLI runs retain the legacy default policy and configured additions;
  programmatic callers can use exact-ID allowlists or family/individual
  selection.
- Each path step reports the stable method ID and display name.

### A6. Add Budget Policies — Single-Cap Core Complete

Estimate: 5-8 engineering days

Status: typed ID-keyed pricing, semantic repricing, legacy CLI price-name
adaptation, the three cost policies, hard-cap pruning, per-metric
comparison/excess, and labeled over-budget completions are complete. UI stale
result invalidation belongs to the later adapter; wallet inputs remain the
ratified later extension.

Tasks:

- Add a typed `PriceBook`.
- Accept a single chaos-equivalent hard cap (ratified: no wallet inputs in
  v1).
- Define separate policies for:
  - first-try cost;
  - retry expected cost;
  - restart-adjusted expected cost.
- Use restart-adjusted expected cost as the default budget policy.
- Prune nodes that can no longer satisfy the selected monotonic budget policy.
- Report why a candidate was rejected as over budget.
- Retain `cost_weight` as an optional preference within the allowed budget.

Acceptance criteria:

- Every under-budget result is within the selected hard cap. A goal-complete
  exemplar may exceed it only when returned with status `over_budget` and an
  explicit exceeded amount; it is never treated as budget-compliant.
- Zero, absent, and extremely large budgets behave predictably.
- Price changes invalidate stale result sets.
- Budget pruning does not change unconstrained seeded results.

Later extension:

- Add wallet inputs (Chaos and Divine holdings) with conversion through the
  `PriceBook`, recorded in the response.
- Track resource vectors for exact quantities of Divines, Essences, Fossils,
  Lifeforce, beasts, and other non-fungible resources.
- Calculate the probability of completing repeatable steps before inventory is
  exhausted.

### A7. Add Progress, Cancellation, and Search Limits — Core Runtime Complete

Estimate: 4-6 engineering days

Tasks:

- Add a `SearchObserver` interface.
- Emit progress after each completed search depth.
- Report states generated and retained, current beam size, best score, whether
  a complete result exists, and elapsed time.
- Add cancellation through an atomic token checked at bounded intervals.
- Add optional wall-clock and expansion limits.
- Return normal typed cancelled/timed-out/limited termination with results from
  the last fully committed generation.
- Keep deterministic results unchanged when no cancellation or time limit is
  used.

Acceptance criteria:

- Cancellation completes promptly without corrupting shared data.
- Progress reporting does not introduce nondeterministic result ordering.
- CLI timeout/expansion flags and service cancellation/observer tests pass.
- Long searches never block the Tauri UI thread (Tauri adapter work remains in
  Stage B).

### A8. Rust Integration and Regression Gate

Estimate: 4-6 engineering days

Tasks:

- Add application-service integration tests using synthetic data.
- Add real-data smoke tests behind the existing local-data boundary.
- Add saved request/response compatibility fixtures.
- Update CLI documentation and goal-file documentation.
- Add benchmarks for representative broad searches.
- Profile data loading and the catalog queries used by the UI.

Required gate:

```bash
cargo fmt -- --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Stage A exit criteria:

- The CLI is a thin adapter over the application service.
- A complete optimization can be submitted and returned as typed JSON.
- Catalog queries can populate every planned selector.
- Method allowlists and hard budgets are enforced by the engine.
- Searches are observable and cancellable.
- Fixed-seed CLI behavior remains covered by regression tests.

## 7. Stage B: Build the Desktop UI

Stage B starts only after the Stage A exit criteria pass.

### B1. Scaffold Tauri and Frontend Foundations

Estimate: 3-4 engineering days

Tasks:

- Add Tauri 2 under `src-tauri/`.
- Add React, TypeScript, and Vite under `ui/`.
- Establish formatting, linting, unit-test, and build scripts.
- Generate TypeScript types from, or validate them against, the Rust contracts.
- Add error boundaries, application logging, and a consistent component theme.
- Define a single application state model for the draft craft and active
  search.

Acceptance criteria:

- A packaged development build starts and loads data through a Tauri command.
- Contract drift fails CI.
- No optimizer call runs on the UI thread.

### B2. Data Installation and Update Experience

Estimate: 3-5 engineering days

Tasks:

- Detect missing or invalid RePoE files.
- Show supported and installed data versions.
- Provide a first-run data-folder picker.
- Optionally download the known supported RePoE snapshot after explicit user
  action.
- Store application settings and data location in the platform app-data
  directory.
- Validate files before replacing the active data set.
- Retain the previous working data set if an update fails.

Acceptance criteria:

- Missing data produces an actionable screen instead of a Rust path error.
- The application never runs a search against a partially updated data set.

### B3. Starting-Item Builder

Estimate: 5-7 engineering days

Tasks:

- Build item-class filters and searchable base selection.
- Add item-level and influence controls.
- Add clean-base and clipboard-import flows.
- Render imported item slots and diagnostics.
- Add manual modifier, tier, roll, fractured, and crafted controls.
- Revalidate dependent choices whenever base, level, influence, or existing
  modifiers change.

Acceptance criteria:

- The UI can reproduce every supported `ItemSpec`.
- Clipboard import warnings remain visible until acknowledged or corrected.
- Impossible item states cannot be submitted.

### B4. Desired-Affix Builder

Estimate: 5-8 engineering days

Tasks:

- Build compatible prefix/suffix browsing and search.
- Render modifier families and tiers clearly.
- Add required/preferred controls.
- Add threshold, tier, and weighting inputs.
- Add per-unit scoring controls.
- Show conflicts and impossible combinations immediately.
- Provide sensible keyboard navigation for large modifier lists.

Acceptance criteria:

- The UI can reproduce legacy wants and the new scoring modes.
- Every selector uses stable IDs internally, never display strings alone.

### B5. Resources and Method Access

Estimate: 4-6 engineering days

Tasks:

- Build chaos-equivalent hard-cap and budget-policy inputs.
- Build family and individual method toggles from the method registry.
- Add custom price editing.
- Add configuration panels for Essences, Fossils/resonators, bench crafts,
  Harvest, Bestiary, influence, and Eldritch methods.
- Summarize the effective method allowlist before search.

Acceptance criteria:

- The request preview exactly matches the visible selections.
- Disabled methods never appear in returned paths.
- Invalid prices and empty method sets are blocked before search.

### B6. Search Execution Experience

Estimate: 3-5 engineering days

Tasks:

- Run optimization through an asynchronous Tauri command.
- Stream progress events.
- Add cancellation and elapsed-time display.
- Prevent accidental duplicate submissions.
- Retain the submitted immutable request beside its results.
- Mark results stale when the draft configuration changes.

Acceptance criteria:

- The application remains responsive during release-mode searches.
- Cancellation and failure return the user to an editable draft.
- A completed result is traceable to its exact request and seed.

### B7. Results and Path Comparison

Estimate: 5-8 engineering days

Tasks:

- Build path summary cards and a side-by-side comparison view.
- Render each step's method, price, probability, retry behavior, and
  approximation state.
- Render representative final items with modifier rolls.
- Display satisfied and unsatisfied goals.
- Show first-try, retry, and restart-adjusted economics distinctly.
- Add sorting and filtering.
- Add rerun actions for broader beam width and alternate seeds.
- Add copy/export for a selected path.

Acceptance criteria:

- The UI never presents sampled probability as exact.
- Infinite/unresolved costs have meaningful text rather than invalid numbers.
- A user can explain why one path ranks above another from the displayed data.

### B8. Save, Load, and Export

Estimate: 2-4 engineering days

Tasks:

- Save versioned craft requests locally.
- Load and migrate supported older request versions.
- Export request JSON and compatible TOML where representable.
- Export a human-readable path report.
- Add recent-craft history without storing large duplicate data sets.

Acceptance criteria:

- A saved craft reproduces the same request and seeded search configuration.
- Unsupported downgrade to legacy TOML is explained rather than lossy.

### B9. Desktop Hardening and Release

Estimate: 5-8 engineering days

Tasks:

- Add frontend unit tests and Tauri integration tests.
- Add keyboard navigation and basic accessibility checks.
- Test small and large displays.
- Test missing, old, malformed, and read-only data directories.
- Test long-running searches and cancellation repeatedly.
- Add signed Windows packaging and update strategy.
- Add crash/log collection that does not upload data without consent.
- Write user documentation and an in-app limitations page.

Release gate:

```bash
cargo fmt -- --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
npm run lint
npm run test
npm run build
tauri build
```

## 8. Milestones and Schedule

The detailed per-task estimates assume one experienced engineer working
sequentially, familiar with Rust and modern TypeScript. The compressed
milestone targets below require meaningful overlap or additional engineering
capacity.

| Milestone | Deliverable | Estimate |
|---|---|---:|
| M0 | Contracts and UX decisions finalized | 2-3 days |
| M1 | Rust application service and DTOs | 1.5-2 weeks |
| M2 | Catalogs, scoring, method access, and budgets | 2-3 weeks |
| M3 | Progress, cancellation, and Rust regression gate | 1 week |
| M4 | Tauri shell, item builder, and affix builder | 2-3 weeks |
| M5 | Resources, search execution, and results | 2-3 weeks |
| M6 | Persistence, packaging, tests, and beta hardening | 1.5-2.5 weeks |

Expected delivery targets with overlapping work or additional capacity:

- Internal functional prototype: 5-7 weeks.
- Usable desktop alpha: 8-10 weeks.
- Dependable beta: 10-14 weeks.

For one engineer executing the detailed A1-A8 and B1-B9 tasks sequentially,
the estimates total approximately 14-21 weeks, plus the 2-3 day M0 decision
phase. The 10-14 week beta target is therefore a parallel-capacity target, not
the sequential planning range.

These estimates exclude implementation of currently unsupported crafting
mechanics and exact finite-inventory probability modeling.

## 9. Testing Strategy

### Rust

- Unit tests beside catalog, scoring, budget, and adapter code.
- Synthetic integration tests for complete request-to-response behavior.
- Fixed-seed regression fixtures for path selection.
- Property tests for budget monotonicity and serialization round trips where
  useful.
- Real-data release smoke tests.
- Benchmarks for catalog search, broad affix queries, and representative
  optimizer requests.

### Frontend

- Component tests for item slots, modifier selection, budget validation, and
  path economics.
- Contract tests using checked-in Rust response fixtures.
- End-to-end tests for:
  - clean-base craft;
  - clipboard-import craft;
  - required and weighted goals;
  - disabled crafting family;
  - over-budget search;
  - cancellation;
  - save and reopen.

### Manual acceptance scenarios

- Finish an existing fractured rare.
- Build a fresh life/resistance body armour.
- Run the same request twice with a fixed seed and compare results.
- Restrict the search to basic currencies.
- Disable Divines and confirm no Divine step appears.
- Enter a budget below every valid completion and receive useful incomplete
  alternatives.
- Import an item with unsupported metadata and confirm the warning cannot be
  mistaken for full support.

## 10. Product and Technical Risks

### Budget semantics

Expected cost is not the same as guaranteed wallet consumption. The first
release must label its chaos-equivalent cap as an expected-cost policy. Exact
inventory exhaustion belongs to a later probability model.

### Sampled probabilities

Several methods use 50 Monte Carlo outcomes per expansion. UI polish must not
make those values appear more exact than the engine supports.

### Search responsiveness

Rayon search work must remain off the Tauri event thread. Cancellation checks
must be frequent enough for a responsive UI without destabilizing deterministic
ordering.

### Modifier presentation

RePoE identifiers are correct but not user-friendly. Display-text rendering,
tier grouping, and search normalization are product-critical, not optional
cosmetics.

### Unsupported mechanics

A polished interface can create false confidence. Unsupported crafting systems
and unsafe imported metadata must remain visible at selection time and in every
affected result.

### Patch-dependent data

The application must display which RePoE version produced a result. Updating
data must be atomic and must not silently imply that newly introduced mechanics
are supported.

## 11. Definition of Done

The desktop UI project is complete when:

- A user can configure and run the full planned workflow without editing TOML
  or using a terminal.
- The CLI and desktop app use the same application service.
- The UI shows only engine-validated bases, modifiers, and crafting methods.
- Required goals, weighted preferences, allowlists, and hard
  chaos-equivalent budgets affect the actual search.
- At least three distinct paths can be compared when available.
- Results clearly distinguish exact and sampled probabilities.
- Search is cancellable and the interface stays responsive.
- Saved requests are versioned and reproducible with a fixed seed.
- Missing data and unsupported mechanics fail clearly and safely.
- Rust and frontend release gates pass.
- A release build installs and runs on a clean supported Windows system.

## 12. Work Not to Mix into the UI Project

The following should remain separate work tracks unless a selected UI workflow
strictly requires them:

- Metamods.
- Veiled currency and unveil choices.
- Awakener's Orb transfer.
- Imprints and locks.
- Recombinators.
- Orb of Conflict.
- Catalysts and quality-based modifier scaling.
- Rog crafting.
- Live market-price ingestion.
- Full recovery-policy optimization after one-shot failures.

Each of these changes the engine's correctness boundary and needs its own
mechanics, probability, search, test, and documentation plan.
