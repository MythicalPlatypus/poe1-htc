//! Crafting bench — deterministically adds one chosen crafted mod.
//!
//! Bench mods live in `mods.json` with `domain = "crafted"`; they are excluded
//! from all random pools (`Mod::is_craftable()` is false for them) and can only
//! be placed by this method. An item holds at most one crafted mod
//! (`ItemState::crafted_mod`), and the crafted mod occupies a normal affix slot
//! and participates in group-conflict checks like any other mod.

use super::CraftingMethod;
use crate::data::mods::{Domain, GenerationType};
use crate::data::GameData;
use crate::engine::mod_pool::random_rolls_pub;
use crate::item::modifier::Modifier;
use crate::item::{state::Rarity, ItemState};
use anyhow::{bail, Result};
use rand::RngCore;

/// One bench-craft option (e.g. "Craft: +# to maximum Life").
pub struct BenchCraft {
    pub display_name: String,
    /// RePoE mod ID with `domain = "crafted"`.
    pub mod_id: String,
    pub cost_chaos: f64,
}

impl BenchCraft {
    /// Full legality check, shared by `can_apply` and `apply`:
    /// mod exists, is a crafted-domain prefix/suffix, the item has no crafted
    /// mod yet, an affix slot of the right kind is open, and no group conflict.
    fn check(&self, item: &ItemState, db: &GameData) -> Result<()> {
        if !item.is_craftable() {
            bail!("{}: item cannot be modified", self.display_name);
        }
        if !matches!(item.rarity, Rarity::Magic | Rarity::Rare) {
            bail!(
                "{}: bench crafts require a Magic or Rare item",
                self.display_name
            );
        }
        if item.crafted_mod.is_some() {
            bail!("{}: item already has a crafted mod", self.display_name);
        }
        let m = db.mods.get(&self.mod_id).ok_or_else(|| {
            anyhow::anyhow!("{}: mod '{}' not in DB", self.display_name, self.mod_id)
        })?;
        if m.domain != Domain::Crafted {
            bail!(
                "{}: mod '{}' is not a bench mod (domain != crafted)",
                self.display_name,
                self.mod_id
            );
        }
        match m.generation_type {
            GenerationType::Prefix if !item.has_open_prefix() => {
                bail!("{}: no open prefix slot", self.display_name)
            }
            GenerationType::Suffix if !item.has_open_suffix() => {
                bail!("{}: no open suffix slot", self.display_name)
            }
            GenerationType::Prefix | GenerationType::Suffix => {}
            _ => bail!(
                "{}: mod '{}' is not a prefix or suffix",
                self.display_name,
                self.mod_id
            ),
        }
        let conflicts = item
            .all_mods_for_conflict()
            .filter_map(|im| db.mods.get(&im.mod_id))
            .flat_map(|im| im.groups.iter())
            .any(|g| m.groups.contains(g));
        if conflicts {
            bail!(
                "{}: mod '{}' shares a mod group with a mod on the item",
                self.display_name,
                self.mod_id
            );
        }
        Ok(())
    }
}

impl CraftingMethod for BenchCraft {
    fn name(&self) -> &str {
        &self.display_name
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
        let m = db.mods.get(&self.mod_id).ok_or_else(|| {
            anyhow::anyhow!("{}: mod '{}' not in DB", self.display_name, self.mod_id)
        })?;
        let mut next = item.clone();
        next.crafted_mod = Some(Modifier {
            mod_id: self.mod_id.clone(),
            generation_type: m.generation_type.clone(),
            rolls: random_rolls_pub(&m.stats, rng),
        });
        Ok(vec![(next, 1.0)])
    }
}
