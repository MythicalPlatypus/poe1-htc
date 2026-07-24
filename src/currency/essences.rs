//! Essence crafting — guarantees one specific mod while rerolling the rest as a Chaos Orb.

use std::collections::HashSet;

use anyhow::{bail, Result};
use rand::RngCore;

use super::{random_rare_affix_count, CraftingMethod, RerollKind, MONTE_CARLO_SAMPLES};
use crate::data::mods::{Domain, GenerationType, Mod};
use crate::data::GameData;
use crate::engine::mod_pool::{random_rolls_pub, roll_mods};
use crate::item::modifier::Modifier;
use crate::item::{state::Rarity, ItemState};

/// An Essence application: guarantees `guaranteed_mod_id` on the item.
#[derive(Debug)]
pub struct Essence {
    pub display_name: String,
    /// RePoE mod ID that this Essence guarantees.
    pub guaranteed_mod_id: String,
    /// Approximate chaos cost.
    pub cost_chaos: f64,
    /// Lower-tier essences cap the level of randomly rolled filler modifiers.
    pub max_item_level: Option<u32>,
    /// Screaming and stronger essences may reforge an existing Rare item.
    pub can_reforge_rare: bool,
}

impl Essence {
    fn validated_mod<'a>(&self, item: &ItemState, db: &'a GameData) -> Result<&'a Mod> {
        if !item.is_craftable() {
            bail!("{}: item is corrupted or mirrored", self.display_name);
        }
        if item.rarity != Rarity::Normal && !(item.rarity == Rarity::Rare && self.can_reforge_rare)
        {
            bail!(
                "{}: this Essence can only be applied to {} items",
                self.display_name,
                if self.can_reforge_rare {
                    "Normal or Rare"
                } else {
                    "Normal"
                }
            );
        }

        let guaranteed = db.mods.get(&self.guaranteed_mod_id).ok_or_else(|| {
            anyhow::anyhow!(
                "{}: guaranteed mod '{}' not found in DB",
                self.display_name,
                self.guaranteed_mod_id
            )
        })?;
        if guaranteed.domain != Domain::Item {
            bail!(
                "{}: guaranteed mod '{}' has non-item domain {:?}",
                self.display_name,
                self.guaranteed_mod_id,
                guaranteed.domain
            );
        }
        if !matches!(
            guaranteed.generation_type,
            GenerationType::Prefix | GenerationType::Suffix
        ) {
            bail!(
                "{}: guaranteed mod '{}' is not a prefix or suffix",
                self.display_name,
                self.guaranteed_mod_id
            );
        }
        // The guaranteed Essence property ignores normal required-level gating.
        // Lower tiers instead cap only the pool used for random filler affixes.
        if let Some(stat) = guaranteed.stats.iter().find(|stat| stat.min > stat.max) {
            bail!(
                "{}: guaranteed mod '{}' has invalid range for stat {}: {} > {}",
                self.display_name,
                self.guaranteed_mod_id,
                stat.id,
                stat.min,
                stat.max
            );
        }

        // RePoE gives essence-only affixes zero normal spawn weight. Their
        // item-class selection happens in essence data that GameData does not
        // load, so the configured ID is authoritative. Ordinary affixes still
        // have enough Mod data to reject an impossible base/tag combination.
        let base_tags: Vec<&str> = item.base_tags.iter().map(String::as_str).collect();
        if !guaranteed.is_essence_only && guaranteed.spawn_weight_for_tags(&base_tags) == 0 {
            bail!(
                "{}: guaranteed mod '{}' cannot spawn on this base",
                self.display_name,
                self.guaranteed_mod_id
            );
        }

        let mut fractured_groups = HashSet::new();
        let mut fractured_prefixes = 0usize;
        let mut fractured_suffixes = 0usize;
        for modifier in &item.fractured {
            let fractured = db.mods.get(&modifier.mod_id).ok_or_else(|| {
                anyhow::anyhow!(
                    "{}: fractured mod '{}' not found in DB",
                    self.display_name,
                    modifier.mod_id
                )
            })?;
            fractured_groups.extend(fractured.groups.iter().map(String::as_str));
            match modifier.generation_type {
                GenerationType::Prefix => fractured_prefixes += 1,
                GenerationType::Suffix => fractured_suffixes += 1,
                _ => {}
            }
        }

        if guaranteed
            .groups
            .iter()
            .any(|group| fractured_groups.contains(group.as_str()))
        {
            bail!(
                "{}: guaranteed mod '{}' shares a mod group with a fractured mod",
                self.display_name,
                self.guaranteed_mod_id
            );
        }
        if fractured_prefixes > 3 || fractured_suffixes > 3 {
            bail!(
                "{}: fractured modifiers exceed Rare affix capacity",
                self.display_name
            );
        }
        match guaranteed.generation_type {
            GenerationType::Prefix if fractured_prefixes >= 3 => bail!(
                "{}: no open prefix slot — all taken by fractured mods",
                self.display_name
            ),
            GenerationType::Suffix if fractured_suffixes >= 3 => bail!(
                "{}: no open suffix slot — all taken by fractured mods",
                self.display_name
            ),
            _ => {}
        }

        Ok(guaranteed)
    }
}

impl CraftingMethod for Essence {
    fn name(&self) -> &str {
        &self.display_name
    }
    fn cost_chaos(&self) -> f64 {
        self.cost_chaos
    }
    // Monte Carlo sampling — weights are 1/N, not probabilities.
    fn weights_are_probabilities(&self) -> bool {
        false
    }
    fn repeatable_on_failure(&self) -> bool {
        self.can_reforge_rare
    }
    fn reroll_kind(&self) -> Option<RerollKind> {
        Some(RerollKind::RareExplicit)
    }

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        self.validated_mod(item, db).is_ok()
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        let guaranteed = self.validated_mod(item, db)?;
        let sample_weight = 1.0 / MONTE_CARLO_SAMPLES as f64;
        let mut outcomes = Vec::with_capacity(MONTE_CARLO_SAMPLES);

        for _ in 0..MONTE_CARLO_SAMPLES {
            let mut next = item.clone();
            next.rarity = Rarity::Rare;
            next.prefixes.clear();
            next.suffixes.clear();
            next.crafted_mod = None;

            let forced = Modifier {
                mod_id: self.guaranteed_mod_id.clone(),
                generation_type: guaranteed.generation_type.clone(),
                rolls: random_rolls_pub(&guaranteed.stats, rng),
            };
            match guaranteed.generation_type {
                GenerationType::Prefix => next.prefixes.push(forced),
                GenerationType::Suffix => next.suffixes.push(forced),
                _ => bail!(
                    "{}: guaranteed mod is not a prefix or suffix (validated above)",
                    self.display_name
                ),
            }

            // Fractured affixes survive the reroll and count toward the 4–6
            // total. The forced affix was just added; the crafted affix was removed.
            let desired_total = random_rare_affix_count(rng);
            let current_total = next.prefix_count() + next.suffix_count();
            let original_item_level = next.item_level;
            if let Some(maximum) = self.max_item_level {
                next.item_level = next.item_level.min(maximum);
            }
            roll_mods(
                &mut next,
                desired_total.saturating_sub(current_total),
                db,
                rng,
            )?;
            next.item_level = original_item_level;
            outcomes.push((next, sample_weight));
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
    use crate::data::mods::{GenerationWeight, ModStat, SpawnWeight};

    fn test_mod(
        generation_type: GenerationType,
        group: &str,
        tag: &str,
        required_level: u32,
    ) -> Mod {
        Mod {
            name: group.to_string(),
            generation_type,
            required_level,
            stats: vec![ModStat {
                id: format!("{group}_stat"),
                min: 1,
                max: 10,
            }],
            spawn_weights: vec![
                SpawnWeight {
                    tag: tag.to_string(),
                    weight: 100,
                },
                SpawnWeight {
                    tag: "default".to_string(),
                    weight: 0,
                },
            ],
            generation_weights: Vec::<GenerationWeight>::new(),
            adds_tags: vec![],
            tags: vec![],
            domain: Domain::Item,
            mod_type: group.to_string(),
            groups: vec![group.to_string()],
            is_essence_only: false,
            text: None,
        }
    }

    fn marker(mod_id: &str, generation_type: GenerationType) -> Modifier {
        Modifier {
            mod_id: mod_id.to_string(),
            generation_type,
            rolls: vec![],
        }
    }

    fn item(rarity: Rarity, item_level: u32) -> ItemState {
        let mut item = ItemState::new_base("TestSword", vec!["sword".to_string()], item_level);
        item.rarity = rarity;
        item
    }

    fn essence() -> Essence {
        Essence {
            display_name: "Test Essence".to_string(),
            guaranteed_mod_id: "forced".to_string(),
            cost_chaos: 5.0,
            max_item_level: None,
            can_reforge_rare: true,
        }
    }

    fn db_with_forced(forced: Mod) -> GameData {
        let mut mods = HashMap::new();
        mods.insert("forced".to_string(), forced);
        for index in 0..8 {
            let generation_type = if index % 2 == 0 {
                GenerationType::Prefix
            } else {
                GenerationType::Suffix
            };
            mods.insert(
                format!("filler{index}"),
                test_mod(generation_type, &format!("Filler{index}"), "sword", 1),
            );
        }
        GameData::new(mods, HashMap::new())
    }

    #[test]
    fn can_apply_only_to_normal_or_rare_craftable_items() {
        let db = db_with_forced(test_mod(GenerationType::Prefix, "Forced", "sword", 1));
        let craft = essence();

        assert!(craft.can_apply(&item(Rarity::Normal, 84), &db));
        assert!(craft.can_apply(&item(Rarity::Rare, 84), &db));
        assert!(!craft.can_apply(&item(Rarity::Magic, 84), &db));
        assert!(!craft.can_apply(&item(Rarity::Unique, 84), &db));

        let mut corrupted = item(Rarity::Rare, 84);
        corrupted.corrupted = true;
        assert!(!craft.can_apply(&corrupted, &db));
    }

    #[test]
    fn can_apply_checks_base_tags_but_guaranteed_mod_ignores_required_level() {
        let db = db_with_forced(test_mod(GenerationType::Prefix, "Forced", "dex_armour", 80));
        let craft = essence();

        assert!(!craft.can_apply(&item(Rarity::Rare, 84), &db));

        let mut matching = item(Rarity::Rare, 1);
        matching.base_tags = vec!["dex_armour".to_string()];
        assert!(craft.can_apply(&matching, &db));
    }

    #[test]
    fn essence_only_mod_bypasses_normal_spawn_weight_but_not_domain() {
        let mut forced = test_mod(GenerationType::Suffix, "Forced", "never", 1);
        forced.is_essence_only = true;
        let db = db_with_forced(forced.clone());
        assert!(essence().can_apply(&item(Rarity::Rare, 84), &db));

        forced.domain = Domain::Crafted;
        let wrong_domain = db_with_forced(forced);
        assert!(!essence().can_apply(&item(Rarity::Rare, 84), &wrong_domain));
    }

    #[test]
    fn can_apply_reports_fractured_conflicts_and_capacity() {
        let forced = test_mod(GenerationType::Prefix, "Forced", "sword", 1);
        let mut db_mods = HashMap::new();
        db_mods.insert("forced".to_string(), forced);
        for index in 0..3 {
            db_mods.insert(
                format!("fractured{index}"),
                test_mod(
                    GenerationType::Prefix,
                    if index == 0 { "Forced" } else { "Other" },
                    "sword",
                    1,
                ),
            );
        }
        let db = GameData::new(db_mods, HashMap::new());

        let mut conflict = item(Rarity::Rare, 84);
        conflict
            .fractured
            .push(marker("fractured0", GenerationType::Prefix));
        assert!(!essence().can_apply(&conflict, &db));
        let error = essence()
            .apply(&conflict, &db, &mut StdRng::seed_from_u64(1))
            .expect_err("fractured group conflict must fail");
        assert!(error.to_string().contains("shares a mod group"));

        let mut full = item(Rarity::Rare, 84);
        for index in 0..3 {
            full.fractured
                .push(marker(&format!("fractured{index}"), GenerationType::Prefix));
        }
        assert!(!essence().can_apply(&full, &db));
    }

    #[test]
    fn crafted_mod_is_removed_before_conflict_and_capacity_checks() {
        let forced = test_mod(GenerationType::Prefix, "Forced", "sword", 1);
        let mut crafted = test_mod(GenerationType::Prefix, "Forced", "sword", 1);
        crafted.domain = Domain::Crafted;
        let mut mods = HashMap::new();
        mods.insert("forced".to_string(), forced);
        mods.insert("crafted".to_string(), crafted);
        let db = GameData::new(mods, HashMap::new());
        let mut input = item(Rarity::Rare, 84);
        input.crafted_mod = Some(marker("crafted", GenerationType::Prefix));

        assert!(essence().can_apply(&input, &db));
        let outcomes = essence()
            .apply(&input, &db, &mut StdRng::seed_from_u64(2))
            .expect("ordinary crafted mod is rerolled away");
        assert!(outcomes
            .iter()
            .all(|(outcome, _)| outcome.crafted_mod.is_none()));
    }

    #[test]
    fn malformed_guaranteed_stat_range_is_rejected_before_sampling() {
        let mut forced = test_mod(GenerationType::Prefix, "Forced", "sword", 1);
        forced.stats[0].min = 11;
        forced.stats[0].max = 10;
        let db = db_with_forced(forced);
        let input = item(Rarity::Rare, 84);

        assert!(!essence().can_apply(&input, &db));
        let error = essence()
            .apply(&input, &db, &mut StdRng::seed_from_u64(3))
            .expect_err("invalid range must return an error, not panic");
        assert!(error.to_string().contains("invalid range"));
    }

    #[test]
    fn fractured_affixes_are_included_in_the_four_to_six_total() {
        let forced = test_mod(GenerationType::Prefix, "Forced", "sword", 1);
        let mut mods = db_with_forced(forced).mods;
        mods.insert(
            "fractured_prefix".to_string(),
            test_mod(GenerationType::Prefix, "FracturedPrefix", "sword", 1),
        );
        mods.insert(
            "fractured_suffix".to_string(),
            test_mod(GenerationType::Suffix, "FracturedSuffix", "sword", 1),
        );
        let db = GameData::new(mods, HashMap::new());
        let mut input = item(Rarity::Rare, 84);
        input
            .fractured
            .push(marker("fractured_prefix", GenerationType::Prefix));
        input
            .fractured
            .push(marker("fractured_suffix", GenerationType::Suffix));

        let outcomes = essence()
            .apply(&input, &db, &mut StdRng::seed_from_u64(4))
            .expect("fractured item should reroll");
        for (outcome, _) in outcomes {
            let affix_count = outcome.prefix_count() + outcome.suffix_count();
            assert!(
                (4..=6).contains(&affix_count),
                "fractures must be inside the target, got {affix_count}"
            );
        }
    }

    #[test]
    fn monte_carlo_outcomes_have_sample_weights() {
        let db = db_with_forced(test_mod(GenerationType::Prefix, "Forced", "sword", 1));
        let craft = essence();
        let outcomes = craft
            .apply(&item(Rarity::Rare, 84), &db, &mut StdRng::seed_from_u64(5))
            .expect("valid essence should sample");

        assert_eq!(outcomes.len(), MONTE_CARLO_SAMPLES);
        assert!(outcomes
            .iter()
            .all(|(_, weight)| (*weight - 1.0 / MONTE_CARLO_SAMPLES as f64).abs() < 1e-12));
        assert!((outcomes.iter().map(|(_, weight)| weight).sum::<f64>() - 1.0).abs() < 1e-12);
        assert!(!craft.weights_are_probabilities());
    }

    #[test]
    fn normal_and_rare_inputs_have_identical_retry_draws_and_cost() {
        let db = db_with_forced(test_mod(GenerationType::Prefix, "Forced", "sword", 1));
        let craft = essence();
        let normal = craft
            .apply(
                &item(Rarity::Normal, 84),
                &db,
                &mut StdRng::seed_from_u64(6),
            )
            .expect("Normal item should be upgraded");
        let rare = craft
            .apply(&item(Rarity::Rare, 84), &db, &mut StdRng::seed_from_u64(6))
            .expect("Rare item should be reforged");

        assert_eq!(normal.len(), rare.len());
        for ((normal_item, normal_weight), (rare_item, rare_weight)) in normal.iter().zip(&rare) {
            assert_eq!(normal_item.rarity, Rarity::Rare);
            assert_eq!(normal_item.prefixes, rare_item.prefixes);
            assert_eq!(normal_item.suffixes, rare_item.suffixes);
            assert_eq!(normal_weight, rare_weight);
        }
        assert_eq!(craft.cost_chaos(), 5.0);
        assert!(craft.repeatable_on_failure());
    }

    #[test]
    fn lower_tier_essence_is_normal_only_and_caps_random_affix_level() {
        let mut mods = db_with_forced(test_mod(GenerationType::Prefix, "Forced", "sword", 1)).mods;
        mods.insert(
            "too_high".to_string(),
            test_mod(GenerationType::Suffix, "TooHigh", "sword", 61),
        );
        let db = GameData::new(mods, HashMap::new());
        let craft = Essence {
            display_name: "Weeping Essence".to_string(),
            guaranteed_mod_id: "forced".to_string(),
            cost_chaos: 1.0,
            max_item_level: Some(60),
            can_reforge_rare: false,
        };

        assert!(craft.can_apply(&item(Rarity::Normal, 86), &db));
        assert!(!craft.can_apply(&item(Rarity::Rare, 86), &db));
        assert!(!craft.repeatable_on_failure());
        let outcomes = craft
            .apply(
                &item(Rarity::Normal, 86),
                &db,
                &mut StdRng::seed_from_u64(8),
            )
            .expect("high-level base should accept a lower-tier Essence");
        assert!(outcomes.iter().all(|(state, _)| {
            state.item_level == 86
                && !state
                    .all_explicit_mods()
                    .any(|modifier| modifier.mod_id == "too_high")
        }));
    }
}
