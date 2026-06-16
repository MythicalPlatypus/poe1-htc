# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands
- `cargo build` — compile
- `cargo test` — run all tests
- `cargo test <name>` — run a single test by name (substring match)
- `cargo clippy` — lint (must pass before any module is considered done)
- `cargo fmt` — format

## Architecture

This is a Path of Exile 1 crafting-path optimizer. Given a base item and a goal, it finds the cheapest sequence of currency orbs to achieve that goal using Beam Search.

**Data flow:** `data/loader.rs` reads RePoE JSON → `GameData` struct (flat `HashMap<String, Mod>` + `HashMap<String, BaseItem>`) → passed by reference to search and currency modules.

**Module layout:**
- `src/data/` — pure loading only. `loader::load_all(data_dir)` returns `GameData`. Never write files here. `mods.rs` defines `Mod`, `SpawnWeight`, `GenerationWeight`, `Domain`, `GenerationType`.
- `src/item/` — `ItemState` (the cloneable per-node state in beam search) and `Modifier`/`StatRoll`. `ItemState` tracks rarity, prefixes/suffixes (Vec<Modifier>), fractured mods, crafted bench mod, corrupted/mirrored flags, eldritch implicits (exarch/eater), item_level.
- `src/engine/mod_pool.rs` — `eligible_mods`, `weighted_pick`, `random_rolls_pub`, `roll_mods`. Also fossil-aware (`eligible_mods_fossil`), eldritch (`eligible_mods_eldritch`), and harvest-tag (`eligible_mods_harvest_tag`) variants.
- `src/currency/` — `CraftingMethod` trait + one file per currency type. The trait: `name() -> &str`, `cost_chaos() -> f64`, `can_apply(&ItemState, &GameData) -> bool`, `apply(&ItemState, &GameData) -> Result<Vec<(ItemState, f64)>>`. Each entry in the returned Vec is a (successor state, probability) pair; probabilities sum to 1.0.
- `src/search/beam.rs` — `BeamSearch` runs parallel expansion via rayon, scoring via a user-supplied `Fn(&ItemState) -> f64`, keeping `beam_width` best nodes per step.
- `src/cli/mod.rs` — clap `Args` (data_dir, base_item, beam_width, max_steps) + `run()` which loads data and invokes search.

## Critical Rules
- Mod conflicts are by `groups` vec (mod_group ID), not display name — NEVER compare display strings
- Always filter eligible mod pool by `required_level <= item_level` before computing probabilities
- `data/` layer is pure loading — engine/currency modules receive `&GameData`, never file paths
- Exact probability math preferred; Monte Carlo only as fallback for complex Harvest chains
- No `unwrap()` in `engine/` or `currency/` — use `Result<>` and `?`
- Derive `Debug` on all structs

## RePoE mods.json Shape
Top-level: `{ "ModId": { ...Mod fields... }, ... }`. Key fields: `name`, `generation_type`, `required_level`, `stats[]`, `spawn_weights[]` (ordered — first matching tag wins, fallback to `"default"`), `generation_weights[]`, `adds_tags[]`, `groups[]`, `domain`, `is_essence_only`. Use `Mod::is_craftable()` to filter to normal-craftable mods and `Mod::spawn_weight_for_tags()` for weighted rolling.

## Current Status
- Working: data loading, `ItemState` (with `item_level`, eldritch implicit slots), `CraftingMethod` trait, `BeamSearch`, CLI args
- Working currency: `OrbOfScouring`, `OrbOfAnnulment`, `OrbOfAlchemy`, `ChaosOrb`, `ExaltedOrb`, `ApplyInfluence`, `Essence`, `FossilCraft`, `HarvestCraft`, `EldritchChaosOrb`, `EldritchExaltedOrb`
- Working engine: `src/engine/mod_pool.rs` — `eligible_mods`, `weighted_pick`, `roll_mods`, fossil/eldritch/harvest variants, unit tests
- Missing: CLI goal spec (TOML), scoring function wired into `cli::run()`, integration test suite, real RePoE data files in `data/`