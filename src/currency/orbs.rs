//! Basic currency orbs: Scouring, Transmutation, Alteration, Augmentation,
//! Regal, Alchemy, Chaos, Exalted, Annulment, Divine.
//!
//! Costs are rough chaos-equivalent market values and only need to be right
//! relative to each other; tune per league via a price source if desired.

use super::{random_rare_affix_count, CraftingMethod, RerollKind, MONTE_CARLO_SAMPLES};
use crate::data::mods::{GenerationType, ModStat};
use crate::data::GameData;
use crate::engine::mod_pool::{eligible_mods, random_rolls_pub, roll_mods};
use crate::item::modifier::Modifier;
use crate::item::{state::Rarity, ItemState};
use anyhow::{bail, Result};
use rand::RngCore;

/// Enumerate every eligible mod with its exact selection weight, sampling the
/// mod's numeric rolls once. This is stratified sampling, not an exact
/// enumeration of concrete successor states.
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
    let mut outcomes = Vec::with_capacity(pool.len());
    for (mod_id, picked, weight) in pool {
        validate_roll_ranges(mod_id, &picked.stats)?;
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
            _ => bail!("{orb_name}: eligible pool contained non-affix mod {mod_id}"),
        }
        outcomes.push((next, weight as f64 / total_weight as f64));
    }
    Ok(outcomes)
}

fn validate_roll_ranges(mod_id: &str, stats: &[ModStat]) -> Result<()> {
    if let Some(stat) = stats.iter().find(|stat| stat.min > stat.max) {
        bail!(
            "mod {mod_id} has invalid range for stat {}: {} > {}",
            stat.id,
            stat.min,
            stat.max
        );
    }
    Ok(())
}

fn fractured_affix_count(item: &ItemState) -> usize {
    item.fractured
        .iter()
        .filter(|modifier| {
            matches!(
                modifier.generation_type,
                GenerationType::Prefix | GenerationType::Suffix
            )
        })
        .count()
}

fn removable_affix_count(item: &ItemState) -> usize {
    item.prefixes.len() + item.suffixes.len() + usize::from(item.crafted_mod.is_some())
}

fn roll_to_total_affixes(
    item: &mut ItemState,
    target_count: usize,
    db: &GameData,
    rng: &mut dyn RngCore,
) -> Result<()> {
    let count = target_count.saturating_sub(fractured_affix_count(item));
    roll_mods(item, count, db, rng)
}

fn has_eligible_mod_after_rarity_change(item: &ItemState, rarity: Rarity, db: &GameData) -> bool {
    let mut candidate = item.clone();
    candidate.rarity = rarity;
    !eligible_mods(&candidate, &[], db).is_empty()
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
        if !item.is_craftable() || !matches!(item.rarity, Rarity::Magic | Rarity::Rare) {
            return false;
        }
        let resulting_rarity = match item.fractured.len() {
            0 => Rarity::Normal,
            1 => Rarity::Magic,
            _ => Rarity::Rare,
        };
        removable_affix_count(item) > 0 || item.rarity != resulting_rarity
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
        next.rarity = match next.fractured.len() {
            0 => Rarity::Normal,
            1 => Rarity::Magic,
            _ => Rarity::Rare,
        };
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
    fn reroll_initializer_kind(&self) -> Option<RerollKind> {
        Some(RerollKind::MagicExplicit)
    }
    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        item.is_craftable()
            && item.rarity == Rarity::Normal
            && item.mod_count() == 0
            && item.fractured.is_empty()
            && has_eligible_mod_after_rarity_change(item, Rarity::Magic, db)
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
            roll_to_total_affixes(&mut next, count, db, rng)?;
            outcomes.push((next, prob));
        }
        Ok(outcomes)
    }
}

// ---------------------------------------------------------------------------
// Orb of Alteration
// ---------------------------------------------------------------------------

/// Rerolls a Magic item with 1–2 total affixes, preserving fractured mods.
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
    fn reroll_kind(&self) -> Option<RerollKind> {
        Some(RerollKind::MagicExplicit)
    }
    fn consumes_reroll_initializer(&self) -> bool {
        true
    }

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        if !item.is_craftable() || item.rarity != Rarity::Magic {
            return false;
        }
        let mut rerolled = item.clone();
        rerolled.prefixes.clear();
        rerolled.suffixes.clear();
        rerolled.crafted_mod = None;
        removable_affix_count(item) > 0 || !eligible_mods(&rerolled, &[], db).is_empty()
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
            next.crafted_mod = None;
            let count = rand::Rng::random_range(&mut *rng, 1..=2);
            roll_to_total_affixes(&mut next, count, db, rng)?;
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
    fn weights_are_probabilities(&self) -> bool {
        false
    }

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        item.is_craftable()
            && item.rarity == Rarity::Magic
            && !eligible_mods(item, &[], db).is_empty()
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
    fn weights_are_probabilities(&self) -> bool {
        false
    }

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        item.is_craftable()
            && item.rarity == Rarity::Magic
            && has_eligible_mod_after_rarity_change(item, Rarity::Rare, db)
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
    fn reroll_initializer_kind(&self) -> Option<RerollKind> {
        Some(RerollKind::RareExplicit)
    }
    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        item.is_craftable()
            && item.rarity == Rarity::Normal
            && item.mod_count() == 0
            && item.fractured.is_empty()
            && has_eligible_mod_after_rarity_change(item, Rarity::Rare, db)
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
            let count = random_rare_affix_count(rng);
            roll_to_total_affixes(&mut next, count, db, rng)?;
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
    fn reroll_kind(&self) -> Option<RerollKind> {
        Some(RerollKind::RareExplicit)
    }
    fn consumes_reroll_initializer(&self) -> bool {
        true
    }

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        if !item.is_craftable() || item.rarity != Rarity::Rare {
            return false;
        }
        let mut rerolled = item.clone();
        rerolled.prefixes.clear();
        rerolled.suffixes.clear();
        rerolled.crafted_mod = None;
        removable_affix_count(item) > 0 || !eligible_mods(&rerolled, &[], db).is_empty()
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
            let count = random_rare_affix_count(rng);
            roll_to_total_affixes(&mut next, count, db, rng)?;
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
    fn weights_are_probabilities(&self) -> bool {
        false
    }

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        item.is_craftable()
            && item.rarity == Rarity::Rare
            && !eligible_mods(item, &[], db).is_empty()
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
        item.is_craftable()
            && matches!(item.rarity, Rarity::Magic | Rarity::Rare)
            && removable_affix_count(item) > 0
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
        let total = removable_affix_count(item) as f64;
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

/// Rerolls the stat values of non-fractured explicit and crafted mods without
/// changing which mods are present.
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

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        item.is_craftable()
            && matches!(item.rarity, Rarity::Magic | Rarity::Rare | Rarity::Unique)
            && item
                .prefixes
                .iter()
                .chain(item.suffixes.iter())
                .chain(item.crafted_mod.iter())
                .filter_map(|modifier| db.mods.get(&modifier.mod_id))
                .any(|modifier| modifier.stats.iter().any(|stat| stat.min < stat.max))
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
            reroll_values(&mut next.prefixes, db, rng)?;
            reroll_values(&mut next.suffixes, db, rng)?;
            if let Some(c) = next.crafted_mod.as_mut() {
                reroll_one(c, db, rng)?;
            }
            outcomes.push((next, prob));
        }
        Ok(outcomes)
    }
}

/// Reroll the stat values of each modifier from its DB stat ranges.
/// Modifiers whose mod ID is missing from the DB keep their current values.
fn reroll_values(mods_list: &mut [Modifier], db: &GameData, rng: &mut dyn RngCore) -> Result<()> {
    for m in mods_list {
        reroll_one(m, db, rng)?;
    }
    Ok(())
}

fn reroll_one(m: &mut Modifier, db: &GameData, rng: &mut dyn RngCore) -> Result<()> {
    if let Some(md) = db.mods.get(&m.mod_id) {
        validate_roll_ranges(&m.mod_id, &md.stats)?;
        m.rolls = random_rolls_pub(&md.stats, rng);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rand::{rngs::StdRng, SeedableRng};

    use super::*;
    use crate::data::mods::{Domain, Mod, SpawnWeight};
    use crate::item::modifier::StatRoll;

    fn test_mod(generation_type: GenerationType, group: &str) -> Mod {
        Mod {
            name: group.to_string(),
            generation_type,
            required_level: 1,
            stats: vec![ModStat {
                id: format!("stat_{group}"),
                min: 1,
                max: 10,
            }],
            spawn_weights: vec![SpawnWeight {
                tag: "sword".to_string(),
                weight: 100,
            }],
            generation_weights: Vec::new(),
            adds_tags: Vec::new(),
            tags: Vec::new(),
            domain: Domain::Item,
            mod_type: group.to_string(),
            groups: vec![group.to_string()],
            is_essence_only: false,
            text: None,
        }
    }

    fn varied_db() -> GameData {
        let mut mods = HashMap::new();
        for i in 0..4 {
            mods.insert(
                format!("P{i}"),
                test_mod(GenerationType::Prefix, &format!("Prefix{i}")),
            );
            mods.insert(
                format!("S{i}"),
                test_mod(GenerationType::Suffix, &format!("Suffix{i}")),
            );
        }
        GameData::new(mods, HashMap::new())
    }

    fn empty_db() -> GameData {
        GameData::new(HashMap::new(), HashMap::new())
    }

    fn sword(rarity: Rarity) -> ItemState {
        let mut item = ItemState::new_base("sword", vec!["sword".to_string()], 84);
        item.rarity = rarity;
        item
    }

    fn marker(mod_id: &str, generation_type: GenerationType, value: i32) -> Modifier {
        Modifier {
            mod_id: mod_id.to_string(),
            generation_type,
            rolls: vec![StatRoll {
                stat_id: format!("stat_{mod_id}"),
                value,
            }],
        }
    }

    fn rng() -> StdRng {
        StdRng::seed_from_u64(0xA11CE)
    }

    fn assert_weight_sum(outcomes: &[(ItemState, f64)]) {
        let sum: f64 = outcomes.iter().map(|(_, weight)| weight).sum();
        assert!((sum - 1.0).abs() < 1e-12, "weights sum to {sum}");
        assert!(outcomes
            .iter()
            .all(|(_, weight)| weight.is_finite() && *weight > 0.0));
    }

    #[test]
    fn scour_preserves_fractures_and_uses_surviving_rarity() -> Result<()> {
        let db = varied_db();
        let mut one_fracture = sword(Rarity::Rare);
        one_fracture
            .fractured
            .push(marker("P0", GenerationType::Prefix, 7));
        one_fracture
            .prefixes
            .push(marker("P1", GenerationType::Prefix, 4));
        one_fracture.crafted_mod = Some(marker("S0", GenerationType::Suffix, 5));

        let outcomes = OrbOfScouring.apply(&one_fracture, &db, &mut rng())?;
        let next = &outcomes[0].0;
        assert_eq!(outcomes[0].1, 1.0);
        assert_eq!(next.rarity, Rarity::Magic);
        assert_eq!(next.fractured, one_fracture.fractured);
        assert!(next.prefixes.is_empty());
        assert!(next.suffixes.is_empty());
        assert!(next.crafted_mod.is_none());

        let mut two_fractures = sword(Rarity::Rare);
        two_fractures
            .fractured
            .push(marker("P0", GenerationType::Prefix, 7));
        two_fractures
            .fractured
            .push(marker("S0", GenerationType::Suffix, 8));
        two_fractures
            .prefixes
            .push(marker("P1", GenerationType::Prefix, 4));
        let outcomes = OrbOfScouring.apply(&two_fractures, &db, &mut rng())?;
        assert_eq!(outcomes[0].0.rarity, Rarity::Rare);
        assert_eq!(outcomes[0].0.fractured, two_fractures.fractured);
        Ok(())
    }

    #[test]
    fn alteration_removes_craft_and_counts_preserved_fracture() -> Result<()> {
        let db = varied_db();
        let mut item = sword(Rarity::Magic);
        item.fractured.push(marker("P0", GenerationType::Prefix, 7));
        item.crafted_mod = Some(marker("S0", GenerationType::Suffix, 5));

        assert!(OrbOfAlteration.can_apply(&item, &db));
        let outcomes = OrbOfAlteration.apply(&item, &db, &mut rng())?;
        assert_weight_sum(&outcomes);
        for (next, _) in outcomes {
            assert_eq!(next.fractured, item.fractured);
            assert!(next.crafted_mod.is_none());
            let total = next.prefixes.len() + next.suffixes.len() + next.fractured.len();
            assert!((1..=2).contains(&total));
            assert!(next.prefix_count() <= 1);
            assert!(next.suffix_count() <= 1);
        }
        Ok(())
    }

    #[test]
    fn chaos_preserves_fracture_and_never_overfills_total_affixes() -> Result<()> {
        let db = varied_db();
        let mut item = sword(Rarity::Rare);
        item.fractured.push(marker("P0", GenerationType::Prefix, 7));
        item.prefixes.push(marker("P1", GenerationType::Prefix, 4));
        item.crafted_mod = Some(marker("S0", GenerationType::Suffix, 5));

        let outcomes = ChaosOrb.apply(&item, &db, &mut rng())?;
        assert_weight_sum(&outcomes);
        for (next, _) in outcomes {
            assert_eq!(next.fractured, item.fractured);
            assert!(next.crafted_mod.is_none());
            let total = next.prefixes.len() + next.suffixes.len() + next.fractured.len();
            assert!((4..=6).contains(&total), "chaos produced {total} affixes");
            assert!(next.prefix_count() <= 3);
            assert!(next.suffix_count() <= 3);
        }
        Ok(())
    }

    #[test]
    fn annulment_applies_to_magic_but_never_targets_fractures() -> Result<()> {
        let db = varied_db();
        let mut item = sword(Rarity::Magic);
        item.fractured.push(marker("P0", GenerationType::Prefix, 7));
        item.crafted_mod = Some(marker("S0", GenerationType::Suffix, 5));

        assert!(OrbOfAnnulment.can_apply(&item, &db));
        let outcomes = OrbOfAnnulment.apply(&item, &db, &mut rng())?;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].1, 1.0);
        assert_eq!(outcomes[0].0.rarity, Rarity::Magic);
        assert_eq!(outcomes[0].0.fractured, item.fractured);
        assert!(outcomes[0].0.crafted_mod.is_none());
        Ok(())
    }

    #[test]
    fn divine_leaves_fractures_fixed_and_rejects_fracture_only_noop() -> Result<()> {
        let db = varied_db();
        let mut item = sword(Rarity::Rare);
        item.fractured
            .push(marker("P0", GenerationType::Prefix, 999));
        item.prefixes
            .push(marker("P1", GenerationType::Prefix, 999));
        item.crafted_mod = Some(marker("S0", GenerationType::Suffix, 999));

        let outcomes = DivineOrb.apply(&item, &db, &mut rng())?;
        assert_weight_sum(&outcomes);
        for (next, _) in outcomes {
            assert_eq!(next.fractured[0].rolls[0].value, 999);
            assert!((1..=10).contains(&next.prefixes[0].rolls[0].value));
            let crafted = next
                .crafted_mod
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Divine removed crafted mod"))?;
            assert!((1..=10).contains(&crafted.rolls[0].value));
        }

        let mut fracture_only = sword(Rarity::Magic);
        fracture_only
            .fractured
            .push(marker("P0", GenerationType::Prefix, 999));
        assert!(!DivineOrb.can_apply(&fracture_only, &db));
        assert!(DivineOrb.apply(&fracture_only, &db, &mut rng()).is_err());
        Ok(())
    }

    #[test]
    fn applicability_rejects_empty_pools_and_state_identical_rerolls() {
        let db = empty_db();
        assert!(!OrbOfTransmutation.can_apply(&sword(Rarity::Normal), &db));
        assert!(!OrbOfAlchemy.can_apply(&sword(Rarity::Normal), &db));
        assert!(!OrbOfAugmentation.can_apply(&sword(Rarity::Magic), &db));
        assert!(!RegalOrb.can_apply(&sword(Rarity::Magic), &db));
        assert!(!ExaltedOrb.can_apply(&sword(Rarity::Rare), &db));
        assert!(!ChaosOrb.can_apply(&sword(Rarity::Rare), &db));

        let mut fracture_only = sword(Rarity::Magic);
        fracture_only
            .fractured
            .push(marker("P0", GenerationType::Prefix, 1));
        assert!(!OrbOfScouring.can_apply(&fracture_only, &db));
        assert!(!OrbOfAlteration.can_apply(&fracture_only, &db));
    }

    #[test]
    fn invalid_stat_range_returns_error_instead_of_panicking() {
        let mut invalid = test_mod(GenerationType::Prefix, "Invalid");
        invalid.stats[0].min = 10;
        invalid.stats[0].max = 1;
        let db = GameData::new(
            HashMap::from([("Invalid".to_string(), invalid)]),
            HashMap::new(),
        );
        let item = sword(Rarity::Rare);

        assert!(ExaltedOrb.can_apply(&item, &db));
        let error = ExaltedOrb.apply(&item, &db, &mut rng());
        assert!(error.is_err());
    }

    #[test]
    fn probability_and_retry_labels_match_execution_model() {
        assert!(!OrbOfTransmutation.weights_are_probabilities());
        assert!(!OrbOfAlteration.weights_are_probabilities());
        assert!(!OrbOfAlchemy.weights_are_probabilities());
        assert!(!ChaosOrb.weights_are_probabilities());
        assert!(!DivineOrb.weights_are_probabilities());
        assert!(!OrbOfAugmentation.weights_are_probabilities());
        assert!(!RegalOrb.weights_are_probabilities());
        assert!(!ExaltedOrb.weights_are_probabilities());
        assert!(OrbOfAnnulment.weights_are_probabilities());

        assert!(!OrbOfTransmutation.repeatable_on_failure());
        assert!(!OrbOfAlchemy.repeatable_on_failure());
        assert!(OrbOfAlteration.repeatable_on_failure());
        assert!(ChaosOrb.repeatable_on_failure());
        assert!(DivineOrb.repeatable_on_failure());
    }

    #[test]
    fn monte_carlo_orbs_are_repeatable_with_seeded_rng() -> Result<()> {
        let db = varied_db();
        let item = sword(Rarity::Rare);
        let first = ChaosOrb.apply(&item, &db, &mut rng())?;
        let second = ChaosOrb.apply(&item, &db, &mut rng())?;
        assert_eq!(format!("{first:?}"), format!("{second:?}"));
        Ok(())
    }
}
