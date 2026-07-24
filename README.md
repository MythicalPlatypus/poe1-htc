# PoE1 HTC

**A Path of Exile 1 crafting-path optimizer written in Rust.**

PoE1 HTC takes a base item, a set of desired modifiers, league prices, and
optional configured crafts. It explores possible craft sequences using its
always-enabled default orb set plus those configured crafts, then reports the
strongest routes it found, their estimated cost, and the chance of one-shot
steps landing.

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

The rest of this README is a condensed overview of the same material plus
contributor notes.

## What Works

- RePoE modifier and base-item loading
- Optional RePoE bench, essence, and fossil catalogs
- Normal, Magic, Rare, fractured, crafted, influenced, and Eldritch item state
- Modifier filtering by item level, base tags, ordered spawn/generation weights,
  affix capacity, and RePoE `groups`
- Parallel, seeded beam search with semantic state deduplication
- TOML goals targeting exact mod IDs, mod groups, stats, and minimum rolls
- Existing-item input for evaluating or finishing a craft in progress
- Clipboard item-text import (`--item-file`) to start from an item you own
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

See [`goals/example_life_chest.toml`](goals/example_life_chest.toml) and
[`goals/finish_fractured_chest.toml`](goals/finish_fractured_chest.toml) for
complete examples.

The default orb set is always enabled. `[[methods]]` adds configured crafts; it
does not form an allowlist or disable default methods.

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
--cost-weight <N>   Penalty per restart-adjusted expected chaos in ranking
--restart-cost <N>  Cost to restore or replace the base after a failed path
--seed <N>          Reproducible random sampling
--top <N>           Number of distinct pathways to print
--base-item <NAME>  Override the goal's base
--item-file <PATH>  Start from pasted item text ("-" for stdin); requires --goal
--data-dir <PATH>   RePoE data directory
```

`cost_weight` is relative to the total weight of your wants. Complete targets
sort ahead of incomplete ones; within the same completion class, ranking
subtracts `cost_weight ×` restart-adjusted expected cost. A value that is too
low favors expensive high-score routes; a value that is too high favors cheap
routes that barely improve the item.

## Known Limitations

- Full rerolls use only 50 samples, so rare outcomes can be missed entirely.
- Eldritch Chaos samples uniformly across legal replacement-affix counts
  because RePoE does not publish the real count distribution.
- Beam search is heuristic. A wider beam improves coverage but does not prove
  global optimality.
- Recovery after a failed one-shot craft is not modeled as a full policy. The
  real cost usually lies between the printed optimistic and restart estimates.
- Multi-step probabilities group sibling outcomes by goal score; equal-scoring
  items do not necessarily support the same continuation.
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

Every action implements this contract; the behavioral classification hooks
have conservative defaults:

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
    fn weights_are_probabilities(&self) -> bool { true }
    fn repeatable_on_failure(&self) -> bool { false }
    fn reroll_kind(&self) -> Option<RerollKind> { None }
    fn reroll_initializer_kind(&self) -> Option<RerollKind> { None }
    fn consumes_reroll_initializer(&self) -> bool { false }
}
```

`apply` returns concrete successor states whose weights sum to 1.0. The
probability hook distinguishes exact enumeration from sampled representatives;
the retry and reroll hooks drive expected-cost pricing and prevent dominated
reroll chains.

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
