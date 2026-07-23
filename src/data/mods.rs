use serde::{Deserialize, Serialize};

/// A single mod entry from RePoE's `mods.json`.
///
/// RePoE ships mods.json as a flat object keyed by the internal mod ID:
/// ```json
/// {
///   "AbyssAddedColdDamage1": { "name": "...", "generation_type": "suffix", ... },
///   ...
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mod {
    /// Human-readable name (e.g. "of the Penguin").
    pub name: String,

    /// How this mod is generated: "prefix", "suffix", "unique", "corrupted",
    /// "enchantment", "blight", "monster", "tempest".
    pub generation_type: GenerationType,

    /// Minimum item level required for this mod to spawn.
    pub required_level: u32,

    /// The stat roll(s) this mod provides.
    pub stats: Vec<ModStat>,

    /// Per-tag weights controlling how likely this mod is to spawn on an item.
    /// Weight 0 means the mod cannot spawn for that tag combination.
    pub spawn_weights: Vec<SpawnWeight>,

    /// Multiplicative adjustments to spawn weight from meta-crafting / fossils.
    #[serde(default)]
    pub generation_weights: Vec<GenerationWeight>,

    /// Tags this mod adds to the item when present (e.g. for further filtering).
    #[serde(default)]
    pub adds_tags: Vec<String>,

    /// Semantic modifier tags used by Harvest and related crafts.
    ///
    /// Current RePoE exports this field as `implicit_tags`; `tags` remains an
    /// accepted alias for older fixtures and exports.
    #[serde(default, rename = "implicit_tags", alias = "tags")]
    pub tags: Vec<String>,

    /// The domain this mod belongs to (controls which items it can appear on).
    pub domain: Domain,

    /// Mod type / group — mods sharing a type are mutually exclusive.
    #[serde(rename = "type")]
    pub mod_type: String,

    /// Groups this mod belongs to (for exclusivity checks).
    #[serde(default)]
    pub groups: Vec<String>,

    /// Whether this mod can only appear via Essences.
    #[serde(default)]
    pub is_essence_only: bool,
}

/// A single stat contribution from a mod.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModStat {
    /// RePoE stat ID (e.g. "minimum_added_cold_damage").
    pub id: String,

    /// Minimum roll value.
    pub min: i32,

    /// Maximum roll value.
    pub max: i32,
}

/// One entry in a mod's `spawn_weights` array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnWeight {
    /// Item tag (e.g. "sword", "str_armour", "default").
    pub tag: String,

    /// Spawn weight. 0 = cannot spawn. Relative to other mods' weights for the same tag.
    pub weight: u32,
}

/// One entry in a mod's `generation_weights` array.
/// These multiply the effective spawn weight (e.g. from fossils or essence reforges).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationWeight {
    pub tag: String,
    pub weight: u32,
}

/// Where a mod can appear.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Domain {
    Item,
    Chest,
    Monster,
    Area,
    Crafted,
    Veiled,
    Delve,
    Abyss,
    Map,
    Stance,
    Tempest,
    Leaguestone,
    Watchstone,
    Synthesis,
    HeistEquipment,
    HeistArea,
    Trinket,
    SentinelTag,
    MemoryLine,
    Affliction,
    Sanctum,
    Expedition,
    Necropolis,
    #[serde(other)]
    Unknown,
}

/// How a mod is placed onto an item.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GenerationType {
    Prefix,
    Suffix,
    Unique,
    Corrupted,
    Enchantment,
    Blight,
    Monster,
    Tempest,
    #[serde(rename = "searing_exarch_implicit", alias = "exarch_implicit")]
    ExarchImplicit,
    #[serde(rename = "eater_of_worlds_implicit", alias = "eater_implicit")]
    EaterImplicit,
    #[serde(other)]
    Unknown,
}

impl Mod {
    /// Returns true if this mod can appear on items via normal crafting
    /// (i.e. domain = item, not essence-only, prefix or suffix).
    pub fn is_craftable(&self) -> bool {
        self.domain == Domain::Item
            && !self.is_essence_only
            && matches!(
                self.generation_type,
                GenerationType::Prefix | GenerationType::Suffix
            )
    }

    /// Eldritch implicit tier of this mod, if it is an eldritch implicit.
    ///
    /// RePoE encodes the tier via a zero-weight gating tag
    /// `no_tier_<N>_eldritch_implicit` on each implicit: the mod gated by
    /// `no_tier_1_...` is the tier-1 (strongest) version. Tier 1 is best and
    /// tier 6 is worst, matching in-game tier numbering. Embers/Ichors grant
    /// tiers 6 (Lesser) through 3 (Exceptional); tiers 2 and 1 come only from
    /// Orb of Conflict, which is not modeled.
    pub fn eldritch_tier(&self) -> Option<u8> {
        if !matches!(
            self.generation_type,
            GenerationType::ExarchImplicit | GenerationType::EaterImplicit
        ) {
            return None;
        }
        self.spawn_weights.iter().find_map(|sw| {
            sw.tag
                .strip_prefix("no_tier_")?
                .strip_suffix("_eldritch_implicit")?
                .parse::<u8>()
                .ok()
        })
    }

    /// Returns the effective spawn weight for a given set of item tags.
    /// Iterates `spawn_weights` in order; first matching tag wins.
    /// Falls back to the "default" entry, or 0 if none found.
    pub fn spawn_weight_for_tags(&self, item_tags: &[&str]) -> u32 {
        for sw in &self.spawn_weights {
            if item_tags.contains(&sw.tag.as_str()) {
                return sw.weight;
            }
        }
        // Fallback: use "default" weight
        self.spawn_weights
            .iter()
            .find(|sw| sw.tag == "default")
            .map(|sw| sw.weight)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_repoe_eldritch_and_implicit_tags() {
        let parsed: Mod = serde_json::from_str(
            r#"{
                "name": "",
                "generation_type": "searing_exarch_implicit",
                "required_level": 75,
                "stats": [],
                "spawn_weights": [{"tag":"gloves","weight":1000}],
                "generation_weights": [],
                "adds_tags": [],
                "implicit_tags": ["attack", "speed"],
                "domain": "item",
                "type": "IncreasedAttackSpeed",
                "groups": ["IncreasedAttackSpeed"],
                "is_essence_only": false
            }"#,
        )
        .expect("current RePoE mod shape should parse");

        assert_eq!(parsed.generation_type, GenerationType::ExarchImplicit);
        assert_eq!(parsed.tags, ["attack", "speed"]);
    }

    #[test]
    fn accepts_legacy_generation_and_tag_names() {
        let parsed: Mod = serde_json::from_str(
            r#"{
                "name": "",
                "generation_type": "eater_implicit",
                "required_level": 1,
                "stats": [],
                "spawn_weights": [],
                "tags": ["cold"],
                "domain": "item",
                "type": "Cold",
                "groups": []
            }"#,
        )
        .expect("legacy fixture shape should parse");

        assert_eq!(parsed.generation_type, GenerationType::EaterImplicit);
        assert_eq!(parsed.tags, ["cold"]);
    }
}
