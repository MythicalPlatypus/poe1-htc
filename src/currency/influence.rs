//! Influenced item crafting: Shaper, Elder, Crusader, Hunter, Redeemer, Warlord.
//! Influenced items have access to exclusive mod pools in addition to regular mods.

use anyhow::{bail, Result};
use crate::data::GameData;
use crate::item::{ItemState, state::Rarity};
use super::CraftingMethod;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Influence {
    Shaper,
    Elder,
    Crusader,  // Warlord's Mark
    Hunter,    // Hunter's Mark
    Redeemer,  // Redeemer's Mark
    Warlord,   // Warlord's Mark (alternate naming)
}

/// Adds an influence tag to a Normal-rarity item (e.g. via an Awakener's Orb outcome,
/// or by acquiring a naturally-influenced base).
pub struct ApplyInfluence {
    pub influence: Influence,
}

impl CraftingMethod for ApplyInfluence {
    fn name(&self) -> &str { "Apply Influence" }
    fn cost_chaos(&self) -> f64 { 0.0 } // Cost is accounted for at the base acquisition stage

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Normal
    }

    fn apply(&self, item: &ItemState, db: &GameData) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) { bail!("Cannot apply influence to this item"); }
        let mut next = item.clone();
        let tag = influence_tag(self.influence);
        if !next.base_tags.contains(&tag.to_string()) {
            next.base_tags.push(tag.to_string());
        }
        Ok(vec![(next, 1.0)])
    }
}

/// Awakener's Orb — merges two influenced items, transferring one influence mod
/// from each onto a new base. Both source items are consumed.
pub struct AwakenersOrb {
    pub source_influence_a: Influence,
    pub source_influence_b: Influence,
}

impl CraftingMethod for AwakenersOrb {
    fn name(&self) -> &str { "Awakener's Orb" }
    fn cost_chaos(&self) -> f64 { 200.0 }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.rarity == Rarity::Normal
    }

    fn apply(&self, item: &ItemState, db: &GameData) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) { bail!("Cannot apply Awakener's Orb"); }
        let mut next = item.clone();
        next.rarity = Rarity::Rare;
        for influence in [self.source_influence_a, self.source_influence_b] {
            let tag = influence_tag(influence).to_string();
            if !next.base_tags.contains(&tag) {
                next.base_tags.push(tag);
            }
        }
        // TODO: transfer one exclusive influence mod from each source item
        Ok(vec![(next, 1.0)])
    }
}

fn influence_tag(influence: Influence) -> &'static str {
    match influence {
        Influence::Shaper   => "shaper_item",
        Influence::Elder    => "elder_item",
        Influence::Crusader => "crusader_item",
        Influence::Hunter   => "hunter_item",
        Influence::Redeemer => "redeemer_item",
        Influence::Warlord  => "warlord_item",
    }
}
