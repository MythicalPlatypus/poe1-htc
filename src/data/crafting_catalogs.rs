//! Typed views over the RePoE crafting-bench, essence, and fossil catalogs.
//!
//! These models intentionally omit fields that are not needed by crafting
//! validation. Serde ignores unknown fields by default, keeping the parsers
//! compatible with additive RePoE schema changes.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub struct CraftingBenchCatalog(pub Vec<CraftingBenchOption>);

#[derive(Debug, Clone, Deserialize)]
pub struct CraftingBenchOption {
    pub actions: CraftingBenchActions,
    #[serde(default)]
    pub item_classes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CraftingBenchActions {
    #[serde(default)]
    pub add_explicit_mod: Option<String>,
}

impl CraftingBenchCatalog {
    /// Finds a bench recipe for `mod_id` and validates its exact RePoE item class.
    pub fn validate_add_explicit_mod(
        &self,
        mod_id: &str,
        item_class: &str,
    ) -> Result<&CraftingBenchOption> {
        let mut matching = self
            .0
            .iter()
            .filter(|option| option.actions.add_explicit_mod.as_deref() == Some(mod_id));
        let option = matching.find(|option| {
            option
                .item_classes
                .iter()
                .any(|allowed| allowed == item_class)
        });

        if let Some(option) = option {
            return Ok(option);
        }

        if self
            .0
            .iter()
            .any(|option| option.actions.add_explicit_mod.as_deref() == Some(mod_id))
        {
            bail!("bench mod '{mod_id}' is not valid for item class '{item_class}'");
        }

        bail!("bench add_explicit_mod '{mod_id}' was not found")
    }
}

pub fn parse_crafting_bench_options(json: &str) -> Result<CraftingBenchCatalog> {
    serde_json::from_str(json).context("failed to parse RePoE crafting_bench_options.json")
}

#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub struct EssenceCatalog(pub HashMap<String, Essence>);

#[derive(Debug, Clone, Deserialize)]
pub struct Essence {
    pub name: String,

    /// Essence strength: 1 is Whispering through 7 Deafening; special
    /// corruption-only essences use level 8 in the current RePoE export.
    pub level: u8,

    /// Maximum item level used for rolling the non-guaranteed modifiers.
    /// This does not prevent applying the Essence to a higher-level base.
    #[serde(default)]
    pub item_level_restriction: Option<u32>,

    /// RePoE's family/corruption tier, distinct from the strength `level`.
    #[serde(rename = "type")]
    pub essence_type: EssenceType,

    /// Base-item class to guaranteed mod ID. Some non-crafting entries, such as
    /// Remnant of Corruption, explicitly map classes to null.
    #[serde(default)]
    pub mods: HashMap<String, Option<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EssenceType {
    pub tier: u8,
    pub is_corruption_only: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedEssence<'a> {
    pub metadata_id: &'a str,
    pub essence: &'a Essence,
    pub guaranteed_mod_id: &'a str,
}

impl EssenceCatalog {
    /// Resolves an exact metadata ID or an unambiguous case-insensitive name.
    pub fn resolve(&self, id_or_name: &str) -> Result<(&str, &Essence)> {
        if let Some((metadata_id, essence)) = self.0.get_key_value(id_or_name) {
            return Ok((metadata_id.as_str(), essence));
        }

        let mut matches = self
            .0
            .iter()
            .filter(|(_, essence)| essence.name.eq_ignore_ascii_case(id_or_name));
        let Some((metadata_id, essence)) = matches.next() else {
            bail!("essence '{id_or_name}' was not found by metadata ID or name");
        };
        if let Some((other_id, _)) = matches.next() {
            bail!(
                "essence name '{id_or_name}' is ambiguous between metadata IDs \
                 '{metadata_id}' and '{other_id}'"
            );
        }

        Ok((metadata_id.as_str(), essence))
    }

    /// Resolves the guaranteed mod for a `BaseItem.item_class`. Lower-tier
    /// item-level restrictions cap random filler mods at application time; they
    /// do not prevent using the Essence on a higher-item-level base.
    pub fn resolve_guaranteed_mod(
        &self,
        id_or_name: &str,
        item_class: &str,
    ) -> Result<ResolvedEssence<'_>> {
        let (metadata_id, essence) = self.resolve(id_or_name)?;

        let guaranteed_mod_id = essence
            .mods
            .get(item_class)
            .and_then(|mod_id| mod_id.as_deref())
            .filter(|mod_id| !mod_id.is_empty())
            .with_context(|| {
                format!(
                    "essence '{}' has no guaranteed mod for item class '{item_class}'",
                    essence.name
                )
            })?;

        Ok(ResolvedEssence {
            metadata_id,
            essence,
            guaranteed_mod_id,
        })
    }

    /// Finds the catalog essence that produces an already-configured mod ID for
    /// this item class. This keeps legacy `mod_id` goal files honest.
    pub fn resolve_by_guaranteed_mod(
        &self,
        mod_id: &str,
        item_class: &str,
    ) -> Result<ResolvedEssence<'_>> {
        let mut matches = self.0.iter().filter(|(_, essence)| {
            essence
                .mods
                .get(item_class)
                .and_then(|candidate| candidate.as_deref())
                == Some(mod_id)
        });
        let Some((metadata_id, essence)) = matches.next() else {
            bail!(
                "essence mod '{mod_id}' is not a guaranteed essence mod for item class \
                 '{item_class}'"
            );
        };
        if let Some((other_id, _)) = matches.next() {
            bail!(
                "essence mod '{mod_id}' is ambiguous between metadata IDs \
                '{metadata_id}' and '{other_id}'"
            );
        }
        let guaranteed_mod_id = essence
            .mods
            .get(item_class)
            .and_then(|candidate| candidate.as_deref())
            .with_context(|| {
                format!(
                    "essence '{}' lost its guaranteed mapping for item class '{item_class}'",
                    essence.name
                )
            })?;
        Ok(ResolvedEssence {
            metadata_id,
            essence,
            guaranteed_mod_id,
        })
    }
}

pub fn parse_essences(json: &str) -> Result<EssenceCatalog> {
    serde_json::from_str(json).context("failed to parse RePoE essences.json")
}

#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub struct FossilCatalog(pub HashMap<String, Fossil>);

#[derive(Debug, Clone, Deserialize)]
pub struct Fossil {
    /// Random Tangled Fossil outcomes have an empty name and remain resolvable
    /// by metadata ID.
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub positive_mod_weights: Vec<ModTagWeight>,
    #[serde(default)]
    pub negative_mod_weights: Vec<ModTagWeight>,
    #[serde(default)]
    pub forced_mods: Vec<String>,
    #[serde(default)]
    pub added_mods: Vec<String>,
    #[serde(default)]
    pub allowed_tags: Vec<String>,
    #[serde(default)]
    pub forbidden_tags: Vec<String>,
    #[serde(default)]
    pub descriptions: HashMap<String, String>,
    #[serde(default)]
    pub changes_quality: bool,
    #[serde(default)]
    pub corrupted_essence_chance: u32,
    #[serde(default)]
    pub mirrors: bool,
    #[serde(default)]
    pub rolls_lucky: bool,
    #[serde(default)]
    pub rolls_white_sockets: bool,
    #[serde(default)]
    pub sell_price_mods: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ModTagWeight {
    pub tag: String,
    /// Raw RePoE generation weight (1000 is the ordinary positive baseline).
    pub weight: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedFossil<'a> {
    pub metadata_id: &'a str,
    pub fossil: &'a Fossil,
}

impl FossilCatalog {
    /// Resolves an exact metadata ID or an unambiguous case-insensitive,
    /// non-empty name.
    pub fn resolve(&self, id_or_name: &str) -> Result<ResolvedFossil<'_>> {
        if let Some((metadata_id, fossil)) = self.0.get_key_value(id_or_name) {
            return Ok(ResolvedFossil {
                metadata_id,
                fossil,
            });
        }

        let mut matches = self.0.iter().filter(|(_, fossil)| {
            !fossil.name.is_empty() && fossil.name.eq_ignore_ascii_case(id_or_name)
        });
        let Some((metadata_id, fossil)) = matches.next() else {
            bail!("fossil '{id_or_name}' was not found by metadata ID or name");
        };
        if let Some((other_id, _)) = matches.next() {
            bail!(
                "fossil name '{id_or_name}' is ambiguous between metadata IDs \
                 '{metadata_id}' and '{other_id}'"
            );
        }

        Ok(ResolvedFossil {
            metadata_id,
            fossil,
        })
    }
}

impl Fossil {
    /// Reject catalog entries with behavior this optimizer does not model.
    pub fn validate_supported_behavior(&self) -> Result<()> {
        let unsupported_description = self.descriptions.keys().find(|key| {
            matches!(
                key.as_str(),
                "BetterSellPrice"
                    | "CorruptedImplicit"
                    | "CorruptEssence"
                    | "Duplicate"
                    | "LuckyMods"
                    | "NoTagless"
                    | "RandomModifier"
            )
        });
        if let Some(description) = unsupported_description {
            bail!(
                "fossil '{}' uses unsupported behavior '{description}'",
                self.name
            );
        }
        if self.changes_quality
            || self.corrupted_essence_chance > 0
            || self.mirrors
            || self.rolls_lucky
            || self.rolls_white_sockets
            || !self.sell_price_mods.is_empty()
        {
            bail!("fossil '{}' uses unsupported special behavior", self.name);
        }
        Ok(())
    }

    pub fn positive_weight_for(&self, mod_tag: &str) -> Option<u32> {
        self.positive_mod_weights
            .iter()
            .find(|entry| entry.tag == mod_tag)
            .map(|entry| entry.weight)
    }

    pub fn negative_weight_for(&self, mod_tag: &str) -> Option<u32> {
        self.negative_mod_weights
            .iter()
            .find(|entry| entry.tag == mod_tag)
            .map(|entry| entry.weight)
    }

    /// Applies RePoE's allow-list/deny-list semantics to an item's base tags.
    pub fn is_allowed_for_item_tags(&self, item_tags: &[&str]) -> bool {
        if self
            .forbidden_tags
            .iter()
            .any(|tag| item_tags.contains(&tag.as_str()))
        {
            return false;
        }

        self.allowed_tags.is_empty()
            || self
                .allowed_tags
                .iter()
                .any(|tag| item_tags.contains(&tag.as_str()))
    }
}

pub fn parse_fossils(json: &str) -> Result<FossilCatalog> {
    serde_json::from_str(json).context("failed to parse RePoE fossils.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_recipe_validates_item_class_and_ignores_future_fields() {
        let catalog = parse_crafting_bench_options(
            r#"[
                {
                    "actions": {
                        "add_explicit_mod": "HelenaMasterIncreasedLife1",
                        "future_action": {"value": 1}
                    },
                    "item_classes": ["Body Armour", "Helmet"],
                    "bench_tier": 1,
                    "future_option_field": true
                },
                {
                    "actions": {"add_explicit_mod": null},
                    "item_classes": ["Body Armour"]
                }
            ]"#,
        )
        .expect("bench fixture should parse");

        let option = catalog
            .validate_add_explicit_mod("HelenaMasterIncreasedLife1", "Body Armour")
            .expect("recipe should support body armour");
        assert_eq!(option.item_classes, ["Body Armour", "Helmet"]);
        assert!(catalog
            .validate_add_explicit_mod("HelenaMasterIncreasedLife1", "Bow")
            .is_err());
        assert!(catalog
            .validate_add_explicit_mod("UnknownCraftedMod", "Body Armour")
            .is_err());
    }

    #[test]
    fn essence_resolves_id_or_name_and_preserves_random_affix_cap() {
        let catalog = parse_essences(
            r#"{
                "Metadata/Items/Currency/CurrencyEssenceHatred1": {
                    "name": "Whispering Essence of Hatred",
                    "level": 1,
                    "item_level_restriction": 35,
                    "type": {"tier": 1, "is_corruption_only": false},
                    "mods": {
                        "Body Armour": "ColdResist1",
                        "Ring": null
                    },
                    "spawn_level_min": 1,
                    "future_field": "ignored"
                }
            }"#,
        )
        .expect("essence fixture should parse");

        let resolved = catalog
            .resolve_guaranteed_mod("whispering essence OF hatred", "Body Armour")
            .expect("case-insensitive name and capped item level should resolve");
        assert_eq!(
            resolved.metadata_id,
            "Metadata/Items/Currency/CurrencyEssenceHatred1"
        );
        assert_eq!(resolved.guaranteed_mod_id, "ColdResist1");
        assert_eq!(resolved.essence.level, 1);
        assert_eq!(resolved.essence.essence_type.tier, 1);
        assert!(!resolved.essence.essence_type.is_corruption_only);
        let reverse = catalog
            .resolve_by_guaranteed_mod("ColdResist1", "Body Armour")
            .expect("legacy mod ID should resolve through the same class mapping");
        assert_eq!(reverse.metadata_id, resolved.metadata_id);
        assert_eq!(reverse.essence.name, resolved.essence.name);

        assert!(catalog
            .resolve_guaranteed_mod(
                "Metadata/Items/Currency/CurrencyEssenceHatred1",
                "Body Armour",
            )
            .is_ok());
        assert!(catalog
            .resolve_guaranteed_mod("Whispering Essence of Hatred", "Ring")
            .is_err());
        assert!(catalog
            .resolve_guaranteed_mod("Whispering Essence of Hatred", "Bow")
            .is_err());
    }

    #[test]
    fn fossil_resolves_named_and_unnamed_entries_with_raw_effects() {
        let catalog = parse_fossils(
            r#"{
                "Metadata/Items/Currency/CurrencyDelveCraftingFire": {
                    "name": "Scorched Fossil",
                    "positive_mod_weights": [{"tag": "fire", "weight": 1000}],
                    "negative_mod_weights": [{"tag": "cold", "weight": 0}],
                    "forced_mods": ["ForcedMod"],
                    "added_mods": ["AddedMod"],
                    "allowed_tags": ["weapon"],
                    "forbidden_tags": ["bow"],
                    "changes_quality": false
                },
                "Metadata/Items/Currency/RandomFossilOutcome1": {
                    "name": "",
                    "positive_mod_weights": [{"tag": "speed", "weight": 3000}],
                    "negative_mod_weights": [],
                    "forced_mods": [],
                    "added_mods": [],
                    "allowed_tags": [],
                    "forbidden_tags": []
                }
            }"#,
        )
        .expect("fossil fixture should parse");

        let named = catalog
            .resolve("sCoRcHeD fOsSiL")
            .expect("case-insensitive fossil name should resolve");
        assert_eq!(
            named.metadata_id,
            "Metadata/Items/Currency/CurrencyDelveCraftingFire"
        );
        assert_eq!(named.fossil.positive_weight_for("fire"), Some(1000));
        assert_eq!(named.fossil.negative_weight_for("cold"), Some(0));
        assert_eq!(named.fossil.forced_mods, ["ForcedMod"]);
        assert_eq!(named.fossil.added_mods, ["AddedMod"]);
        assert!(named.fossil.is_allowed_for_item_tags(&["weapon"]));
        assert!(!named.fossil.is_allowed_for_item_tags(&["weapon", "bow"]));
        assert!(!named.fossil.is_allowed_for_item_tags(&["body_armour"]));

        let unnamed = catalog
            .resolve("Metadata/Items/Currency/RandomFossilOutcome1")
            .expect("unnamed outcome should resolve by metadata ID");
        assert_eq!(unnamed.fossil.positive_weight_for("speed"), Some(3000));
        assert!(catalog.resolve("").is_err());
    }

    #[test]
    fn case_insensitive_name_lookup_rejects_ambiguity() {
        let catalog = parse_fossils(
            r#"{
                "Fossil1": {"name": "Test Fossil"},
                "Fossil2": {"name": "test fossil"}
            }"#,
        )
        .expect("minimal fossil fixture should parse");

        let error = catalog
            .resolve("TEST FOSSIL")
            .expect_err("duplicate folded names must be rejected");
        assert!(error.to_string().contains("ambiguous"));
    }

    #[test]
    fn special_fossil_behavior_fails_closed() {
        let catalog = parse_fossils(
            r#"{
                "MirrorFossil": {
                    "name": "Fractured Fossil",
                    "mirrors": true,
                    "descriptions": {"Duplicate": "Creates a split copy"}
                }
            }"#,
        )
        .unwrap();
        let fossil = catalog.resolve("Fractured Fossil").unwrap();
        let error = fossil
            .fossil
            .validate_supported_behavior()
            .expect_err("unmodeled split behavior must not become a plain reroll");
        assert!(error.to_string().contains("unsupported"));
    }
}
