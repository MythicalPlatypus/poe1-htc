//! Fossil crafting — rerolls the item as a Chaos Orb but with modified spawn weight tables.
//! Up to 4 fossils can be combined in one resonator.

use std::collections::HashSet;

use anyhow::{bail, Result};
use rand::{rng, Rng};

use crate::data::GameData;
use crate::data::mods::GenerationType;
use crate::engine::mod_pool::{eligible_mods_fossil, random_rolls_pub, weighted_pick};
use crate::item::modifier::Modifier;
use crate::item::{ItemState, state::Rarity};
use super::{CraftingMethod, MONTE_CARLO_SAMPLES};

/// Describes how a fossil modifies the mod pool.
#[derive(Debug, Clone)]
pub struct FossilModifier {
    /// Tags whose mods receive a positive generation_weight multiplier.
    pub boosted_tags: Vec<String>,
    /// Tags whose mods receive a negative generation_weight multiplier.
    pub reduced_tags: Vec<String>,
    /// Specific mod IDs completely blocked from the pool.
    pub blocked_mod_ids: Vec<String>,
    /// Specific mod IDs forced onto the item before random rolling.
    pub forced_mod_ids: Vec<String>,
}

/// A resonator + fossil combination applied as one crafting operation.
pub struct FossilCraft {
    pub display_name: String,
    pub cost_chaos: f64,
    pub fossils: Vec<FossilModifier>,
}

impl CraftingMethod for FossilCraft {
    fn name(&self) -> &str { &self.display_name }
    fn cost_chaos(&self) -> f64 { self.cost_chaos }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable()
            && matches!(item.rarity, Rarity::Normal | Rarity::Magic | Rarity::Rare)
    }

    fn apply(&self, item: &ItemState, db: &GameData) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) { bail!("Cannot apply {}", self.display_name); }

        // Collect blocked mod IDs and fossil generation tags once (shared across samples).
        let blocked_mod_ids: Vec<String> = self.fossils.iter()
            .flat_map(|f| f.blocked_mod_ids.iter().cloned())
            .collect();
        let fossil_gen_tags: Vec<&str> = self.fossils.iter()
            .flat_map(|f| {
                f.boosted_tags.iter().map(|s| s.as_str())
                    .chain(f.reduced_tags.iter().map(|s| s.as_str()))
            })
            .collect();

        // Validate all forced mods before sampling: existence, type, group conflicts, slot capacity.
        // After apply, prefixes/suffixes/crafted_mod are cleared; fractured mods remain.
        // Track occupied groups and slots as forced mods are accepted in order.
        let mut occupied_groups: HashSet<&str> = item.fractured.iter()
            .filter_map(|m| db.mods.get(&m.mod_id))
            .flat_map(|m| m.groups.iter().map(|g| g.as_str()))
            .collect();
        let mut forced_prefixes = item.fractured.iter()
            .filter(|m| m.generation_type == GenerationType::Prefix).count();
        let mut forced_suffixes = item.fractured.iter()
            .filter(|m| m.generation_type == GenerationType::Suffix).count();

        for fossil in &self.fossils {
            for mod_id in &fossil.forced_mod_ids {
                let forced_mod = db.mods.get(mod_id)
                    .ok_or_else(|| anyhow::anyhow!("Fossil forced mod '{}' not in DB", mod_id))?;
                if !matches!(forced_mod.generation_type, GenerationType::Prefix | GenerationType::Suffix) {
                    bail!("{}: forced mod '{}' is not a prefix or suffix", self.display_name, mod_id);
                }
                if forced_mod.groups.iter().any(|g| occupied_groups.contains(g.as_str())) {
                    bail!(
                        "{}: forced mod '{}' shares a mod group with a fractured mod or another forced mod",
                        self.display_name, mod_id
                    );
                }
                match forced_mod.generation_type {
                    GenerationType::Prefix if forced_prefixes >= 3 =>
                        bail!("{}: no open prefix slot for forced mod '{}'", self.display_name, mod_id),
                    GenerationType::Suffix if forced_suffixes >= 3 =>
                        bail!("{}: no open suffix slot for forced mod '{}'", self.display_name, mod_id),
                    _ => {}
                }
                for g in &forced_mod.groups { occupied_groups.insert(g.as_str()); }
                match forced_mod.generation_type {
                    GenerationType::Prefix => forced_prefixes += 1,
                    GenerationType::Suffix => forced_suffixes += 1,
                    _ => {}
                }
            }
        }

        let prob = 1.0 / MONTE_CARLO_SAMPLES as f64;
        let mut rng = rng();
        let mut outcomes = Vec::with_capacity(MONTE_CARLO_SAMPLES);

        for _ in 0..MONTE_CARLO_SAMPLES {
            let mut next = item.clone();
            next.rarity = Rarity::Rare;
            next.prefixes.clear();
            next.suffixes.clear();
            next.crafted_mod = None;

            // Place forced mods first (validity confirmed in upfront checks above).
            for fossil in &self.fossils {
                for mod_id in &fossil.forced_mod_ids {
                    let forced_mod = db.mods.get(mod_id)
                        .ok_or_else(|| anyhow::anyhow!("{}: forced mod '{}' missing from DB", self.display_name, mod_id))?;
                    let rolls = random_rolls_pub(&forced_mod.stats, &mut rng);
                    let modifier = Modifier {
                        mod_id: mod_id.clone(),
                        generation_type: forced_mod.generation_type.clone(),
                        rolls,
                    };
                    match forced_mod.generation_type {
                        GenerationType::Prefix => next.prefixes.push(modifier),
                        GenerationType::Suffix => next.suffixes.push(modifier),
                        _ => bail!("{}: forced mod '{}' is not a prefix or suffix (validated above)", self.display_name, mod_id),
                    }
                }
            }

            // Roll remaining mods using fossil-modified pool (4–6 total).
            let forced_count = next.prefixes.len() + next.suffixes.len();
            let total: usize = rng.random_range(4..=6);
            let remaining = total.saturating_sub(forced_count);

            let mut extra_tags: Vec<String> = Vec::new();
            for _ in 0..remaining {
                let pool = eligible_mods_fossil(&next, &extra_tags, &blocked_mod_ids, &fossil_gen_tags, db);
                if pool.is_empty() { break; }
                let (mod_id, picked) = match weighted_pick(&pool, &mut rng) {
                    Some(m) => m,
                    None => break,
                };
                let rolls = random_rolls_pub(&picked.stats, &mut rng);
                let modifier = Modifier {
                    mod_id: mod_id.to_string(),
                    generation_type: picked.generation_type.clone(),
                    rolls,
                };
                match picked.generation_type {
                    GenerationType::Prefix => next.prefixes.push(modifier),
                    GenerationType::Suffix => next.suffixes.push(modifier),
                    _ => bail!("{}: rolled non-prefix/suffix mod: {mod_id}", self.display_name),
                }
                extra_tags.extend(picked.adds_tags.iter().cloned());
            }
            outcomes.push((next, prob));
        }

        Ok(outcomes)
    }
}
