pub mod base_items;
pub mod loader;
pub mod mods;

/// Central database holding all loaded RePoE data.
///
/// Construct via [`GameData::new`], which precomputes the craftable-mod index.
/// The `mods` and `base_items` maps are public for lookups but must not be
/// mutated after construction, or the index goes stale.
pub struct GameData {
    pub mods: std::collections::HashMap<String, mods::Mod>,
    pub base_items: std::collections::HashMap<String, base_items::BaseItem>,
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
            craftable_ids,
        }
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
