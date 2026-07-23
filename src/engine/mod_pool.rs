//! Mod pool logic: filtering, weighted selection, and iterative mod rolling.

use std::collections::HashSet;

use anyhow::{bail, Result};
use rand::Rng;

use crate::data::{
    mods::{Domain, GenerationType, Mod, ModStat},
    GameData,
};
use crate::item::{
    modifier::{Modifier, StatRoll},
    state::ItemState,
};

/// Builds the complete tag set visible to mod generation: base-item tags,
/// tags contributed by every affix already on the item, and caller-provided
/// tags accumulated during an in-progress roll.
fn effective_item_tags<'a>(
    item: &'a ItemState,
    extra_tags: &'a [String],
    db: &'a GameData,
) -> Vec<&'a str> {
    item.base_tags
        .iter()
        .map(String::as_str)
        .chain(
            item.all_mods_for_conflict()
                .filter_map(|modifier| db.mods.get(&modifier.mod_id))
                .flat_map(|m| m.adds_tags.iter().map(String::as_str)),
        )
        .chain(extra_tags.iter().map(String::as_str))
        .collect()
}

/// RePoE generation weights use the same ordered, first-matching-tag rule as
/// spawn weights. A missing generation table is neutral (100%).
fn generation_weight_for_tags(m: &Mod, tags: &[&str]) -> u32 {
    m.generation_weights
        .iter()
        .find(|gw| tags.contains(&gw.tag.as_str()))
        .or_else(|| m.generation_weights.iter().find(|gw| gw.tag == "default"))
        .map_or(100, |gw| gw.weight)
}

/// Applies the ordered spawn and generation weight tables without allowing
/// integer multiplication to wrap. RePoE stores both components as `u32`, so
/// the public pool weight is saturated when their scaled product exceeds it.
fn effective_mod_weight(m: &Mod, tags: &[&str]) -> u32 {
    let spawn_weight = m.spawn_weight_for_tags(tags);
    let generation_weight = generation_weight_for_tags(m, tags);
    if spawn_weight == 0 || generation_weight == 0 {
        return 0;
    }

    let scaled = (u64::from(spawn_weight) * u64::from(generation_weight) + 50) / 100;
    scaled.min(u64::from(u32::MAX)) as u32
}

/// Returns all mods eligible to be added to `item`, given the current tags
/// (base_tags + any `adds_tags` accumulated during an in-progress roll session).
///
/// Each entry is `(mod_id, &Mod, spawn_weight)`.
/// Filters applied (all must pass):
///   - `mod.is_craftable()`  — domain=Item, not essence_only, prefix or suffix
///   - `mod.required_level <= item.item_level`
///   - spawn weight > 0 for the item's effective tags
///   - no `groups` overlap with any mod already on the item (exclusivity)
///   - capacity: open prefix slot for prefixes, open suffix slot for suffixes
pub fn eligible_mods<'a>(
    item: &ItemState,
    extra_tags: &[String],
    db: &'a GameData,
) -> Vec<(&'a str, &'a crate::data::mods::Mod, u32)> {
    // Collect all groups from mods already occupying affix slots.
    let existing_groups: HashSet<&str> = item
        .all_mods_for_conflict()
        .filter_map(|m| db.mods.get(&m.mod_id))
        .flat_map(|m| m.groups.iter().map(|g| g.as_str()))
        .collect();

    let effective_tags = effective_item_tags(item, extra_tags, db);

    // craftable_mods() is pre-filtered to is_craftable() and iterates in
    // sorted-ID order, which seeded searches rely on for reproducibility.
    db.craftable_mods()
        .filter_map(|(id, m)| {
            if m.required_level > item.item_level {
                return None;
            }
            let weight = effective_mod_weight(m, &effective_tags);
            if weight == 0 {
                return None;
            }
            if m.groups
                .iter()
                .any(|g| existing_groups.contains(g.as_str()))
            {
                return None;
            }
            match m.generation_type {
                GenerationType::Prefix if !item.has_open_prefix() => return None,
                GenerationType::Suffix if !item.has_open_suffix() => return None,
                _ => {}
            }
            Some((id, m, weight))
        })
        .collect()
}

/// Picks one mod from `pool` using weighted random selection.
/// Returns `None` if `pool` is empty or all entries have zero weight.
pub fn weighted_pick<'a, R: Rng + ?Sized>(
    pool: &[(&'a str, &'a crate::data::mods::Mod, u32)],
    rng: &mut R,
) -> Option<(&'a str, &'a crate::data::mods::Mod)> {
    let total: u64 = pool.iter().map(|(_, _, w)| *w as u64).sum();
    if total == 0 {
        return None;
    }
    let mut roll = rng.random_range(0..total);
    for (id, m, w) in pool {
        if roll < *w as u64 {
            return Some((id, m));
        }
        roll -= *w as u64;
    }
    // Fallback (shouldn't be reached due to integer rounding)
    pool.last().map(|(id, m, _)| (*id, *m))
}

/// Rolls random stat values (uniform in [min, max]) for a slice of `ModStat`.
pub fn random_rolls_pub<R: Rng + ?Sized>(stats: &[ModStat], rng: &mut R) -> Vec<StatRoll> {
    stats
        .iter()
        .map(|s| {
            let value = if s.min == s.max {
                s.min
            } else {
                rng.random_range(s.min..=s.max)
            };
            StatRoll {
                stat_id: s.id.clone(),
                value,
            }
        })
        .collect()
}

/// Groups occupied by all affix-slot mods on `item` (prefixes, suffixes,
/// fractured, crafted) — the starting point for incremental conflict tracking.
pub fn conflict_groups(item: &ItemState, db: &GameData) -> std::collections::HashSet<String> {
    item.all_mods_for_conflict()
        .filter_map(|m| db.mods.get(&m.mod_id))
        .flat_map(|m| m.groups.iter().cloned())
        .collect()
}

/// Precomputed candidate pool for iterative mod rolling — the hot path of every
/// Monte Carlo reroll method (Chaos, Alchemy, Alteration, Essence, Fossil).
///
/// Construction scans the craftable index ONCE, caching each candidate's
/// base-tag spawn weight (with fossil generation-weight multipliers already
/// applied). Each pick then only checks group conflicts and slot capacity —
/// unless `extra_tags` from placed mods' `adds_tags` are in play, in which case
/// weights are recomputed (adds_tags can flip a spawn weight from 0 to nonzero
/// and vice versa because spawn_weights are first-match-wins).
///
/// Pool iteration order follows the sorted craftable index, so seeded searches
/// stay deterministic. Produces pools identical to `eligible_mods` /
/// `eligible_mods_fossil` (verified by unit test).
pub struct RollPool<'a> {
    entries: Vec<PoolEntry<'a>>,
    base_tags: Vec<String>,
    generation_tags: Vec<String>,
}

struct PoolEntry<'a> {
    id: &'a str,
    m: &'a crate::data::mods::Mod,
    /// Spawn x generation weight under base and craft tags (cached fast path).
    cached_weight: u32,
}

impl<'a> RollPool<'a> {
    /// Pool for plain rerolls (no fossil modifiers).
    pub fn new(item: &ItemState, db: &'a GameData) -> Self {
        Self::with_fossils(item, &[], &[], db)
    }

    /// Pool with fossil-specific modifications (blocked IDs excluded, boosted /
    /// reduced tags baked into the multiplier).
    pub fn with_fossils(
        item: &ItemState,
        blocked_mod_ids: &[String],
        fossil_gen_tags: &[&str],
        db: &'a GameData,
    ) -> Self {
        let base_tags: Vec<String> = item.base_tags.clone();
        let generation_tags: Vec<String> = fossil_gen_tags
            .iter()
            .map(|tag| (*tag).to_string())
            .collect();
        let tag_refs: Vec<&str> = base_tags
            .iter()
            .chain(generation_tags.iter())
            .map(String::as_str)
            .collect();
        let entries = db
            .craftable_mods()
            .filter_map(|(id, m)| {
                if m.required_level > item.item_level {
                    return None;
                }
                if blocked_mod_ids.iter().any(|b| b == id) {
                    return None;
                }
                let cached_weight = effective_mod_weight(m, &tag_refs);
                Some(PoolEntry {
                    id,
                    m,
                    cached_weight,
                })
            })
            .collect();
        Self {
            entries,
            base_tags,
            generation_tags,
        }
    }

    /// The currently eligible pool, given the occupied groups and the item's
    /// slot state. `extra_tags` non-empty triggers the slow reweigh path.
    pub fn eligible(
        &self,
        item: &ItemState,
        existing_groups: &std::collections::HashSet<String>,
        extra_tags: &[String],
    ) -> Vec<(&'a str, &'a crate::data::mods::Mod, u32)> {
        let effective: Vec<&str> = self
            .base_tags
            .iter()
            .chain(extra_tags.iter())
            .chain(self.generation_tags.iter())
            .map(String::as_str)
            .collect();
        self.entries
            .iter()
            .filter_map(|e| {
                let weight = if extra_tags.is_empty() {
                    e.cached_weight
                } else {
                    effective_mod_weight(e.m, &effective)
                };
                if weight == 0 {
                    return None;
                }
                if e.m.groups.iter().any(|g| existing_groups.contains(g)) {
                    return None;
                }
                match e.m.generation_type {
                    GenerationType::Prefix if !item.has_open_prefix() => return None,
                    GenerationType::Suffix if !item.has_open_suffix() => return None,
                    _ => {}
                }
                Some((e.id, e.m, weight))
            })
            .collect()
    }
}

/// Rolls up to `count` mods onto `item` iteratively, re-computing the eligible
/// pool after each pick so that `adds_tags` from placed mods are respected.
///
/// Stops early if the eligible pool is exhausted before `count` is reached
/// (partial rolls are valid PoE behaviour).
pub fn roll_mods<R: Rng + ?Sized>(
    item: &mut ItemState,
    count: usize,
    db: &GameData,
    rng: &mut R,
) -> Result<()> {
    let pool = RollPool::new(item, db);
    roll_mods_from_pool(item, count, &pool, db, rng)
}

/// The pick-place loop behind `roll_mods`, reusing a prebuilt `RollPool` so
/// Monte Carlo callers pay the pool-construction scan once per `apply`, not
/// once per pick per sample.
pub fn roll_mods_from_pool<R: Rng + ?Sized>(
    item: &mut ItemState,
    count: usize,
    pool: &RollPool<'_>,
    db: &GameData,
    rng: &mut R,
) -> Result<()> {
    // Seed occupied groups from the item (fractured/crafted/forced mods live
    // outside the pool, so this must go through the db), then track
    // incrementally instead of rescanning the item per pick.
    let mut existing_groups = conflict_groups(item, db);
    let mut extra_tags: Vec<String> = item
        .all_mods_for_conflict()
        .filter_map(|modifier| db.mods.get(&modifier.mod_id))
        .flat_map(|m| m.adds_tags.iter().cloned())
        .collect();

    for _ in 0..count {
        let candidates = pool.eligible(item, &existing_groups, &extra_tags);
        if candidates.is_empty() {
            break;
        }
        let (mod_id, picked) = weighted_pick(&candidates, rng)
            .ok_or_else(|| anyhow::anyhow!("weighted_pick returned None on non-empty pool"))?;

        let rolls = random_rolls_pub(&picked.stats, rng);
        let modifier = Modifier {
            mod_id: mod_id.to_string(),
            generation_type: picked.generation_type.clone(),
            rolls,
        };

        match picked.generation_type {
            GenerationType::Prefix => item.prefixes.push(modifier),
            GenerationType::Suffix => item.suffixes.push(modifier),
            _ => bail!("weighted_pick returned a non-prefix/suffix mod: {mod_id}"),
        }

        existing_groups.extend(picked.groups.iter().cloned());
        extra_tags.extend(picked.adds_tags.iter().cloned());
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────

/// Like `eligible_mods` but applies fossil-specific modifications:
///   - `blocked_mod_ids`: mod IDs completely excluded from the pool.
///   - `fossil_gen_tags`: fossil generation tags; used to apply `generation_weights`
///     multipliers on each mod (`weight * gw.weight / 100` for each matching tag).
pub fn eligible_mods_fossil<'a>(
    item: &ItemState,
    extra_tags: &[String],
    blocked_mod_ids: &[String],
    fossil_gen_tags: &[&str],
    db: &'a GameData,
) -> Vec<(&'a str, &'a crate::data::mods::Mod, u32)> {
    let existing_groups: HashSet<&str> = item
        .all_mods_for_conflict()
        .filter_map(|m| db.mods.get(&m.mod_id))
        .flat_map(|m| m.groups.iter().map(|g| g.as_str()))
        .collect();

    let mut effective_tags = effective_item_tags(item, extra_tags, db);
    effective_tags.extend(fossil_gen_tags.iter().copied());

    db.craftable_mods()
        .filter_map(|(id, m)| {
            if m.required_level > item.item_level {
                return None;
            }
            if blocked_mod_ids.iter().any(|b| b == id) {
                return None;
            }
            let effective_weight = effective_mod_weight(m, &effective_tags);
            if effective_weight == 0 {
                return None;
            }
            if m.groups
                .iter()
                .any(|g| existing_groups.contains(g.as_str()))
            {
                return None;
            }
            match m.generation_type {
                GenerationType::Prefix if !item.has_open_prefix() => return None,
                GenerationType::Suffix if !item.has_open_suffix() => return None,
                _ => {}
            }
            Some((id, m, effective_weight))
        })
        .collect()
}

/// Returns mods eligible for eldritch implicit rolling.
/// Filters by `generation_type` (ExarchImplicit or EaterImplicit) and item tags.
/// Does NOT use `is_craftable()` since eldritch mods are not normal prefix/suffix.
pub fn eligible_mods_eldritch<'a>(
    item: &ItemState,
    gen_type: &GenerationType,
    db: &'a GameData,
) -> Vec<(&'a str, &'a crate::data::mods::Mod, u32)> {
    let effective_tags = effective_item_tags(item, &[], db);
    let mut pool: Vec<(&str, &crate::data::mods::Mod, u32)> = db
        .mods
        .iter()
        .filter_map(|(id, m)| {
            if &m.generation_type != gen_type {
                return None;
            }
            if m.domain != Domain::Item || m.is_essence_only {
                return None;
            }
            if m.required_level > item.item_level {
                return None;
            }
            let weight = effective_mod_weight(m, &effective_tags);
            if weight == 0 {
                return None;
            }
            Some((id.as_str(), m, weight))
        })
        .collect();
    // Eldritch mods bypass the craftable index (different domain), so sort here
    // to keep pool order deterministic for seeded searches.
    pool.sort_by_key(|(id, _, _)| *id);
    pool
}

/// Returns mods eligible for harvest add/augment operations, filtered to only
/// those whose `tags` field contains `harvest_tag` (e.g. "attack", "life").
pub fn eligible_mods_harvest_tag<'a>(
    item: &ItemState,
    harvest_tag: &str,
    db: &'a GameData,
) -> Vec<(&'a str, &'a crate::data::mods::Mod, u32)> {
    let existing_groups: HashSet<&str> = item
        .all_mods_for_conflict()
        .filter_map(|m| db.mods.get(&m.mod_id))
        .flat_map(|m| m.groups.iter().map(|g| g.as_str()))
        .collect();
    let effective_tags = effective_item_tags(item, &[], db);
    db.craftable_mods()
        .filter_map(|(id, m)| {
            if m.required_level > item.item_level {
                return None;
            }
            if !m.tags.iter().any(|t| t == harvest_tag) {
                return None;
            }
            let weight = effective_mod_weight(m, &effective_tags);
            if weight == 0 {
                return None;
            }
            if m.groups
                .iter()
                .any(|g| existing_groups.contains(g.as_str()))
            {
                return None;
            }
            match m.generation_type {
                GenerationType::Prefix if !item.has_open_prefix() => return None,
                GenerationType::Suffix if !item.has_open_suffix() => return None,
                _ => {}
            }
            Some((id, m, weight))
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use rand::rng;
    use std::collections::HashMap;

    use super::*;
    use crate::data::{
        mods::{Domain, GenerationType, GenerationWeight, Mod, ModStat, SpawnWeight},
        GameData,
    };
    use crate::item::modifier::Modifier;
    use crate::item::state::{ItemState, Rarity};

    // ── helpers ──────────────────────────────────────────────────────────────

    fn make_mod(
        gen_type: GenerationType,
        required_level: u32,
        mod_type: &str,
        tag: &str,
        weight: u32,
    ) -> Mod {
        Mod {
            name: mod_type.to_string(),
            generation_type: gen_type,
            required_level,
            stats: vec![ModStat {
                id: "stat".to_string(),
                min: 1,
                max: 10,
            }],
            spawn_weights: vec![SpawnWeight {
                tag: tag.to_string(),
                weight,
            }],
            generation_weights: vec![],
            adds_tags: vec![],
            tags: vec![],
            domain: Domain::Item,
            mod_type: mod_type.to_string(),
            // Each test mod gets a single group named after its mod_type so that
            // group-based conflict detection works correctly in tests.
            groups: vec![mod_type.to_string()],
            is_essence_only: false,
        }
    }

    fn one_mod_db(id: &str, m: Mod) -> GameData {
        let mut mods = HashMap::new();
        mods.insert(id.to_string(), m);
        GameData::new(mods, HashMap::new())
    }

    fn sword_item(item_level: u32) -> ItemState {
        ItemState::new_base("sword", vec!["sword".to_string()], item_level)
    }

    fn rare_sword(item_level: u32) -> ItemState {
        let mut item = sword_item(item_level);
        item.rarity = Rarity::Rare;
        item
    }

    // ── eligible_mods ────────────────────────────────────────────────────────

    #[test]
    fn eligible_filters_by_ilvl() {
        let db = one_mod_db("M", make_mod(GenerationType::Prefix, 80, "T", "sword", 100));
        // ilvl 70 — mod requires 80, should not appear
        let pool = eligible_mods(&rare_sword(70), &[], &db);
        assert!(
            pool.is_empty(),
            "mod requiring ilvl 80 must not appear on ilvl 70 item"
        );

        // ilvl 80 — exactly meets requirement, should appear
        let pool = eligible_mods(&rare_sword(80), &[], &db);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn eligible_filters_zero_weight() {
        let db = one_mod_db("M", make_mod(GenerationType::Prefix, 1, "T", "axe", 100));
        // item has tag "sword", mod only has weight for "axe" → zero weight on this item
        let pool = eligible_mods(&rare_sword(84), &[], &db);
        assert!(pool.is_empty());
    }

    #[test]
    fn eligible_spawn_weight_uses_first_matching_repoe_entry() {
        let mut m = make_mod(GenerationType::Prefix, 1, "T", "sword", 100);
        m.spawn_weights = vec![
            SpawnWeight {
                tag: "weapon".to_string(),
                weight: 0,
            },
            SpawnWeight {
                tag: "sword".to_string(),
                weight: 100,
            },
        ];
        let db = one_mod_db("M", m);
        let mut item = rare_sword(84);
        item.base_tags.insert(0, "weapon".to_string());

        assert!(
            eligible_mods(&item, &[], &db).is_empty(),
            "a later positive tag must not override the first matching zero weight"
        );
    }

    #[test]
    fn eligible_applies_first_matching_generation_weight() {
        let mut m = make_mod(GenerationType::Prefix, 1, "T", "sword", 100);
        m.generation_weights = vec![
            GenerationWeight {
                tag: "weapon".to_string(),
                weight: 50,
            },
            GenerationWeight {
                tag: "sword".to_string(),
                weight: 300,
            },
            GenerationWeight {
                tag: "default".to_string(),
                weight: 100,
            },
        ];
        let db = one_mod_db("M", m);
        let mut item = rare_sword(84);
        item.base_tags.push("weapon".to_string());

        let pool = eligible_mods(&item, &[], &db);
        assert_eq!(pool.len(), 1);
        assert_eq!(
            pool[0].2, 50,
            "generation weights are ordered alternatives, not multipliers"
        );
    }

    #[test]
    fn eligible_excludes_zero_generation_weight_and_saturates_large_weights() {
        let mut blocked = make_mod(GenerationType::Prefix, 1, "Blocked", "sword", 100);
        blocked.generation_weights = vec![GenerationWeight {
            tag: "sword".to_string(),
            weight: 0,
        }];

        let mut huge = make_mod(GenerationType::Suffix, 1, "Huge", "sword", u32::MAX);
        huge.generation_weights = vec![GenerationWeight {
            tag: "sword".to_string(),
            weight: u32::MAX,
        }];

        let mut mods = HashMap::new();
        mods.insert("blocked".to_string(), blocked);
        mods.insert("huge".to_string(), huge);
        let db = GameData::new(mods, HashMap::new());
        let pool = eligible_mods(&rare_sword(84), &[], &db);

        assert_eq!(ids_and_weights(&pool), vec![("huge", u32::MAX)]);
    }

    #[test]
    fn eligible_excludes_essence_only() {
        let mut m = make_mod(GenerationType::Prefix, 1, "T", "sword", 100);
        m.is_essence_only = true;
        let db = one_mod_db("M", m);
        let pool = eligible_mods(&rare_sword(84), &[], &db);
        assert!(pool.is_empty());
    }

    #[test]
    fn eligible_excludes_conflict_same_mod_type() {
        // Put a mod of type "T" on the item already.
        let m = make_mod(GenerationType::Prefix, 1, "T", "sword", 100);
        let db = one_mod_db("M", m.clone());

        let mut item = rare_sword(84);
        item.prefixes.push(Modifier {
            mod_id: "M".to_string(),
            generation_type: GenerationType::Prefix,
            rolls: vec![],
        });

        // Another mod of same mod_type should be excluded.
        let pool = eligible_mods(&item, &[], &db);
        assert!(
            pool.is_empty(),
            "mod_type conflict must exclude the candidate"
        );
    }

    #[test]
    fn eligible_uses_adds_tags_from_existing_mods() {
        let mut existing = make_mod(GenerationType::Prefix, 1, "Existing", "sword", 100);
        existing.adds_tags = vec!["blocks_candidate".to_string()];
        let mut candidate = make_mod(GenerationType::Suffix, 1, "Candidate", "sword", 100);
        candidate.spawn_weights.insert(
            0,
            SpawnWeight {
                tag: "blocks_candidate".to_string(),
                weight: 0,
            },
        );

        let mut mods = HashMap::new();
        mods.insert("existing".to_string(), existing);
        mods.insert("candidate".to_string(), candidate);
        let db = GameData::new(mods, HashMap::new());
        let mut item = rare_sword(84);
        item.prefixes.push(Modifier {
            mod_id: "existing".to_string(),
            generation_type: GenerationType::Prefix,
            rolls: vec![],
        });

        assert!(
            eligible_mods(&item, &[], &db).is_empty(),
            "tags added by an existing affix must affect a later add-one roll"
        );
    }

    #[test]
    fn eligible_conflicts_with_fractured_and_crafted_groups() {
        let mut occupied = make_mod(GenerationType::Prefix, 1, "Occupied", "sword", 100);
        occupied.groups = vec!["shared".to_string()];
        let mut candidate = make_mod(GenerationType::Suffix, 1, "Candidate", "sword", 100);
        candidate.groups = vec!["shared".to_string()];

        let mut mods = HashMap::new();
        mods.insert("occupied".to_string(), occupied);
        mods.insert("candidate".to_string(), candidate);
        let db = GameData::new(mods, HashMap::new());
        let occupied_modifier = Modifier {
            mod_id: "occupied".to_string(),
            generation_type: GenerationType::Prefix,
            rolls: vec![],
        };

        let mut fractured_item = rare_sword(84);
        fractured_item.fractured.push(occupied_modifier.clone());
        assert!(eligible_mods(&fractured_item, &[], &db).is_empty());

        let mut crafted_item = rare_sword(84);
        crafted_item.crafted_mod = Some(occupied_modifier);
        assert!(eligible_mods(&crafted_item, &[], &db).is_empty());
    }

    #[test]
    fn eligible_excludes_when_prefix_slots_full() {
        let m = make_mod(GenerationType::Prefix, 1, "T", "sword", 100);
        let db = one_mod_db("M", m);
        let mut item = rare_sword(84);
        // Fill all 3 prefix slots with dummy mods of different types.
        for i in 0..3 {
            item.prefixes.push(Modifier {
                mod_id: format!("other{i}"),
                generation_type: GenerationType::Prefix,
                rolls: vec![],
            });
        }
        let pool = eligible_mods(&item, &[], &db);
        assert!(pool.is_empty(), "no prefix mods when all prefix slots full");
    }

    #[test]
    fn eligible_includes_suffix_when_only_prefix_full() {
        // One suffix mod, prefix slots all taken.
        let m = make_mod(GenerationType::Suffix, 1, "T", "sword", 100);
        let db = one_mod_db("M", m);
        let mut item = rare_sword(84);
        for i in 0..3 {
            item.prefixes.push(Modifier {
                mod_id: format!("p{i}"),
                generation_type: GenerationType::Prefix,
                rolls: vec![],
            });
        }
        let pool = eligible_mods(&item, &[], &db);
        assert_eq!(
            pool.len(),
            1,
            "suffix mod must still appear when only prefix slots are full"
        );
    }

    #[test]
    fn eligible_counts_fractured_and_crafted_mods_toward_capacity() {
        let m = make_mod(GenerationType::Prefix, 1, "T", "sword", 100);
        let db = one_mod_db("M", m);
        let mut item = rare_sword(84);
        for i in 0..2 {
            item.prefixes.push(Modifier {
                mod_id: format!("regular{i}"),
                generation_type: GenerationType::Prefix,
                rolls: vec![],
            });
        }
        item.fractured.push(Modifier {
            mod_id: "fractured".to_string(),
            generation_type: GenerationType::Prefix,
            rolls: vec![],
        });
        assert!(eligible_mods(&item, &[], &db).is_empty());

        item.fractured.clear();
        item.crafted_mod = Some(Modifier {
            mod_id: "crafted".to_string(),
            generation_type: GenerationType::Prefix,
            rolls: vec![],
        });
        assert!(eligible_mods(&item, &[], &db).is_empty());
    }

    #[test]
    fn eligible_pool_order_is_stable_across_hashmap_insertion_order() {
        let mut mods = HashMap::new();
        mods.insert(
            "Z".to_string(),
            make_mod(GenerationType::Prefix, 1, "TZ", "sword", 100),
        );
        mods.insert(
            "A".to_string(),
            make_mod(GenerationType::Suffix, 1, "TA", "sword", 100),
        );
        let db = GameData::new(mods, HashMap::new());

        let ids: Vec<&str> = eligible_mods(&rare_sword(84), &[], &db)
            .iter()
            .map(|(id, _, _)| *id)
            .collect();
        assert_eq!(ids, vec!["A", "Z"]);
    }

    // ── weighted_pick ────────────────────────────────────────────────────────

    #[test]
    fn weighted_pick_empty_returns_none() {
        let pool: Vec<(&str, &Mod, u32)> = vec![];
        let result = weighted_pick(&pool, &mut rng());
        assert!(result.is_none());
    }

    #[test]
    fn weighted_pick_all_zero_returns_none() {
        let m = make_mod(GenerationType::Prefix, 1, "T", "sword", 100);
        let pool = vec![("ID", &m, 0u32)];
        assert!(weighted_pick(&pool, &mut rng()).is_none());
    }

    #[test]
    fn weighted_pick_sums_weights_wider_than_u32() {
        let a = make_mod(GenerationType::Prefix, 1, "A", "sword", 100);
        let b = make_mod(GenerationType::Suffix, 1, "B", "sword", 100);
        let pool = vec![("A", &a, u32::MAX), ("B", &b, u32::MAX)];

        for _ in 0..20 {
            assert!(weighted_pick(&pool, &mut rng()).is_some());
        }
    }

    #[test]
    fn weighted_pick_single_entry_always_returns_it() {
        let m = make_mod(GenerationType::Prefix, 1, "T", "sword", 999);
        let pool = vec![("ID", &m, 999u32)];
        for _ in 0..20 {
            let (id, _) = weighted_pick(&pool, &mut rng()).unwrap();
            assert_eq!(id, "ID");
        }
    }

    #[test]
    fn weighted_pick_returns_id_from_pool() {
        let a = make_mod(GenerationType::Prefix, 1, "A", "sword", 100);
        let b = make_mod(GenerationType::Suffix, 1, "B", "sword", 200);
        let pool = vec![("A_ID", &a, 100u32), ("B_ID", &b, 200u32)];
        for _ in 0..50 {
            let (id, _) = weighted_pick(&pool, &mut rng()).unwrap();
            assert!(id == "A_ID" || id == "B_ID");
        }
    }

    // ── random_rolls_pub ─────────────────────────────────────────────────────

    #[test]
    fn random_rolls_within_range() {
        let stats = vec![
            ModStat {
                id: "s1".to_string(),
                min: 5,
                max: 20,
            },
            ModStat {
                id: "s2".to_string(),
                min: 42,
                max: 42,
            },
        ];
        let mut r = rng();
        for _ in 0..100 {
            let rolls = random_rolls_pub(&stats, &mut r);
            assert!(
                (5..=20).contains(&rolls[0].value),
                "s1 out of range: {}",
                rolls[0].value
            );
            assert_eq!(rolls[1].value, 42, "fixed stat must be exactly 42");
        }
    }

    // ── roll_mods ────────────────────────────────────────────────────────────

    #[test]
    fn roll_mods_places_correct_count() {
        // Build a db with 3 prefix mods and 3 suffix mods, all eligible.
        let mut mods = HashMap::new();
        for i in 0..3u32 {
            mods.insert(
                format!("P{i}"),
                make_mod(GenerationType::Prefix, 1, &format!("PT{i}"), "sword", 100),
            );
            mods.insert(
                format!("S{i}"),
                make_mod(GenerationType::Suffix, 1, &format!("ST{i}"), "sword", 100),
            );
        }
        let db = GameData::new(mods, HashMap::new());
        let mut item = rare_sword(84);

        roll_mods(&mut item, 4, &db, &mut rng()).unwrap();
        assert_eq!(item.prefixes.len() + item.suffixes.len(), 4);
    }

    #[test]
    fn roll_mods_no_duplicate_groups() {
        // Many mods, each with a unique group (via make_mod's groups = [mod_type]).
        let mut mods = HashMap::new();
        for i in 0..10u32 {
            let gen = if i % 2 == 0 {
                GenerationType::Prefix
            } else {
                GenerationType::Suffix
            };
            mods.insert(
                format!("M{i}"),
                make_mod(gen, 1, &format!("Type{i}"), "sword", 100),
            );
        }
        let db = GameData::new(mods, HashMap::new());
        let mut item = rare_sword(84);

        roll_mods(&mut item, 6, &db, &mut rng()).unwrap();

        // Verify no two placed mods share a group — the real conflict criterion.
        let mut seen_groups = std::collections::HashSet::<String>::new();
        for m in item.all_explicit_mods() {
            if let Some(md) = db.mods.get(&m.mod_id) {
                for g in &md.groups {
                    assert!(
                        seen_groups.insert(g.clone()),
                        "duplicate group '{g}' found on item after rolling"
                    );
                }
            }
        }
    }

    #[test]
    fn roll_mods_stops_early_when_pool_exhausted() {
        // Only 2 mods available (1 prefix slot, 1 suffix slot) but we ask for 6.
        let mut mods = HashMap::new();
        mods.insert(
            "P0".to_string(),
            make_mod(GenerationType::Prefix, 1, "PT0", "sword", 100),
        );
        mods.insert(
            "S0".to_string(),
            make_mod(GenerationType::Suffix, 1, "ST0", "sword", 100),
        );
        let db = GameData::new(mods, HashMap::new());
        let mut item = rare_sword(84);

        roll_mods(&mut item, 6, &db, &mut rng()).unwrap();
        // Should have at most 2 mods total, no panic.
        assert!(item.prefixes.len() + item.suffixes.len() <= 2);
    }

    #[test]
    fn roll_mods_seeds_tags_from_mods_already_on_item() {
        let mut existing = make_mod(GenerationType::Prefix, 1, "Existing", "sword", 100);
        existing.adds_tags = vec!["blocks_candidate".to_string()];
        let mut candidate = make_mod(GenerationType::Suffix, 1, "Candidate", "sword", 100);
        candidate.spawn_weights.insert(
            0,
            SpawnWeight {
                tag: "blocks_candidate".to_string(),
                weight: 0,
            },
        );

        let mut mods = HashMap::new();
        mods.insert("existing".to_string(), existing);
        mods.insert("candidate".to_string(), candidate);
        let db = GameData::new(mods, HashMap::new());
        let mut item = rare_sword(84);
        item.prefixes.push(Modifier {
            mod_id: "existing".to_string(),
            generation_type: GenerationType::Prefix,
            rolls: vec![],
        });

        roll_mods(&mut item, 1, &db, &mut rng()).unwrap();
        assert!(
            item.suffixes.is_empty(),
            "cached rolling must not ignore tags contributed by forced or existing mods"
        );
    }

    // ── RollPool (hot-path cache) ────────────────────────────────────────────

    fn ids_and_weights<'a>(pool: &[(&'a str, &'a Mod, u32)]) -> Vec<(&'a str, u32)> {
        pool.iter().map(|(id, _, w)| (*id, *w)).collect()
    }

    #[test]
    fn roll_pool_matches_eligible_mods() {
        // Mixed db: eligible, ilvl-gated, and wrong-tag mods.
        let mut mods = HashMap::new();
        mods.insert(
            "A".to_string(),
            make_mod(GenerationType::Prefix, 1, "TA", "sword", 100),
        );
        mods.insert(
            "B".to_string(),
            make_mod(GenerationType::Suffix, 1, "TB", "sword", 200),
        );
        mods.insert(
            "C".to_string(),
            make_mod(GenerationType::Prefix, 90, "TC", "sword", 100),
        );
        mods.insert(
            "D".to_string(),
            make_mod(GenerationType::Prefix, 1, "TD", "axe", 100),
        );
        let db = GameData::new(mods, HashMap::new());
        let item = rare_sword(84);

        let pool = RollPool::new(&item, &db);
        let via_pool = pool.eligible(&item, &conflict_groups(&item, &db), &[]);
        let direct = eligible_mods(&item, &[], &db);
        assert_eq!(
            ids_and_weights(&via_pool),
            ids_and_weights(&direct),
            "RollPool must produce the same pool (and order) as eligible_mods"
        );
        assert_eq!(via_pool.len(), 2, "only A and B are eligible");
    }

    #[test]
    fn roll_pool_reweighs_with_extra_tags() {
        // "D" only has weight for tag "axe" — dormant until adds_tags provide it.
        let db = one_mod_db("D", make_mod(GenerationType::Prefix, 1, "TD", "axe", 100));
        let item = rare_sword(84);
        let pool = RollPool::new(&item, &db);
        let groups = conflict_groups(&item, &db);

        assert!(pool.eligible(&item, &groups, &[]).is_empty());

        let extra = vec!["axe".to_string()];
        let awakened = pool.eligible(&item, &groups, &extra);
        let direct = eligible_mods(&item, &extra, &db);
        assert_eq!(
            ids_and_weights(&awakened),
            ids_and_weights(&direct),
            "extra-tag reweigh must match eligible_mods"
        );
        assert_eq!(awakened.len(), 1);
    }

    #[test]
    fn roll_pool_fossil_matches_eligible_mods_fossil() {
        // "A" gets a 3x generation-weight boost from the "life" fossil tag;
        // "B" is blocked outright.
        let mut a = make_mod(GenerationType::Prefix, 1, "TA", "sword", 100);
        a.generation_weights = vec![GenerationWeight {
            tag: "life".to_string(),
            weight: 300,
        }];
        let mut mods = HashMap::new();
        mods.insert("A".to_string(), a);
        mods.insert(
            "B".to_string(),
            make_mod(GenerationType::Suffix, 1, "TB", "sword", 50),
        );
        let db = GameData::new(mods, HashMap::new());
        let item = rare_sword(84);
        let blocked = vec!["B".to_string()];
        let fossil_tags = ["life"];

        let pool = RollPool::with_fossils(&item, &blocked, &fossil_tags, &db);
        let via_pool = pool.eligible(&item, &conflict_groups(&item, &db), &[]);
        let direct = eligible_mods_fossil(&item, &[], &blocked, &fossil_tags, &db);
        assert_eq!(ids_and_weights(&via_pool), ids_and_weights(&direct));
        assert_eq!(via_pool.len(), 1);
        assert_eq!(via_pool[0].2, 300, "100 base x 3.0 fossil multiplier");
    }

    #[test]
    fn fossil_generation_weights_are_first_match_not_multiplicative() {
        let mut m = make_mod(GenerationType::Prefix, 1, "T", "sword", 100);
        m.generation_weights = vec![
            GenerationWeight {
                tag: "life".to_string(),
                weight: 200,
            },
            GenerationWeight {
                tag: "attack".to_string(),
                weight: 300,
            },
        ];
        let db = one_mod_db("M", m);
        let item = rare_sword(84);
        let fossil_tags = ["life", "attack"];

        let direct = eligible_mods_fossil(&item, &[], &[], &fossil_tags, &db);
        let cached = RollPool::with_fossils(&item, &[], &fossil_tags, &db).eligible(
            &item,
            &conflict_groups(&item, &db),
            &[],
        );

        assert_eq!(direct[0].2, 200);
        assert_eq!(ids_and_weights(&cached), ids_and_weights(&direct));
    }

    // ── Harvest and eldritch special pools ──────────────────────────────────

    #[test]
    fn harvest_pool_applies_generation_weights() {
        let mut life = make_mod(GenerationType::Prefix, 1, "Life", "sword", 100);
        life.tags = vec!["life".to_string()];
        life.generation_weights = vec![GenerationWeight {
            tag: "sword".to_string(),
            weight: 50,
        }];
        let db = one_mod_db("life", life);

        let pool = eligible_mods_harvest_tag(&rare_sword(84), "life", &db);
        assert_eq!(ids_and_weights(&pool), vec![("life", 50)]);
    }

    #[test]
    fn eldritch_pool_filters_domain_and_is_deterministically_weighted() {
        let mut z = make_mod(GenerationType::ExarchImplicit, 1, "Z", "sword", 100);
        z.generation_weights = vec![GenerationWeight {
            tag: "sword".to_string(),
            weight: 50,
        }];
        let a = make_mod(GenerationType::ExarchImplicit, 1, "A", "sword", 200);
        let mut wrong_domain = make_mod(GenerationType::ExarchImplicit, 1, "Wrong", "sword", 999);
        wrong_domain.domain = Domain::Monster;
        let mut essence_only = make_mod(GenerationType::ExarchImplicit, 1, "Essence", "sword", 999);
        essence_only.is_essence_only = true;

        let mut mods = HashMap::new();
        mods.insert("Z".to_string(), z);
        mods.insert("A".to_string(), a);
        mods.insert("wrong_domain".to_string(), wrong_domain);
        mods.insert("essence_only".to_string(), essence_only);
        let db = GameData::new(mods, HashMap::new());

        let pool = eligible_mods_eldritch(&rare_sword(84), &GenerationType::ExarchImplicit, &db);
        assert_eq!(ids_and_weights(&pool), vec![("A", 200), ("Z", 50)]);
    }
}
