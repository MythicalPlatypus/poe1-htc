# PoE1 HTC

**A Path of Exile 1 crafting-path optimizer written in Rust.**

PoE1 HTC takes a base item, a set of desired modifiers, league prices, and
optional configured crafts. It explores possible craft sequences using its
legacy CLI default orb set plus those configured crafts, then reports the
strongest routes it found, their estimated cost, and the chance of one-shot
steps landing. Programmatic service callers can instead supply an explicit
semantic-ID allowlist.

The practical question is:

> Given this item and this target, what should I do next, and what is that plan
> likely to cost?

This is an early guild beta, not a replacement for Craft of Exile. The core CLI
works end to end and the implemented mechanics are tested, but several advanced
crafting systems remain unsupported. Read [Known Limitations](#known-limitations)
before spending expensive currency.

## Documentation

New here? Start with the guides in [`docs/`](docs):

1. [Getting Started](docs/getting-started.md) — install, download data, first run
2. [Writing Goal Files](docs/writing-goals.md) — tutorial and complete TOML reference
3. [Importing Your Item](docs/importing-your-item.md) — start from a `Ctrl+C` item paste
4. [Understanding Results](docs/understanding-results.md) — what every number means before you spend
5. [FAQ & Troubleshooting](docs/faq.md) — common errors, stability, limitations
6. [In-Game Validation](docs/in-game-validation.md) — safe live-item checks for imports and state transitions

The rest of this README is a condensed overview of the same material plus
contributor notes.

## What Works

- RePoE modifier and base-item loading
- Optional RePoE bench, essence, and fossil catalogs
- Normal, Magic, Rare, fractured, crafted, influenced, and Eldritch item state
- Modifier filtering by item level, base tags, ordered spawn/generation weights,
  affix capacity, and RePoE `groups`
- Parallel, seeded beam search with semantic state deduplication
- Presence, threshold, and per-unit TOML goals targeting exact mod IDs, mod
  groups, stats, minimum/maximum rolls, and capped value-scaled preferences
- Required/preferred completion semantics plus conservative pre-search
  diagnostics for provably impossible required combinations
- Existing-item input for evaluating or finishing a craft in progress
- Clipboard item-text import (`--item-file`) to start from an item you own
- League price overrides and multiple reported pathways
- A reusable, structured application service shared with the CLI
- Stable semantic crafting-method IDs, structured registry metadata, ID-keyed
  prices, and explicit service-level family/method selection
- Immutable data provenance with an exact RePoE bundle fingerprint on every
  prepared job and result
- Service-level hard budgets over first-try, retry-expected, or
  restart-adjusted expected cost, with explicit finite/unbounded/unavailable
  values and labeled complete over-budget exemplars
- Generation-committed progress, cancellation, timeout/expansion limits,
  typed termination reasons, and one resolved replayable seed per run
- Strict version-1 JSON request/response DTOs, checked request schema, and
  complete/incomplete/sampled/impossible/over-budget compatibility fixtures
- Read-only application-service catalogs for normalized base search and
  engine-equivalent normal affixes on a clean base at a chosen item level

Implemented crafting actions:

- Scouring, Transmutation, Alteration, Augmentation, Regal
- Alchemy, Chaos, Exalted, Annulment, Divine, and Fracturing Orbs
- Remove Crafted Mods and configured bench crafts
- Named or manually configured Essences
- Named or manually configured Fossils, including multi-fossil resonators
- Harvest Reforge and Augment
- Eldritch Chaos, Exalted, and Annulment Orbs
- Crusader, Hunter, Redeemer, and Warlord Exalted Orbs
- Bestiary add-prefix/remove-suffix and add-suffix/remove-prefix crafts

## Data

The JSON exports are large and patch-dependent, so they are intentionally not
committed. This branch has been tested against RePoE Fork `3.28.0.16`. Refresh
the files when a new league export becomes available.

Required:

```text
data/mods.json
data/base_items.json
```

Strongly recommended:

```text
data/crafting_bench_options.json
data/essences.json
data/fossils.json
```

Download them from the maintained [RePoE Fork](https://repoe-fork.github.io/):

```bash
curl -o data/mods.json https://repoe-fork.github.io/mods.json
curl -o data/base_items.json https://repoe-fork.github.io/base_items.json
curl -o data/crafting_bench_options.json https://repoe-fork.github.io/crafting_bench_options.json
curl -o data/essences.json https://repoe-fork.github.io/essences.json
curl -o data/fossils.json https://repoe-fork.github.io/fossils.json
```

The optional catalogs let the CLI resolve names such as `Pristine Fossil`,
select the correct Essence mod for the chosen item class, enforce lower-tier
Essence random-modifier caps, and reject invalid bench crafts before search.

At load time the optimizer computes a versioned SHA-256 fingerprint over the
exact bytes and presence of all five JSON inputs. Paths, timestamps, search
settings, and the descriptive RePoE version are excluded. This lets saved
results identify the data snapshot that actually produced them rather than
assuming that the README's tested version is installed.

The RePoE JSON files do not identify their own release. A trusted updater or
manual installation may put the version in `data/repoe-version.txt`; otherwise
the CLI honestly reports the version as `unknown`. Whitespace around the
sidecar value is ignored, and an empty or malformed value is rejected. The
sidecar never changes the data fingerprint.

## Install

Requires a stable Rust toolchain.

```bash
git clone https://github.com/MythicalPlatypus/poe1-htc.git
cd poe1-htc
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
```

Release mode is strongly recommended. Search repeatedly scans and samples a
large modifier pool.

## Quick Start

Run the included life-chest example:

```bash
cargo run --release -- --goal goals/example_life_chest.toml --seed 42
```

Ask for several distinct routes:

```bash
cargo run --release -- --goal goals/finish_fractured_chest.toml --top 3
```

Without `--goal`, the CLI validates and summarizes the loaded data.

## Goal Files

A minimal goal:

```toml
[item]
base = "Astral Plate"       # display name or RePoE metadata ID
item_level = 86

[[wants]]
group = "IncreasedLife"     # any mod in this RePoE conflict group
weight = 10.0

[[wants]]
stat = "base_maximum_life"
min_value = 90
weight = 5.0
required = false           # preference: adds score but does not block COMPLETE

[search]
beam_width = 40
max_steps = 10
cost_weight = 0.02
restart_cost = 1.0
seed = 42
top = 3
```

Every want is required by default, preserving existing goal files. Set
`required = false` for a preference: it still contributes score and guides
search, but only required wants determine whether a result is `COMPLETE`.
Presence and threshold goals contribute their weight when satisfied.
`per_unit` goals contribute `weight ×` their non-negative attained units,
optionally limited by `cap`. A completed result always ranks ahead of an
incomplete one, even when the incomplete item satisfies higher-scoring
preferences.

For value-scaled scoring:

```toml
[[wants]]
stat = "base_maximum_energy_shield"
mode = "per_unit"
min_value = 100             # required satisfaction threshold
cap = 300                   # score no more than 300 attained units
weight = 0.05
required = true

[[wants]]
stat = "local_attribute_requirements_+%"
mode = "per_unit"
max_value = -10             # lower is better
cap = 0                     # required for lower-is-better per-unit scoring
weight = 1.0
required = false
```

A required `per_unit` goal must declare exactly one threshold bound. A
higher-is-better preferred goal may omit both threshold and cap, but then its
maximum possible score is reported as unknown.

Start from an item you already own:

```toml
[item]
base = "Astral Plate"
item_level = 86
rarity = "rare"

[[item.mods]]
mod_id = "IncreasedLife12"
values = [180]
fractured = true
```

Configure catalog-backed crafts:

```toml
[[methods]]
type = "essence"
essence = "Deafening Essence of Greed"
cost = 5.0

[[methods]]
type = "fossil"
fossil = "Pristine Fossil"
cost = 10.0

[[methods]]
type = "bench"
mod_id = "EinharMasterIncreasedLife5_"
cost = 3.0

[[methods]]
type = "harvest"
op = "augment"              # reforge | augment
target = "life"
cost = 30.0

[[methods]]
type = "bestiary_swap"
add = "prefix"              # prefix | suffix
beast_level = 83
cost = 12.0
```

For a multi-fossil resonator:

```toml
[[methods]]
type = "fossil"
fossil = "Pristine Fossil"
cost = 25.0

[[methods.fossils]]
fossil = "Dense Fossil"
```

Override built-in prices by exact method display name:

```toml
[prices]
"Divine Orb" = 220.0
"Exalted Orb" = 45.0
```

This display-name table is the legacy TOML format. The CLI resolves it to
semantic `MethodId` values before invoking the application service.

See [`goals/example_life_chest.toml`](goals/example_life_chest.toml) and
[`goals/finish_fractured_chest.toml`](goals/finish_fractured_chest.toml) for
complete examples.

Legacy TOML/CLI runs retain the default orb set and treat `[[methods]]` as
additions. The application service also supports exact `MethodId` allowlists
and explicit selections that combine family-wide switches with per-method
additions or exclusions. Filtering never reorders methods: built-ins retain
registry order, followed by configured-method request order.

## Importing an Item

Instead of describing your starting item in `[[item.mods]]`, copy it in game
(hover the item and press `Ctrl+C`), save the text to a file, and pass it with
`--item-file`:

```bash
cargo run --release -- --goal goals/example_life_chest.toml --item-file my_item.txt
```

Use `-` to read the item text from stdin. A goal file is **still required**:
the import only replaces the goal's starting item; the `[[wants]]`,
`[[methods]]`, `[prices]`, and `[search]` sections continue to drive the
search. `--item-file` cannot be combined with `--base-item`.

### Imported metadata versus modeled crafting state

The importer preserves the supported fields listed below and warns when it
recognizes metadata that the optimizer does not carry into crafting state:

- **Modeled state** — rarity, item level, prefixes, suffixes, fractured and
  crafted mods, corrupted/mirrored status, and Eldritch implicits. These drive
  eligibility, capacity, group conflicts, and probabilities exactly as they do
  for TOML-described items.
- **Preserved metadata** — generic (non-Eldritch) implicits and enchantments
  are carried through every crafting action unchanged, never consume explicit
  affix slots or join explicit group conflicts, and *can* satisfy `[[wants]]`.
- **Descriptive metadata** — quality, the socket description, and the
  displayed total Energy Shield are kept for reporting only. Socket crafting
  and derived total defences are **not** modeled: no crafting probability or
  goal calculation reads them, and the displayed total is not recomputed as
  explicit mods change.

Clipboard influence, Synthesised, and Split status lines are currently reported
as `unsupported_metadata` and are not preserved in `ItemState`. Do not optimize
an influenced or Synthesised item from `--item-file`: describe influence with
`[item].influences` and `[[item.mods]]` instead; Synthesised starting items are
not modeled faithfully yet. Review every import warning before trusting a plan.

Existing mods on an imported item are validated strictly (affix type, item
level, roll ranges, capacity, group conflicts, one crafted mod, fractured
versus crafted), but they are **not** required to be currently rollable:
fractured, recombinated, Delve, unveiled, or legacy mods are accepted as
existing state even when their spawn weight on the base is zero. Random roll
pools are unaffected — such mods still never appear in new rolls.

## Reading Results

The CLI reports:

- Whether every required want is complete, plus required and total counts
- Raw goal score and its known maximum, when bounded
- The chosen method sequence
- Per-step cost and hit chance
- First-try, retry-expected, and restart-adjusted expected cost
- Selected-budget comparison/excess and each path's
  `complete`/`incomplete`/`over_budget` status
- Combined one-shot path probability
- Final modifiers and satisfied wants
- Search termination reason, committed progress counters, elapsed time, and
  the resolved replay seed

Repeatable rerolls are priced as `craft cost / observed hit chance`. One-shot
actions are paid once and retain their miss probability. The CLI also reports a
pessimistic restart estimate for paths where a one-shot miss would force the
whole plan to restart, including the configured replacement or reset cost.
Programmatic service callers may bind a hard chaos-equivalent cap to any of
the three cost models. This constrains a modeled expectation, not guaranteed
wallet consumption. Legacy TOML/CLI requests remain unbounded until a UI or
machine-readable adapter exposes budget input.

Full rerolls use 50 Monte Carlo samples per expansion. Methods that enumerate
modifier identities exactly but sample numeric rolls are also marked as
estimates. Set a seed to reproduce a run; change the seed or increase the beam
width to test whether a recommendation is stable.

Per-step hit chances mean "this roll scores at least as highly as the shown
outcome." Equal-scoring outcomes can have different useful follow-ups, so a
multi-step path probability is a heuristic policy estimate, not a proof that
every counted branch can execute the exact printed continuation.

## Search Controls

Command-line flags override the goal file:

```text
--beam-width <N>    More candidates retained per depth; slower and broader
--max-steps <N>     Maximum actions in a path
--cost-weight <N>   Penalty per restart-adjusted expected chaos in ranking
--restart-cost <N>  Cost to restore or replace the base after a failed path
--seed <N>          Reproducible random sampling
--top <N>           Number of distinct pathways to print
--expansion-limit <N> Maximum generated successor states
--timeout-ms <N>    Wall-clock search limit in milliseconds
--base-item <NAME>  Override the goal's base
--item-file <PATH>  Start from pasted item text ("-" for stdin); requires --goal
--data-dir <PATH>   RePoE data directory
```

`cost_weight` is relative to the attainable raw-score scale, which may be
unbounded for uncapped `per_unit` goals. Complete targets sort ahead of
incomplete ones; within the same completion class, ranking subtracts
`cost_weight ×` restart-adjusted expected cost. A value that is too low favors
expensive high-score routes; a value that is too high favors cheap routes that
barely improve the item.

The CLI exposes timeout and expansion limits. Cooperative cancellation and
streaming progress observers are currently application-service APIs for a
future desktop adapter.

## Known Limitations

- Full rerolls use only 50 samples, so rare outcomes can be missed entirely.
- Eldritch Chaos samples uniformly across legal replacement-affix counts
  because RePoE does not publish the real count distribution.
- Beam search is heuristic. A wider beam improves coverage but does not prove
  global optimality.
- Recovery after a failed one-shot craft is not modeled as a full policy. The
  real cost usually lies between the printed optimistic and restart estimates.
- Multi-step probabilities group sibling outcomes by required completion and
  goal score; equal-ranked items do not necessarily support the same
  continuation.
- Numeric stat-roll probabilities are sampled for several otherwise exact
  actions.
- Imported quality, sockets, and displayed total Energy Shield are descriptive
  only. Socket crafting, catalysts/quality effects, and derived total defences
  are not part of the crafting or goal math.
- Clipboard influence, Synthesised, and Split statuses are recognized but not
  carried into crafting state. Influenced and Synthesised clipboard starts can
  therefore produce invalid recommendations and must not be optimized as-is.
- Metamods, Veiled currency, Awakener's Orb transfer, imprints, recombinators,
  Orb of Conflict, locks, catalysts, Rog, and several league-specific systems
  are not implemented.
- Market prices are user-supplied snapshots; the application does not fetch
  live trade prices.
- New-league mechanics require fresh RePoE data and code when their behavior is
  not expressible by existing catalogs.

## Architecture

```text
src/
  app/        Reusable application services shared by CLI and future UI adapters
  cli/        Argument parsing, validation, and result reporting
  currency/   CraftingMethod implementations
  data/       RePoE schemas and pure JSON loading
  engine/     Eligible pools, weighting, and modifier rolling
  goal/       TOML schema, starting items, validation, and scoring
  item/       ItemState and rolled modifiers
  search/     Parallel beam search and expected-cost ranking
goals/        Example goal files
tests/        Synthetic end-to-end integration tests
```

`OptimizerService` is the reusable boundary between adapters and the crafting
engine. It accepts owned, file-independent requests, validates and prepares a
job, then runs search and returns evaluated results without printing. The CLI
now only reads files/stdin, resolves flag precedence, and renders those
structured results. These Stage A request types are intentionally internal
until the versioned desktop request and result schemas are finalized.

Each service also owns one immutable pairing of `GameData` and
`DataProvenance`. Preparation summaries and optimization responses carry the
optional RePoE version and stable bundle fingerprint. Prepared work remains
tied to the exact service dataset allocation; matching fingerprints are useful
for replay diagnostics but never substitute for that runtime safety check.

Every crafting method has a stable semantic `MethodId` independent of its
display name and price. Search identity, returned path steps, service-level
allowlists, and `PriceBook` overrides all use that ID. The CLI preserves its
legacy TOML behavior by resolving display-name `[prices]` entries before
calling the service. A returned path step carries both the semantic ID and the
human-readable name; path signatures and repeat suppression use the ID.

The application registry also exposes each method's family, description,
intrinsic price, setup/catalog requirement, coarse item-class support, and
probability model. Probability metadata distinguishes exact enumeration,
50-sample Monte Carlo, exact identity weights with sampled numeric rolls,
additional mechanic approximations, and unavailable compatibility operations.
Exact catalog entries and item-class lists remain part of the planned catalog
APIs rather than this effective-instance registry.

Every action implements this contract; the behavioral classification hooks
have conservative defaults:

```rust
pub trait CraftingMethod: Send + Sync {
    fn id(&self) -> MethodId;
    fn family(&self) -> MethodFamily;
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn cost_chaos(&self) -> f64;
    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool;
    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>>;
    fn weights_are_probabilities(&self) -> bool { true }
    fn probability_model(&self) -> ProbabilityModel;
    fn repeatable_on_failure(&self) -> bool { false }
    fn reroll_kind(&self) -> Option<RerollKind> { None }
    fn reroll_initializer_kind(&self) -> Option<RerollKind> { None }
    fn consumes_reroll_initializer(&self) -> bool { false }
}
```

`apply` returns concrete successor states whose weights sum to 1.0. The
probability hooks distinguish exact enumeration, sampled representatives, and
known mechanic approximations; the retry and reroll hooks drive expected-cost
pricing and prevent dominated reroll chains.

`GameData` is immutable after loading and shared by reference. Search nodes
clone only `ItemState`. Seeded runs are deterministic because the craftable
modifier index and outcome ordering are stable.

## Development

For the verified baseline, next-work candidates, and concurrent-writer
protocol, see [`NEXT_PASS.md`](NEXT_PASS.md).

```bash
cargo fmt -- --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

With local RePoE files installed, exercise the read-only catalog against an
item-level-86 Astral Plate:

```bash
cargo run --release --example catalog_smoke
```

Core rules:

- Modifier conflicts use RePoE `groups`, never display names.
- `required_level <= item_level` is checked before probability calculation.
- Ordered RePoE spawn and generation weights use first-match behavior.
- Fractured and crafted affixes count toward capacity and group conflicts.
- Exact probability math is preferred; sampled behavior must be labeled.
- Engine and currency code return errors instead of panicking.

## Disclaimer

PoE1 HTC is an unofficial fan project. It is not affiliated with or endorsed
by Grinding Gear Games. Path of Exile is a trademark of Grinding Gear Games.
