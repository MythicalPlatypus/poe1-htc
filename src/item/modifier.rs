use crate::data::mods::{GenerationType, ModStat};

/// A rolled instance of a mod on an item — the mod ID plus its actual rolled values.
#[derive(Debug, Clone, PartialEq)]
pub struct Modifier {
    /// RePoE mod ID (key into `GameData::mods`).
    pub mod_id: String,

    /// Whether this is a prefix or suffix.
    pub generation_type: GenerationType,

    /// The actual rolled stat values (one per stat in the mod's stat list).
    pub rolls: Vec<StatRoll>,
}

/// One stat roll — the stat ID and the specific value rolled.
#[derive(Debug, Clone, PartialEq)]
pub struct StatRoll {
    pub stat_id: String,
    pub value: i32,
}

impl Modifier {
    /// Create a modifier with all stats rolled to their minimum values.
    /// Useful for deterministic testing.
    pub fn from_min_rolls(mod_id: impl Into<String>, gen_type: GenerationType, stats: &[ModStat]) -> Self {
        Self {
            mod_id: mod_id.into(),
            generation_type: gen_type,
            rolls: stats
                .iter()
                .map(|s| StatRoll { stat_id: s.id.clone(), value: s.min })
                .collect(),
        }
    }

    /// Create a modifier with all stats rolled to their maximum values.
    pub fn from_max_rolls(mod_id: impl Into<String>, gen_type: GenerationType, stats: &[ModStat]) -> Self {
        Self {
            mod_id: mod_id.into(),
            generation_type: gen_type,
            rolls: stats
                .iter()
                .map(|s| StatRoll { stat_id: s.id.clone(), value: s.max })
                .collect(),
        }
    }

    pub fn is_prefix(&self) -> bool {
        self.generation_type == GenerationType::Prefix
    }

    pub fn is_suffix(&self) -> bool {
        self.generation_type == GenerationType::Suffix
    }
}
