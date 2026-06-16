//! Essence crafting — guarantees one specific mod while rerolling the rest as a Chaos Orb.

use anyhow::{bail, Result};
use rand::{rng, Rng};

use crate::data::GameData;
use crate::data::mods::GenerationType;
use crate::engine::mod_pool::{random_rolls_pub, roll_mods};
use crate::item::modifier::Modifier;
use crate::item::{ItemState, state::Rarity};
use super::{CraftingMethod, MONTE_CARLO_SAMPLES};

/// An Essence application: guarantees `guaranteed_mod_id` on the item.
pub struct Essence {
    pub display_name: String,
    /// RePoE mod ID that this Essence guarantees.
    pub guaranteed_mod_id: String,
    /// Approximate chaos cost.
    pub cost_chaos: f64,
}

impl CraftingMethod for Essence {
    fn name(&self) -> &str { &self.display_name }
    fn cost_chaos(&self) -> f64 { self.cost_chaos }

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        item.is_craftable()
            && matches!(item.rarity, Rarity::Normal | Rarity::Magic | Rarity::Rare)
            && db.mods.contains_key(&self.guaranteed_mod_id)
    }

    fn apply(&self, item: &ItemState, db: &GameData) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) { bail!("Cannot apply {}", self.display_name); }

        let guaranteed = db.mods.get(&self.guaranteed_mod_id)
            .ok_or_else(|| anyhow::anyhow!("Essence mod '{}' not found in DB", self.guaranteed_mod_id))?;

        // Validate generation_type once before sampling.
        if !matches!(guaranteed.generation_type, GenerationType::Prefix | GenerationType::Suffix) {
            bail!(
                "{}: guaranteed mod '{}' is not a prefix or suffix",
                self.display_name,
                self.guaranteed_mod_id
            );
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

            // Place the guaranteed mod first.
            let rolls = random_rolls_pub(&guaranteed.stats, &mut rng);
            let forced = Modifier {
                mod_id: self.guaranteed_mod_id.clone(),
                generation_type: guaranteed.generation_type.clone(),
                rolls,
            };
            match guaranteed.generation_type {
                GenerationType::Prefix => next.prefixes.push(forced),
                GenerationType::Suffix => next.suffixes.push(forced),
                _ => unreachable!("validated above"),
            }

            // Fill remaining slots (4–6 total like Chaos Orb; 1 already placed).
            let total: usize = rng.random_range(4..=6);
            roll_mods(&mut next, total - 1, db, &mut rng)?;
            outcomes.push((next, prob));
        }

        Ok(outcomes)
    }
}
