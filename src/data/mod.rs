pub mod loader;
pub mod mods;
pub mod base_items;

/// Central database holding all loaded RePoE data.
pub struct GameData {
    pub mods: std::collections::HashMap<String, mods::Mod>,
    pub base_items: std::collections::HashMap<String, base_items::BaseItem>,
}
