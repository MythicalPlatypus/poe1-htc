//! Eldritch implicit system (Searing Exarch / Eater of Worlds implicits).
//! Applies to helmets, gloves, boots, and body armours.

use anyhow::{bail, Result};
use rand::rng;

use crate::data::GameData;
use crate::data::mods::GenerationType;
use crate::engine::mod_pool::{eligible_mods_eldritch, random_rolls_pub};
use crate::item::modifier::Modifier;
use crate::item::{ItemState, state::Rarity};
use super::CraftingMethod;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EldritchGod {
    SearingExarch,
    EaterOfWorlds,
}

/// Item base tags that can receive eldritch implicits.
const ELDRITCH_ITEM_TAGS: &[&str] = &["helmet", "gloves", "boots", "body_armour"];

fn eldritch_gen_type(god: EldritchGod) -> GenerationType {
    match god {
        EldritchGod::SearingExarch => GenerationType::ExarchImplicit,
        EldritchGod::EaterOfWorlds => GenerationType::EaterImplicit,
    }
}

fn item_supports_eldritch(item: &ItemState) -> bool {
    item.base_tags.iter().any(|t| ELDRITCH_ITEM_TAGS.contains(&t.as_str()))
}

// ── Eldritch Chaos Orb ───────────────────────────────────────────────────────

/// Eldritch Chaos Orb — rerolls the implicit for the specified god.
/// Returns one outcome per eligible implicit, weighted by spawn weight.
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
    fn cost_chaos(&self) -> f64 { 5.0 }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Rare && item_supports_eldritch(item)
    }

    fn apply(&self, item: &ItemState, db: &GameData) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) { bail!("Cannot apply {}", self.name()); }

        let gen_type = eldritch_gen_type(self.god);
        let pool = eligible_mods_eldritch(item, &gen_type, db);
        let total_weight: u64 = pool.iter().map(|(_, _, w)| *w as u64).sum();
        if total_weight == 0 {
            bail!("{}: no eligible eldritch implicits for this item", self.name());
        }

        let mut rng = rng();
        let outcomes = pool.iter().map(|(mod_id, picked, weight)| {
            let mut next = item.clone();
            let rolls = random_rolls_pub(&picked.stats, &mut rng);
            let modifier = Modifier {
                mod_id: mod_id.to_string(),
                generation_type: picked.generation_type.clone(),
                rolls,
            };
            match self.god {
                EldritchGod::SearingExarch => next.exarch_implicit = Some(modifier),
                EldritchGod::EaterOfWorlds => next.eater_implicit = Some(modifier),
            }
            (next, *weight as f64 / total_weight as f64)
        }).collect();

        Ok(outcomes)
    }
}

// ── Eldritch Exalted Orb ─────────────────────────────────────────────────────

/// Eldritch Exalted Orb — upgrades the implicit to the next tier.
/// Finds the mod with the same `mod_type` and the next-higher `required_level`.
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
    fn cost_chaos(&self) -> f64 { 20.0 }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        if !item.is_craftable() || item.rarity != Rarity::Rare { return false; }
        if !item_supports_eldritch(item) { return false; }
        match self.god {
            EldritchGod::SearingExarch => item.exarch_implicit.is_some(),
            EldritchGod::EaterOfWorlds => item.eater_implicit.is_some(),
        }
    }

    fn apply(&self, item: &ItemState, db: &GameData) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) { bail!("Cannot apply {}", self.name()); }

        let current_mod_id = match self.god {
            EldritchGod::SearingExarch => item.exarch_implicit.as_ref().map(|m| &m.mod_id),
            EldritchGod::EaterOfWorlds => item.eater_implicit.as_ref().map(|m| &m.mod_id),
        }.ok_or_else(|| anyhow::anyhow!("{}: implicit slot is empty", self.name()))?;

        let current_mod = db.mods.get(current_mod_id)
            .ok_or_else(|| anyhow::anyhow!("Implicit mod '{}' not in DB", current_mod_id))?;

        let gen_type = eldritch_gen_type(self.god);

        // Next-tier mod: same mod_type, same generation_type, lowest required_level above current.
        let upgrade = db.mods.iter()
            .filter(|(_, m)| {
                m.mod_type == current_mod.mod_type
                    && m.generation_type == gen_type
                    && m.required_level > current_mod.required_level
            })
            .min_by_key(|(_, m)| m.required_level);

        let (upgrade_id, upgrade_mod) = upgrade
            .ok_or_else(|| anyhow::anyhow!("{}: already at maximum tier", self.name()))?;

        let mut rng = rng();
        let mut next = item.clone();
        let rolls = random_rolls_pub(&upgrade_mod.stats, &mut rng);
        let modifier = Modifier {
            mod_id: upgrade_id.clone(),
            generation_type: upgrade_mod.generation_type.clone(),
            rolls,
        };
        match self.god {
            EldritchGod::SearingExarch => next.exarch_implicit = Some(modifier),
            EldritchGod::EaterOfWorlds => next.eater_implicit = Some(modifier),
        }

        Ok(vec![(next, 1.0)])
    }
}
