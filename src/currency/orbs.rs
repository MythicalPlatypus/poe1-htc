//! Basic currency orbs: Orb of Transmutation, Alteration, Augmentation,
//! Regal, Chaos, Exalted, Annulment, Blessed, Divine, Scour, Alchemy.

use anyhow::{bail, Result};
use rand::{rng, Rng};
use crate::data::GameData;
use crate::item::{ItemState, state::Rarity};
use crate::engine::mod_pool::{eligible_mods, random_rolls_pub, roll_mods};
use crate::item::modifier::Modifier;
use crate::data::mods::GenerationType;
use super::{CraftingMethod, MONTE_CARLO_SAMPLES};

// ---------------------------------------------------------------------------
// Orb of Scouring
// ---------------------------------------------------------------------------

pub struct OrbOfScouring;

impl CraftingMethod for OrbOfScouring {
    fn name(&self) -> &str { "Orb of Scouring" }
    fn cost_chaos(&self) -> f64 { 1.0 }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable()
            && matches!(item.rarity, Rarity::Magic | Rarity::Rare)
            && item.fractured.is_empty()
    }

    fn apply(&self, item: &ItemState, _db: &GameData) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, _db) { bail!("Cannot apply Orb of Scouring"); }
        let mut next = item.clone();
        next.rarity = Rarity::Normal;
        next.prefixes.clear();
        next.suffixes.clear();
        next.crafted_mod = None;
        Ok(vec![(next, 1.0)])
    }
}

// ---------------------------------------------------------------------------
// Orb of Alchemy
// ---------------------------------------------------------------------------

pub struct OrbOfAlchemy;

impl CraftingMethod for OrbOfAlchemy {
    fn name(&self) -> &str { "Orb of Alchemy" }
    fn cost_chaos(&self) -> f64 { 2.0 }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Normal
    }

    fn apply(&self, item: &ItemState, db: &GameData) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) { bail!("Cannot apply Orb of Alchemy"); }
        let prob = 1.0 / MONTE_CARLO_SAMPLES as f64;
        let mut rng = rng();
        let mut outcomes = Vec::with_capacity(MONTE_CARLO_SAMPLES);
        for _ in 0..MONTE_CARLO_SAMPLES {
            let mut next = item.clone();
            next.rarity = Rarity::Rare;
            let count = rng.random_range(4..=6);
            roll_mods(&mut next, count, db, &mut rng)?;
            outcomes.push((next, prob));
        }
        Ok(outcomes)
    }
}

// ---------------------------------------------------------------------------
// Chaos Orb
// ---------------------------------------------------------------------------

pub struct ChaosOrb;

impl CraftingMethod for ChaosOrb {
    fn name(&self) -> &str { "Chaos Orb" }
    fn cost_chaos(&self) -> f64 { 1.0 }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Rare
    }

    fn apply(&self, item: &ItemState, db: &GameData) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) { bail!("Cannot apply Chaos Orb"); }
        let prob = 1.0 / MONTE_CARLO_SAMPLES as f64;
        let mut rng = rng();
        let mut outcomes = Vec::with_capacity(MONTE_CARLO_SAMPLES);
        for _ in 0..MONTE_CARLO_SAMPLES {
            let mut next = item.clone();
            next.prefixes.clear();
            next.suffixes.clear();
            next.crafted_mod = None;
            let count = rng.random_range(4..=6);
            roll_mods(&mut next, count, db, &mut rng)?;
            outcomes.push((next, prob));
        }
        Ok(outcomes)
    }
}

// ---------------------------------------------------------------------------
// Exalted Orb
// ---------------------------------------------------------------------------

pub struct ExaltedOrb;

impl CraftingMethod for ExaltedOrb {
    fn name(&self) -> &str { "Exalted Orb" }
    fn cost_chaos(&self) -> f64 { 100.0 }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Rare && !item.is_full()
    }

    fn apply(&self, item: &ItemState, db: &GameData) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) { bail!("Cannot apply Exalted Orb"); }
        let pool = eligible_mods(item, &[], db);
        let total_weight: u64 = pool.iter().map(|(_, _, w)| *w as u64).sum();
        if total_weight == 0 {
            bail!("Exalted Orb: no eligible mods for this item");
        }
        let mut rng = rng();
        // Enumerate all possible outcomes with exact weighted probabilities.
        let outcomes = pool.iter().map(|(mod_id, picked, weight)| {
            let mut next = item.clone();
            let rolls = random_rolls_pub(&picked.stats, &mut rng);
            let modifier = Modifier {
                mod_id: mod_id.to_string(),
                generation_type: picked.generation_type.clone(),
                rolls,
            };
            match picked.generation_type {
                GenerationType::Prefix => next.prefixes.push(modifier),
                GenerationType::Suffix => next.suffixes.push(modifier),
                _ => {} // eligible_mods only returns prefix/suffix
            }
            (next, *weight as f64 / total_weight as f64)
        }).collect();
        Ok(outcomes)
    }
}

// ---------------------------------------------------------------------------
// Orb of Annulment
// ---------------------------------------------------------------------------

pub struct OrbOfAnnulment;

impl CraftingMethod for OrbOfAnnulment {
    fn name(&self) -> &str { "Orb of Annulment" }
    fn cost_chaos(&self) -> f64 { 40.0 }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable()
            && item.rarity == Rarity::Rare
            && item.mod_count() > 0
    }

    fn apply(&self, item: &ItemState, db: &GameData) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) { bail!("Cannot apply Orb of Annulment"); }
        let total = item.mod_count() as f64;
        let mut outcomes: Vec<(ItemState, f64)> = Vec::new();

        // Each explicit mod (not crafted) is equally likely to be removed.
        for (i, _) in item.prefixes.iter().enumerate() {
            let mut next = item.clone();
            next.prefixes.remove(i);
            outcomes.push((next, 1.0 / total));
        }
        for (i, _) in item.suffixes.iter().enumerate() {
            let mut next = item.clone();
            next.suffixes.remove(i);
            outcomes.push((next, 1.0 / total));
        }
        if item.crafted_mod.is_some() {
            let mut next = item.clone();
            next.crafted_mod = None;
            outcomes.push((next, 1.0 / total));
        }

        Ok(outcomes)
    }
}
