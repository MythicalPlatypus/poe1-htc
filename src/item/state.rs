use crate::data::mods::GenerationType;
use super::Modifier;

/// Rarity of an item — governs prefix/suffix capacity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rarity {
    Normal,
    Magic,  // 1 prefix, 1 suffix
    Rare,   // up to 3 prefixes, 3 suffixes
    Unique,
}

/// Represents the complete state of an item at a point in the crafting sequence.
///
/// This struct is cloned for each branch in the beam search, so keep it lean.
#[derive(Debug, Clone)]
pub struct ItemState {
    /// RePoE base item ID (key into `GameData::base_items`).
    pub base_id: String,

    /// Tags inherited from the base item (used for mod pool filtering).
    pub base_tags: Vec<String>,

    /// Item level — controls which mods are eligible (mod.required_level <= item_level).
    pub item_level: u32,

    /// Current rarity.
    pub rarity: Rarity,

    /// Explicit modifiers currently on the item.
    pub prefixes: Vec<Modifier>,
    pub suffixes: Vec<Modifier>,

    /// Fractured mods (locked, cannot be removed by most currencies).
    pub fractured: Vec<Modifier>,

    /// Crafted mod (bench craft), if any — at most one.
    pub crafted_mod: Option<Modifier>,

    /// Whether the item is corrupted (most crafting methods are blocked).
    pub corrupted: bool,

    /// Whether the item is mirrored (cannot be modified at all).
    pub mirrored: bool,

    /// Searing Exarch eldritch implicit, if present.
    pub exarch_implicit: Option<Modifier>,

    /// Eater of Worlds eldritch implicit, if present.
    pub eater_implicit: Option<Modifier>,
}

impl ItemState {
    /// Create a fresh Normal-rarity base with no mods.
    pub fn new_base(base_id: impl Into<String>, base_tags: Vec<String>, item_level: u32) -> Self {
        Self {
            base_id: base_id.into(),
            base_tags,
            item_level,
            rarity: Rarity::Normal,
            prefixes: Vec::new(),
            suffixes: Vec::new(),
            fractured: Vec::new(),
            crafted_mod: None,
            corrupted: false,
            mirrored: false,
            exarch_implicit: None,
            eater_implicit: None,
        }
    }

    // --- Capacity checks ---

    pub fn prefix_count(&self) -> usize {
        self.prefixes.len()
            + self.fractured.iter().filter(|m| m.generation_type == GenerationType::Prefix).count()
            + self.crafted_mod.as_ref()
                .filter(|m| m.generation_type == GenerationType::Prefix)
                .map_or(0, |_| 1)
    }

    pub fn suffix_count(&self) -> usize {
        self.suffixes.len()
            + self.fractured.iter().filter(|m| m.generation_type == GenerationType::Suffix).count()
            + self.crafted_mod.as_ref()
                .filter(|m| m.generation_type == GenerationType::Suffix)
                .map_or(0, |_| 1)
    }

    pub fn max_prefixes(&self) -> usize {
        match self.rarity {
            Rarity::Magic => 1,
            Rarity::Rare | Rarity::Unique => 3,
            Rarity::Normal => 0,
        }
    }

    pub fn max_suffixes(&self) -> usize {
        match self.rarity {
            Rarity::Magic => 1,
            Rarity::Rare | Rarity::Unique => 3,
            Rarity::Normal => 0,
        }
    }

    pub fn has_open_prefix(&self) -> bool {
        self.prefix_count() < self.max_prefixes()
    }

    pub fn has_open_suffix(&self) -> bool {
        self.suffix_count() < self.max_suffixes()
    }

    pub fn is_full(&self) -> bool {
        !self.has_open_prefix() && !self.has_open_suffix()
    }

    /// Prefix + suffix explicit mods only (not fractured or crafted).
    /// Used by operations like Annulment that can only remove these.
    pub fn all_explicit_mods(&self) -> impl Iterator<Item = &Modifier> {
        self.prefixes.iter().chain(self.suffixes.iter())
    }

    /// All mods that occupy affix slots and participate in group-conflict checks:
    /// prefixes, suffixes, fractured mods, and the crafted mod (if present).
    pub fn all_mods_for_conflict(&self) -> impl Iterator<Item = &Modifier> {
        self.prefixes.iter()
            .chain(self.suffixes.iter())
            .chain(self.fractured.iter())
            .chain(self.crafted_mod.iter())
    }

    /// Total mod count (explicit + crafted bench mod).
    pub fn mod_count(&self) -> usize {
        self.prefixes.len() + self.suffixes.len() + self.crafted_mod.as_ref().map_or(0, |_| 1)
    }

    /// Whether the item can currently be crafted on (not corrupted/mirrored).
    pub fn is_craftable(&self) -> bool {
        !self.corrupted && !self.mirrored
    }
}
