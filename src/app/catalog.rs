//! Read-only catalog queries for application adapters.
//!
//! This module deliberately returns owned summaries: UI and other adapters can
//! move results to another thread without borrowing [`GameData`]. The clean-base
//! affix query delegates eligibility to [`eligible_mods`] so catalog results
//! cannot drift from the normal crafting pool.

use crate::data::mods::{Domain, GenerationType};
use crate::data::GameData;
use crate::engine::mod_pool::eligible_mods;
use crate::item::state::Rarity;
use crate::item::ItemState;
use thiserror::Error;

use super::service::OptimizerService;

/// Searchable, owned facts about one RePoE base item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseItemSummary {
    pub id: String,
    pub name: String,
    pub item_class: String,
    pub drop_level: u32,
    pub tags: Vec<String>,
    pub implicit_mod_ids: Vec<String>,
    pub inventory_height: u32,
    pub inventory_width: u32,
}

/// Prefix or suffix placement for a normal explicit affix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AffixKind {
    Prefix,
    Suffix,
}

/// One owned RePoE stat range exposed by an affix catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffixStatSummary {
    pub id: String,
    pub min: i32,
    pub max: i32,
}

/// Owned facts about one normal affix that can roll on a clean Rare base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffixSummary {
    pub id: String,
    pub name: String,
    pub text_template: Option<String>,
    pub kind: AffixKind,
    pub required_level: u32,
    pub stats: Vec<AffixStatSummary>,
    pub domain: Domain,
    pub mod_type: String,
    pub groups: Vec<String>,
    pub tags: Vec<String>,
    pub adds_tags: Vec<String>,
    /// Effective normal-pool weight after ordered spawn and generation weights.
    pub effective_weight: u32,
}

/// Input for a clean-base compatible-affix query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanBaseAffixQuery {
    pub base_id: String,
    pub item_level: u32,
}

/// Compatible normal affixes, split by slot and kept in sorted mod-ID order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanBaseAffixCatalog {
    pub base: BaseItemSummary,
    pub item_level: u32,
    pub prefixes: Vec<AffixSummary>,
    pub suffixes: Vec<AffixSummary>,
}

/// A rejected read-only catalog query.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CatalogError {
    #[error("unknown base item metadata ID '{base_id}'")]
    UnknownBase { base_id: String },
    #[error("item level must be between 1 and 100, got {item_level}")]
    InvalidItemLevel { item_level: u32 },
}

/// Search base metadata IDs, display names, and item classes.
///
/// Matching is case-insensitive, collapses runs of whitespace, and uses one
/// normalized substring. An empty query returns every base. Results sort by
/// normalized display name and then metadata ID, so duplicate display names
/// always remain in stable metadata-ID order.
pub fn search_base_items(db: &GameData, query: &str) -> Vec<BaseItemSummary> {
    let query = normalize_search_text(query);
    let mut matches = db
        .base_items
        .iter()
        .filter(|(id, base)| {
            query.is_empty()
                || normalize_search_text(&format!("{id} {} {}", base.name, base.item_class))
                    .contains(&query)
        })
        .map(|(id, base)| base_summary(id, base))
        .collect::<Vec<_>>();

    matches.sort_by(|left, right| {
        normalize_search_text(&left.name)
            .cmp(&normalize_search_text(&right.name))
            .then_with(|| left.id.cmp(&right.id))
    });
    matches
}

/// List normal explicit affixes compatible with an empty Rare base.
///
/// The returned set is exactly the public normal eligibility pool for the
/// selected base tags and item level. Current-item capacity, occupied groups,
/// influences, Fossils, Harvest targeting, and other specialized pools are
/// intentionally outside this clean-base query.
pub fn compatible_clean_base_affixes(
    db: &GameData,
    query: &CleanBaseAffixQuery,
) -> Result<CleanBaseAffixCatalog, CatalogError> {
    if !(1..=100).contains(&query.item_level) {
        return Err(CatalogError::InvalidItemLevel {
            item_level: query.item_level,
        });
    }
    let base = db
        .base_items
        .get(&query.base_id)
        .ok_or_else(|| CatalogError::UnknownBase {
            base_id: query.base_id.clone(),
        })?;

    let mut item = ItemState::new_base(query.base_id.clone(), base.tags.clone(), query.item_level);
    item.rarity = Rarity::Rare;

    let mut prefixes = Vec::new();
    let mut suffixes = Vec::new();
    for (id, modifier, effective_weight) in eligible_mods(&item, &[], db) {
        let (kind, destination) = match modifier.generation_type {
            GenerationType::Prefix => (AffixKind::Prefix, &mut prefixes),
            GenerationType::Suffix => (AffixKind::Suffix, &mut suffixes),
            _ => continue,
        };
        destination.push(AffixSummary {
            id: id.to_string(),
            name: modifier.name.clone(),
            text_template: modifier.text.clone(),
            kind,
            required_level: modifier.required_level,
            stats: modifier
                .stats
                .iter()
                .map(|stat| AffixStatSummary {
                    id: stat.id.clone(),
                    min: stat.min,
                    max: stat.max,
                })
                .collect(),
            domain: modifier.domain.clone(),
            mod_type: modifier.mod_type.clone(),
            groups: modifier.groups.clone(),
            tags: modifier.tags.clone(),
            adds_tags: modifier.adds_tags.clone(),
            effective_weight,
        });
    }

    // `GameData::craftable_mods` already iterates by sorted ID. Keep explicit
    // sorts here so this API's contract remains local and robust to refactors.
    prefixes.sort_by(|left, right| left.id.cmp(&right.id));
    suffixes.sort_by(|left, right| left.id.cmp(&right.id));

    Ok(CleanBaseAffixCatalog {
        base: base_summary(&query.base_id, base),
        item_level: query.item_level,
        prefixes,
        suffixes,
    })
}

impl OptimizerService {
    /// Search the immutable service data for owned base-item summaries.
    pub fn search_base_items(&self, query: &str) -> Vec<BaseItemSummary> {
        search_base_items(self.game_data(), query)
    }

    /// List normal affixes compatible with a clean Rare base.
    pub fn compatible_clean_base_affixes(
        &self,
        query: &CleanBaseAffixQuery,
    ) -> Result<CleanBaseAffixCatalog, CatalogError> {
        compatible_clean_base_affixes(self.game_data(), query)
    }
}

fn base_summary(id: &str, base: &crate::data::base_items::BaseItem) -> BaseItemSummary {
    BaseItemSummary {
        id: id.to_string(),
        name: base.name.clone(),
        item_class: base.item_class.clone(),
        drop_level: base.drop_level,
        tags: base.tags.clone(),
        implicit_mod_ids: base.implicits.clone(),
        inventory_height: base.inventory_height,
        inventory_width: base.inventory_width,
    }
}

fn normalize_search_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::data::base_items::BaseItem;
    use crate::data::mods::{GenerationWeight, Mod, ModStat, SpawnWeight};

    use super::*;

    fn base(name: &str, item_class: &str, tags: &[&str]) -> BaseItem {
        BaseItem {
            name: name.to_string(),
            item_class: item_class.to_string(),
            tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
            implicits: vec!["BaseImplicit".to_string()],
            drop_level: 12,
            inventory_height: 3,
            inventory_width: 2,
        }
    }

    fn modifier(
        kind: GenerationType,
        domain: Domain,
        required_level: u32,
        spawn_weights: &[(&str, u32)],
    ) -> Mod {
        Mod {
            name: "Catalog modifier".to_string(),
            generation_type: kind,
            required_level,
            stats: vec![ModStat {
                id: "catalog_stat".to_string(),
                min: 10,
                max: 20,
            }],
            spawn_weights: spawn_weights
                .iter()
                .map(|(tag, weight)| SpawnWeight {
                    tag: (*tag).to_string(),
                    weight: *weight,
                })
                .collect(),
            generation_weights: Vec::new(),
            adds_tags: vec!["added_tag".to_string()],
            tags: vec!["catalog".to_string()],
            domain,
            mod_type: "CatalogType".to_string(),
            groups: vec!["CatalogGroup".to_string()],
            is_essence_only: false,
            text: Some("+{0} to Catalog Stat".to_string()),
        }
    }

    #[test]
    fn base_search_is_normalized_owned_and_deterministic() {
        let first = GameData::new(
            HashMap::new(),
            HashMap::from([
                (
                    "base/z".to_string(),
                    base("Astral Plate", "A Class", &["z"]),
                ),
                (
                    "base/a".to_string(),
                    base("Astral Plate", "Z Class", &["a"]),
                ),
                (
                    "base/sword".to_string(),
                    base("Épée Astrale", "One Hand Swords", &["sword"]),
                ),
            ]),
        );
        let second = GameData::new(
            HashMap::new(),
            [
                (
                    "base/sword".to_string(),
                    base("Épée Astrale", "One Hand Swords", &["sword"]),
                ),
                (
                    "base/a".to_string(),
                    base("Astral Plate", "Z Class", &["a"]),
                ),
                (
                    "base/z".to_string(),
                    base("Astral Plate", "A Class", &["z"]),
                ),
            ]
            .into_iter()
            .collect(),
        );

        let expected = vec!["base/a", "base/z"];
        assert_eq!(
            search_base_items(&first, "  ASTRAL    plate ")
                .iter()
                .map(|summary| summary.id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            search_base_items(&first, ""),
            search_base_items(&second, "")
        );
        let sword_matches = search_base_items(&first, "one hand");
        assert_eq!(sword_matches.len(), 1);
        assert_eq!(sword_matches[0].id, "base/sword");

        let mut owned = search_base_items(&first, "base/a").remove(0);
        owned.name.push_str(" changed");
        assert_eq!(first.base_items["base/a"].name, "Astral Plate");
    }

    #[test]
    fn clean_base_affixes_match_normal_engine_eligibility_and_weights() {
        let mut allowed_prefix = modifier(
            GenerationType::Prefix,
            Domain::Item,
            1,
            &[("other", 999), ("sword", 200), ("default", 300)],
        );
        allowed_prefix.generation_weights = vec![GenerationWeight {
            tag: "sword".to_string(),
            weight: 50,
        }];
        let allowed_suffix = modifier(GenerationType::Suffix, Domain::Item, 80, &[("sword", 75)]);
        let zero_first_match = modifier(
            GenerationType::Prefix,
            Domain::Item,
            1,
            &[("sword", 0), ("default", 999)],
        );
        let above_level = modifier(GenerationType::Prefix, Domain::Item, 87, &[("sword", 999)]);
        let crafted = modifier(
            GenerationType::Prefix,
            Domain::Crafted,
            1,
            &[("sword", 999)],
        );
        let mut essence_only = modifier(GenerationType::Suffix, Domain::Item, 1, &[("sword", 999)]);
        essence_only.is_essence_only = true;

        let db = GameData::new(
            HashMap::from([
                ("AllowedPrefix".to_string(), allowed_prefix),
                ("AllowedSuffix".to_string(), allowed_suffix),
                ("ZeroFirstMatch".to_string(), zero_first_match),
                ("AboveLevel".to_string(), above_level),
                ("Crafted".to_string(), crafted),
                ("EssenceOnly".to_string(), essence_only),
            ]),
            HashMap::from([(
                "base/sword".to_string(),
                base("Catalog Sword", "One Hand Swords", &["sword", "default"]),
            )]),
        );
        let query = CleanBaseAffixQuery {
            base_id: "base/sword".to_string(),
            item_level: 86,
        };

        let catalog = compatible_clean_base_affixes(&db, &query).unwrap();
        assert_eq!(
            catalog
                .prefixes
                .iter()
                .map(|entry| (entry.id.as_str(), entry.effective_weight))
                .collect::<Vec<_>>(),
            [("AllowedPrefix", 100)]
        );
        assert_eq!(
            catalog
                .suffixes
                .iter()
                .map(|entry| (entry.id.as_str(), entry.effective_weight))
                .collect::<Vec<_>>(),
            [("AllowedSuffix", 75)]
        );

        let mut clean = ItemState::new_base(
            "base/sword",
            vec!["sword".to_string(), "default".to_string()],
            86,
        );
        clean.rarity = Rarity::Rare;
        let engine_ids = eligible_mods(&clean, &[], &db)
            .into_iter()
            .map(|(id, _, weight)| (id, weight))
            .collect::<Vec<_>>();
        let mut catalog_ids = catalog
            .prefixes
            .iter()
            .chain(&catalog.suffixes)
            .map(|entry| (entry.id.as_str(), entry.effective_weight))
            .collect::<Vec<_>>();
        catalog_ids.sort_unstable();
        assert_eq!(catalog_ids, engine_ids);

        let prefix = &catalog.prefixes[0];
        assert_eq!(prefix.kind, AffixKind::Prefix);
        assert_eq!(prefix.domain, Domain::Item);
        assert_eq!(prefix.mod_type, "CatalogType");
        assert_eq!(prefix.stats[0].id, "catalog_stat");
        assert_eq!(
            prefix.text_template.as_deref(),
            Some("+{0} to Catalog Stat")
        );
    }

    #[test]
    fn clean_base_affix_query_rejects_unknown_bases_and_invalid_levels() {
        let db = GameData::new(HashMap::new(), HashMap::new());
        let unknown = compatible_clean_base_affixes(
            &db,
            &CleanBaseAffixQuery {
                base_id: "missing".to_string(),
                item_level: 86,
            },
        )
        .unwrap_err();
        assert_eq!(
            unknown,
            CatalogError::UnknownBase {
                base_id: "missing".to_string()
            }
        );

        for item_level in [0, 101] {
            assert_eq!(
                compatible_clean_base_affixes(
                    &db,
                    &CleanBaseAffixQuery {
                        base_id: "missing".to_string(),
                        item_level,
                    },
                )
                .unwrap_err(),
                CatalogError::InvalidItemLevel { item_level }
            );
        }
    }
}
