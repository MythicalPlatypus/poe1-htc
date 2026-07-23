//! Harvest crafting operations.
//! Harvest offers targeted rerolls, augments, and removals by mod type/tag.

use anyhow::{bail, Result};
use rand::RngCore;

use super::CraftingMethod;
use crate::data::mods::GenerationType;
use crate::data::GameData;
use crate::engine::mod_pool::{eligible_mods_harvest_tag, random_rolls_pub};
use crate::item::modifier::Modifier;
use crate::item::{state::Rarity, ItemState};

/// The category of mods targeted by a Harvest operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarvestTarget {
    Attack,
    Caster,
    Speed,
    Life,
    Defence,
    Resistance,
    Chaos,
    Fire,
    Cold,
    Lightning,
    Physical,
    Critical,
}

impl HarvestTarget {
    /// Returns the RePoE mod `tags` value that corresponds to this harvest target.
    fn as_tag(self) -> &'static str {
        match self {
            Self::Attack => "attack",
            Self::Caster => "caster",
            Self::Speed => "speed",
            Self::Life => "life",
            Self::Defence => "defences",
            Self::Resistance => "elemental",
            Self::Chaos => "chaos",
            Self::Fire => "fire",
            Self::Cold => "cold",
            Self::Lightning => "lightning",
            Self::Physical => "physical",
            Self::Critical => "critical",
        }
    }

    /// Parse a goal-file target string (the same words as `as_tag`, plus the
    /// variant names in lowercase). Returns None for unknown targets.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "attack" => Some(Self::Attack),
            "caster" => Some(Self::Caster),
            "speed" => Some(Self::Speed),
            "life" => Some(Self::Life),
            "defence" | "defences" => Some(Self::Defence),
            "resistance" | "elemental" => Some(Self::Resistance),
            "chaos" => Some(Self::Chaos),
            "fire" => Some(Self::Fire),
            "cold" => Some(Self::Cold),
            "lightning" => Some(Self::Lightning),
            "physical" => Some(Self::Physical),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

/// Which type of Harvest operation to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarvestOp {
    /// Remove one mod of the target type (random if multiple match).
    Remove,
    /// Add one mod of the target type (item must have an open affix slot).
    Add,
    /// Remove one mod of the target type, then add a new one of the same type.
    RemoveAdd,
    /// Add one mod of the target type, but only if the item has no such mod yet.
    Augment,
}

impl HarvestOp {
    /// Parse a goal-file op string. Returns None for unknown ops.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "remove" => Some(Self::Remove),
            "add" => Some(Self::Add),
            "remove_add" => Some(Self::RemoveAdd),
            "augment" => Some(Self::Augment),
            _ => None,
        }
    }
}

pub struct HarvestCraft {
    pub display_name: String,
    pub cost_chaos: f64,
    pub target: HarvestTarget,
    pub op: HarvestOp,
}

impl CraftingMethod for HarvestCraft {
    fn name(&self) -> &str {
        &self.display_name
    }
    fn cost_chaos(&self) -> f64 {
        self.cost_chaos
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        if !item.is_craftable() {
            return false;
        }
        if !matches!(item.rarity, Rarity::Magic | Rarity::Rare) {
            return false;
        }
        match self.op {
            HarvestOp::Add | HarvestOp::Augment => !item.is_full(),
            _ => true,
        }
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply {}", self.display_name);
        }
        let tag = self.target.as_tag();
        match self.op {
            HarvestOp::Remove => harvest_remove(item, tag, db, &self.display_name),
            HarvestOp::Add => harvest_add(item, tag, db, &self.display_name, rng),
            HarvestOp::RemoveAdd => harvest_remove_add(item, tag, db, &self.display_name, rng),
            HarvestOp::Augment => harvest_augment(item, tag, db, &self.display_name, rng),
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Returns (prefix_indices, suffix_indices) of mods on `item` whose DB entry
/// contains `tag` in the mod's `tags` field.
fn matching_mod_indices(item: &ItemState, tag: &str, db: &GameData) -> (Vec<usize>, Vec<usize>) {
    let prefix_matches = item
        .prefixes
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            db.mods
                .get(&m.mod_id)
                .is_some_and(|md| md.tags.iter().any(|t| t == tag))
        })
        .map(|(i, _)| i)
        .collect();
    let suffix_matches = item
        .suffixes
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            db.mods
                .get(&m.mod_id)
                .is_some_and(|md| md.tags.iter().any(|t| t == tag))
        })
        .map(|(i, _)| i)
        .collect();
    (prefix_matches, suffix_matches)
}

fn harvest_remove(
    item: &ItemState,
    tag: &str,
    db: &GameData,
    name: &str,
) -> Result<Vec<(ItemState, f64)>> {
    let (prefix_matches, suffix_matches) = matching_mod_indices(item, tag, db);
    let total = prefix_matches.len() + suffix_matches.len();
    if total == 0 {
        bail!("{}: no '{}' mod found on item", name, tag);
    }
    let prob = 1.0 / total as f64;
    let mut outcomes = Vec::with_capacity(total);
    for i in prefix_matches {
        let mut next = item.clone();
        next.prefixes.remove(i);
        outcomes.push((next, prob));
    }
    for i in suffix_matches {
        let mut next = item.clone();
        next.suffixes.remove(i);
        outcomes.push((next, prob));
    }
    Ok(outcomes)
}

fn harvest_add(
    item: &ItemState,
    tag: &str,
    db: &GameData,
    name: &str,
    rng: &mut dyn RngCore,
) -> Result<Vec<(ItemState, f64)>> {
    let pool = eligible_mods_harvest_tag(item, tag, db);
    let total_weight: u64 = pool.iter().map(|(_, _, w)| *w as u64).sum();
    if total_weight == 0 {
        bail!("{}: no eligible '{}' mods to add", name, tag);
    }
    let outcomes = pool
        .iter()
        .map(|(mod_id, picked, weight)| {
            let mut next = item.clone();
            let rolls = random_rolls_pub(&picked.stats, rng);
            let modifier = Modifier {
                mod_id: mod_id.to_string(),
                generation_type: picked.generation_type.clone(),
                rolls,
            };
            match picked.generation_type {
                GenerationType::Prefix => next.prefixes.push(modifier),
                GenerationType::Suffix => next.suffixes.push(modifier),
                _ => {} // eligible_mods_harvest_tag only returns prefix/suffix
            }
            (next, *weight as f64 / total_weight as f64)
        })
        .collect();
    Ok(outcomes)
}

fn harvest_augment(
    item: &ItemState,
    tag: &str,
    db: &GameData,
    name: &str,
    rng: &mut dyn RngCore,
) -> Result<Vec<(ItemState, f64)>> {
    let (pref, suf) = matching_mod_indices(item, tag, db);
    if !pref.is_empty() || !suf.is_empty() {
        bail!(
            "{}: item already has a '{}' mod; use Add or RemoveAdd instead",
            name,
            tag
        );
    }
    harvest_add(item, tag, db, name, rng)
}

fn harvest_remove_add(
    item: &ItemState,
    tag: &str,
    db: &GameData,
    name: &str,
    rng: &mut dyn RngCore,
) -> Result<Vec<(ItemState, f64)>> {
    let (prefix_matches, suffix_matches) = matching_mod_indices(item, tag, db);
    let n_remove = prefix_matches.len() + suffix_matches.len();
    if n_remove == 0 {
        bail!("{}: no '{}' mod to remove", name, tag);
    }

    let remove_prob = 1.0 / n_remove as f64;
    let mut outcomes: Vec<(ItemState, f64)> = Vec::new();

    // Enumerate every (remove choice × add choice) pair. The removed slot is
    // rebuilt as `removed`, then each eligible add lands on a clone of it.
    let mut removal_bases: Vec<ItemState> = Vec::with_capacity(n_remove);
    for &i in &prefix_matches {
        let mut removed = item.clone();
        removed.prefixes.remove(i);
        removal_bases.push(removed);
    }
    for &i in &suffix_matches {
        let mut removed = item.clone();
        removed.suffixes.remove(i);
        removal_bases.push(removed);
    }

    for removed in &removal_bases {
        let pool = eligible_mods_harvest_tag(removed, tag, db);
        let total_weight: u64 = pool.iter().map(|(_, _, w)| *w as u64).sum();
        if total_weight == 0 {
            continue;
        }
        for (mod_id, picked, weight) in &pool {
            let mut next = removed.clone();
            let rolls = random_rolls_pub(&picked.stats, rng);
            let modifier = Modifier {
                mod_id: mod_id.to_string(),
                generation_type: picked.generation_type.clone(),
                rolls,
            };
            match picked.generation_type {
                GenerationType::Prefix => next.prefixes.push(modifier),
                GenerationType::Suffix => next.suffixes.push(modifier),
                _ => {}
            }
            outcomes.push((next, remove_prob * (*weight as f64 / total_weight as f64)));
        }
    }

    if outcomes.is_empty() {
        bail!(
            "{}: removal succeeded but no '{}' mods available to add",
            name,
            tag
        );
    }
    // Normalise in case some remove-branches were skipped (no add candidates).
    let sum: f64 = outcomes.iter().map(|(_, p)| p).sum();
    for (_, p) in &mut outcomes {
        *p /= sum;
    }
    Ok(outcomes)
}
