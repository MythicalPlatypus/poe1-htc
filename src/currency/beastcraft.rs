//! Bestiary crafts that exchange one random affix for one affix of the other side.
//!
//! Removal is enumerated exactly and uniformly across removable affixes of the
//! requested side. Fractured modifiers are never removal candidates. For each
//! post-removal state, the added mod is enumerated with its exact RePoE pool
//! weight. The configured beast level, rather than the item's item level,
//! controls which added mods are eligible. Numeric stat values are sampled once
//! per mod branch, so the returned weights are exact for affix identities but
//! not for concrete rolled states.

use anyhow::{bail, Result};
use rand::RngCore;

use super::{CraftingMethod, MethodFamily, MethodId, MethodSetup, ProbabilityModel};
use crate::data::mods::{GenerationType, ModStat};
use crate::data::GameData;
use crate::engine::mod_pool::{eligible_mods, random_rolls_pub};
use crate::item::modifier::Modifier;
use crate::item::{state::Rarity, ItemState};

/// The two opposite-side affix exchanges offered by Bestiary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BestiaryAffixSwapKind {
    AddPrefixRemoveSuffix,
    AddSuffixRemovePrefix,
}

impl BestiaryAffixSwapKind {
    fn id_token(self) -> &'static str {
        match self {
            Self::AddPrefixRemoveSuffix => "add-prefix-remove-suffix",
            Self::AddSuffixRemovePrefix => "add-suffix-remove-prefix",
        }
    }

    fn source_type(self) -> GenerationType {
        match self {
            Self::AddPrefixRemoveSuffix => GenerationType::Suffix,
            Self::AddSuffixRemovePrefix => GenerationType::Prefix,
        }
    }

    fn destination_type(self) -> GenerationType {
        match self {
            Self::AddPrefixRemoveSuffix => GenerationType::Prefix,
            Self::AddSuffixRemovePrefix => GenerationType::Suffix,
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::AddPrefixRemoveSuffix => "Add a Prefix, Remove a Random Suffix",
            Self::AddSuffixRemovePrefix => "Add a Suffix, Remove a Random Prefix",
        }
    }
}

/// Configurable Bestiary affix-swap craft.
///
/// `goal/mod.rs` can construct this with [`BestiaryAffixSwapCraft::new`] and
/// supply the current league's chaos-equivalent cost.
#[derive(Debug, Clone)]
pub struct BestiaryAffixSwapCraft {
    pub kind: BestiaryAffixSwapKind,
    pub cost_chaos: f64,
    beast_level: u32,
}

impl BestiaryAffixSwapCraft {
    /// Creates a craft using the main sacrificed beast's monster level.
    ///
    /// Beast levels use the same supported 1 through 100 level domain as item
    /// levels. The level affects destination-mod eligibility only; applying the
    /// craft never changes [`ItemState::item_level`].
    pub fn new(kind: BestiaryAffixSwapKind, beast_level: u32, cost_chaos: f64) -> Result<Self> {
        if !(1..=100).contains(&beast_level) {
            bail!("beast_level must be between 1 and 100");
        }
        Ok(Self {
            kind,
            cost_chaos,
            beast_level,
        })
    }

    pub fn beast_level(&self) -> u32 {
        self.beast_level
    }

    fn check(&self, item: &ItemState, db: &GameData) -> Result<()> {
        if !item.is_craftable() {
            bail!("{} requires a craftable item", self.name());
        }
        if item.rarity != Rarity::Rare {
            bail!("{} requires a Rare item", self.name());
        }

        let choices = removal_choices(item, self.kind);
        if choices.is_empty() {
            bail!("{} has no removable source affix", self.name());
        }

        // Every random removal must lead to a valid add. Requiring only one
        // successful branch would bias removal probabilities by silently
        // discarding branches whose changed groups or tags exhaust the pool.
        for choice in choices {
            let mut post_removal = item.clone();
            remove_choice(&mut post_removal, choice, self.kind)?;
            if eligible_destination_mods(&post_removal, self.kind, self.beast_level, db).is_empty()
            {
                bail!(
                    "{} has a removal branch with no legal destination affix",
                    self.name()
                );
            }
        }

        Ok(())
    }
}

impl CraftingMethod for BestiaryAffixSwapCraft {
    fn id(&self) -> MethodId {
        let beast_level = self.beast_level.to_string();
        MethodId::semantic(
            "bestiary",
            "affix-swap",
            &[self.kind.id_token(), "beast-level", &beast_level],
        )
    }

    fn family(&self) -> MethodFamily {
        MethodFamily::Bestiary
    }

    fn description(&self) -> &str {
        match self.kind {
            BestiaryAffixSwapKind::AddPrefixRemoveSuffix => {
                "Removes one random suffix and adds one prefix using the configured beast level."
            }
            BestiaryAffixSwapKind::AddSuffixRemovePrefix => {
                "Removes one random prefix and adds one suffix using the configured beast level."
            }
        }
    }

    fn setup(&self) -> MethodSetup {
        MethodSetup::Configured
    }

    fn name(&self) -> &str {
        self.kind.display_name()
    }

    fn cost_chaos(&self) -> f64 {
        self.cost_chaos
    }

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        self.check(item, db).is_ok()
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        self.check(item, db)?;

        let choices = removal_choices(item, self.kind);
        let removal_probability = 1.0 / choices.len() as f64;
        let mut outcomes = Vec::new();

        for choice in choices {
            let mut post_removal = item.clone();
            remove_choice(&mut post_removal, choice, self.kind)?;

            let pool = eligible_destination_mods(&post_removal, self.kind, self.beast_level, db);
            let total_weight: u64 = pool.iter().map(|(_, _, weight)| u64::from(*weight)).sum();
            if total_weight == 0 {
                bail!(
                    "{} produced a removal branch with zero destination weight",
                    self.name()
                );
            }

            for (mod_id, picked, weight) in pool {
                validate_roll_ranges(mod_id, &picked.stats)?;
                let mut next = post_removal.clone();
                let modifier = Modifier {
                    mod_id: mod_id.to_string(),
                    generation_type: picked.generation_type.clone(),
                    rolls: random_rolls_pub(&picked.stats, rng),
                };

                match picked.generation_type {
                    GenerationType::Prefix => next.prefixes.push(modifier),
                    GenerationType::Suffix => next.suffixes.push(modifier),
                    _ => bail!(
                        "{} destination pool contained non-affix mod {mod_id}",
                        self.name()
                    ),
                }

                let add_probability = f64::from(weight) / total_weight as f64;
                outcomes.push((next, removal_probability * add_probability));
            }
        }

        Ok(outcomes)
    }

    fn weights_are_probabilities(&self) -> bool {
        false
    }

    fn probability_model(&self) -> ProbabilityModel {
        ProbabilityModel::ExactIdentitySampledRolls
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemovalChoice {
    Explicit(usize),
    Crafted,
}

fn removal_choices(item: &ItemState, kind: BestiaryAffixSwapKind) -> Vec<RemovalChoice> {
    let source = kind.source_type();
    let explicit_count = match source {
        GenerationType::Prefix => item.prefixes.len(),
        GenerationType::Suffix => item.suffixes.len(),
        _ => 0,
    };
    let mut choices: Vec<RemovalChoice> =
        (0..explicit_count).map(RemovalChoice::Explicit).collect();

    if item
        .crafted_mod
        .as_ref()
        .is_some_and(|modifier| modifier.generation_type == source)
    {
        choices.push(RemovalChoice::Crafted);
    }
    choices
}

fn remove_choice(
    item: &mut ItemState,
    choice: RemovalChoice,
    kind: BestiaryAffixSwapKind,
) -> Result<()> {
    match (kind.source_type(), choice) {
        (GenerationType::Prefix, RemovalChoice::Explicit(index)) => {
            if index >= item.prefixes.len() {
                bail!("prefix removal choice {index} is out of bounds");
            }
            item.prefixes.remove(index);
        }
        (GenerationType::Suffix, RemovalChoice::Explicit(index)) => {
            if index >= item.suffixes.len() {
                bail!("suffix removal choice {index} is out of bounds");
            }
            item.suffixes.remove(index);
        }
        (source, RemovalChoice::Crafted) => {
            let is_source = item
                .crafted_mod
                .as_ref()
                .is_some_and(|modifier| modifier.generation_type == source);
            if !is_source {
                bail!("crafted removal choice is not on the requested source side");
            }
            item.crafted_mod = None;
        }
        _ => bail!("unsupported Bestiary affix-swap removal side"),
    }
    Ok(())
}

fn eligible_destination_mods<'a>(
    item: &ItemState,
    kind: BestiaryAffixSwapKind,
    beast_level: u32,
    db: &'a GameData,
) -> Vec<(&'a str, &'a crate::data::mods::Mod, u32)> {
    let destination = kind.destination_type();
    let mut pool_view = item.clone();
    pool_view.item_level = beast_level;
    eligible_mods(&pool_view, &[], db)
        .into_iter()
        .filter(|(_, modifier, _)| modifier.generation_type == destination)
        .collect()
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rand::rngs::StdRng;
    use rand::SeedableRng;

    use super::*;
    use crate::data::mods::{Domain, Mod, SpawnWeight};
    use crate::item::modifier::StatRoll;

    fn modifier(id: &str, generation_type: GenerationType) -> Modifier {
        Modifier {
            mod_id: id.to_string(),
            generation_type,
            rolls: vec![],
        }
    }

    fn candidate(generation_type: GenerationType, weight: u32, group: &str) -> Mod {
        Mod {
            name: group.to_string(),
            generation_type,
            required_level: 1,
            stats: vec![],
            spawn_weights: vec![SpawnWeight {
                tag: "sword".to_string(),
                weight,
            }],
            generation_weights: vec![],
            adds_tags: vec![],
            tags: vec![],
            domain: Domain::Item,
            mod_type: group.to_string(),
            groups: vec![group.to_string()],
            is_essence_only: false,
            text: None,
        }
    }

    fn db(mods: impl IntoIterator<Item = (&'static str, Mod)>) -> GameData {
        GameData::new(
            mods.into_iter()
                .map(|(id, modifier)| (id.to_string(), modifier))
                .collect(),
            HashMap::new(),
        )
    }

    fn rare_item() -> ItemState {
        let mut item = ItemState::new_base("base", vec!["sword".to_string()], 86);
        item.rarity = Rarity::Rare;
        item
    }

    fn craft(kind: BestiaryAffixSwapKind) -> BestiaryAffixSwapCraft {
        craft_at_level(kind, 86)
    }

    fn craft_at_level(kind: BestiaryAffixSwapKind, beast_level: u32) -> BestiaryAffixSwapCraft {
        BestiaryAffixSwapCraft::new(kind, beast_level, 17.5)
            .expect("test beast level should be valid")
    }

    fn probability_for(outcomes: &[(ItemState, f64)], remaining_source: &str, added: &str) -> f64 {
        outcomes
            .iter()
            .filter(|(item, _)| {
                item.suffixes
                    .iter()
                    .any(|modifier| modifier.mod_id == remaining_source)
                    && item
                        .prefixes
                        .iter()
                        .any(|modifier| modifier.mod_id == added)
            })
            .map(|(_, probability)| probability)
            .sum()
    }

    #[test]
    fn exposes_stable_configuration_api_and_names() {
        let add_prefix = craft(BestiaryAffixSwapKind::AddPrefixRemoveSuffix);
        assert_eq!(add_prefix.name(), "Add a Prefix, Remove a Random Suffix");
        assert_eq!(add_prefix.cost_chaos(), 17.5);
        assert_eq!(add_prefix.beast_level(), 86);

        let add_suffix = craft(BestiaryAffixSwapKind::AddSuffixRemovePrefix);
        assert_eq!(add_suffix.name(), "Add a Suffix, Remove a Random Prefix");
        assert!(!add_suffix.weights_are_probabilities());
    }

    #[test]
    fn constructor_rejects_levels_outside_the_supported_domain() {
        assert!(
            BestiaryAffixSwapCraft::new(BestiaryAffixSwapKind::AddPrefixRemoveSuffix, 0, 17.5,)
                .is_err()
        );
        assert!(BestiaryAffixSwapCraft::new(
            BestiaryAffixSwapKind::AddPrefixRemoveSuffix,
            101,
            17.5,
        )
        .is_err());
        assert!(
            BestiaryAffixSwapCraft::new(BestiaryAffixSwapKind::AddPrefixRemoveSuffix, 1, 17.5,)
                .is_ok()
        );
        assert!(BestiaryAffixSwapCraft::new(
            BestiaryAffixSwapKind::AddPrefixRemoveSuffix,
            100,
            17.5,
        )
        .is_ok());
    }

    #[test]
    fn add_prefix_recomputes_exact_weighted_pool_after_each_uniform_removal() {
        let mut blocker = candidate(GenerationType::Suffix, 100, "shared");
        blocker.name = "blocker".to_string();
        let data = db([
            ("blocker", blocker),
            (
                "other",
                candidate(GenerationType::Suffix, 100, "other_group"),
            ),
            ("prefix_a", candidate(GenerationType::Prefix, 300, "shared")),
            ("prefix_b", candidate(GenerationType::Prefix, 100, "free")),
        ]);
        let mut item = rare_item();
        item.suffixes
            .push(modifier("blocker", GenerationType::Suffix));
        item.suffixes
            .push(modifier("other", GenerationType::Suffix));

        let outcomes = craft(BestiaryAffixSwapKind::AddPrefixRemoveSuffix)
            .apply(&item, &data, &mut StdRng::seed_from_u64(1))
            .expect("valid swap should enumerate");

        assert_eq!(outcomes.len(), 3);
        assert!((outcomes.iter().map(|(_, p)| p).sum::<f64>() - 1.0).abs() < 1e-12);
        assert!((probability_for(&outcomes, "other", "prefix_a") - 0.375).abs() < 1e-12);
        assert!((probability_for(&outcomes, "other", "prefix_b") - 0.125).abs() < 1e-12);
        assert!((probability_for(&outcomes, "blocker", "prefix_b") - 0.5).abs() < 1e-12);
        assert_eq!(probability_for(&outcomes, "blocker", "prefix_a"), 0.0);
    }

    #[test]
    fn crafted_source_affix_is_an_equal_removal_choice() {
        let mut crafted = candidate(GenerationType::Suffix, 0, "crafted_group");
        crafted.domain = Domain::Crafted;
        let data = db([
            (
                "natural",
                candidate(GenerationType::Suffix, 100, "natural_group"),
            ),
            ("crafted", crafted),
            (
                "prefix_a",
                candidate(GenerationType::Prefix, 100, "prefix_a_group"),
            ),
            (
                "prefix_b",
                candidate(GenerationType::Prefix, 300, "prefix_b_group"),
            ),
        ]);
        let mut item = rare_item();
        item.suffixes
            .push(modifier("natural", GenerationType::Suffix));
        item.crafted_mod = Some(modifier("crafted", GenerationType::Suffix));

        let outcomes = craft(BestiaryAffixSwapKind::AddPrefixRemoveSuffix)
            .apply(&item, &data, &mut StdRng::seed_from_u64(2))
            .expect("crafted suffix should be removable");

        assert_eq!(outcomes.len(), 4);
        let crafted_removed: f64 = outcomes
            .iter()
            .filter(|(state, _)| state.crafted_mod.is_none())
            .map(|(_, probability)| probability)
            .sum();
        let natural_removed: f64 = outcomes
            .iter()
            .filter(|(state, _)| state.suffixes.is_empty())
            .map(|(_, probability)| probability)
            .sum();
        assert!((crafted_removed - 0.5).abs() < 1e-12);
        assert!((natural_removed - 0.5).abs() < 1e-12);
    }

    #[test]
    fn reverse_craft_removes_prefix_and_only_adds_suffix() {
        let data = db([
            (
                "source",
                candidate(GenerationType::Prefix, 100, "source_group"),
            ),
            (
                "wrong_side",
                candidate(GenerationType::Prefix, 10_000, "wrong_group"),
            ),
            (
                "suffix",
                candidate(GenerationType::Suffix, 100, "suffix_group"),
            ),
        ]);
        let mut item = rare_item();
        item.prefixes
            .push(modifier("source", GenerationType::Prefix));

        let outcomes = craft(BestiaryAffixSwapKind::AddSuffixRemovePrefix)
            .apply(&item, &data, &mut StdRng::seed_from_u64(3))
            .expect("reverse swap should apply");

        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].0.prefixes.is_empty());
        assert_eq!(outcomes[0].0.suffixes[0].mod_id, "suffix");
        assert_eq!(outcomes[0].1, 1.0);
    }

    #[test]
    fn preserves_rarity_implicits_fractures_and_unaffected_affixes() {
        let data = db([
            (
                "source",
                candidate(GenerationType::Suffix, 100, "source_group"),
            ),
            (
                "kept_prefix",
                candidate(GenerationType::Prefix, 0, "kept_group"),
            ),
            (
                "added_prefix",
                candidate(GenerationType::Prefix, 100, "added_group"),
            ),
        ]);
        let mut item = rare_item();
        item.prefixes
            .push(modifier("kept_prefix", GenerationType::Prefix));
        item.suffixes
            .push(modifier("source", GenerationType::Suffix));
        item.fractured
            .push(modifier("fractured_suffix", GenerationType::Suffix));
        item.exarch_implicit = Some(modifier("exarch", GenerationType::ExarchImplicit));
        item.eater_implicit = Some(modifier("eater", GenerationType::EaterImplicit));

        let outcomes = craft(BestiaryAffixSwapKind::AddPrefixRemoveSuffix)
            .apply(&item, &data, &mut StdRng::seed_from_u64(4))
            .expect("swap should preserve unrelated state");
        let next = &outcomes[0].0;

        assert_eq!(next.rarity, Rarity::Rare);
        assert_eq!(next.base_id, item.base_id);
        assert_eq!(next.base_tags, item.base_tags);
        assert_eq!(next.item_level, item.item_level);
        assert_eq!(next.fractured, item.fractured);
        assert_eq!(next.exarch_implicit, item.exarch_implicit);
        assert_eq!(next.eater_implicit, item.eater_implicit);
        assert!(next
            .prefixes
            .iter()
            .any(|modifier| modifier.mod_id == "kept_prefix"));
    }

    #[test]
    fn fractures_are_never_removal_candidates() {
        let data = db([(
            "prefix",
            candidate(GenerationType::Prefix, 100, "prefix_group"),
        )]);
        let mut item = rare_item();
        item.fractured
            .push(modifier("fractured_suffix", GenerationType::Suffix));

        assert!(!craft(BestiaryAffixSwapKind::AddPrefixRemoveSuffix).can_apply(&item, &data));

        item.suffixes
            .push(modifier("natural_suffix", GenerationType::Suffix));
        let outcomes = craft(BestiaryAffixSwapKind::AddPrefixRemoveSuffix)
            .apply(&item, &data, &mut StdRng::seed_from_u64(5))
            .expect("natural suffix should enable the craft");
        assert!(outcomes
            .iter()
            .all(|(state, _)| state.fractured == item.fractured));
    }

    #[test]
    fn requires_rare_items_and_respects_destination_capacity() {
        let data = db([(
            "prefix",
            candidate(GenerationType::Prefix, 100, "prefix_group"),
        )]);
        let mut item = ItemState::new_base("base", vec!["sword".to_string()], 86);
        item.rarity = Rarity::Magic;
        item.suffixes
            .push(modifier("source", GenerationType::Suffix));
        let swap = craft(BestiaryAffixSwapKind::AddPrefixRemoveSuffix);

        assert!(!swap.can_apply(&item, &data));
        item.rarity = Rarity::Rare;
        assert!(swap.can_apply(&item, &data));
        let next = swap
            .apply(&item, &data, &mut StdRng::seed_from_u64(6))
            .expect("rare item with an open prefix should work")
            .remove(0)
            .0;
        assert_eq!(next.rarity, Rarity::Rare);
        assert_eq!(next.prefixes.len(), 1);
        assert!(next.suffixes.is_empty());

        for index in 0..3 {
            item.prefixes.push(modifier(
                &format!("occupied_prefix_{index}"),
                GenerationType::Prefix,
            ));
        }
        assert!(!swap.can_apply(&item, &data));
    }

    #[test]
    fn rejects_noncraftable_rarities_and_item_flags() {
        let data = db([(
            "prefix",
            candidate(GenerationType::Prefix, 100, "prefix_group"),
        )]);
        let swap = craft(BestiaryAffixSwapKind::AddPrefixRemoveSuffix);
        let mut item = rare_item();
        item.suffixes
            .push(modifier("source", GenerationType::Suffix));

        item.rarity = Rarity::Normal;
        assert!(!swap.can_apply(&item, &data));
        item.rarity = Rarity::Unique;
        assert!(!swap.can_apply(&item, &data));
        item.rarity = Rarity::Rare;
        item.corrupted = true;
        assert!(!swap.can_apply(&item, &data));
        item.corrupted = false;
        item.mirrored = true;
        assert!(!swap.can_apply(&item, &data));
    }

    #[test]
    fn requires_a_candidate_after_every_possible_removal() {
        let mut tag_source = candidate(GenerationType::Suffix, 100, "tag_source");
        tag_source.adds_tags = vec!["enables_prefix".to_string()];
        let mut gated_prefix = candidate(GenerationType::Prefix, 100, "gated_prefix_group");
        gated_prefix.spawn_weights = vec![SpawnWeight {
            tag: "enables_prefix".to_string(),
            weight: 100,
        }];
        let data = db([
            ("tag_source", tag_source),
            (
                "other_source",
                candidate(GenerationType::Suffix, 100, "other_source"),
            ),
            ("gated_prefix", gated_prefix),
        ]);
        let mut item = rare_item();
        item.suffixes
            .push(modifier("tag_source", GenerationType::Suffix));
        item.suffixes
            .push(modifier("other_source", GenerationType::Suffix));

        let swap = craft(BestiaryAffixSwapKind::AddPrefixRemoveSuffix);
        assert!(
            !swap.can_apply(&item, &data),
            "removing the tag-granting suffix leaves no legal add candidate"
        );
        assert!(swap
            .apply(&item, &data, &mut StdRng::seed_from_u64(7))
            .is_err());
    }

    #[test]
    fn beast_level_controls_eligibility_without_changing_item_level() {
        let mut eligible = candidate(GenerationType::Prefix, 100, "eligible_group");
        eligible.required_level = 87;
        eligible.stats = vec![ModStat {
            id: "local_value".to_string(),
            min: 10,
            max: 20,
        }];
        let mut too_high = candidate(GenerationType::Prefix, 10_000, "high_group");
        too_high.required_level = 88;
        let data = db([("eligible", eligible), ("too_high", too_high)]);
        let mut item = rare_item();
        item.item_level = 1;
        item.suffixes
            .push(modifier("source", GenerationType::Suffix));

        let swap = craft_at_level(BestiaryAffixSwapKind::AddPrefixRemoveSuffix, 87);
        let outcomes = swap
            .apply(&item, &data, &mut StdRng::seed_from_u64(8))
            .expect("beast-level-eligible candidate should roll");

        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].0.item_level, 1);
        assert_eq!(outcomes[0].0.prefixes[0].mod_id, "eligible");
        assert!(matches!(
            outcomes[0].0.prefixes[0].rolls.as_slice(),
            [StatRoll {
                stat_id,
                value: 10..=20
            }] if stat_id == "local_value"
        ));
        assert!(!swap.weights_are_probabilities());
    }

    #[test]
    fn invalid_roll_ranges_return_an_error_instead_of_panicking() {
        let mut invalid = candidate(GenerationType::Prefix, 100, "invalid_group");
        invalid.stats = vec![ModStat {
            id: "broken".to_string(),
            min: 20,
            max: 10,
        }];
        let data = db([("invalid", invalid)]);
        let mut item = rare_item();
        item.suffixes
            .push(modifier("source", GenerationType::Suffix));

        let result = craft(BestiaryAffixSwapKind::AddPrefixRemoveSuffix).apply(
            &item,
            &data,
            &mut StdRng::seed_from_u64(9),
        );
        assert!(result.is_err());
    }
}
