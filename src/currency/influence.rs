//! Influenced-item currency.
//!
//! Conqueror Exalted Orbs can be modeled from one item: they add the chosen
//! influence and exactly one modifier exclusive to that influence. Awakener's
//! Orb cannot be represented by `CraftingMethod`, because that operation
//! consumes two source items and transfers a modifier from each.

use anyhow::{bail, Result};
use rand::RngCore;

use super::CraftingMethod;
use crate::data::mods::{GenerationType, Mod, ModStat};
use crate::data::GameData;
use crate::engine::mod_pool::{eligible_mods, random_rolls_pub};
use crate::item::modifier::Modifier;
use crate::item::{state::Rarity, ItemState};

const INFLUENCE_MARKERS: &[&str] = &[
    "shaper_item",
    "elder_item",
    "crusader_item",
    "hunter_item",
    "redeemer_item",
    "warlord_item",
];

const REPOE_INFLUENCE_SUFFIXES: &[&str] = &[
    "shaper",
    "elder",
    "crusader",
    "basilisk",
    "eyrie",
    "adjudicator",
];

// Ordered from more specific tags to less specific tags. RePoE uses a
// synthetic `2h_*` influence class for two-handed axes, maces, and swords.
const INFLUENCE_CLASSES: &[&str] = &[
    "body_armour",
    "rune_dagger",
    "warstaff",
    "amulet",
    "belt",
    "boots",
    "bow",
    "claw",
    "dagger",
    "gloves",
    "helmet",
    "quiver",
    "ring",
    "sceptre",
    "shield",
    "staff",
    "wand",
    "axe",
    "mace",
    "sword",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Influence {
    Shaper,
    Elder,
    Crusader,
    Hunter,
    Redeemer,
    Warlord,
}

impl Influence {
    fn marker(self) -> &'static str {
        match self {
            Self::Shaper => "shaper_item",
            Self::Elder => "elder_item",
            Self::Crusader => "crusader_item",
            Self::Hunter => "hunter_item",
            Self::Redeemer => "redeemer_item",
            Self::Warlord => "warlord_item",
        }
    }

    fn repoe_suffix(self) -> &'static str {
        match self {
            Self::Shaper => "shaper",
            Self::Elder => "elder",
            Self::Crusader => "crusader",
            Self::Hunter => "basilisk",
            Self::Redeemer => "eyrie",
            Self::Warlord => "adjudicator",
        }
    }

    fn conqueror_orb_name(self) -> Option<&'static str> {
        match self {
            Self::Crusader => Some("Crusader's Exalted Orb"),
            Self::Hunter => Some("Hunter's Exalted Orb"),
            Self::Redeemer => Some("Redeemer's Exalted Orb"),
            Self::Warlord => Some("Warlord's Exalted Orb"),
            Self::Shaper | Self::Elder => None,
        }
    }
}

/// Adds the configured Conqueror influence and one exclusive influence affix.
///
/// Modifier identities are enumerated with their exact relative RePoE weights.
/// Numeric stat values are sampled once per identity, so concrete successor
/// states and any roll-sensitive scoring derived from them are estimates.
#[derive(Debug, Clone, Copy)]
pub struct ConquerorExaltedOrb {
    pub influence: Influence,
}

impl CraftingMethod for ConquerorExaltedOrb {
    fn name(&self) -> &str {
        self.influence
            .conqueror_orb_name()
            .unwrap_or("Invalid Conqueror Exalted Orb")
    }

    fn cost_chaos(&self) -> f64 {
        100.0
    }

    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        self.influence.conqueror_orb_name().is_some()
            && base_item_is_eligible(item)
            && !eligible_influence_mods(item, self.influence, db).is_empty()
    }

    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        if self.influence.conqueror_orb_name().is_none() {
            bail!("Conqueror Exalted Orbs only support Crusader, Hunter, Redeemer, or Warlord");
        }
        if !base_item_is_eligible(item) {
            bail!(
                "{} requires a craftable, unfractured, non-influenced Rare item without Eldritch implicits",
                self.name()
            );
        }

        let pool_tag = influence_pool_tag(item, self.influence)
            .ok_or_else(|| anyhow::anyhow!("{}: unsupported item class", self.name()))?;
        let pool = eligible_influence_mods(item, self.influence, db);
        let total_weight: u64 = pool.iter().map(|(_, _, weight)| u64::from(*weight)).sum();
        if total_weight == 0 {
            bail!(
                "{}: no eligible exclusive influence modifier for this item",
                self.name()
            );
        }

        let mut outcomes = Vec::with_capacity(pool.len());
        for (mod_id, selected, weight) in pool {
            validate_roll_ranges(mod_id, &selected.stats)?;

            let mut next = item.clone();
            push_tag_if_missing(&mut next.base_tags, self.influence.marker());
            push_tag_if_missing(&mut next.base_tags, &pool_tag);

            let modifier = Modifier {
                mod_id: mod_id.to_string(),
                generation_type: selected.generation_type.clone(),
                rolls: random_rolls_pub(&selected.stats, rng),
            };
            match selected.generation_type {
                GenerationType::Prefix => next.prefixes.push(modifier),
                GenerationType::Suffix => next.suffixes.push(modifier),
                _ => bail!(
                    "{}: influence pool contained non-affix mod {mod_id}",
                    self.name()
                ),
            }

            outcomes.push((next, f64::from(weight) / total_weight as f64));
        }
        Ok(outcomes)
    }

    fn weights_are_probabilities(&self) -> bool {
        false
    }
}

/// Retained for API compatibility, but direct influence acquisition is not a
/// currency operation that this optimizer can price or derive from one item.
#[derive(Debug, Clone, Copy)]
pub struct ApplyInfluence {
    pub influence: Influence,
}

impl CraftingMethod for ApplyInfluence {
    fn name(&self) -> &str {
        "Apply Influence (unsupported)"
    }

    fn cost_chaos(&self) -> f64 {
        f64::INFINITY
    }

    fn can_apply(&self, _item: &ItemState, _db: &GameData) -> bool {
        false
    }

    fn apply(
        &self,
        _item: &ItemState,
        _db: &GameData,
        _rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        bail!(
            "Direct influence acquisition is not modeled; provide an influenced base or use a Conqueror Exalted Orb"
        )
    }
}

/// Retained for API compatibility, but deliberately unavailable until the
/// crafting interface can accept and consume both source items.
#[derive(Debug, Clone, Copy)]
pub struct AwakenersOrb {
    pub source_influence_a: Influence,
    pub source_influence_b: Influence,
}

impl CraftingMethod for AwakenersOrb {
    fn name(&self) -> &str {
        "Awakener's Orb (unsupported)"
    }

    fn cost_chaos(&self) -> f64 {
        200.0
    }

    fn can_apply(&self, _item: &ItemState, _db: &GameData) -> bool {
        false
    }

    fn apply(
        &self,
        _item: &ItemState,
        _db: &GameData,
        _rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        bail!("Awakener's Orb requires two source items and cannot be modeled by CraftingMethod")
    }
}

fn base_item_is_eligible(item: &ItemState) -> bool {
    item.is_craftable()
        && item.rarity == Rarity::Rare
        && item.fractured.is_empty()
        && item.exarch_implicit.is_none()
        && item.eater_implicit.is_none()
        && !has_influence(item)
}

fn has_influence(item: &ItemState) -> bool {
    item.base_tags.iter().any(|tag| {
        INFLUENCE_MARKERS.contains(&tag.as_str())
            || REPOE_INFLUENCE_SUFFIXES.iter().any(|suffix| {
                tag.strip_suffix(suffix)
                    .is_some_and(|stem| stem.ends_with('_'))
            })
    })
}

fn influence_pool_tag(item: &ItemState, influence: Influence) -> Option<String> {
    let class = influence_class(item)?;
    Some(format!("{class}_{}", influence.repoe_suffix()))
}

fn influence_class(item: &ItemState) -> Option<&'static str> {
    if item.base_tags.iter().any(|tag| tag == "two_hand_weapon") {
        for weapon_type in ["axe", "mace", "sword"] {
            if item.base_tags.iter().any(|tag| tag == weapon_type) {
                return match weapon_type {
                    "axe" => Some("2h_axe"),
                    "mace" => Some("2h_mace"),
                    "sword" => Some("2h_sword"),
                    _ => None,
                };
            }
        }
    }

    INFLUENCE_CLASSES
        .iter()
        .copied()
        .find(|class| item.base_tags.iter().any(|tag| tag == class))
}

fn eligible_influence_mods<'a>(
    item: &ItemState,
    influence: Influence,
    db: &'a GameData,
) -> Vec<(&'a str, &'a Mod, u32)> {
    let Some(pool_tag) = influence_pool_tag(item, influence) else {
        return Vec::new();
    };

    let ordinary_tags = effective_item_tags(item, db);
    let mut influenced = item.clone();
    push_tag_if_missing(&mut influenced.base_tags, influence.marker());
    push_tag_if_missing(&mut influenced.base_tags, &pool_tag);

    eligible_mods(&influenced, &[], db)
        .into_iter()
        .filter(|(_, modifier, _)| {
            modifier.spawn_weight_for_tags(&ordinary_tags) == 0
                && modifier
                    .spawn_weights
                    .iter()
                    .any(|weight| weight.tag == pool_tag && weight.weight > 0)
        })
        .collect()
}

fn effective_item_tags<'a>(item: &'a ItemState, db: &'a GameData) -> Vec<&'a str> {
    item.base_tags
        .iter()
        .map(String::as_str)
        .chain(
            item.all_mods_for_conflict()
                .filter_map(|modifier| db.mods.get(&modifier.mod_id))
                .flat_map(|modifier| modifier.adds_tags.iter().map(String::as_str)),
        )
        .collect()
}

fn push_tag_if_missing(tags: &mut Vec<String>, tag: &str) {
    if !tags.iter().any(|existing| existing == tag) {
        tags.push(tag.to_string());
    }
}

fn validate_roll_ranges(mod_id: &str, stats: &[ModStat]) -> Result<()> {
    if let Some(stat) = stats.iter().find(|stat| stat.min > stat.max) {
        bail!(
            "mod {mod_id} has invalid range for stat {}: {} > {}",
            stat.id,
            stat.min,
            stat.max
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use anyhow::Result;
    use rand::{rngs::StdRng, SeedableRng};

    use super::*;
    use crate::data::mods::{Domain, GenerationWeight, SpawnWeight};

    fn test_mod(
        generation_type: GenerationType,
        group: &str,
        spawn_weights: Vec<SpawnWeight>,
    ) -> Mod {
        Mod {
            name: group.to_string(),
            generation_type,
            required_level: 1,
            stats: vec![ModStat {
                id: format!("stat_{group}"),
                min: 10,
                max: 20,
            }],
            spawn_weights,
            generation_weights: Vec::new(),
            adds_tags: Vec::new(),
            tags: Vec::new(),
            domain: Domain::Item,
            mod_type: group.to_string(),
            groups: vec![group.to_string()],
            is_essence_only: false,
        }
    }

    fn exclusive_mod(
        generation_type: GenerationType,
        group: &str,
        pool_tag: &str,
        weight: u32,
    ) -> Mod {
        test_mod(
            generation_type,
            group,
            vec![
                SpawnWeight {
                    tag: pool_tag.to_string(),
                    weight,
                },
                SpawnWeight {
                    tag: "default".to_string(),
                    weight: 0,
                },
            ],
        )
    }

    fn sword() -> ItemState {
        let mut item = ItemState::new_base(
            "sword",
            vec!["sword".to_string(), "one_hand_weapon".to_string()],
            84,
        );
        item.rarity = Rarity::Rare;
        item
    }

    fn marker(mod_id: &str, generation_type: GenerationType) -> Modifier {
        Modifier {
            mod_id: mod_id.to_string(),
            generation_type,
            rolls: Vec::new(),
        }
    }

    fn rng() -> StdRng {
        StdRng::seed_from_u64(0x001F_1ECE)
    }

    #[test]
    fn enumerates_only_exclusive_mods_with_exact_identity_weights() -> Result<()> {
        let mut prefix = exclusive_mod(
            GenerationType::Prefix,
            "InfluencePrefix",
            "sword_crusader",
            100,
        );
        prefix.generation_weights = vec![GenerationWeight {
            tag: "sword_crusader".to_string(),
            weight: 50,
        }];
        let suffix = exclusive_mod(
            GenerationType::Suffix,
            "InfluenceSuffix",
            "sword_crusader",
            150,
        );
        let ordinary = test_mod(
            GenerationType::Prefix,
            "Ordinary",
            vec![
                SpawnWeight {
                    tag: "sword".to_string(),
                    weight: 1_000,
                },
                SpawnWeight {
                    tag: "sword_crusader".to_string(),
                    weight: 1_000,
                },
            ],
        );
        let other_influence = exclusive_mod(
            GenerationType::Prefix,
            "HunterOnly",
            "sword_basilisk",
            10_000,
        );
        let db = GameData::new(
            HashMap::from([
                ("prefix".to_string(), prefix),
                ("suffix".to_string(), suffix),
                ("ordinary".to_string(), ordinary),
                ("hunter".to_string(), other_influence),
            ]),
            HashMap::new(),
        );
        let orb = ConquerorExaltedOrb {
            influence: Influence::Crusader,
        };

        let outcomes = orb.apply(&sword(), &db, &mut rng())?;

        assert_eq!(outcomes.len(), 2);
        let weights: HashMap<&str, f64> = outcomes
            .iter()
            .map(|(item, probability)| {
                let added = item
                    .prefixes
                    .iter()
                    .chain(item.suffixes.iter())
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("outcome did not add an affix"))?;
                Ok((added.mod_id.as_str(), *probability))
            })
            .collect::<Result<_>>()?;
        assert!((weights["prefix"] - 0.25).abs() < 1e-12);
        assert!((weights["suffix"] - 0.75).abs() < 1e-12);

        for (item, _) in outcomes {
            assert!(item.base_tags.iter().any(|tag| tag == "crusader_item"));
            assert!(item.base_tags.iter().any(|tag| tag == "sword_crusader"));
            let added = item
                .prefixes
                .iter()
                .chain(item.suffixes.iter())
                .next()
                .ok_or_else(|| anyhow::anyhow!("outcome did not add an affix"))?;
            assert!((10..=20).contains(&added.rolls[0].value));
        }
        assert!(!orb.weights_are_probabilities());
        Ok(())
    }

    #[test]
    fn respects_level_group_conflicts_and_affix_capacity() -> Result<()> {
        let prefix = exclusive_mod(
            GenerationType::Prefix,
            "BlockedGroup",
            "sword_crusader",
            100,
        );
        let mut high_level =
            exclusive_mod(GenerationType::Suffix, "HighLevel", "sword_crusader", 100);
        high_level.required_level = 85;
        let available = exclusive_mod(GenerationType::Suffix, "Available", "sword_crusader", 100);
        let existing = test_mod(
            GenerationType::Prefix,
            "BlockedGroup",
            vec![SpawnWeight {
                tag: "sword".to_string(),
                weight: 100,
            }],
        );
        let db = GameData::new(
            HashMap::from([
                ("blocked".to_string(), prefix),
                ("high".to_string(), high_level),
                ("available".to_string(), available),
                ("existing".to_string(), existing),
            ]),
            HashMap::new(),
        );
        let mut item = sword();
        item.prefixes
            .push(marker("existing", GenerationType::Prefix));
        item.prefixes.push(marker("p2", GenerationType::Prefix));
        item.prefixes.push(marker("p3", GenerationType::Prefix));
        let orb = ConquerorExaltedOrb {
            influence: Influence::Crusader,
        };

        let outcomes = orb.apply(&item, &db, &mut rng())?;

        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].0.suffixes[0].mod_id, "available");
        assert_eq!(outcomes[0].1, 1.0);
        Ok(())
    }

    #[test]
    fn rejects_every_forbidden_item_state_and_empty_pool() {
        let db = GameData::new(
            HashMap::from([(
                "influence".to_string(),
                exclusive_mod(GenerationType::Prefix, "Influence", "sword_crusader", 100),
            )]),
            HashMap::new(),
        );
        let orb = ConquerorExaltedOrb {
            influence: Influence::Crusader,
        };

        let mut normal = sword();
        normal.rarity = Rarity::Normal;
        assert!(!orb.can_apply(&normal, &db));

        let mut fractured = sword();
        fractured
            .fractured
            .push(marker("fractured", GenerationType::Prefix));
        assert!(!orb.can_apply(&fractured, &db));

        let mut influenced = sword();
        influenced.base_tags.push("sword_eyrie".to_string());
        assert!(!orb.can_apply(&influenced, &db));

        let mut legacy_influenced = sword();
        legacy_influenced.base_tags.push("hunter_item".to_string());
        assert!(!orb.can_apply(&legacy_influenced, &db));

        let mut eldritch = sword();
        eldritch.exarch_implicit = Some(marker("implicit", GenerationType::ExarchImplicit));
        assert!(!orb.can_apply(&eldritch, &db));

        let mut eater = sword();
        eater.eater_implicit = Some(marker("implicit", GenerationType::EaterImplicit));
        assert!(!orb.can_apply(&eater, &db));

        let mut corrupted = sword();
        corrupted.corrupted = true;
        assert!(!orb.can_apply(&corrupted, &db));

        let mut mirrored = sword();
        mirrored.mirrored = true;
        assert!(!orb.can_apply(&mirrored, &db));

        let mut full = sword();
        for index in 0..3 {
            full.prefixes
                .push(marker(&format!("p{index}"), GenerationType::Prefix));
            full.suffixes
                .push(marker(&format!("s{index}"), GenerationType::Suffix));
        }
        assert!(!orb.can_apply(&full, &db));

        let empty_db = GameData::new(HashMap::new(), HashMap::new());
        assert!(!orb.can_apply(&sword(), &empty_db));
    }

    #[test]
    fn maps_all_conquerors_and_two_handed_repoe_tags() -> Result<()> {
        for (influence, suffix, name) in [
            (Influence::Crusader, "crusader", "Crusader's Exalted Orb"),
            (Influence::Hunter, "basilisk", "Hunter's Exalted Orb"),
            (Influence::Redeemer, "eyrie", "Redeemer's Exalted Orb"),
            (Influence::Warlord, "adjudicator", "Warlord's Exalted Orb"),
        ] {
            let mut item = ItemState::new_base(
                "two-handed sword",
                vec![
                    "sword".to_string(),
                    "two_hand_weapon".to_string(),
                    "weapon".to_string(),
                ],
                84,
            );
            item.rarity = Rarity::Rare;
            let pool_tag = format!("2h_sword_{suffix}");
            let db = GameData::new(
                HashMap::from([(
                    suffix.to_string(),
                    exclusive_mod(GenerationType::Prefix, suffix, &pool_tag, 100),
                )]),
                HashMap::new(),
            );
            let orb = ConquerorExaltedOrb { influence };

            assert_eq!(orb.name(), name);
            let outcomes = orb.apply(&item, &db, &mut rng())?;
            assert_eq!(outcomes.len(), 1);
            assert!(outcomes[0].0.base_tags.iter().any(|tag| tag == &pool_tag));
        }
        Ok(())
    }

    #[test]
    fn invalid_ranges_error_and_unsupported_operations_fail_closed() {
        let mut invalid = exclusive_mod(GenerationType::Prefix, "Invalid", "sword_crusader", 100);
        invalid.stats[0].min = 20;
        invalid.stats[0].max = 10;
        let db = GameData::new(
            HashMap::from([("invalid".to_string(), invalid)]),
            HashMap::new(),
        );
        let item = sword();
        let conqueror = ConquerorExaltedOrb {
            influence: Influence::Crusader,
        };
        assert!(conqueror.apply(&item, &db, &mut rng()).is_err());

        let shaper = ConquerorExaltedOrb {
            influence: Influence::Shaper,
        };
        assert!(!shaper.can_apply(&item, &db));
        assert!(shaper.apply(&item, &db, &mut rng()).is_err());

        let direct = ApplyInfluence {
            influence: Influence::Crusader,
        };
        assert!(!direct.can_apply(&item, &db));
        assert!(direct.apply(&item, &db, &mut rng()).is_err());
        assert!(direct.cost_chaos().is_infinite());

        let awakener = AwakenersOrb {
            source_influence_a: Influence::Crusader,
            source_influence_b: Influence::Hunter,
        };
        assert!(!awakener.can_apply(&item, &db));
        assert!(awakener.apply(&item, &db, &mut rng()).is_err());
    }
}
