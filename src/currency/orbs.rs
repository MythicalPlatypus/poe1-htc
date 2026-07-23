//! Basic currency orbs: Scouring, Transmutation, Alteration, Augmentation,
//! Regal, Alchemy, Chaos, Exalted, Annulment, Divine.
//!
//! Costs are rough chaos-equivalent market values and only need to be right
//! relative to each other; tune per league via a price source if desired.

use super::{CraftingMethod, MONTE_CARLO_SAMPLES};
use crate::data::mods::GenerationType;
use crate::data::GameData;
use crate::engine::mod_pool::{eligible_mods, random_rolls_pub, roll_mods};
use crate::item::modifier::Modifier;
use crate::item::{state::Rarity, ItemState};
use anyhow::{bail, Result};
use rand::RngCore;

/// Enumerate "add one random eligible mod" outcomes with exact weighted
/// probabilities. Shared by Exalted, Augmentation, and Regal — the three orbs
/// whose effect is exactly one weighted pick from the eligible pool.
fn enumerate_add_one(
    item: &ItemState,
    db: &GameData,
    rng: &mut dyn RngCore,
    orb_name: &str,
) -> Result<Vec<(ItemState, f64)>> {
    let pool = eligible_mods(item, &[], db);
    let total_weight: u64 = pool.iter().map(|(_, _, w)| *w as u64).sum();
    if total_weight == 0 {
        bail!("{orb_name}: no eligible mods for this item");
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
                _ => {} // eligible_mods only returns prefix/suffix
            }
            (next, *weight as f64 / total_weight as f64)
        })
        .collect();
    Ok(outcomes)
}

// ---------------------------------------------------------------------------
// Orb of Scouring
// ---------------------------------------------------------------------------

pub struct OrbOfScouring;

impl CraftingMethod for OrbOfScouring {
    fn name(&self) -> &str {
        "Orb of Scouring"
    }
    fn cost_chaos(&self) -> f64 {
        1.0
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable()
            && matches!(item.rarity, Rarity::Magic | Rarity::Rare)
            && item.fractured.is_empty()
    }

    fn apply(
        &self,
        item: &ItemState,
        _db: &GameData,
        _rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, _db) {
            bail!("Cannot apply Orb of Scouring");
        }
        let mut next = item.clone();
        next.rarity = Rarity::Normal;
        next.prefixes.clear();
        next.suffixes.clear();
        next.crafted_mod = None;
        Ok(vec![(next, 1.0)])
    }
}

// ---------------------------------------------------------------------------
// Orb of Transmutation
// ---------------------------------------------------------------------------

/// Normal → Magic with 1–2 random mods.
pub struct OrbOfTransmutation;

impl CraftingMethod for OrbOfTransmutation {
    fn name(&self) -> &str {
        "Orb of Transmutation"
    }
    fn cost_chaos(&self) -> f64 {
        0.05
    }
    // Monte Carlo sampling — weights are 1/N, not probabilities.
    fn weights_are_probabilities(&self) -> bool {
        false
    }
    // Retrying strictly needs a Scouring Orb first; the ~1c overhead is ignored.
    fn repeatable_on_failure(&self) -> bool {
        true
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Normal
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply Orb of Transmutation");
        }
        let prob = 1.0 / MONTE_CARLO_SAMPLES as f64;
        let mut outcomes = Vec::with_capacity(MONTE_CARLO_SAMPLES);
        for _ in 0..MONTE_CARLO_SAMPLES {
            let mut next = item.clone();
            next.rarity = Rarity::Magic;
            let count = rand::Rng::random_range(&mut *rng, 1..=2);
            roll_mods(&mut next, count, db, rng)?;
            outcomes.push((next, prob));
        }
        Ok(outcomes)
    }
}

// ---------------------------------------------------------------------------
// Orb of Alteration
// ---------------------------------------------------------------------------

/// Rerolls a Magic item with 1–2 new random mods.
/// Blocked when a crafted mod is present (conservative model of in-game rules).
pub struct OrbOfAlteration;

impl CraftingMethod for OrbOfAlteration {
    fn name(&self) -> &str {
        "Orb of Alteration"
    }
    fn cost_chaos(&self) -> f64 {
        0.1
    }
    // Monte Carlo sampling — weights are 1/N, not probabilities.
    fn weights_are_probabilities(&self) -> bool {
        false
    }
    fn repeatable_on_failure(&self) -> bool {
        true
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Magic && item.crafted_mod.is_none()
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply Orb of Alteration");
        }
        let prob = 1.0 / MONTE_CARLO_SAMPLES as f64;
        let mut outcomes = Vec::with_capacity(MONTE_CARLO_SAMPLES);
        for _ in 0..MONTE_CARLO_SAMPLES {
            let mut next = item.clone();
            next.prefixes.clear();
            next.suffixes.clear();
            let count = rand::Rng::random_range(&mut *rng, 1..=2);
            roll_mods(&mut next, count, db, rng)?;
            outcomes.push((next, prob));
        }
        Ok(outcomes)
    }
}

// ---------------------------------------------------------------------------
// Orb of Augmentation
// ---------------------------------------------------------------------------

/// Adds one random mod to a Magic item with an open affix slot.
pub struct OrbOfAugmentation;

impl CraftingMethod for OrbOfAugmentation {
    fn name(&self) -> &str {
        "Orb of Augmentation"
    }
    fn cost_chaos(&self) -> f64 {
        0.05
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Magic && !item.is_full()
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply Orb of Augmentation");
        }
        enumerate_add_one(item, db, rng, "Orb of Augmentation")
    }
}

// ---------------------------------------------------------------------------
// Regal Orb
// ---------------------------------------------------------------------------

/// Upgrades a Magic item to Rare, adding one random mod.
pub struct RegalOrb;

impl CraftingMethod for RegalOrb {
    fn name(&self) -> &str {
        "Regal Orb"
    }
    fn cost_chaos(&self) -> f64 {
        1.0
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Magic
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply Regal Orb");
        }
        // Upgrade rarity first so the added mod rolls against Rare capacity (3/3).
        let mut upgraded = item.clone();
        upgraded.rarity = Rarity::Rare;
        enumerate_add_one(&upgraded, db, rng, "Regal Orb")
    }
}

// ---------------------------------------------------------------------------
// Orb of Alchemy
// ---------------------------------------------------------------------------

pub struct OrbOfAlchemy;

impl CraftingMethod for OrbOfAlchemy {
    fn name(&self) -> &str {
        "Orb of Alchemy"
    }
    fn cost_chaos(&self) -> f64 {
        2.0
    }
    // Monte Carlo sampling — weights are 1/N, not probabilities.
    fn weights_are_probabilities(&self) -> bool {
        false
    }
    // Retrying strictly needs a Scouring Orb first; the ~1c overhead is ignored.
    fn repeatable_on_failure(&self) -> bool {
        true
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Normal
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply Orb of Alchemy");
        }
        let prob = 1.0 / MONTE_CARLO_SAMPLES as f64;
        let mut outcomes = Vec::with_capacity(MONTE_CARLO_SAMPLES);
        for _ in 0..MONTE_CARLO_SAMPLES {
            let mut next = item.clone();
            next.rarity = Rarity::Rare;
            let count = rand::Rng::random_range(&mut *rng, 4..=6);
            roll_mods(&mut next, count, db, rng)?;
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
    fn name(&self) -> &str {
        "Chaos Orb"
    }
    fn cost_chaos(&self) -> f64 {
        1.0
    }
    // Monte Carlo sampling — weights are 1/N, not probabilities.
    fn weights_are_probabilities(&self) -> bool {
        false
    }
    fn repeatable_on_failure(&self) -> bool {
        true
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Rare
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply Chaos Orb");
        }
        let prob = 1.0 / MONTE_CARLO_SAMPLES as f64;
        let mut outcomes = Vec::with_capacity(MONTE_CARLO_SAMPLES);
        for _ in 0..MONTE_CARLO_SAMPLES {
            let mut next = item.clone();
            next.prefixes.clear();
            next.suffixes.clear();
            next.crafted_mod = None;
            let count = rand::Rng::random_range(&mut *rng, 4..=6);
            roll_mods(&mut next, count, db, rng)?;
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
    fn name(&self) -> &str {
        "Exalted Orb"
    }
    fn cost_chaos(&self) -> f64 {
        100.0
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Rare && !item.is_full()
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply Exalted Orb");
        }
        enumerate_add_one(item, db, rng, "Exalted Orb")
    }
}

// ---------------------------------------------------------------------------
// Orb of Annulment
// ---------------------------------------------------------------------------

pub struct OrbOfAnnulment;

impl CraftingMethod for OrbOfAnnulment {
    fn name(&self) -> &str {
        "Orb of Annulment"
    }
    fn cost_chaos(&self) -> f64 {
        40.0
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Rare && item.mod_count() > 0
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        _rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply Orb of Annulment");
        }
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

// ---------------------------------------------------------------------------
// Divine Orb
// ---------------------------------------------------------------------------

/// Rerolls the stat values of all mods on the item (explicit, fractured, and
/// crafted) without changing which mods are present.
pub struct DivineOrb;

impl CraftingMethod for DivineOrb {
    fn name(&self) -> &str {
        "Divine Orb"
    }
    fn cost_chaos(&self) -> f64 {
        150.0
    }
    // Monte Carlo sampling — weights are 1/N, not probabilities.
    fn weights_are_probabilities(&self) -> bool {
        false
    }
    fn repeatable_on_failure(&self) -> bool {
        true
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable()
            && matches!(item.rarity, Rarity::Magic | Rarity::Rare)
            && (item.mod_count() > 0 || !item.fractured.is_empty())
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply Divine Orb");
        }
        let prob = 1.0 / MONTE_CARLO_SAMPLES as f64;
        let mut outcomes = Vec::with_capacity(MONTE_CARLO_SAMPLES);
        for _ in 0..MONTE_CARLO_SAMPLES {
            let mut next = item.clone();
            reroll_values(&mut next.prefixes, db, rng);
            reroll_values(&mut next.suffixes, db, rng);
            reroll_values(&mut next.fractured, db, rng);
            if let Some(c) = next.crafted_mod.as_mut() {
                reroll_one(c, db, rng);
            }
            outcomes.push((next, prob));
        }
        Ok(outcomes)
    }
}

/// Reroll the stat values of each modifier from its DB stat ranges.
/// Modifiers whose mod ID is missing from the DB keep their current values.
fn reroll_values(mods_list: &mut [Modifier], db: &GameData, rng: &mut dyn RngCore) {
    for m in mods_list {
        reroll_one(m, db, rng);
    }
}

fn reroll_one(m: &mut Modifier, db: &GameData, rng: &mut dyn RngCore) {
    if let Some(md) = db.mods.get(&m.mod_id) {
        m.rolls = random_rolls_pub(&md.stats, rng);
    }
}
