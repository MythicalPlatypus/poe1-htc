//! Fracturing Orb support.
//!
//! A Fracturing Orb locks one uniformly selected explicit modifier on an
//! eligible rare item. It does not reroll the item.

use anyhow::{bail, Result};
use rand::RngCore;

use super::CraftingMethod;
use crate::data::GameData;
use crate::item::{state::Rarity, ItemState};

const INFLUENCE_TAGS: &[&str] = &[
    "shaper_item",
    "elder_item",
    "crusader_item",
    "hunter_item",
    "redeemer_item",
    "warlord_item",
];

/// Fractures one random explicit modifier on an eligible rare item.
#[derive(Debug, Clone, Copy)]
pub struct FracturingOrb;

impl FracturingOrb {
    fn has_influence(item: &ItemState) -> bool {
        item.base_tags
            .iter()
            .any(|tag| INFLUENCE_TAGS.contains(&tag.as_str()))
    }

    fn candidate_count(item: &ItemState) -> usize {
        item.prefixes.len() + item.suffixes.len() + usize::from(item.crafted_mod.is_some())
    }
}

impl CraftingMethod for FracturingOrb {
    fn name(&self) -> &str {
        "Fracturing Orb"
    }

    fn cost_chaos(&self) -> f64 {
        250.0
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable()
            && item.rarity == Rarity::Rare
            && item.fractured.is_empty()
            && !Self::has_influence(item)
            && Self::candidate_count(item) >= 4
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        _rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!(
                "Fracturing Orb requires a craftable, non-influenced, unfractured Rare item with at least 4 explicit modifiers"
            );
        }

        let candidate_count = Self::candidate_count(item);
        let probability = 1.0 / candidate_count as f64;
        let mut outcomes = Vec::with_capacity(candidate_count);

        for index in 0..item.prefixes.len() {
            let mut next = item.clone();
            let fractured = next.prefixes.remove(index);
            next.fractured.push(fractured);
            outcomes.push((next, probability));
        }

        for index in 0..item.suffixes.len() {
            let mut next = item.clone();
            let fractured = next.suffixes.remove(index);
            next.fractured.push(fractured);
            outcomes.push((next, probability));
        }

        if item.crafted_mod.is_some() {
            let mut next = item.clone();
            let fractured = next
                .crafted_mod
                .take()
                .ok_or_else(|| anyhow::anyhow!("crafted fracture candidate disappeared"))?;
            next.fractured.push(fractured);
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
    use crate::data::mods::{Domain, GenerationType, Mod, SpawnWeight};
    use crate::item::Modifier;

    fn modifier(id: &str, generation_type: GenerationType) -> Modifier {
        Modifier {
            mod_id: id.to_string(),
            generation_type,
            rolls: vec![],
        }
    }

    fn db() -> GameData {
        let mut mods = HashMap::new();
        for (id, generation_type, domain) in [
            ("p1", GenerationType::Prefix, Domain::Item),
            ("p2", GenerationType::Prefix, Domain::Item),
            ("s1", GenerationType::Suffix, Domain::Item),
            ("crafted", GenerationType::Suffix, Domain::Crafted),
        ] {
            mods.insert(
                id.to_string(),
                Mod {
                    name: id.to_string(),
                    generation_type,
                    required_level: 1,
                    stats: vec![],
                    spawn_weights: vec![SpawnWeight {
                        tag: "default".to_string(),
                        weight: 100,
                    }],
                    generation_weights: vec![],
                    adds_tags: vec![],
                    tags: vec![],
                    domain,
                    mod_type: id.to_string(),
                    groups: vec![id.to_string()],
                    is_essence_only: false,
                    text: None,
                },
            );
        }
        GameData::new(mods, HashMap::new())
    }

    fn eligible_item() -> ItemState {
        let mut item = ItemState::new_base("base", vec!["sword".to_string()], 86);
        item.rarity = Rarity::Rare;
        item.prefixes.push(modifier("p1", GenerationType::Prefix));
        item.prefixes.push(modifier("p2", GenerationType::Prefix));
        item.suffixes.push(modifier("s1", GenerationType::Suffix));
        item.crafted_mod = Some(modifier("crafted", GenerationType::Suffix));
        item
    }

    #[test]
    fn enumerates_every_explicit_with_equal_probability() {
        let item = eligible_item();
        let outcomes = FracturingOrb
            .apply(&item, &db(), &mut StdRng::seed_from_u64(1))
            .expect("fracturing should succeed");

        assert_eq!(outcomes.len(), 4);
        assert!(outcomes
            .iter()
            .all(|(_, probability)| (*probability - 0.25).abs() < f64::EPSILON));
        assert!(outcomes.iter().all(|(state, _)| state.fractured.len() == 1));
        assert!(outcomes.iter().any(
            |(state, _)| state.fractured[0].mod_id == "crafted" && state.crafted_mod.is_none()
        ));
    }

    #[test]
    fn rejects_influenced_or_already_fractured_items() {
        let data = db();
        let mut item = eligible_item();
        item.base_tags.push("shaper_item".to_string());
        assert!(!FracturingOrb.can_apply(&item, &data));

        item.base_tags.pop();
        item.fractured.push(modifier("p1", GenerationType::Prefix));
        assert!(!FracturingOrb.can_apply(&item, &data));
    }
}
