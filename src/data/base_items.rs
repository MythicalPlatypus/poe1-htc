use serde::{Deserialize, Serialize};

/// A base item entry from RePoE's `base_items.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseItem {
    /// Display name (e.g. "Astral Plate").
    pub name: String,

    /// Item class (e.g. "Body Armours", "Swords").
    pub item_class: String,

    /// Tags that determine which mods can spawn on this item.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Implicit mods granted by the base.
    #[serde(default)]
    pub implicits: Vec<String>,

    /// Required level to equip.
    #[serde(default)]
    pub drop_level: u32,

    /// Item height in stash grid cells.
    #[serde(default)]
    pub inventory_height: u32,

    /// Item width in stash grid cells.
    #[serde(default)]
    pub inventory_width: u32,
}
