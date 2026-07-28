//! Crafting bench — deterministically adds one chosen crafted mod.
//!
//! Bench mods live in `mods.json` with `domain = "crafted"`; they are excluded
//! from all random pools (`Mod::is_craftable()` is false for them) and can only
//! be placed by this method. An item holds at most one crafted mod
//! (`ItemState::crafted_mod`), and the crafted mod occupies a normal affix slot
//! and participates in group-conflict checks like any other mod.

use super::{
    CraftingMethod, ItemClassSupport, MethodCatalog, MethodFamily, MethodId, MethodSetup,
    ProbabilityModel,
};
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
        let has_fractured_crafted_mod = item.fractured.iter().any(|modifier| {
            db.mods
                .get(&modifier.mod_id)
                .is_some_and(|m| m.domain == Domain::Crafted)
        });
        if item.crafted_mod.is_some() || has_fractured_crafted_mod {
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

/// Removes the item's removable bench-crafted modifier.
#[derive(Debug, Clone, Copy)]
pub struct RemoveCraftedMods;

impl CraftingMethod for RemoveCraftedMods {
    fn id(&self) -> MethodId {
        MethodId::semantic("bench", "remove-crafted", &[])
    }

    fn family(&self) -> MethodFamily {
        MethodFamily::Bench
    }

    fn description(&self) -> &str {
        "Removes the item's removable bench-crafted modifier."
    }

    fn name(&self) -> &str {
        "Remove Crafted Mods"
    }

    fn cost_chaos(&self) -> f64 {
        1.0
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && item.crafted_mod.is_some()
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        _rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Remove Crafted Mods requires a removable crafted modifier");
        }
        let mut next = item.clone();
        next.crafted_mod = None;
        Ok(vec![(next, 1.0)])
    }
}

impl CraftingMethod for BenchCraft {
    fn id(&self) -> MethodId {
        MethodId::semantic("bench", "add-explicit", &[&self.mod_id])
    }

    fn family(&self) -> MethodFamily {
        MethodFamily::Bench
    }

    fn description(&self) -> &str {
        "Adds one configured crafted modifier, consuming the item's crafted-mod slot."
    }

    fn setup(&self) -> MethodSetup {
        MethodSetup::CatalogOrConfigured(MethodCatalog::CraftingBench)
    }

    fn item_class_support(&self) -> ItemClassSupport {
        ItemClassSupport::CatalogRestricted
    }

    fn name(&self) -> &str {
        &self.display_name
    }
    fn cost_chaos(&self) -> f64 {
        self.cost_chaos
    }

    fn provided_mod_ids(&self) -> Vec<&str> {
        vec![self.mod_id.as_str()]
    }

    fn weights_are_probabilities(&self) -> bool {
        false
    }

    fn probability_model(&self) -> ProbabilityModel {
        ProbabilityModel::ExactIdentitySampledRolls
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
