//! Eldritch currency for Searing Exarch / Eater of Worlds influenced armour.
//!
//! The implicit mods only establish dominance. Eldritch Chaos, Exalted, and
//! Annulment operate on explicit prefixes or suffixes selected by that dominance.

use std::cmp::Ordering;

use anyhow::{bail, Result};
use rand::{Rng, RngCore};

use super::{CraftingMethod, MONTE_CARLO_SAMPLES};
use crate::data::mods::GenerationType;
use crate::data::GameData;
use crate::engine::mod_pool::{eligible_mods, random_rolls_pub, weighted_pick};
use crate::item::modifier::Modifier;
use crate::item::{state::Rarity, ItemState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EldritchGod {
    SearingExarch,
    EaterOfWorlds,
}

/// Item base tags that can receive Eldritch implicits.
const ELDRITCH_ITEM_TAGS: &[&str] = &["helmet", "gloves", "boots", "body_armour"];

fn item_supports_eldritch(item: &ItemState) -> bool {
    item.base_tags
        .iter()
        .any(|tag| ELDRITCH_ITEM_TAGS.contains(&tag.as_str()))
}

/// RePoE identifies an Eldritch implicit tier with a zero-weight
/// `no_tier_N_eldritch_implicit` spawn gate. Lower N is the stronger tier.
fn implicit_tier(modifier: &Modifier, db: &GameData) -> Option<u8> {
    db.mods
        .get(&modifier.mod_id)?
        .spawn_weights
        .iter()
        .find_map(|spawn_weight| {
            if spawn_weight.weight != 0 {
                return None;
            }
            spawn_weight
                .tag
                .strip_prefix("no_tier_")?
                .strip_suffix("_eldritch_implicit")?
                .parse()
                .ok()
        })
}

fn dominant_god(item: &ItemState, db: &GameData) -> Option<EldritchGod> {
    match (&item.exarch_implicit, &item.eater_implicit) {
        (Some(_), None) => Some(EldritchGod::SearingExarch),
        (None, Some(_)) => Some(EldritchGod::EaterOfWorlds),
        (Some(exarch), Some(eater)) => {
            let exarch_tier = implicit_tier(exarch, db)?;
            let eater_tier = implicit_tier(eater, db)?;
            match exarch_tier.cmp(&eater_tier) {
                Ordering::Less => Some(EldritchGod::SearingExarch),
                Ordering::Greater => Some(EldritchGod::EaterOfWorlds),
                Ordering::Equal => None,
            }
        }
        (None, None) => None,
    }
}

fn target_generation_type(god: EldritchGod) -> GenerationType {
    match god {
        EldritchGod::SearingExarch => GenerationType::Prefix,
        EldritchGod::EaterOfWorlds => GenerationType::Suffix,
    }
}

fn has_open_target(item: &ItemState, target: &GenerationType) -> bool {
    match target {
        GenerationType::Prefix => item.has_open_prefix(),
        GenerationType::Suffix => item.has_open_suffix(),
        _ => false,
    }
}

fn open_target_slots(item: &ItemState, target: &GenerationType) -> usize {
    match target {
        GenerationType::Prefix => item.max_prefixes().saturating_sub(item.prefix_count()),
        GenerationType::Suffix => item.max_suffixes().saturating_sub(item.suffix_count()),
        _ => 0,
    }
}

fn removable_target_count(item: &ItemState, target: &GenerationType) -> usize {
    let explicit_count = match target {
        GenerationType::Prefix => item.prefixes.len(),
        GenerationType::Suffix => item.suffixes.len(),
        _ => 0,
    };
    explicit_count
        + usize::from(
            item.crafted_mod
                .as_ref()
                .is_some_and(|modifier| &modifier.generation_type == target),
        )
}

fn clear_target_modifiers(item: &mut ItemState, target: &GenerationType) {
    match target {
        GenerationType::Prefix => item.prefixes.clear(),
        GenerationType::Suffix => item.suffixes.clear(),
        _ => return,
    }
    if item
        .crafted_mod
        .as_ref()
        .is_some_and(|modifier| &modifier.generation_type == target)
    {
        item.crafted_mod = None;
    }
}

fn target_pool<'a>(
    item: &ItemState,
    extra_tags: &[String],
    target: &GenerationType,
    db: &'a GameData,
) -> Vec<(&'a str, &'a crate::data::mods::Mod, u32)> {
    eligible_mods(item, extra_tags, db)
        .into_iter()
        .filter(|(_, modifier, _)| &modifier.generation_type == target)
        .collect()
}

fn add_target_modifier(
    item: &mut ItemState,
    target: &GenerationType,
    db: &GameData,
    rng: &mut dyn RngCore,
    extra_tags: &mut Vec<String>,
) -> bool {
    let pool = target_pool(item, extra_tags, target, db);
    let Some((mod_id, picked)) = weighted_pick(&pool, rng) else {
        return false;
    };
    let modifier = Modifier {
        mod_id: mod_id.to_string(),
        generation_type: picked.generation_type.clone(),
        rolls: random_rolls_pub(&picked.stats, rng),
    };
    match target {
        GenerationType::Prefix => item.prefixes.push(modifier),
        GenerationType::Suffix => item.suffixes.push(modifier),
        _ => return false,
    }
    extra_tags.extend(picked.adds_tags.iter().cloned());
    true
}

fn base_can_apply(
    item: &ItemState,
    db: &GameData,
    rarity_allowed: bool,
    expected_dominance: EldritchGod,
) -> bool {
    item.is_craftable()
        && rarity_allowed
        && item_supports_eldritch(item)
        && dominant_god(item, db) == Some(expected_dominance)
}

// Eldritch Chaos Orb

/// Rerolls prefixes under Exarch dominance or suffixes under Eater dominance.
///
/// `god` selects the dominance branch represented by this configured method.
#[derive(Debug, Clone, Copy)]
pub struct EldritchChaosOrb {
    pub god: EldritchGod,
}

impl CraftingMethod for EldritchChaosOrb {
    fn name(&self) -> &str {
        match self.god {
            EldritchGod::SearingExarch => "Eldritch Chaos Orb (Exarch)",
            EldritchGod::EaterOfWorlds => "Eldritch Chaos Orb (Eater)",
        }
    }

    fn cost_chaos(&self) -> f64 {
        5.0
    }

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        if !base_can_apply(item, db, item.rarity == Rarity::Rare, self.god) {
            return false;
        }

        let target = target_generation_type(self.god);
        let mut cleared = item.clone();
        clear_target_modifiers(&mut cleared, &target);
        has_open_target(&cleared, &target) && !target_pool(&cleared, &[], &target, db).is_empty()
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply {}", self.name());
        }

        let target = target_generation_type(self.god);
        let sample_weight = 1.0 / MONTE_CARLO_SAMPLES as f64;
        let mut outcomes = Vec::with_capacity(MONTE_CARLO_SAMPLES);

        for _ in 0..MONTE_CARLO_SAMPLES {
            let mut next = item.clone();
            clear_target_modifiers(&mut next, &target);

            // RePoE supplies mod weights, but not the game's affix-count odds.
            // Sample every legal target-side count without presenting the
            // resulting sample weights as exact in-game probabilities.
            let add_count = rng.random_range(0..=open_target_slots(&next, &target));
            let mut extra_tags = Vec::new();
            for _ in 0..add_count {
                if !add_target_modifier(&mut next, &target, db, rng, &mut extra_tags) {
                    break;
                }
            }
            outcomes.push((next, sample_weight));
        }

        Ok(outcomes)
    }

    fn weights_are_probabilities(&self) -> bool {
        false
    }

    fn repeatable_on_failure(&self) -> bool {
        true
    }
}

// Eldritch Exalted Orb

/// Adds a prefix under Exarch dominance or a suffix under Eater dominance.
#[derive(Debug, Clone, Copy)]
pub struct EldritchExaltedOrb {
    pub god: EldritchGod,
}

impl CraftingMethod for EldritchExaltedOrb {
    fn name(&self) -> &str {
        match self.god {
            EldritchGod::SearingExarch => "Eldritch Exalted Orb (Exarch)",
            EldritchGod::EaterOfWorlds => "Eldritch Exalted Orb (Eater)",
        }
    }

    fn cost_chaos(&self) -> f64 {
        20.0
    }

    fn weights_are_probabilities(&self) -> bool {
        false
    }

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        if !base_can_apply(item, db, item.rarity == Rarity::Rare, self.god) {
            return false;
        }
        let target = target_generation_type(self.god);
        has_open_target(item, &target) && !target_pool(item, &[], &target, db).is_empty()
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply {}", self.name());
        }

        let target = target_generation_type(self.god);
        let pool = target_pool(item, &[], &target, db);
        let total_weight: u64 = pool.iter().map(|(_, _, weight)| *weight as u64).sum();
        if total_weight == 0 {
            bail!("{}: no eligible explicit modifiers", self.name());
        }

        let outcomes = pool
            .iter()
            .map(|(mod_id, picked, weight)| {
                let mut next = item.clone();
                let modifier = Modifier {
                    mod_id: mod_id.to_string(),
                    generation_type: picked.generation_type.clone(),
                    rolls: random_rolls_pub(&picked.stats, rng),
                };
                match target {
                    GenerationType::Prefix => next.prefixes.push(modifier),
                    GenerationType::Suffix => next.suffixes.push(modifier),
                    _ => {}
                }
                (next, *weight as f64 / total_weight as f64)
            })
            .collect();

        Ok(outcomes)
    }
}

// Eldritch Orb of Annulment

/// Removes a prefix under Exarch dominance or a suffix under Eater dominance.
#[derive(Debug, Clone, Copy)]
pub struct EldritchOrbOfAnnulment {
    pub god: EldritchGod,
}

impl CraftingMethod for EldritchOrbOfAnnulment {
    fn name(&self) -> &str {
        match self.god {
            EldritchGod::SearingExarch => "Eldritch Orb of Annulment (Exarch)",
            EldritchGod::EaterOfWorlds => "Eldritch Orb of Annulment (Eater)",
        }
    }

    fn cost_chaos(&self) -> f64 {
        10.0
    }

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        let rarity_allowed = matches!(item.rarity, Rarity::Magic | Rarity::Rare);
        if !base_can_apply(item, db, rarity_allowed, self.god) {
            return false;
        }
        removable_target_count(item, &target_generation_type(self.god)) > 0
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        _rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply {}", self.name());
        }

        let target = target_generation_type(self.god);
        let total = removable_target_count(item, &target);
        let probability = 1.0 / total as f64;
        let mut outcomes = Vec::with_capacity(total);

        let target_modifier_count = match target {
            GenerationType::Prefix => item.prefixes.len(),
            GenerationType::Suffix => item.suffixes.len(),
            _ => 0,
        };
        for index in 0..target_modifier_count {
            let mut next = item.clone();
            match target {
                GenerationType::Prefix => {
                    next.prefixes.remove(index);
                }
                GenerationType::Suffix => {
                    next.suffixes.remove(index);
                }
                _ => {}
            }
            outcomes.push((next, probability));
        }

        if item
            .crafted_mod
            .as_ref()
            .is_some_and(|modifier| modifier.generation_type == target)
        {
            let mut next = item.clone();
            next.crafted_mod = None;
            outcomes.push((next, probability));
        }

        Ok(outcomes)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rand::rngs::StdRng;
    use rand::SeedableRng;

    use super::*;
    use crate::data::mods::{Domain, Mod, SpawnWeight};

    fn make_mod(generation_type: GenerationType, group: &str, spawn_weight: u32) -> Mod {
        Mod {
            name: String::new(),
            generation_type,
            required_level: 1,
            stats: Vec::new(),
            spawn_weights: vec![SpawnWeight {
                tag: "helmet".to_string(),
                weight: spawn_weight,
            }],
            generation_weights: Vec::new(),
            adds_tags: Vec::new(),
            tags: Vec::new(),
            domain: Domain::Item,
            mod_type: group.to_string(),
            groups: vec![group.to_string()],
            is_essence_only: false,
        }
    }

    fn make_implicit(generation_type: GenerationType, tier: u8) -> Mod {
        let mut modifier = make_mod(generation_type, "implicit", 1_000);
        modifier.spawn_weights.insert(
            0,
            SpawnWeight {
                tag: format!("no_tier_{tier}_eldritch_implicit"),
                weight: 0,
            },
        );
        modifier
    }

    fn modifier(mod_id: &str, generation_type: GenerationType) -> Modifier {
        Modifier {
            mod_id: mod_id.to_string(),
            generation_type,
            rolls: Vec::new(),
        }
    }

    fn test_db() -> GameData {
        let mods = HashMap::from([
            (
                "exarch_strong".to_string(),
                make_implicit(GenerationType::ExarchImplicit, 2),
            ),
            (
                "exarch_equal".to_string(),
                make_implicit(GenerationType::ExarchImplicit, 4),
            ),
            (
                "eater_weak".to_string(),
                make_implicit(GenerationType::EaterImplicit, 5),
            ),
            (
                "eater_strong".to_string(),
                make_implicit(GenerationType::EaterImplicit, 1),
            ),
            (
                "eater_equal".to_string(),
                make_implicit(GenerationType::EaterImplicit, 4),
            ),
            (
                "prefix_a".to_string(),
                make_mod(GenerationType::Prefix, "prefix_a", 100),
            ),
            (
                "prefix_b".to_string(),
                make_mod(GenerationType::Prefix, "prefix_b", 300),
            ),
            (
                "suffix_a".to_string(),
                make_mod(GenerationType::Suffix, "suffix_a", 200),
            ),
        ]);
        GameData::new(mods, HashMap::new())
    }

    fn rare_helmet() -> ItemState {
        let mut item = ItemState::new_base("helmet", vec!["helmet".to_string()], 84);
        item.rarity = Rarity::Rare;
        item.exarch_implicit = Some(modifier("exarch_strong", GenerationType::ExarchImplicit));
        item.eater_implicit = Some(modifier("eater_weak", GenerationType::EaterImplicit));
        item
    }

    #[test]
    fn dominance_uses_repoe_tier_gates_and_rejects_equal_tiers() {
        let db = test_db();
        let mut item = rare_helmet();
        assert_eq!(dominant_god(&item, &db), Some(EldritchGod::SearingExarch));

        item.exarch_implicit = Some(modifier("exarch_equal", GenerationType::ExarchImplicit));
        item.eater_implicit = Some(modifier("eater_equal", GenerationType::EaterImplicit));
        assert_eq!(dominant_god(&item, &db), None);

        let chaos = EldritchChaosOrb {
            god: EldritchGod::SearingExarch,
        };
        assert!(!chaos.can_apply(&item, &db));
    }

    #[test]
    fn chaos_rerolls_only_the_dominant_explicit_side_and_is_monte_carlo() {
        let db = test_db();
        let mut item = rare_helmet();
        item.prefixes
            .push(modifier("old_prefix", GenerationType::Prefix));
        item.suffixes
            .push(modifier("kept_suffix", GenerationType::Suffix));
        item.crafted_mod = Some(modifier("crafted_prefix", GenerationType::Prefix));
        let original_exarch = item.exarch_implicit.clone();
        let original_eater = item.eater_implicit.clone();

        let chaos = EldritchChaosOrb {
            god: EldritchGod::SearingExarch,
        };
        let outcomes = chaos
            .apply(&item, &db, &mut StdRng::seed_from_u64(7))
            .expect("Eldritch Chaos should apply");

        assert_eq!(outcomes.len(), MONTE_CARLO_SAMPLES);
        assert!(!chaos.weights_are_probabilities());
        assert!(chaos.repeatable_on_failure());
        assert!((outcomes.iter().map(|(_, weight)| weight).sum::<f64>() - 1.0).abs() < 1e-9);
        for (next, _) in outcomes {
            assert_eq!(next.suffixes, item.suffixes);
            assert_eq!(next.exarch_implicit, original_exarch);
            assert_eq!(next.eater_implicit, original_eater);
            assert!(next
                .prefixes
                .iter()
                .all(|modifier| modifier.mod_id != "old_prefix"));
            assert!(next.crafted_mod.is_none());
            assert!(next.prefix_count() <= next.max_prefixes());
        }
    }

    #[test]
    fn exalt_adds_only_the_dominance_selected_affix_with_exact_weights() {
        let db = test_db();
        let item = rare_helmet();
        let exalt = EldritchExaltedOrb {
            god: EldritchGod::SearingExarch,
        };
        let outcomes = exalt
            .apply(&item, &db, &mut StdRng::seed_from_u64(11))
            .expect("Eldritch Exalted should apply");

        assert_eq!(outcomes.len(), 2);
        assert!(!exalt.weights_are_probabilities());
        assert_eq!(outcomes[0].0.prefixes.len(), 1);
        assert_eq!(outcomes[1].0.prefixes.len(), 1);
        assert!(outcomes.iter().all(|(next, _)| next.suffixes.is_empty()));
        let weights: HashMap<&str, f64> = outcomes
            .iter()
            .map(|(next, weight)| (next.prefixes[0].mod_id.as_str(), *weight))
            .collect();
        assert_eq!(weights.get("prefix_a"), Some(&0.25));
        assert_eq!(weights.get("prefix_b"), Some(&0.75));
    }

    #[test]
    fn exalt_rejects_a_full_target_side_even_if_the_other_side_is_open() {
        let db = test_db();
        let mut item = rare_helmet();
        item.prefixes = vec![
            modifier("p1", GenerationType::Prefix),
            modifier("p2", GenerationType::Prefix),
            modifier("p3", GenerationType::Prefix),
        ];

        let exalt = EldritchExaltedOrb {
            god: EldritchGod::SearingExarch,
        };
        assert!(!exalt.can_apply(&item, &db));
    }

    #[test]
    fn eater_dominance_targets_suffixes() {
        let db = test_db();
        let mut item = rare_helmet();
        item.eater_implicit = Some(modifier("eater_strong", GenerationType::EaterImplicit));
        let exalt = EldritchExaltedOrb {
            god: EldritchGod::EaterOfWorlds,
        };
        let outcomes = exalt
            .apply(&item, &db, &mut StdRng::seed_from_u64(12))
            .expect("Eater-dominant Eldritch Exalted should apply");

        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].0.prefixes.is_empty());
        assert_eq!(outcomes[0].0.suffixes[0].mod_id, "suffix_a");
        assert_eq!(outcomes[0].1, 1.0);
    }

    #[test]
    fn annulment_enumerates_only_removable_target_affixes() {
        let db = test_db();
        let mut item = rare_helmet();
        item.prefixes = vec![
            modifier("p1", GenerationType::Prefix),
            modifier("p2", GenerationType::Prefix),
        ];
        item.suffixes
            .push(modifier("protected_suffix", GenerationType::Suffix));
        item.crafted_mod = Some(modifier("crafted_prefix", GenerationType::Prefix));
        item.fractured
            .push(modifier("fractured_prefix", GenerationType::Prefix));

        let annul = EldritchOrbOfAnnulment {
            god: EldritchGod::SearingExarch,
        };
        let outcomes = annul
            .apply(&item, &db, &mut StdRng::seed_from_u64(13))
            .expect("Eldritch Annulment should apply");

        assert_eq!(outcomes.len(), 3);
        assert!(outcomes
            .iter()
            .all(|(next, probability)| *probability == 1.0 / 3.0
                && next.suffixes == item.suffixes
                && next.fractured == item.fractured
                && next.exarch_implicit == item.exarch_implicit
                && next.eater_implicit == item.eater_implicit));
        assert_eq!(
            outcomes
                .iter()
                .filter(|(next, _)| next.crafted_mod.is_none())
                .count(),
            1
        );
    }
}
