//! Fossil crafting — rerolls the item as a Chaos Orb but with modified spawn weight tables.
//! Up to 4 fossils can be combined in one resonator.

use std::collections::HashSet;

use anyhow::{bail, Result};
use rand::RngCore;

use super::{
    random_rare_affix_count, CraftingMethod, ItemClassSupport, MethodCatalog, MethodFamily,
    MethodId, MethodSetup, RerollKind, MONTE_CARLO_SAMPLES,
};
use crate::data::mods::GenerationType;
use crate::data::GameData;
use crate::engine::mod_pool::{
    random_rolls_pub, roll_mods_from_pool, FossilTagSet, FossilWeightRule, RollPool,
};
use crate::item::modifier::Modifier;
use crate::item::{state::Rarity, ItemState};

/// Describes how a fossil modifies the mod pool.
#[derive(Debug, Clone)]
pub struct FossilModifier {
    /// Semantic `Mod::tags` whose mods receive a 10x weight multiplier.
    pub boosted_tags: Vec<String>,
    /// Semantic `Mod::tags` whose mods receive a 0.1x weight multiplier.
    pub reduced_tags: Vec<String>,
    /// Specific mod IDs completely blocked from the pool.
    pub blocked_mod_ids: Vec<String>,
    /// Specific mod IDs forced onto the item before random rolling.
    pub forced_mod_ids: Vec<String>,
    /// Delve-domain mods added to the ordinary candidate pool by this fossil.
    pub added_mod_ids: Vec<String>,
    /// Exact raw RePoE tag weights (100 is neutral, 0 blocks).
    pub tag_weights: Vec<FossilWeightRule>,
}

/// A resonator + fossil combination applied as one crafting operation.
pub struct FossilCraft {
    pub display_name: String,
    pub cost_chaos: f64,
    pub fossils: Vec<FossilModifier>,
}

fn pool_configuration(fossils: &[FossilModifier]) -> (Vec<String>, Vec<FossilTagSet>, Vec<String>) {
    let blocked_mod_ids = fossils
        .iter()
        .flat_map(|f| f.blocked_mod_ids.iter().cloned())
        .collect();
    let fossil_tag_sets = fossils
        .iter()
        .map(|f| FossilTagSet {
            boosted_tags: f.boosted_tags.clone(),
            reduced_tags: f.reduced_tags.clone(),
            weight_rules: f.tag_weights.clone(),
        })
        .collect();
    let added_mod_ids = fossils
        .iter()
        .flat_map(|f| f.added_mod_ids.iter().cloned())
        .collect();
    (blocked_mod_ids, fossil_tag_sets, added_mod_ids)
}

fn canonical_configuration_json(fossils: &[FossilModifier]) -> String {
    let mut output = String::from("{\"parts\":[");
    for (index, fossil) in fossils.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str("{\"boosted_tags\":");
        push_json_string_array(&mut output, &fossil.boosted_tags);
        output.push_str(",\"reduced_tags\":");
        push_json_string_array(&mut output, &fossil.reduced_tags);
        output.push_str(",\"blocked_mod_ids\":");
        push_json_string_array(&mut output, &fossil.blocked_mod_ids);
        output.push_str(",\"forced_mod_ids\":");
        push_json_string_array(&mut output, &fossil.forced_mod_ids);
        output.push_str(",\"added_mod_ids\":");
        push_json_string_array(&mut output, &fossil.added_mod_ids);
        output.push_str(",\"tag_weights\":[");
        for (rule_index, rule) in fossil.tag_weights.iter().enumerate() {
            if rule_index > 0 {
                output.push(',');
            }
            output.push_str("{\"tag\":");
            push_json_string(&mut output, &rule.tag);
            output.push_str(",\"weight\":");
            output.push_str(&rule.weight.to_string());
            output.push('}');
        }
        output.push_str("]}");
    }
    output.push_str("]}");
    output
}

fn push_json_string_array(output: &mut String, values: &[String]) {
    output.push('[');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        push_json_string(output, value);
    }
    output.push(']');
}

fn push_json_string(output: &mut String, value: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0C}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{00}'..='\u{1F}' => {
                let code = character as u8;
                output.push_str("\\u00");
                output.push(char::from(HEX[(code >> 4) as usize]));
                output.push(char::from(HEX[(code & 0x0F) as usize]));
            }
            _ => output.push(character),
        }
    }
    output.push('"');
}

impl CraftingMethod for FossilCraft {
    fn id(&self) -> MethodId {
        let configuration = canonical_configuration_json(&self.fossils);
        MethodId::semantic("fossil", "resonator", &["config", &configuration])
    }

    fn family(&self) -> MethodFamily {
        MethodFamily::Fossil
    }

    fn description(&self) -> &str {
        "Rerolls an item as Rare using the configured resonator's fossil-modified pool."
    }

    fn setup(&self) -> MethodSetup {
        MethodSetup::CatalogOrConfigured(MethodCatalog::Fossils)
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
        self.fossils
            .iter()
            .flat_map(|fossil| {
                fossil
                    .forced_mod_ids
                    .iter()
                    .chain(fossil.added_mod_ids.iter())
            })
            .map(String::as_str)
            .collect()
    }
    // Monte Carlo sampling — weights are 1/N, not probabilities.
    fn weights_are_probabilities(&self) -> bool {
        false
    }
    // Full reroll: reapplying is an independent draw from the same distribution.
    fn repeatable_on_failure(&self) -> bool {
        true
    }
    fn reroll_kind(&self) -> Option<RerollKind> {
        Some(RerollKind::RareExplicit)
    }

    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.is_craftable() && matches!(item.rarity, Rarity::Normal | Rarity::Magic | Rarity::Rare)
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if !self.can_apply(item, db) {
            bail!("Cannot apply {}", self.display_name);
        }

        // Keep each fossil's semantic tag rules separate: one fossil contributes
        // at most one boost and one reduction, while multiple fossils compose.
        let (blocked_mod_ids, fossil_tag_sets, added_mod_ids) = pool_configuration(&self.fossils);

        // Validate all forced mods before sampling: existence, type, group conflicts, slot capacity.
        // After apply, prefixes/suffixes/crafted_mod are cleared; fractured mods remain.
        // Track occupied groups and slots as forced mods are accepted in order.
        let mut occupied_groups: HashSet<&str> = item
            .fractured
            .iter()
            .filter_map(|m| db.mods.get(&m.mod_id))
            .flat_map(|m| m.groups.iter().map(|g| g.as_str()))
            .collect();
        let mut forced_prefixes = item
            .fractured
            .iter()
            .filter(|m| m.generation_type == GenerationType::Prefix)
            .count();
        let mut forced_suffixes = item
            .fractured
            .iter()
            .filter(|m| m.generation_type == GenerationType::Suffix)
            .count();

        for fossil in &self.fossils {
            for mod_id in &fossil.forced_mod_ids {
                let forced_mod = db
                    .mods
                    .get(mod_id)
                    .ok_or_else(|| anyhow::anyhow!("Fossil forced mod '{}' not in DB", mod_id))?;
                if !matches!(
                    forced_mod.generation_type,
                    GenerationType::Prefix | GenerationType::Suffix
                ) {
                    bail!(
                        "{}: forced mod '{}' is not a prefix or suffix",
                        self.display_name,
                        mod_id
                    );
                }
                if forced_mod
                    .groups
                    .iter()
                    .any(|g| occupied_groups.contains(g.as_str()))
                {
                    bail!(
                        "{}: forced mod '{}' shares a mod group with a fractured mod or another forced mod",
                        self.display_name,
                        mod_id
                    );
                }
                match forced_mod.generation_type {
                    GenerationType::Prefix if forced_prefixes >= 3 => bail!(
                        "{}: no open prefix slot for forced mod '{}'",
                        self.display_name,
                        mod_id
                    ),
                    GenerationType::Suffix if forced_suffixes >= 3 => bail!(
                        "{}: no open suffix slot for forced mod '{}'",
                        self.display_name,
                        mod_id
                    ),
                    _ => {}
                }
                for g in &forced_mod.groups {
                    occupied_groups.insert(g.as_str());
                }
                match forced_mod.generation_type {
                    GenerationType::Prefix => forced_prefixes += 1,
                    GenerationType::Suffix => forced_suffixes += 1,
                    _ => {}
                }
            }
        }

        // Build the fossil-modified candidate pool once; each sample only pays
        // the cheap per-pick filtering inside roll_mods_from_pool.
        let pool = RollPool::with_fossil_added_mods(
            item,
            &blocked_mod_ids,
            &fossil_tag_sets,
            &added_mod_ids,
            db,
        );

        let prob = 1.0 / MONTE_CARLO_SAMPLES as f64;
        let mut outcomes = Vec::with_capacity(MONTE_CARLO_SAMPLES);

        for _ in 0..MONTE_CARLO_SAMPLES {
            let mut next = item.clone();
            next.rarity = Rarity::Rare;
            next.prefixes.clear();
            next.suffixes.clear();
            next.crafted_mod = None;

            // Place forced mods first (validity confirmed in upfront checks above).
            for fossil in &self.fossils {
                for mod_id in &fossil.forced_mod_ids {
                    let forced_mod = db.mods.get(mod_id).ok_or_else(|| {
                        anyhow::anyhow!(
                            "{}: forced mod '{}' missing from DB",
                            self.display_name,
                            mod_id
                        )
                    })?;
                    let rolls = random_rolls_pub(&forced_mod.stats, rng);
                    let modifier = Modifier {
                        mod_id: mod_id.clone(),
                        generation_type: forced_mod.generation_type.clone(),
                        rolls,
                    };
                    match forced_mod.generation_type {
                        GenerationType::Prefix => next.prefixes.push(modifier),
                        GenerationType::Suffix => next.suffixes.push(modifier),
                        _ => bail!(
                            "{}: forced mod '{}' is not a prefix or suffix (validated above)",
                            self.display_name,
                            mod_id
                        ),
                    }
                }
            }

            // Roll remaining mods from the fossil-modified pool (4–6 total).
            let total = random_rare_affix_count(rng);
            let remaining = total.saturating_sub(next.mod_count());
            roll_mods_from_pool(&mut next, remaining, &pool, db, rng)?;
            outcomes.push((next, prob));
        }

        Ok(outcomes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_modifier() -> FossilModifier {
        FossilModifier {
            boosted_tags: vec![],
            reduced_tags: vec![],
            blocked_mod_ids: vec![],
            forced_mod_ids: vec![],
            added_mod_ids: vec![],
            tag_weights: vec![],
        }
    }

    fn craft(fossils: Vec<FossilModifier>, display_name: &str, cost_chaos: f64) -> FossilCraft {
        FossilCraft {
            display_name: display_name.to_string(),
            cost_chaos,
            fossils,
        }
    }

    #[test]
    fn pool_configuration_preserves_fossil_boundaries_and_polarity() {
        let fossils = vec![
            FossilModifier {
                boosted_tags: vec!["life".to_string()],
                reduced_tags: vec!["attack".to_string()],
                blocked_mod_ids: vec!["blocked_a".to_string()],
                forced_mod_ids: vec![],
                added_mod_ids: vec![],
                tag_weights: vec![],
            },
            FossilModifier {
                boosted_tags: vec!["defences".to_string()],
                reduced_tags: vec!["caster".to_string()],
                blocked_mod_ids: vec!["blocked_b".to_string()],
                forced_mod_ids: vec![],
                added_mod_ids: vec![],
                tag_weights: vec![],
            },
        ];

        let (blocked, tag_sets, added) = pool_configuration(&fossils);

        assert_eq!(blocked, ["blocked_a", "blocked_b"]);
        assert!(added.is_empty());
        assert_eq!(
            tag_sets,
            [
                FossilTagSet {
                    boosted_tags: vec!["life".to_string()],
                    reduced_tags: vec!["attack".to_string()],
                    weight_rules: vec![],
                },
                FossilTagSet {
                    boosted_tags: vec!["defences".to_string()],
                    reduced_tags: vec!["caster".to_string()],
                    weight_rules: vec![],
                },
            ]
        );
    }

    #[test]
    fn canonical_configuration_has_fixed_fields_and_json_escaping() {
        let mut first = empty_modifier();
        first.boosted_tags = vec!["life".to_string(), "quoted\"slash\\line\nÉ".to_string()];
        first.reduced_tags = vec!["attack".to_string()];
        first.blocked_mod_ids = vec!["blocked".to_string()];
        first.forced_mod_ids = vec!["forced".to_string()];
        first.added_mod_ids = vec!["added".to_string()];
        first.tag_weights = vec![
            FossilWeightRule {
                tag: "caster".to_string(),
                weight: 125,
            },
            FossilWeightRule {
                tag: "cold".to_string(),
                weight: 0,
            },
        ];

        assert_eq!(
            canonical_configuration_json(&[first, empty_modifier()]),
            r#"{"parts":[{"boosted_tags":["life","quoted\"slash\\line\nÉ"],"reduced_tags":["attack"],"blocked_mod_ids":["blocked"],"forced_mod_ids":["forced"],"added_mod_ids":["added"],"tag_weights":[{"tag":"caster","weight":125},{"tag":"cold","weight":0}]},{"boosted_tags":[],"reduced_tags":[],"blocked_mod_ids":[],"forced_mod_ids":[],"added_mod_ids":[],"tag_weights":[]}]}"#
        );
    }

    #[test]
    fn semantic_id_covers_every_ordered_configuration_field_and_part_boundary() {
        let baseline = craft(vec![empty_modifier()], "Original name", 1.0);
        let cosmetic_change = craft(vec![empty_modifier()], "Renamed and repriced", 999.0);
        assert_eq!(baseline.id(), cosmetic_change.id());

        let mut variants = Vec::new();
        for field in 0..5 {
            let mut modifier = empty_modifier();
            let value = vec!["value/with delimiter".to_string()];
            match field {
                0 => modifier.boosted_tags = value,
                1 => modifier.reduced_tags = value,
                2 => modifier.blocked_mod_ids = value,
                3 => modifier.forced_mod_ids = value,
                4 => modifier.added_mod_ids = value,
                _ => unreachable!("test iterates over the five string-vector fields"),
            }
            variants.push(craft(vec![modifier], "Variant", 1.0).id());
        }

        let mut weighted = empty_modifier();
        weighted.tag_weights = vec![FossilWeightRule {
            tag: "life".to_string(),
            weight: 1_000,
        }];
        variants.push(craft(vec![weighted], "Variant", 1.0).id());

        let mut ordered = empty_modifier();
        ordered.boosted_tags = vec!["first".to_string(), "second".to_string()];
        let ordered_id = craft(vec![ordered.clone()], "Variant", 1.0).id();
        ordered.boosted_tags.reverse();
        let reversed_id = craft(vec![ordered], "Variant", 1.0).id();
        assert_ne!(ordered_id, reversed_id);

        let mut first_part = empty_modifier();
        first_part.boosted_tags = vec!["first".to_string()];
        let mut second_part = empty_modifier();
        second_part.boosted_tags = vec!["second".to_string()];
        let split_parts_id = craft(vec![first_part, second_part], "Split parts", 1.0).id();
        assert_ne!(ordered_id, split_parts_id);

        let mut unique = HashSet::new();
        assert!(unique.insert(baseline.id()));
        for id in variants {
            assert!(unique.insert(id), "configuration fields must not collide");
        }
        assert!(unique.insert(ordered_id));
        assert!(unique.insert(reversed_id));
        assert!(unique.insert(split_parts_id.clone()));

        let encoded = split_parts_id
            .as_str()
            .strip_prefix("fossil/resonator/config/")
            .expect("fossil ID should use the stable family and operation");
        assert!(
            !encoded.contains('/'),
            "the complete canonical JSON must occupy one encoded segment"
        );
        assert_eq!(
            MethodId::parse(split_parts_id.as_str()).expect("generated ID should validate"),
            split_parts_id
        );
    }
}
