pub mod base_items;
pub mod crafting_catalogs;
pub mod loader;
pub mod mods;

use crafting_catalogs::{CraftingBenchCatalog, EssenceCatalog, FossilCatalog};

/// Central database holding all loaded RePoE data.
///
/// Construct via [`GameData::new`], which precomputes the craftable-mod index.
/// The `mods` and `base_items` maps are public for lookups but must not be
/// mutated after construction, or the index goes stale.
#[derive(Debug)]
pub struct GameData {
    pub mods: std::collections::HashMap<String, mods::Mod>,
    pub base_items: std::collections::HashMap<String, base_items::BaseItem>,
    /// Optional RePoE catalogs used to validate and configure named crafts.
    pub crafting_bench: Option<CraftingBenchCatalog>,
    pub essences: Option<EssenceCatalog>,
    pub fossils: Option<FossilCatalog>,
    /// IDs of all mods where `Mod::is_craftable()` — the hot-loop subset used by
    /// `eligible_mods*`. Sorted so pool iteration order is deterministic, which
    /// seeded (reproducible) searches rely on.
    craftable_ids: Vec<String>,
}

impl GameData {
    pub fn new(
        mods: std::collections::HashMap<String, mods::Mod>,
        base_items: std::collections::HashMap<String, base_items::BaseItem>,
    ) -> Self {
        let mut craftable_ids: Vec<String> = mods
            .iter()
            .filter(|(_, m)| m.is_craftable())
            .map(|(id, _)| id.clone())
            .collect();
        craftable_ids.sort();
        Self {
            mods,
            base_items,
            crafting_bench: None,
            essences: None,
            fossils: None,
            craftable_ids,
        }
    }

    pub fn with_crafting_catalogs(
        mut self,
        crafting_bench: Option<CraftingBenchCatalog>,
        essences: Option<EssenceCatalog>,
        fossils: Option<FossilCatalog>,
    ) -> Self {
        self.crafting_bench = crafting_bench;
        self.essences = essences;
        self.fossils = fossils;
        self
    }

    /// Iterate all normal-craftable mods (domain=item, prefix/suffix, not
    /// essence-only) in deterministic (sorted-ID) order. Skips the ~90% of
    /// `mods.json` that can never roll, so hot loops avoid a full-map scan.
    pub fn craftable_mods(&self) -> impl Iterator<Item = (&str, &mods::Mod)> {
        self.craftable_ids
            .iter()
            .filter_map(|id| self.mods.get(id).map(|m| (id.as_str(), m)))
    }
}
