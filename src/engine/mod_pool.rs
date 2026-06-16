//! Mod pool logic: filtering, weighted selection, and iterative mod rolling.

use std::collections::HashSet;

use anyhow::{bail, Result};
use rand::Rng;

use crate::data::{mods::{GenerationType, ModStat}, GameData};
use crate::item::{modifier::{Modifier, StatRoll}, state::ItemState};

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

    // Build effective tag list: base tags + extra_tags from this roll session.
    let effective_tags: Vec<&str> = item
        .base_tags
        .iter()
        .map(|t| t.as_str())
        .chain(extra_tags.iter().map(|t| t.as_str()))
        .collect();

    db.mods
        .iter()
        .filter_map(|(id, m)| {
            if !m.is_craftable() {
                return None;
            }
            if m.required_level > item.item_level {
                return None;
            }
            let weight = m.spawn_weight_for_tags(&effective_tags);
            if weight == 0 {
                return None;
            }
            if m.groups.iter().any(|g| existing_groups.contains(g.as_str())) {
                return None;
            }
            match m.generation_type {
                GenerationType::Prefix if !item.has_open_prefix() => return None,
                GenerationType::Suffix if !item.has_open_suffix() => return None,
                _ => {}
            }
            Some((id.as_str(), m, weight))
        })
        .collect()
}

/// Picks one mod from `pool` using weighted random selection.
/// Returns `None` only if `pool` is empty.
pub fn weighted_pick<'a, R: Rng>(
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
pub fn random_rolls_pub<R: Rng>(stats: &[ModStat], rng: &mut R) -> Vec<StatRoll> {
    stats
        .iter()
        .map(|s| {
            let value = if s.min == s.max {
                s.min
            } else {
                rng.random_range(s.min..=s.max)
            };
            StatRoll { stat_id: s.id.clone(), value }
        })
        .collect()
}

/// Rolls up to `count` mods onto `item` iteratively, re-computing the eligible
/// pool after each pick so that `adds_tags` from placed mods are respected.
///
/// Stops early if the eligible pool is exhausted before `count` is reached
/// (partial rolls are valid PoE behaviour).
pub fn roll_mods<R: Rng>(
    item: &mut ItemState,
    count: usize,
    db: &GameData,
    rng: &mut R,
) -> Result<()> {
    let mut extra_tags: Vec<String> = Vec::new();

    for _ in 0..count {
        let pool = eligible_mods(item, &extra_tags, db);
        if pool.is_empty() {
            break;
        }
        let (mod_id, picked) = weighted_pick(&pool, rng)
            .expect("pool non-empty but weighted_pick returned None");

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

    let effective_tags: Vec<&str> = item
        .base_tags.iter().map(|t| t.as_str())
        .chain(extra_tags.iter().map(|t| t.as_str()))
        .collect();

    db.mods.iter().filter_map(|(id, m)| {
        if !m.is_craftable() { return None; }
        if m.required_level > item.item_level { return None; }
        if blocked_mod_ids.iter().any(|b| b == id) { return None; }
        let base_weight = m.spawn_weight_for_tags(&effective_tags);
        if base_weight == 0 { return None; }
        if m.groups.iter().any(|g| existing_groups.contains(g.as_str())) { return None; }
        match m.generation_type {
            GenerationType::Prefix if !item.has_open_prefix() => return None,
            GenerationType::Suffix if !item.has_open_suffix() => return None,
            _ => {}
        }
        // Apply generation_weight multipliers from fossil tags.
        let gen_multiplier: f64 = m.generation_weights.iter()
            .filter(|gw| fossil_gen_tags.contains(&gw.tag.as_str()))
            .fold(1.0_f64, |acc, gw| acc * gw.weight as f64 / 100.0);
        let effective_weight = (base_weight as f64 * gen_multiplier).round() as u32;
        if effective_weight == 0 { return None; }
        Some((id.as_str(), m, effective_weight))
    }).collect()
}

/// Returns mods eligible for eldritch implicit rolling.
/// Filters by `generation_type` (ExarchImplicit or EaterImplicit) and item tags.
/// Does NOT use `is_craftable()` since eldritch mods are not normal prefix/suffix.
pub fn eligible_mods_eldritch<'a>(
    item: &ItemState,
    gen_type: &GenerationType,
    db: &'a GameData,
) -> Vec<(&'a str, &'a crate::data::mods::Mod, u32)> {
    let effective_tags: Vec<&str> = item.base_tags.iter().map(|t| t.as_str()).collect();
    db.mods.iter().filter_map(|(id, m)| {
        if &m.generation_type != gen_type { return None; }
        if m.required_level > item.item_level { return None; }
        let weight = m.spawn_weight_for_tags(&effective_tags);
        if weight == 0 { return None; }
        Some((id.as_str(), m, weight))
    }).collect()
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
    let effective_tags: Vec<&str> = item.base_tags.iter().map(|t| t.as_str()).collect();
    db.mods.iter().filter_map(|(id, m)| {
        if !m.is_craftable() { return None; }
        if m.required_level > item.item_level { return None; }
        if !m.tags.iter().any(|t| t == harvest_tag) { return None; }
        let weight = m.spawn_weight_for_tags(&effective_tags);
        if weight == 0 { return None; }
        if m.groups.iter().any(|g| existing_groups.contains(g.as_str())) { return None; }
        match m.generation_type {
            GenerationType::Prefix if !item.has_open_prefix() => return None,
            GenerationType::Suffix if !item.has_open_suffix() => return None,
            _ => {}
        }
        Some((id.as_str(), m, weight))
    }).collect()
}

// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use rand::rng;

    use super::*;
    use crate::data::{
        GameData,
        mods::{Domain, GenerationType, Mod, ModStat, SpawnWeight},
    };
    use crate::item::state::{ItemState, Rarity};
    use crate::item::modifier::Modifier;

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
            stats: vec![ModStat { id: "stat".to_string(), min: 1, max: 10 }],
            spawn_weights: vec![SpawnWeight { tag: tag.to_string(), weight }],
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
        GameData { mods, base_items: HashMap::new() }
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
        assert!(pool.is_empty(), "mod requiring ilvl 80 must not appear on ilvl 70 item");

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
        assert!(pool.is_empty(), "mod_type conflict must exclude the candidate");
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
        assert_eq!(pool.len(), 1, "suffix mod must still appear when only prefix slots are full");
    }

    // ── weighted_pick ────────────────────────────────────────────────────────

    #[test]
    fn weighted_pick_empty_returns_none() {
        let pool: Vec<(&str, &Mod, u32)> = vec![];
        let result = weighted_pick(&pool, &mut rng());
        assert!(result.is_none());
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
            ModStat { id: "s1".to_string(), min: 5, max: 20 },
            ModStat { id: "s2".to_string(), min: 42, max: 42 },
        ];
        let mut r = rng();
        for _ in 0..100 {
            let rolls = random_rolls_pub(&stats, &mut r);
            assert!((5..=20).contains(&rolls[0].value), "s1 out of range: {}", rolls[0].value);
            assert_eq!(rolls[1].value, 42, "fixed stat must be exactly 42");
        }
    }

    // ── roll_mods ────────────────────────────────────────────────────────────

    #[test]
    fn roll_mods_places_correct_count() {
        // Build a db with 3 prefix mods and 3 suffix mods, all eligible.
        let mut mods = HashMap::new();
        for i in 0..3u32 {
            mods.insert(format!("P{i}"), make_mod(GenerationType::Prefix, 1, &format!("PT{i}"), "sword", 100));
            mods.insert(format!("S{i}"), make_mod(GenerationType::Suffix, 1, &format!("ST{i}"), "sword", 100));
        }
        let db = GameData { mods, base_items: HashMap::new() };
        let mut item = rare_sword(84);

        roll_mods(&mut item, 4, &db, &mut rng()).unwrap();
        assert_eq!(item.prefixes.len() + item.suffixes.len(), 4);
    }

    #[test]
    fn roll_mods_no_duplicate_groups() {
        // Many mods, each with a unique group (via make_mod's groups = [mod_type]).
        let mut mods = HashMap::new();
        for i in 0..10u32 {
            let gen = if i % 2 == 0 { GenerationType::Prefix } else { GenerationType::Suffix };
            mods.insert(format!("M{i}"), make_mod(gen, 1, &format!("Type{i}"), "sword", 100));
        }
        let db = GameData { mods, base_items: HashMap::new() };
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
        mods.insert("P0".to_string(), make_mod(GenerationType::Prefix, 1, "PT0", "sword", 100));
        mods.insert("S0".to_string(), make_mod(GenerationType::Suffix, 1, "ST0", "sword", 100));
        let db = GameData { mods, base_items: HashMap::new() };
        let mut item = rare_sword(84);

        roll_mods(&mut item, 6, &db, &mut rng()).unwrap();
        // Should have at most 2 mods total, no panic.
        assert!(item.prefixes.len() + item.suffixes.len() <= 2);
    }
}
