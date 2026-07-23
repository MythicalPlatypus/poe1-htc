//! Current Harvest crafts that materially affect explicit modifiers.
//!
//! The removed legacy "add", "remove", and "remove/add the same tag" crafts
//! are intentionally not modeled. The supported operations match the current
//! item crafts:
//!
//! - reforge a Rare item, including a modifier with a selected tag;
//! - add a selected-tag modifier and remove another random modifier from a
//!   non-influenced item.

use anyhow::{bail, Result};
use rand::RngCore;

use super::{random_rare_affix_count, CraftingMethod, RerollKind, MONTE_CARLO_SAMPLES};
use crate::data::mods::GenerationType;
use crate::data::GameData;
use crate::engine::mod_pool::{
    eligible_mods_harvest_tag, random_rolls_pub, roll_mods, weighted_pick,
};
use crate::item::modifier::Modifier;
use crate::item::{state::Rarity, ItemState};

const INFLUENCE_TAGS: &[&str] = &[
    "shaper_item",
    "elder_item",
    "crusader_item",
    "hunter_item",
    "redeemer_item",
    "warlord_item",
];

/// The category of modifiers targeted by a Harvest operation.
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
    Minion,
    Mana,
}

impl HarvestTarget {
    /// RePoE modifier tag corresponding to this Harvest target.
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
            Self::Minion => "minion",
            Self::Mana => "mana",
        }
    }

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
            "minion" => Some(Self::Minion),
            "mana" => Some(Self::Mana),
            _ => None,
        }
    }
}

/// Supported current Harvest item operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarvestOp {
    /// Reforge a Rare item and guarantee at least one modifier of the target tag.
    Reforge,
    /// Add a target-tag modifier and remove another random modifier.
    Augment,
}

impl HarvestOp {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reforge" => Some(Self::Reforge),
            "augment" => Some(Self::Augment),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HarvestCraft {
    pub display_name: String,
    pub cost_chaos: f64,
    pub target: HarvestTarget,
    pub op: HarvestOp,
}

impl HarvestCraft {
    fn has_influence(item: &ItemState) -> bool {
        item.base_tags
            .iter()
            .any(|tag| INFLUENCE_TAGS.contains(&tag.as_str()))
    }

    fn removable_count(item: &ItemState) -> usize {
        item.prefixes.len() + item.suffixes.len() + usize::from(item.crafted_mod.is_some())
    }
}

impl CraftingMethod for HarvestCraft {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn cost_chaos(&self) -> f64 {
        self.cost_chaos
    }

    fn weights_are_probabilities(&self) -> bool {
        false
    }

    fn repeatable_on_failure(&self) -> bool {
        self.op == HarvestOp::Reforge
    }
    fn reroll_kind(&self) -> Option<RerollKind> {
        (self.op == HarvestOp::Reforge).then_some(RerollKind::RareExplicit)
    }
    fn consumes_reroll_initializer(&self) -> bool {
        self.op == HarvestOp::Reforge
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        if !item.is_craftable() || item.rarity != Rarity::Rare {
            return false;
        }
        match self.op {
            HarvestOp::Reforge => true,
            HarvestOp::Augment => !Self::has_influence(item) && Self::removable_count(item) > 0,
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
        match self.op {
            HarvestOp::Reforge => harvest_reforge(item, self.target.as_tag(), db, rng),
            HarvestOp::Augment => harvest_augment(item, self.target.as_tag(), db, rng),
        }
    }
}

fn add_modifier(
    item: &mut ItemState,
    mod_id: &str,
    picked: &crate::data::mods::Mod,
    rng: &mut dyn RngCore,
) -> Result<()> {
    let modifier = Modifier {
        mod_id: mod_id.to_string(),
        generation_type: picked.generation_type.clone(),
        rolls: random_rolls_pub(&picked.stats, rng),
    };
    match picked.generation_type {
        GenerationType::Prefix => item.prefixes.push(modifier),
        GenerationType::Suffix => item.suffixes.push(modifier),
        _ => bail!("Harvest selected non-affix mod '{mod_id}'"),
    }
    Ok(())
}

fn harvest_reforge(
    item: &ItemState,
    tag: &str,
    db: &GameData,
    rng: &mut dyn RngCore,
) -> Result<Vec<(ItemState, f64)>> {
    let sample_weight = 1.0 / MONTE_CARLO_SAMPLES as f64;
    let mut outcomes = Vec::with_capacity(MONTE_CARLO_SAMPLES);

    for _ in 0..MONTE_CARLO_SAMPLES {
        let mut next = item.clone();
        next.prefixes.clear();
        next.suffixes.clear();
        next.crafted_mod = None;

        let target_pool = eligible_mods_harvest_tag(&next, tag, db);
        let (mod_id, picked) = weighted_pick(&target_pool, rng)
            .ok_or_else(|| anyhow::anyhow!("no eligible '{tag}' modifier for Harvest reforge"))?;
        add_modifier(&mut next, mod_id, picked, rng)?;

        let desired_total = random_rare_affix_count(rng);
        let remaining = desired_total.saturating_sub(next.mod_count());
        roll_mods(&mut next, remaining, db, rng)?;
        outcomes.push((next, sample_weight));
    }

    Ok(outcomes)
}

fn harvest_augment(
    item: &ItemState,
    tag: &str,
    db: &GameData,
    rng: &mut dyn RngCore,
) -> Result<Vec<(ItemState, f64)>> {
    let removal_count = HarvestCraft::removable_count(item);
    if removal_count == 0 {
        bail!("Harvest augment requires a removable modifier");
    }
    let removal_weight = 1.0 / removal_count as f64;
    let mut outcomes = Vec::new();

    for prefix_index in 0..item.prefixes.len() {
        let mut removed = item.clone();
        removed.prefixes.remove(prefix_index);
        append_augment_outcomes(removed, tag, removal_weight, db, rng, &mut outcomes)?;
    }
    for suffix_index in 0..item.suffixes.len() {
        let mut removed = item.clone();
        removed.suffixes.remove(suffix_index);
        append_augment_outcomes(removed, tag, removal_weight, db, rng, &mut outcomes)?;
    }
    if item.crafted_mod.is_some() {
        let mut removed = item.clone();
        removed.crafted_mod = None;
        append_augment_outcomes(removed, tag, removal_weight, db, rng, &mut outcomes)?;
    }

    let total: f64 = outcomes.iter().map(|(_, weight)| *weight).sum();
    if total <= 0.0 || !total.is_finite() {
        bail!("no eligible '{tag}' modifier after Harvest removal");
    }
    for (_, weight) in &mut outcomes {
        *weight /= total;
    }
    Ok(outcomes)
}

fn append_augment_outcomes(
    removed: ItemState,
    tag: &str,
    removal_weight: f64,
    db: &GameData,
    rng: &mut dyn RngCore,
    outcomes: &mut Vec<(ItemState, f64)>,
) -> Result<()> {
    let pool = eligible_mods_harvest_tag(&removed, tag, db);
    let total_weight: u64 = pool.iter().map(|(_, _, weight)| u64::from(*weight)).sum();
    if total_weight == 0 {
        return Ok(());
    }

    for (mod_id, picked, weight) in pool {
        let mut next = removed.clone();
        add_modifier(&mut next, mod_id, picked, rng)?;
        outcomes.push((
            next,
            removal_weight * f64::from(weight) / total_weight as f64,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rand::rngs::StdRng;
    use rand::SeedableRng;

    use super::*;
    use crate::data::mods::{Domain, Mod, ModStat, SpawnWeight};

    fn tagged_mod(id: &str, generation_type: GenerationType, weight: u32) -> Mod {
        Mod {
            name: id.to_string(),
            generation_type,
            required_level: 1,
            stats: vec![ModStat {
                id: "value".to_string(),
                min: 1,
                max: 2,
            }],
            spawn_weights: vec![SpawnWeight {
                tag: "sword".to_string(),
                weight,
            }],
            generation_weights: vec![],
            adds_tags: vec![],
            tags: vec!["life".to_string()],
            domain: Domain::Item,
            mod_type: id.to_string(),
            groups: vec![id.to_string()],
            is_essence_only: false,
        }
    }

    fn db() -> GameData {
        let mut mods = HashMap::new();
        mods.insert(
            "life_prefix".to_string(),
            tagged_mod("life_prefix", GenerationType::Prefix, 100),
        );
        mods.insert(
            "life_suffix".to_string(),
            tagged_mod("life_suffix", GenerationType::Suffix, 300),
        );
        GameData::new(mods, HashMap::new())
    }

    fn rare_item() -> ItemState {
        let mut item = ItemState::new_base("base", vec!["sword".to_string()], 86);
        item.rarity = Rarity::Rare;
        item.prefixes.push(Modifier {
            mod_id: "junk".to_string(),
            generation_type: GenerationType::Prefix,
            rolls: vec![],
        });
        item
    }

    #[test]
    fn reforge_always_contains_target_tag() {
        let data = db();
        let craft = HarvestCraft {
            display_name: "Harvest reforge life".to_string(),
            cost_chaos: 5.0,
            target: HarvestTarget::Life,
            op: HarvestOp::Reforge,
        };
        let outcomes = craft
            .apply(&rare_item(), &data, &mut StdRng::seed_from_u64(2))
            .expect("reforge should succeed");
        assert_eq!(outcomes.len(), MONTE_CARLO_SAMPLES);
        assert!(outcomes.iter().all(|(state, _)| state
            .prefixes
            .iter()
            .chain(&state.suffixes)
            .any(|modifier| modifier.mod_id.starts_with("life_"))));
    }

    #[test]
    fn augment_removes_one_and_adds_weighted_target() {
        let data = db();
        let craft = HarvestCraft {
            display_name: "Harvest augment life".to_string(),
            cost_chaos: 100.0,
            target: HarvestTarget::Life,
            op: HarvestOp::Augment,
        };
        let outcomes = craft
            .apply(&rare_item(), &data, &mut StdRng::seed_from_u64(3))
            .expect("augment should succeed");
        assert_eq!(outcomes.len(), 2);
        let total: f64 = outcomes.iter().map(|(_, probability)| *probability).sum();
        assert!((total - 1.0).abs() < 1e-12);
        assert!(outcomes
            .iter()
            .all(|(state, _)| !state.prefixes.iter().any(|m| m.mod_id == "junk")));
    }

    #[test]
    fn augment_rejects_influenced_items() {
        let data = db();
        let craft = HarvestCraft {
            display_name: "Harvest augment life".to_string(),
            cost_chaos: 100.0,
            target: HarvestTarget::Life,
            op: HarvestOp::Augment,
        };
        let mut item = rare_item();
        item.base_tags.push("hunter_item".to_string());
        assert!(!craft.can_apply(&item, &data));
    }
}
