# PoE1 HTC

**A Path of Exile 1 crafting-path optimizer written in Rust.**

PoE1 HTC takes a base item, a set of desired modifiers, league prices, and the
crafting methods you are willing to use. It explores possible craft sequences
and reports the strongest routes it found, their estimated cost, and the chance
of one-shot steps landing.

The practical question is:

> Given this item and this target, what should I do next, and what is that plan
> likely to cost?

This is an early guild beta, not a replacement for Craft of Exile. The core CLI
works end to end and the implemented mechanics are tested, but several advanced
crafting systems remain unsupported. Read [Known Limitations](#known-limitations)
before spending expensive currency.

## What Works

- RePoE modifier and base-item loading
- Optional RePoE bench, essence, and fossil catalogs
- Normal, Magic, Rare, fractured, crafted, influenced, and Eldritch item state
- Modifier filtering by item level, base tags, ordered spawn/generation weights,
  affix capacity, and RePoE `groups`
- Parallel, seeded beam search with semantic state deduplication
- TOML goals targeting exact mod IDs, mod groups, stats, and minimum rolls
- Existing-item input for evaluating or finishing a craft in progress
- League price overrides and multiple reported pathways

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

[search]
beam_width = 40
max_steps = 10
cost_weight = 0.02
restart_cost = 1.0
seed = 42
top = 3
```

Start from an item you already own:

```toml
[item]
base = "Astral Plate"
item_level = 86
rarity = "rare"
influences = ["hunter"]

[[item.mods]]
mod_id = "IncreasedLife9"
values = [95]
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

See [`goals/example_life_chest.toml`](goals/example_life_chest.toml) and
[`goals/finish_fractured_chest.toml`](goals/finish_fractured_chest.toml) for
complete examples.

## Reading Results

The CLI reports:

- Whether the target is complete
- Raw goal score and search ranking score
- The chosen method sequence
- Per-step cost and hit chance
- Estimated total cost
- Combined one-shot path probability
- Final modifiers and satisfied wants

Repeatable rerolls are priced as `craft cost / observed hit chance`. One-shot
actions are paid once and retain their miss probability. The CLI also reports a
pessimistic restart estimate for paths where a one-shot miss would force the
whole plan to restart, including the configured replacement or reset cost.

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
--cost-weight <N>   Penalty per expected chaos in ranking
--restart-cost <N>  Cost to restore or replace the base after a failed path
--seed <N>          Reproducible random sampling
--top <N>           Number of distinct pathways to print
--base-item <NAME>  Override the goal's base
--data-dir <PATH>   RePoE data directory
```

`cost_weight` is relative to the total weight of your wants. A value that is
too low favors expensive high-score routes; a value that is too high favors
cheap routes that barely improve the item.

## Known Limitations

- Full rerolls use only 50 samples, so rare outcomes can be missed entirely.
- Beam search is heuristic. A wider beam improves coverage but does not prove
  global optimality.
- Recovery after a failed one-shot craft is not modeled as a full policy. The
  real cost usually lies between the printed optimistic and restart estimates.
- Multi-step probabilities group sibling outcomes by goal score; equal-scoring
  items do not necessarily support the same continuation.
- Numeric stat-roll probabilities are sampled for several otherwise exact
  actions.
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

Every action implements:

```rust
pub trait CraftingMethod: Send + Sync {
    fn name(&self) -> &str;
    fn cost_chaos(&self) -> f64;
    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool;
    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>>;
}
```

`GameData` is immutable after loading and shared by reference. Search nodes
clone only `ItemState`. Seeded runs are deterministic because the craftable
modifier index and outcome ordering are stable.

## Development

```bash
cargo fmt -- --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
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
