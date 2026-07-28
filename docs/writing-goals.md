# Writing Goal Files

A goal file is a small TOML document that tells the optimizer three things:
what item you start with, what mods you want on it, and which crafting
methods and prices to plan with. This page is both a tutorial and the
complete reference.

Run any goal with:

```bash
cargo run --release -- --goal my_goal.toml
```

Unknown or misspelled fields are rejected at load time with an error, so
typos fail fast instead of silently changing the search.

## A minimal goal

```toml
[item]
base = "Astral Plate"
item_level = 86

[[wants]]
group = "IncreasedLife"
weight = 10.0

[search]
cost_weight = 0.02
```

That is enough: start from a fresh item-level-86 Astral Plate, value any
tier of the flat-life prefix at 10 points, and penalize restart-adjusted
expected spending at 0.02 points per chaos.

## `[item]` — the starting item

| Field | Required | Meaning |
|---|---|---|
| `base` | yes | Base item display name (`"Astral Plate"`) or RePoE metadata ID (`"Metadata/Items/Armours/BodyArmours/BodyStr15"`) |
| `item_level` | no (default 84) | Gates which mods can roll: a mod is only available when its `required_level <= item_level` |
| `rarity` | when `[[item.mods]]` present | `"normal"` (default), `"magic"`, or `"rare"` |
| `influences` | no | Up to two of `shaper`, `elder`, `crusader`, `hunter`, `redeemer`, `warlord` |
| `exarch_implicit` | no | Existing Searing Exarch implicit (RePoE mod ID), for mid-craft starts |
| `eater_implicit` | no | Existing Eater of Worlds implicit (RePoE mod ID) |

`item_level` must be between 1 and 100. A starting item cannot combine a
fractured modifier with an influence, or combine Shaper/Elder/Conqueror
influence with Eldritch implicits.

### `[[item.mods]]` — mods already on the item

Describe a mid-craft item by listing its current mods:

```toml
[item]
base = "Astral Plate"
item_level = 86
rarity = "rare"

[[item.mods]]
mod_id = "IncreasedLife12"   # RePoE mod ID
values = [180]               # one value per stat, in the mod's stat order
fractured = true             # locked: survives rerolls, blocks its group

[[item.mods]]
mod_id = "Strength4"         # values omitted = midpoint rolls
```

| Field | Meaning |
|---|---|
| `mod_id` | RePoE mod ID; must exist in `mods.json` |
| `values` | Rolled value per stat. Omitted = midpoint of each range. Out-of-range values are rejected |
| `fractured` | The mod is fractured (locked against removal) |
| `crafted` | The mod occupies the single bench-craft slot (must be a `domain = "crafted"` mod) |

Every declared mod is validated against the database: affix type, item
level, roll ranges, prefix/suffix capacity for the declared rarity, and
group conflicts. An impossible starting item is an error, not a warning.

> **Tip:** if the item exists in your stash, skip `[[item.mods]]` entirely
> and use [`--item-file`](importing-your-item.md) instead — the game's own
> item text is harder to get wrong.

## `[[wants]]` — what you are crafting toward

Each `[[wants]]` entry is one desired outcome. At least one entry is required.
Presence and threshold criteria must hold on the same mod; `per_unit` scoring
instead sums the selected stat across every matching mod:

```toml
# Any tier of the flat-life prefix group.
[[wants]]
group = "IncreasedLife"
weight = 10.0

# A T1 body-armour life roll in RePoE 3.28.0.16.
[[wants]]
group = "IncreasedLife"
stat = "base_maximum_life"
min_value = 175
weight = 5.0

# One exact tier, by mod ID.
[[wants]]
mod_id = "IncreasedLife12"
weight = 2.0
required = false           # nice-to-have, not required for COMPLETE

# Value-scaled preference: 0.05 points per Life, up to 300 Life.
[[wants]]
stat = "base_maximum_life"
mode = "per_unit"
cap = 300
weight = 0.05
required = false

# Lower is better. At -10 or lower the threshold is satisfied; score is
# weight × max(0, cap - attained).
[[wants]]
stat = "local_attribute_requirements_+%"
mode = "per_unit"
max_value = -10
cap = 0
weight = 1.0
required = false
```

| Field | Meaning |
|---|---|
| `mod_id` | Exact RePoE mod ID (a specific tier) |
| `group` | RePoE conflict group — matches any tier in the group |
| `stat` | RePoE stat ID (e.g. `base_maximum_life`) |
| `mode` | `presence`, `threshold`, or `per_unit`. Omitted mode preserves legacy inference: a bound means `threshold`; otherwise `presence` |
| `min_value` | Higher-is-better satisfaction threshold |
| `max_value` | Lower-is-better satisfaction threshold; mutually exclusive with `min_value` |
| `cap` | Optional higher-is-better `per_unit` score cap; mandatory for lower-is-better `per_unit` |
| `weight` | Presence/threshold points when satisfied, or points per attained unit for `per_unit`. Default 1.0, must be > 0 |
| `required` | Whether this want is mandatory for `COMPLETE`. Default `true` |

Use `required = true` for outcomes the final item must have and
`required = false` for preferences. Presence/threshold wants contribute their
full weight when satisfied. Higher-is-better `per_unit` contributes
`weight × max(0, min(attained, cap))`; without `cap`, it uses
`weight × max(0, attained)` and has no known maximum. Lower-is-better
`per_unit` contributes `weight × max(0, cap - attained)`. Only required wants
determine completion. Complete targets sort ahead of incomplete states even
when an incomplete state has a larger preferred score. Within the same
completion class, the search maximizes total score minus restart-adjusted cost
(see `cost_weight` below).

`presence` rejects bounds and `cap`. `threshold` requires exactly one bound
and rejects `cap`. A required `per_unit` want also requires exactly one bound;
a preferred higher-is-better `per_unit` want may omit one. Numeric selectors
may use `stat`, `mod_id`, or `group`; without an explicit `stat`, the first
RePoE-declared stat is used and a multi-stat match emits a warning.

An all-preferred goal set is valid: it is complete by definition, but the
search still explores until it finds the best preferred score or reaches its
limits. Existing files that omit `required` remain all-required.

Wants are satisfied by any mod on the final item — prefixes, suffixes,
fractured mods, crafted mods, Eldritch implicits, and (on imported items)
generic implicits and enchantments all count.

### Finding mod IDs, groups, and stat names

Three practical options:

1. **Import your item.** Run with `--item-file` and read the IDs the report
   prints, e.g. `Prime [IncreasedLife12] base_maximum_life = 180`. Fastest
   way to learn the vocabulary for mods you already own.
2. **Search the data files.** `data/mods.json` is plain JSON: search for the
   in-game text ("to maximum Life") and read the entry's key (mod ID),
   `groups`, and `stats[].id` fields.
3. **Community databases.** [poedb.tw](https://poedb.tw) lists the same mod
   and group identifiers RePoE exports.

## `[[methods]]` — extra crafting methods

The search always has the default orb set: Scouring, Transmutation,
Alteration, Augmentation, Regal, Alchemy, Chaos, Exalted, Annulment,
Divine, Fracturing, and Remove Crafted Mods. `[[methods]]` entries add
configured crafts on top; they cannot disable or restrict the default set.
Every entry is selected by `type`:

### `essence` — guarantee one mod, reroll the rest

```toml
[[methods]]
type = "essence"
essence = "Deafening Essence of Greed"   # display name or metadata ID
cost = 5.0
```

With `essences.json` present, the essence's guaranteed mod is resolved for
your item class and lower-tier random-mod caps are enforced.

For manual configuration, replace `essence` with one exact RePoE modifier ID:

```toml
[[methods]]
type = "essence"
mod_id = "IncreasedLife11"
cost = 5.0
name = "Manual Essence"
```

Specify exactly one of `essence` or `mod_id`. Manual configuration does not
infer catalog restrictions.

### `bench` — deterministically add a crafted mod

```toml
[[methods]]
type = "bench"
mod_id = "EinharMasterIncreasedLife5_"   # any mods.json entry with domain = "crafted"
cost = 3.0                               # default 2.0
```

With `crafting_bench_options.json` present, invalid bench crafts for your
item class are rejected before the search starts.

### `fossil` — weighted reroll, single or multi-fossil

```toml
[[methods]]
type = "fossil"
fossil = "Pristine Fossil"
cost = 10.0
```

Multi-fossil resonator — add `[[methods.fossils]]` sub-tables:

```toml
[[methods]]
type = "fossil"
fossil = "Pristine Fossil"
cost = 25.0

[[methods.fossils]]
fossil = "Dense Fossil"
```

Without the fossil catalog you can configure weights manually with
`boosted_tags`, `reduced_tags`, `blocked_mod_ids`, and `forced_mod_ids`.
Each resonator supports at most four fossil parts. A part must use either a
named `fossil` or manual tag/mod fields, never both.

### `harvest` — reforge or augment by tag

```toml
[[methods]]
type = "harvest"
op = "reforge"      # "reforge" | "augment"
target = "life"
cost = 30.0
```

Valid targets: `attack`, `caster`, `speed`, `life`, `defence`,
`resistance`, `chaos`, `fire`, `cold`, `lightning`, `physical`,
`critical`, `minion`, `mana`.

Reforge rerolls a Rare item guaranteeing at least one mod with the target
tag. Augment requires a non-influenced Rare with at least one removable
explicit or crafted modifier; it removes one modifier first, then adds a
target-tag modifier from the pool that remains.

### `eldritch_chaos` / `eldritch_exalt` / `eldritch_annul`

```toml
[[methods]]
type = "eldritch_exalt"
god = "exarch"      # "exarch" | "eater" — the currently dominant side
```

Eldritch currency requires a dominant Eldritch implicit on the item, so
these only fire on items with `exarch_implicit` / `eater_implicit` set (or
imported items that have one).

### `conqueror_exalt` — influenced slam

```toml
[[methods]]
type = "conqueror_exalt"
influence = "hunter"   # "crusader" | "hunter" | "redeemer" | "warlord"
```

Adds one conqueror-exclusive affix and applies that influence.

### `bestiary_swap` — beastcraft affix swap

```toml
[[methods]]
type = "bestiary_swap"
add = "prefix"       # "prefix": remove a random suffix, add a prefix
beast_level = 83     # caps the level of the added mod
cost = 12.0
```

Essence, bench, and fossil entries accept an optional `name` to control how
the step appears in reports and how `[prices]` keys match. Other method types
use their generated display name.

Configured costs, want weights, and `[prices]` values must be positive finite
numbers. `beast_level` must be between 1 and 100, and every built method must
have a unique display name.

## `[prices]` — your league's prices

Built-in costs are placeholders. Override them with current league prices,
keyed by the exact method display name:

```toml
[prices]
"Divine Orb" = 220.0
"Exalted Orb" = 45.0
"Orb of Annulment" = 25.0
"Fracturing Orb" = 180.0
```

Built-in defaults (chaos):

| Method | Default | Method | Default |
|---|---|---|---|
| Orb of Transmutation | 0.05 | Orb of Annulment | 40 |
| Orb of Augmentation | 0.05 | Exalted Orb | 100 |
| Orb of Alteration | 0.1 | Divine Orb | 150 |
| Orb of Scouring | 1 | Fracturing Orb | 250 |
| Chaos Orb | 1 | Eldritch Chaos Orb | 5 |
| Regal Orb | 1 | Eldritch Orb of Annulment | 10 |
| Orb of Alchemy | 2 | Eldritch Exalted Orb | 20 |
| Remove Crafted Mods | 1 | Conqueror Exalted Orbs | 100 |

A `[prices]` key that matches no method name prints a warning, so typos are
visible. The Eldritch and Conqueror entries above use per-god / per-conqueror
display names, e.g. `"Eldritch Exalted Orb (Exarch)"` and
`"Hunter's Exalted Orb"` — copy the name exactly as a report prints it.

## `[search]` — search parameters

```toml
[search]
beam_width = 40
max_steps = 12
cost_weight = 0.02
restart_cost = 1.0
seed = 42
top = 3
expansion_limit = 100000
timeout_ms = 30000
```

| Field | Default | Meaning |
|---|---|---|
| `beam_width` | 50 | Candidate states kept per depth. Wider = more thorough, slower |
| `max_steps` | 10 | Maximum crafting actions in a plan |
| `cost_weight` | 0.0 | Ranking penalty per restart-adjusted expected chaos spent. **The default ignores cost entirely — always set this** |
| `restart_cost` | 1.0 | Chaos to restore/replace the base if a failed one-shot forces a restart |
| `seed` | random | RNG seed; set it for reproducible runs |
| `top` | 1 | Distinct pathways to report, best first |
| `expansion_limit` | none | Maximum concrete successor states generated |
| `timeout_ms` | none | Wall-clock limit in milliseconds |

Every field can be overridden on the command line (`--beam-width`,
`--max-steps`, `--cost-weight`, `--restart-cost`, `--seed`, `--top`,
`--expansion-limit`, `--timeout-ms`); CLI flag beats goal file beats built-in
default.

`beam_width`, `max_steps`, and `top` must be greater than zero.
`cost_weight` and `restart_cost` must be non-negative finite numbers.

### Choosing `cost_weight`

`cost_weight` is the exchange rate between attainable raw score and money. The
cost term is the restart-on-one-shot-miss estimate, including `restart_cost`,
rather than the optimistic expected cost printed above it. Rule of thumb:
decide how much restart-adjusted chaos one point of score is worth to you and
invert. If the relevant attainable score range is ~25 points and you would pay
about 50 chaos per point, set `0.02`. Uncapped `per_unit` goals may make that
score range unbounded. Too low and the search happily recommends 500-chaos
routes for marginal gains; too high and it stops at cheap, mediocre items.
Completion is still a separate priority: any complete target sorts ahead of
every incomplete state.

The examples in [`goals/`](../goals) are annotated with this reasoning —
copy one and adjust.
