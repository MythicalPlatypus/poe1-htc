//! Goal specification — what the user wants the finished item to look like.
//!
//! A goal is loaded from a TOML file and compiled into a scoring function for
//! the beam search. Example:
//!
//! ```toml
//! [item]
//! base = "Astral Plate"   # display name or RePoE metadata ID
//! item_level = 86
//!
//! [[wants]]
//! group = "IncreasedLife" # match any mod in this RePoE mod group
//! weight = 10.0
//!
//! [[wants]]
//! stat = "base_maximum_energy_shield"
//! min_value = 50          # only satisfied when the rolled value reaches 50
//! weight = 5.0
//!
//! [search]
//! beam_width = 20
//! max_steps = 8
//! cost_weight = 0.05
//! ```
//!
//! ## Scoring semantics
//! A want is satisfied when its selector and optional threshold match the item.
//! Presence and threshold wants contribute their weight when satisfied.
//! `per_unit` wants instead aggregate the selected stat across every matching
//! modifier and contribute a non-negative, optionally capped value-scaled
//! amount. Selectors can inspect prefix, suffix, fractured, crafted, implicit,
//! and enchantment modifiers:
//!
//! - `mod_id` — the mod's RePoE ID equals this string exactly.
//! - `group`  — the mod's DB entry lists this group in `groups` (the same
//!   field used for conflict checks — never display names).
//! - `stat`   — the mod has a rolled stat with this RePoE stat ID.
//! - `min_value` / `max_value` — higher/lower satisfaction bounds.
//!
//! Wants are required by default. Setting `required = false` keeps the score
//! contribution as a search preference but removes that want from the
//! definition of target completion.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::currency::{
    beastcraft::{BestiaryAffixSwapCraft, BestiaryAffixSwapKind},
    bench::BenchCraft,
    eldritch::{EldritchChaosOrb, EldritchExaltedOrb, EldritchGod, EldritchOrbOfAnnulment},
    essences::Essence,
    fossils::{FossilCraft, FossilModifier},
    harvest::{HarvestCraft, HarvestOp, HarvestTarget},
    influence::{ConquerorExaltedOrb, Influence},
    CraftingMethod,
};
use crate::data::base_items::BaseItem;
use crate::data::mods::{Domain, GenerationType, Mod};
use crate::data::GameData;
use crate::engine::mod_pool::FossilWeightRule;
use crate::item::modifier::StatRoll;
use crate::item::state::Rarity;
use crate::item::{ItemState, Modifier};

fn default_item_level() -> u32 {
    84
}

fn default_weight() -> f64 {
    1.0
}

const fn default_required() -> bool {
    true
}

/// How one desired modifier contributes to preference score.
///
/// Omitted modes retain the legacy goal-file behavior: a want with a bound is
/// a threshold, while an unbounded want checks presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalScoringMode {
    Presence,
    Threshold,
    PerUnit,
}

impl GoalScoringMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Presence => "presence",
            Self::Threshold => "threshold",
            Self::PerUnit => "per_unit",
        }
    }
}

fn repoe_influence_pool_tag(base_tags: &[String], influence: &str) -> Option<String> {
    let class = if base_tags.iter().any(|tag| tag == "two_hand_weapon") {
        ["axe", "mace", "sword"]
            .into_iter()
            .find(|class| base_tags.iter().any(|tag| tag == class))
            .map(|class| format!("2h_{class}"))
    } else {
        [
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
        ]
        .into_iter()
        .find(|class| base_tags.iter().any(|tag| tag == class))
        .map(str::to_string)
    }?;
    let suffix = match influence {
        "shaper" => "shaper",
        "elder" => "elder",
        "crusader" => "crusader",
        "hunter" => "basilisk",
        "redeemer" => "eyrie",
        "warlord" => "adjudicator",
        _ => return None,
    };
    Some(format!("{class}_{suffix}"))
}

/// Top-level goal file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalSpec {
    pub item: ItemSpec,
    /// Desired mods. At least one entry is required.
    pub wants: Vec<WantSpec>,
    /// Extra crafting methods (essences, fossils, harvest, bench, eldritch)
    /// made available to the search on top of the default orb set.
    #[serde(default)]
    pub methods: Vec<MethodSpec>,
    /// Per-method chaos-cost overrides keyed by method display name
    /// (e.g. `"Divine Orb" = 220.0`). Applied to default orbs and [[methods]]
    /// alike, so league prices can live in the goal file.
    #[serde(default)]
    pub prices: HashMap<String, f64>,
    /// Optional search-parameter overrides.
    #[serde(default)]
    pub search: SearchSpec,
}

/// The item to start crafting from — a fresh base by default, or a mid-craft
/// item when `rarity` / `[[item.mods]]` describe existing state. Example:
///
/// ```toml
/// [item]
/// base = "Astral Plate"
/// item_level = 86
/// rarity = "rare"                  # normal (default) | magic | rare
/// influences = ["hunter"]          # up to two existing influences
/// exarch_implicit = "ExampleEldritchImplicit3"
/// eater_implicit = "OtherEldritchImplicit4"
///
/// [[item.mods]]
/// mod_id = "IncreasedLife9"
/// values = [97]                    # one per stat; omitted = midpoint rolls
/// fractured = true                 # locked against removal
///
/// [[item.mods]]
/// mod_id = "FireResist5"           # plain suffix at midpoint roll
/// ```
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemSpec {
    /// Base item display name (e.g. "Astral Plate") or RePoE metadata ID
    /// (e.g. "Metadata/Items/Armours/BodyArmours/BodyStr15").
    pub base: String,
    /// Item level — gates which mods can roll (`required_level <= item_level`).
    #[serde(default = "default_item_level")]
    pub item_level: u32,
    /// Starting rarity: "normal" (default), "magic", or "rare". Must be set
    /// when `[[item.mods]]` are present (Normal items hold no explicit mods).
    #[serde(default)]
    pub rarity: Option<String>,
    /// Mods already on the item when crafting starts.
    #[serde(default)]
    pub mods: Vec<StartingModSpec>,
    /// Existing item influences: shaper, elder, crusader, hunter, redeemer,
    /// or warlord. At most two may be present.
    #[serde(default)]
    pub influences: Vec<String>,
    /// Existing Searing Exarch implicit mod ID, for starting from a mid-craft.
    pub exarch_implicit: Option<String>,
    /// Existing Eater of Worlds implicit mod ID, for starting from a mid-craft.
    pub eater_implicit: Option<String>,
}

/// One existing mod on the starting item.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartingModSpec {
    /// RePoE mod ID (e.g. "IncreasedLife9").
    pub mod_id: String,
    /// Rolled value per stat, in the mod's stat order. Omitted stats roll at
    /// the midpoint of their range. Values outside [min, max] are rejected.
    pub values: Option<Vec<i32>>,
    /// Fractured — locked, survives rerolls, blocks its group.
    #[serde(default)]
    pub fractured: bool,
    /// Bench-crafted — occupies the single crafted-mod slot
    /// (requires a `domain = "crafted"` mod).
    #[serde(default)]
    pub crafted: bool,
}

impl ItemSpec {
    /// Construct the starting `ItemState`, validating every declared mod
    /// against the DB: existence, prefix/suffix type, roll ranges, slot
    /// capacity for the declared rarity, and group conflicts.
    pub fn build_state(
        &self,
        base_id: String,
        base_tags: Vec<String>,
        db: &GameData,
    ) -> Result<ItemState> {
        let mut item = ItemState::new_base(base_id, base_tags, self.item_level);
        item.rarity = match self.rarity.as_deref() {
            None | Some("normal") => Rarity::Normal,
            Some("magic") => Rarity::Magic,
            Some("rare") => Rarity::Rare,
            Some(other) => bail!("[item] rarity '{other}' (expected normal, magic, or rare)"),
        };

        if self.influences.len() > 2 {
            bail!("[item] influences may contain at most two entries");
        }
        let mut influence_tags = std::collections::HashSet::new();
        for influence in &self.influences {
            let tag = match influence.as_str() {
                "shaper" => "shaper_item",
                "elder" => "elder_item",
                "crusader" => "crusader_item",
                "hunter" => "hunter_item",
                "redeemer" => "redeemer_item",
                "warlord" => "warlord_item",
                other => bail!(
                    "[item] unknown influence '{other}' (expected shaper, elder, crusader, hunter, redeemer, or warlord)"
                ),
            };
            if !influence_tags.insert(tag) {
                bail!("[item] duplicate influence '{influence}'");
            }
            item.base_tags.push(tag.to_string());
            let pool_tag =
                repoe_influence_pool_tag(&item.base_tags, influence).ok_or_else(|| {
                    anyhow::anyhow!(
                        "[item] influence '{influence}' is unsupported for this base item class"
                    )
                })?;
            item.base_tags.push(pool_tag);
        }
        if !self.influences.is_empty()
            && (self.exarch_implicit.is_some() || self.eater_implicit.is_some())
        {
            bail!("[item] influenced items cannot have Eldritch implicits");
        }

        item.exarch_implicit = self.build_eldritch_implicit(
            self.exarch_implicit.as_deref(),
            GenerationType::ExarchImplicit,
            "exarch_implicit",
            &item.base_tags,
            db,
        )?;
        item.eater_implicit = self.build_eldritch_implicit(
            self.eater_implicit.as_deref(),
            GenerationType::EaterImplicit,
            "eater_implicit",
            &item.base_tags,
            db,
        )?;

        for (i, spec) in self.mods.iter().enumerate() {
            let m = db.mods.get(&spec.mod_id).ok_or_else(|| {
                anyhow::anyhow!(
                    "[[item.mods]] entry {i}: mod '{}' not in mods.json",
                    spec.mod_id
                )
            })?;
            if spec.fractured && spec.crafted {
                bail!("[[item.mods]] entry {i}: a mod cannot be both fractured and crafted");
            }
            if spec.fractured && !self.influences.is_empty() {
                bail!("[[item.mods]] entry {i}: influenced items cannot be fractured");
            }
            if !matches!(
                m.generation_type,
                GenerationType::Prefix | GenerationType::Suffix
            ) {
                bail!(
                    "[[item.mods]] entry {i}: '{}' is not a prefix or suffix",
                    spec.mod_id
                );
            }
            if m.required_level > self.item_level {
                bail!(
                    "[[item.mods]] entry {i}: '{}' requires item level {}, but [item] item_level is {}",
                    spec.mod_id,
                    m.required_level,
                    self.item_level
                );
            }
            let base_tag_refs: Vec<&str> = item.base_tags.iter().map(String::as_str).collect();
            if m.domain != Domain::Crafted && m.spawn_weight_for_tags(&base_tag_refs) == 0 {
                bail!(
                    "[[item.mods]] entry {i}: '{}' cannot appear on base '{}'",
                    spec.mod_id,
                    item.base_id
                );
            }
            if spec.crafted {
                if m.domain != Domain::Crafted {
                    bail!(
                        "[[item.mods]] entry {i}: crafted = true but '{}' has domain {:?}",
                        spec.mod_id,
                        m.domain
                    );
                }
                if item.crafted_mod.is_some() {
                    bail!("[[item.mods]] entry {i}: only one crafted mod is allowed");
                }
            }

            // Group conflicts against everything placed so far.
            let conflict = item
                .all_mods_for_conflict()
                .filter_map(|placed| db.mods.get(&placed.mod_id))
                .flat_map(|placed| placed.groups.iter())
                .any(|g| m.groups.contains(g));
            if conflict {
                bail!(
                    "[[item.mods]] entry {i}: '{}' shares a mod group with another starting mod",
                    spec.mod_id
                );
            }

            // Capacity for the declared rarity (crafted mods occupy a slot too).
            let open = match m.generation_type {
                GenerationType::Prefix => item.has_open_prefix(),
                _ => item.has_open_suffix(),
            };
            if !open {
                bail!(
                    "[[item.mods]] entry {i}: no open {} slot on a {:?} item",
                    if m.generation_type == GenerationType::Prefix {
                        "prefix"
                    } else {
                        "suffix"
                    },
                    item.rarity
                );
            }

            // Roll values: explicit (validated against the mod's ranges) or midpoint.
            let rolls: Vec<StatRoll> = match &spec.values {
                Some(values) => {
                    if values.len() != m.stats.len() {
                        bail!(
                            "[[item.mods]] entry {i}: '{}' has {} stats but {} values given",
                            spec.mod_id,
                            m.stats.len(),
                            values.len()
                        );
                    }
                    m.stats
                        .iter()
                        .zip(values)
                        .map(|(s, &v)| {
                            if v < s.min || v > s.max {
                                bail!(
                                    "[[item.mods]] entry {i}: {} = {v} outside [{}, {}]",
                                    s.id,
                                    s.min,
                                    s.max
                                );
                            }
                            Ok(StatRoll {
                                stat_id: s.id.clone(),
                                value: v,
                            })
                        })
                        .collect::<Result<_>>()?
                }
                None => m
                    .stats
                    .iter()
                    .map(|s| StatRoll {
                        stat_id: s.id.clone(),
                        value: s.min + (s.max - s.min) / 2,
                    })
                    .collect(),
            };

            let modifier = Modifier {
                mod_id: spec.mod_id.clone(),
                generation_type: m.generation_type.clone(),
                rolls,
            };
            if spec.crafted {
                item.crafted_mod = Some(modifier);
            } else if spec.fractured {
                item.fractured.push(modifier);
            } else {
                match m.generation_type {
                    GenerationType::Prefix => item.prefixes.push(modifier),
                    _ => item.suffixes.push(modifier),
                }
            }
        }
        Ok(item)
    }

    fn build_eldritch_implicit(
        &self,
        mod_id: Option<&str>,
        expected_type: GenerationType,
        field: &str,
        base_tags: &[String],
        db: &GameData,
    ) -> Result<Option<Modifier>> {
        let Some(mod_id) = mod_id else {
            return Ok(None);
        };
        let m = db
            .mods
            .get(mod_id)
            .ok_or_else(|| anyhow::anyhow!("[item] {field}: mod '{mod_id}' not in mods.json"))?;
        if m.generation_type != expected_type {
            bail!(
                "[item] {field}: mod '{mod_id}' has generation type {:?}, expected {:?}",
                m.generation_type,
                expected_type
            );
        }
        if m.required_level > self.item_level {
            bail!(
                "[item] {field}: mod '{mod_id}' requires item level {}, but [item] item_level is {}",
                m.required_level,
                self.item_level
            );
        }
        let base_tag_refs: Vec<&str> = base_tags.iter().map(String::as_str).collect();
        if m.spawn_weight_for_tags(&base_tag_refs) == 0 {
            bail!("[item] {field}: mod '{mod_id}' cannot appear on the selected base");
        }
        let rolls = m
            .stats
            .iter()
            .map(|stat| StatRoll {
                stat_id: stat.id.clone(),
                value: stat.min + (stat.max - stat.min) / 2,
            })
            .collect();
        Ok(Some(Modifier {
            mod_id: mod_id.to_string(),
            generation_type: m.generation_type.clone(),
            rolls,
        }))
    }
}

/// Build stat rolls for an imported mod: explicit values are validated against
/// the mod's roll ranges. Unlike `[[item.mods]]` (where omitted values mean
/// midpoint rolls), the import path requires one value per stat — the importer
/// captured the real rolls from the clipboard text, so a missing value is a
/// bug or a truncated paste, never a request for a default.
fn imported_rolls(
    label: &str,
    mod_id: &str,
    m: &crate::data::mods::Mod,
    values: &[i32],
) -> Result<Vec<StatRoll>> {
    if values.len() != m.stats.len() {
        bail!(
            "{label}: '{mod_id}' has {} stats but {} values were imported",
            m.stats.len(),
            values.len()
        );
    }
    m.stats
        .iter()
        .zip(values)
        .map(|(s, &v)| {
            if v < s.min || v > s.max {
                bail!(
                    "{label}: '{mod_id}' {} = {v} outside [{}, {}]",
                    s.id,
                    s.min,
                    s.max
                );
            }
            Ok(StatRoll {
                stat_id: s.id.clone(),
                value: v,
            })
        })
        .collect()
}

/// Build a validated `ItemState` from a clipboard-imported item.
///
/// Mirrors the `[[item.mods]]` validation in [`ItemSpec::build_state`] —
/// strict about prefix/suffix generation type, item level, roll ranges,
/// prefix/suffix capacity, group conflicts, fractured-versus-crafted
/// exclusivity, and the single crafted-mod slot — with one deliberate
/// relaxation: an imported **existing** modifier does not need a positive
/// current spawn weight for the base. Fractured, recombinated, Delve,
/// unveiled, legacy, or otherwise transferred modifiers are valid existing
/// state even when they can no longer roll naturally. This relaxation applies
/// only here; normal crafting pools and TOML starting-item validation are
/// unchanged, so such mods still never appear in random rolls.
///
/// Quality, the socket description, and the displayed Energy Shield total are
/// preserved as descriptive metadata only — crafting probabilities and goal
/// math do not model socket crafting or derived total defences.
pub fn build_imported_state(
    imported: &crate::import::ImportedItem,
    base_id: String,
    base_tags: Vec<String>,
    db: &GameData,
) -> Result<ItemState> {
    let item_level = imported
        .item_level
        .ok_or_else(|| anyhow::anyhow!("imported item: item level is required to validate mods"))?;
    if item_level == 0 || item_level > 100 {
        bail!("imported item: item level {item_level} must be between 1 and 100");
    }

    let mut item = ItemState::new_base(base_id, base_tags, item_level);
    item.rarity = match imported.rarity {
        Rarity::Normal => Rarity::Normal,
        Rarity::Magic => Rarity::Magic,
        Rarity::Rare => Rarity::Rare,
        Rarity::Unique => bail!("imported item: unique items cannot be crafted on"),
    };
    item.corrupted = imported.corrupted;
    item.mirrored = imported.mirrored;
    if let Some(quality) = imported.quality {
        item.quality = u8::try_from(quality)
            .ok()
            .filter(|q| *q <= 100)
            .ok_or_else(|| {
                anyhow::anyhow!("imported item: quality {quality}% outside the plausible 0-100%")
            })?;
    }
    item.sockets = imported.sockets.clone();
    item.displayed_energy_shield = imported
        .displayed_energy_shield
        .map(|es| {
            u32::try_from(es).map_err(|_| {
                anyhow::anyhow!("imported item: displayed Energy Shield {es} cannot be negative")
            })
        })
        .transpose()?;

    for spec in &imported.implicit_mods {
        let label = "imported implicit";
        let m = db
            .mods
            .get(&spec.mod_id)
            .ok_or_else(|| anyhow::anyhow!("{label}: mod '{}' not in mods.json", spec.mod_id))?;
        let modifier = Modifier {
            mod_id: spec.mod_id.clone(),
            generation_type: m.generation_type.clone(),
            rolls: imported_rolls(label, &spec.mod_id, m, &spec.values)?,
        };
        match m.generation_type {
            GenerationType::Prefix | GenerationType::Suffix => bail!(
                "{label}: '{}' is an explicit affix, not an implicit",
                spec.mod_id
            ),
            GenerationType::ExarchImplicit => {
                if item.exarch_implicit.is_some() {
                    bail!("{label}: more than one Searing Exarch implicit");
                }
                item.exarch_implicit = Some(modifier);
            }
            GenerationType::EaterImplicit => {
                if item.eater_implicit.is_some() {
                    bail!("{label}: more than one Eater of Worlds implicit");
                }
                item.eater_implicit = Some(modifier);
            }
            _ => item.implicits.push(modifier),
        }
    }

    for spec in &imported.enchantments {
        let label = "imported enchantment";
        let m = db
            .mods
            .get(&spec.mod_id)
            .ok_or_else(|| anyhow::anyhow!("{label}: mod '{}' not in mods.json", spec.mod_id))?;
        // RePoE classifies some enchants (e.g. Heist blueprint enchants)
        // under non-enchantment generation types, so only explicit affixes
        // are certainly wrong in the enchant slot.
        if matches!(
            m.generation_type,
            GenerationType::Prefix | GenerationType::Suffix
        ) {
            bail!(
                "{label}: '{}' has generation type {:?}, expected enchantment",
                spec.mod_id,
                m.generation_type
            );
        }
        item.enchants.push(Modifier {
            mod_id: spec.mod_id.clone(),
            generation_type: m.generation_type.clone(),
            rolls: imported_rolls(label, &spec.mod_id, m, &spec.values)?,
        });
    }

    for spec in &imported.explicit_mods {
        let label = "imported explicit";
        let m = db
            .mods
            .get(&spec.mod_id)
            .ok_or_else(|| anyhow::anyhow!("{label}: mod '{}' not in mods.json", spec.mod_id))?;
        if spec.fractured && spec.crafted {
            bail!(
                "{label}: '{}' cannot be both fractured and crafted",
                spec.mod_id
            );
        }
        if !matches!(
            m.generation_type,
            GenerationType::Prefix | GenerationType::Suffix
        ) {
            bail!("{label}: '{}' is not a prefix or suffix", spec.mod_id);
        }
        if m.required_level > item_level {
            bail!(
                "{label}: '{}' requires item level {}, but the imported item level is {item_level}",
                spec.mod_id,
                m.required_level
            );
        }
        // Deliberately NO spawn-weight check here (see the function docs):
        // existing imported mods may be fractured/legacy/Delve mods that can
        // no longer roll. `eligible_mods` still excludes them from every
        // random pool because they fail `is_craftable()` or have zero weight.
        if spec.crafted {
            if m.domain != Domain::Crafted {
                bail!(
                    "{label}: crafted mod '{}' has domain {:?}, expected crafted",
                    spec.mod_id,
                    m.domain
                );
            }
            if item.crafted_mod.is_some() {
                bail!("{label}: only one crafted mod is allowed");
            }
        } else if m.domain == Domain::Crafted {
            bail!(
                "{label}: '{}' is a crafted-domain mod but the line is not \
                 annotated (crafted); it must occupy the crafted-mod slot",
                spec.mod_id
            );
        }

        let conflict = item
            .all_mods_for_conflict()
            .filter_map(|placed| db.mods.get(&placed.mod_id))
            .flat_map(|placed| placed.groups.iter())
            .any(|g| m.groups.contains(g));
        if conflict {
            bail!(
                "{label}: '{}' shares a mod group with another imported mod",
                spec.mod_id
            );
        }

        let open = match m.generation_type {
            GenerationType::Prefix => item.has_open_prefix(),
            _ => item.has_open_suffix(),
        };
        if !open {
            bail!(
                "{label}: no open {} slot for '{}' on a {:?} item",
                if m.generation_type == GenerationType::Prefix {
                    "prefix"
                } else {
                    "suffix"
                },
                spec.mod_id,
                item.rarity
            );
        }

        let modifier = Modifier {
            mod_id: spec.mod_id.clone(),
            generation_type: m.generation_type.clone(),
            rolls: imported_rolls(label, &spec.mod_id, m, &spec.values)?,
        };
        if spec.crafted {
            item.crafted_mod = Some(modifier);
        } else if spec.fractured {
            item.fractured.push(modifier);
        } else {
            match m.generation_type {
                GenerationType::Prefix => item.prefixes.push(modifier),
                _ => item.suffixes.push(modifier),
            }
        }
    }

    Ok(item)
}

/// One desired mod. All specified criteria must hold on a single mod
/// for the want to be satisfied (see module docs).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WantSpec {
    /// Exact RePoE mod ID (e.g. "IncreasedLife7").
    pub mod_id: Option<String>,
    /// RePoE mod group (e.g. "IncreasedLife") — matches any tier in the group.
    pub group: Option<String>,
    /// RePoE stat ID (e.g. "base_maximum_life").
    pub stat: Option<String>,
    /// Minimum rolled value for `stat`. Requires `stat` to be set.
    pub min_value: Option<i32>,
    /// Maximum rolled value for lower-is-better numeric goals.
    pub max_value: Option<i32>,
    /// Explicit scoring mode. When omitted, legacy presence/threshold
    /// inference is used.
    pub mode: Option<GoalScoringMode>,
    /// Optional score cap for per-unit goals. It is mandatory for
    /// lower-is-better per-unit scoring.
    pub cap: Option<i32>,
    /// Score contribution when satisfied. Defaults to 1.0; must be > 0.
    #[serde(default = "default_weight")]
    pub weight: f64,
    /// Whether this want is mandatory for target completion. Defaults to true
    /// so existing goal files retain their exact completion semantics.
    #[serde(default = "default_required")]
    pub required: bool,
}

impl WantSpec {
    /// Effective scoring mode after applying legacy goal-file inference.
    pub const fn scoring_mode(&self) -> GoalScoringMode {
        match self.mode {
            Some(mode) => mode,
            None if self.min_value.is_some() || self.max_value.is_some() => {
                GoalScoringMode::Threshold
            }
            None => GoalScoringMode::Presence,
        }
    }
}

/// Search-parameter overrides. Precedence: CLI flag > goal file > built-in default.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchSpec {
    pub beam_width: Option<usize>,
    pub max_steps: Option<usize>,
    /// Cost penalty per expected chaos in node ranking; tune relative to the sum
    /// of want weights. 0.0 ignores cost entirely (the search will happily exalt-spam).
    pub cost_weight: Option<f64>,
    /// Cost to restore or replace the starting base after a failed one-shot path.
    pub restart_cost: Option<f64>,
    /// RNG seed for reproducible searches. Omit for a fresh random search per run.
    pub seed: Option<u64>,
    /// How many distinct pathways to report (default 1).
    pub top: Option<usize>,
    /// Optional maximum number of concrete successor states generated.
    pub expansion_limit: Option<u64>,
    /// Optional wall-clock limit in milliseconds.
    pub timeout_ms: Option<u64>,
}

fn default_bench_cost() -> f64 {
    2.0
}

/// One extra crafting method, selected by `type`. Example:
///
/// ```toml
/// [[methods]]
/// type = "essence"
/// essence = "Deafening Essence of Greed" # metadata ID also accepted
/// cost = 5.0
///
/// [[methods]]
/// type = "harvest"
/// op = "reforge"                # reforge | augment
/// target = "life"               # attack, caster, speed, life, defences, ...
/// cost = 30.0
///
/// [[methods]]
/// type = "bench"
/// mod_id = "EinharMasterAddedLife1"   # any mods.json entry with domain = "crafted"
///
/// [[methods]]
/// type = "fossil"
/// fossil = "Pristine Fossil"    # metadata ID also accepted
/// cost = 10.0
///
/// [[methods]]
/// type = "eldritch_chaos"       # or "eldritch_exalt" / "eldritch_annul"
/// god = "exarch"                # exarch | eater
///
/// [[methods]]
/// type = "conqueror_exalt"
/// influence = "hunter"          # crusader | hunter | redeemer | warlord
///
/// [[methods]]
/// type = "bestiary_swap"
/// add = "prefix"                # prefix | suffix
/// beast_level = 83
/// cost = 12.0
/// ```
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MethodSpec {
    /// Guarantees `mod_id`, rerolls the rest (Monte Carlo).
    Essence {
        name: Option<String>,
        /// RePoE essence metadata ID or case-insensitive display name.
        #[serde(default)]
        essence: Option<String>,
        /// Legacy escape hatch. Catalog-backed runs validate it against the
        /// selected base class; prefer `essence`.
        #[serde(default)]
        mod_id: Option<String>,
        cost: f64,
    },
    /// Deterministically adds crafted mod `mod_id` (must have domain = "crafted").
    Bench {
        name: Option<String>,
        mod_id: String,
        #[serde(default = "default_bench_cost")]
        cost: f64,
    },
    /// Fossil/resonator reroll with modified spawn weights. The flat tag/id
    /// fields describe a single fossil; add `[[methods.fossils]]` sub-tables
    /// to socket more fossils into the same resonator.
    Fossil {
        name: Option<String>,
        /// RePoE fossil metadata ID or case-insensitive display name.
        #[serde(default)]
        fossil: Option<String>,
        cost: f64,
        #[serde(default)]
        boosted_tags: Vec<String>,
        #[serde(default)]
        reduced_tags: Vec<String>,
        #[serde(default)]
        blocked_mod_ids: Vec<String>,
        #[serde(default)]
        forced_mod_ids: Vec<String>,
        /// Additional fossils in the same resonator (multi-fossil crafts).
        #[serde(default)]
        fossils: Vec<FossilPartSpec>,
    },
    /// Current targeted Harvest craft: "reforge" or "augment".
    Harvest {
        op: String,
        target: String,
        cost: f64,
    },
    /// Rerolls explicit prefixes/suffixes selected by Eldritch dominance.
    EldritchChaos { god: String },
    /// Adds an explicit prefix/suffix selected by Eldritch dominance.
    EldritchExalt { god: String },
    /// Removes an explicit prefix/suffix selected by Eldritch dominance.
    EldritchAnnul { god: String },
    /// Adds one Conqueror-exclusive affix and applies that influence.
    ConquerorExalt { influence: String },
    /// Bestiary: remove a random opposite-side affix and add the requested side.
    BestiarySwap {
        add: String,
        beast_level: u32,
        cost: f64,
    },
}

/// One fossil inside a multi-fossil resonator (`[[methods.fossils]]`).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FossilPartSpec {
    /// RePoE fossil metadata ID or case-insensitive display name.
    #[serde(default)]
    pub fossil: Option<String>,
    #[serde(default)]
    pub boosted_tags: Vec<String>,
    #[serde(default)]
    pub reduced_tags: Vec<String>,
    #[serde(default)]
    pub blocked_mod_ids: Vec<String>,
    #[serde(default)]
    pub forced_mod_ids: Vec<String>,
}

impl MethodSpec {
    fn configured_cost(&self) -> Option<f64> {
        match self {
            Self::Essence { cost, .. }
            | Self::Bench { cost, .. }
            | Self::Fossil { cost, .. }
            | Self::Harvest { cost, .. }
            | Self::BestiarySwap { cost, .. } => Some(*cost),
            Self::EldritchChaos { .. }
            | Self::EldritchExalt { .. }
            | Self::EldritchAnnul { .. }
            | Self::ConquerorExalt { .. } => None,
        }
    }

    /// Build the runtime `CraftingMethod`, validating references against the DB
    /// so a typo'd mod ID fails at load time rather than mid-search.
    pub fn build(&self, db: &GameData) -> Result<Arc<dyn CraftingMethod>> {
        self.build_with_item(db, None)
    }

    /// Build with the selected base context, enabling class-aware validation
    /// and named essence/fossil resolution from the optional RePoE catalogs.
    pub fn build_for_item(
        &self,
        db: &GameData,
        base: &BaseItem,
        _item_level: u32,
    ) -> Result<Arc<dyn CraftingMethod>> {
        self.build_with_item(db, Some(base))
    }

    fn build_with_item(
        &self,
        db: &GameData,
        base: Option<&BaseItem>,
    ) -> Result<Arc<dyn CraftingMethod>> {
        match self {
            MethodSpec::Essence {
                name,
                essence,
                mod_id,
                cost,
            } => {
                let mut catalog_name = None;
                let mut maximum_item_level = None;
                let mut can_reforge_rare = true;
                let guaranteed_mod_id = match (essence, mod_id) {
                    (Some(selector), None) => {
                        let catalog = db.essences.as_ref().ok_or_else(|| {
                            anyhow::anyhow!(
                                "[[methods]] essence: named essences require data/essences.json"
                            )
                        })?;
                        let base = base.ok_or_else(|| {
                            anyhow::anyhow!(
                                "[[methods]] essence: named essence requires base-item context"
                            )
                        })?;
                        let resolved =
                            catalog.resolve_guaranteed_mod(selector, &base.item_class)?;
                        catalog_name = Some(resolved.essence.name.clone());
                        maximum_item_level = resolved.essence.item_level_restriction;
                        can_reforge_rare = resolved.essence.item_level_restriction.is_none();
                        resolved.guaranteed_mod_id.to_string()
                    }
                    (None, Some(mod_id)) => {
                        if let (Some(catalog), Some(base)) = (&db.essences, base) {
                            let resolved =
                                catalog.resolve_by_guaranteed_mod(mod_id, &base.item_class)?;
                            catalog_name = Some(resolved.essence.name.clone());
                            maximum_item_level = resolved.essence.item_level_restriction;
                            can_reforge_rare = resolved.essence.item_level_restriction.is_none();
                        }
                        mod_id.clone()
                    }
                    _ => bail!("[[methods]] essence: specify exactly one of 'essence' or 'mod_id'"),
                };
                if !db.mods.contains_key(&guaranteed_mod_id) {
                    bail!(
                        "[[methods]] essence: mod_id '{guaranteed_mod_id}' not found in mods.json"
                    );
                }
                Ok(Arc::new(Essence {
                    display_name: name
                        .clone()
                        .or(catalog_name)
                        .unwrap_or_else(|| format!("Essence ({guaranteed_mod_id})")),
                    guaranteed_mod_id,
                    cost_chaos: *cost,
                    max_item_level: maximum_item_level,
                    can_reforge_rare,
                }))
            }
            MethodSpec::Bench { name, mod_id, cost } => {
                let m = db.mods.get(mod_id).ok_or_else(|| {
                    anyhow::anyhow!("[[methods]] bench: mod_id '{mod_id}' not found in mods.json")
                })?;
                if m.domain != Domain::Crafted {
                    bail!(
                        "[[methods]] bench: mod '{mod_id}' has domain {:?}, expected crafted",
                        m.domain
                    );
                }
                if let (Some(catalog), Some(base)) = (&db.crafting_bench, base) {
                    catalog.validate_add_explicit_mod(mod_id, &base.item_class)?;
                }
                Ok(Arc::new(BenchCraft {
                    display_name: name
                        .clone()
                        .unwrap_or_else(|| format!("Bench Craft ({mod_id})")),
                    mod_id: mod_id.clone(),
                    cost_chaos: *cost,
                }))
            }
            MethodSpec::Fossil {
                name,
                fossil,
                cost,
                boosted_tags,
                reduced_tags,
                blocked_mod_ids,
                forced_mod_ids,
                fossils,
            } => {
                let mut parts = Vec::with_capacity(1 + fossils.len());
                let mut catalog_names = Vec::new();
                let mut catalog_ids = std::collections::HashSet::new();
                let raw_parts = std::iter::once((
                    fossil.as_deref(),
                    boosted_tags,
                    reduced_tags,
                    blocked_mod_ids,
                    forced_mod_ids,
                ))
                .chain(fossils.iter().map(|part| {
                    (
                        part.fossil.as_deref(),
                        &part.boosted_tags,
                        &part.reduced_tags,
                        &part.blocked_mod_ids,
                        &part.forced_mod_ids,
                    )
                }));
                for (selector, boosted, reduced, blocked, forced) in raw_parts {
                    if let Some(selector) = selector {
                        let catalog = db.fossils.as_ref().ok_or_else(|| {
                            anyhow::anyhow!(
                                "[[methods]] fossil: named fossils require data/fossils.json"
                            )
                        })?;
                        let resolved = catalog.resolve(selector)?;
                        resolved.fossil.validate_supported_behavior()?;
                        if !catalog_ids.insert(resolved.metadata_id) {
                            bail!(
                                "[[methods]] fossil: duplicate fossil '{}' in one resonator",
                                resolved.fossil.name
                            );
                        }
                        if let Some(base) = base {
                            let tags: Vec<&str> = base.tags.iter().map(String::as_str).collect();
                            if !resolved.fossil.is_allowed_for_item_tags(&tags) {
                                bail!(
                                    "[[methods]] fossil: '{}' cannot be used on item class '{}'",
                                    resolved.fossil.name,
                                    base.item_class
                                );
                            }
                        }
                        catalog_names.push(resolved.fossil.name.clone());
                        let tag_weights = resolved
                            .fossil
                            .positive_mod_weights
                            .iter()
                            .chain(&resolved.fossil.negative_mod_weights)
                            .map(|rule| FossilWeightRule {
                                tag: rule.tag.clone(),
                                weight: rule.weight,
                            })
                            .collect();
                        parts.push(FossilModifier {
                            boosted_tags: vec![],
                            reduced_tags: vec![],
                            blocked_mod_ids: vec![],
                            forced_mod_ids: resolved.fossil.forced_mods.clone(),
                            added_mod_ids: resolved.fossil.added_mods.clone(),
                            tag_weights,
                        });
                    } else {
                        if boosted.is_empty()
                            && reduced.is_empty()
                            && blocked.is_empty()
                            && forced.is_empty()
                        {
                            bail!(
                                "[[methods]] fossil: each resonator slot must name a fossil \
                                 or define at least one manual effect"
                            );
                        }
                        parts.push(FossilModifier {
                            boosted_tags: boosted.clone(),
                            reduced_tags: reduced.clone(),
                            blocked_mod_ids: blocked.clone(),
                            forced_mod_ids: forced.clone(),
                            added_mod_ids: vec![],
                            tag_weights: vec![],
                        });
                    }
                }
                for id in parts.iter().flat_map(|p| {
                    p.blocked_mod_ids
                        .iter()
                        .chain(&p.forced_mod_ids)
                        .chain(&p.added_mod_ids)
                }) {
                    if !db.mods.contains_key(id) {
                        bail!("[[methods]] fossil: mod_id '{id}' not found in mods.json");
                    }
                }
                Ok(Arc::new(FossilCraft {
                    display_name: name.clone().unwrap_or_else(|| {
                        if catalog_names.is_empty() {
                            "Fossil Craft".to_string()
                        } else {
                            catalog_names.join(" + ")
                        }
                    }),
                    cost_chaos: *cost,
                    fossils: parts,
                }))
            }
            MethodSpec::Harvest { op, target, cost } => {
                let parsed_op = HarvestOp::parse(op).ok_or_else(|| {
                    anyhow::anyhow!(
                        "[[methods]] harvest: unknown op '{op}' (expected reforge or augment)"
                    )
                })?;
                let parsed_target = HarvestTarget::parse(target).ok_or_else(|| {
                    anyhow::anyhow!("[[methods]] harvest: unknown target '{target}'")
                })?;
                Ok(Arc::new(HarvestCraft {
                    display_name: format!("Harvest {op} {target}"),
                    cost_chaos: *cost,
                    target: parsed_target,
                    op: parsed_op,
                }))
            }
            MethodSpec::EldritchChaos { god } => Ok(Arc::new(EldritchChaosOrb {
                god: parse_god(god)?,
            })),
            MethodSpec::EldritchExalt { god } => Ok(Arc::new(EldritchExaltedOrb {
                god: parse_god(god)?,
            })),
            MethodSpec::EldritchAnnul { god } => Ok(Arc::new(EldritchOrbOfAnnulment {
                god: parse_god(god)?,
            })),
            MethodSpec::ConquerorExalt { influence } => Ok(Arc::new(ConquerorExaltedOrb {
                influence: parse_conqueror_influence(influence)?,
            })),
            MethodSpec::BestiarySwap {
                add,
                beast_level,
                cost,
            } => {
                let kind = match add.as_str() {
                    "prefix" => BestiaryAffixSwapKind::AddPrefixRemoveSuffix,
                    "suffix" => BestiaryAffixSwapKind::AddSuffixRemovePrefix,
                    other => bail!(
                        "[[methods]] bestiary_swap: unknown add side '{other}' \
                         (expected prefix or suffix)"
                    ),
                };
                Ok(Arc::new(BestiaryAffixSwapCraft::new(
                    kind,
                    *beast_level,
                    *cost,
                )?))
            }
        }
    }
}

/// Validate configured crafting-method requests independently of goal-file parsing.
pub(crate) fn validate_method_specs(methods: &[MethodSpec]) -> Result<()> {
    for (i, method) in methods.iter().enumerate() {
        if let Some(cost) = method.configured_cost() {
            if cost <= 0.0 || !cost.is_finite() {
                bail!("[[methods]] entry {i}: cost must be a positive finite number");
            }
        }
        if let MethodSpec::Essence {
            essence, mod_id, ..
        } = method
        {
            if essence.is_some() == mod_id.is_some() {
                bail!(
                    "[[methods]] entry {i}: essence requires exactly one of \
                     'essence' or 'mod_id'"
                );
            }
        }
        if let MethodSpec::Fossil {
            fossil,
            boosted_tags,
            reduced_tags,
            blocked_mod_ids,
            forced_mod_ids,
            fossils,
            ..
        } = method
        {
            if fossils.len() > 3 {
                bail!("[[methods]] entry {i}: a resonator can contain at most 4 fossils");
            }
            if fossil.is_some()
                && (!boosted_tags.is_empty()
                    || !reduced_tags.is_empty()
                    || !blocked_mod_ids.is_empty()
                    || !forced_mod_ids.is_empty())
            {
                bail!("[[methods]] entry {i}: named fossil cannot also use manual tag/mod fields");
            }
            if let Some(part) = fossils.iter().find(|part| {
                part.fossil.is_some()
                    && (!part.boosted_tags.is_empty()
                        || !part.reduced_tags.is_empty()
                        || !part.blocked_mod_ids.is_empty()
                        || !part.forced_mod_ids.is_empty())
            }) {
                bail!(
                    "[[methods]] entry {i}: named fossil '{}' cannot also use manual tag/mod fields",
                    part.fossil.as_deref().unwrap_or_default()
                );
            }
            let blocked = blocked_mod_ids
                .iter()
                .chain(fossils.iter().flat_map(|f| f.blocked_mod_ids.iter()));
            let forced: std::collections::HashSet<&str> = forced_mod_ids
                .iter()
                .chain(fossils.iter().flat_map(|f| f.forced_mod_ids.iter()))
                .map(String::as_str)
                .collect();
            if let Some(id) = blocked.map(String::as_str).find(|id| forced.contains(id)) {
                bail!("[[methods]] entry {i}: fossil mod '{id}' cannot be both blocked and forced");
            }
        }
        if let MethodSpec::BestiarySwap { beast_level, .. } = method {
            if !(1..=100).contains(beast_level) {
                bail!("[[methods]] entry {i}: beast_level must be between 1 and 100");
            }
        }
    }
    Ok(())
}

fn parse_god(s: &str) -> Result<EldritchGod> {
    match s {
        "exarch" => Ok(EldritchGod::SearingExarch),
        "eater" => Ok(EldritchGod::EaterOfWorlds),
        other => bail!("[[methods]] eldritch: unknown god '{other}' (expected exarch or eater)"),
    }
}

fn parse_conqueror_influence(s: &str) -> Result<Influence> {
    match s {
        "crusader" => Ok(Influence::Crusader),
        "hunter" => Ok(Influence::Hunter),
        "redeemer" => Ok(Influence::Redeemer),
        "warlord" => Ok(Influence::Warlord),
        other => bail!(
            "[[methods]] conqueror_exalt: unknown influence '{other}' (expected crusader, hunter, redeemer, or warlord)"
        ),
    }
}

/// Evaluates an owned caller's goal selectors without depending on a goal-file
/// or CLI representation.
#[derive(Debug, Clone, Copy)]
pub struct GoalEvaluator<'a> {
    wants: &'a [WantSpec],
}

/// Allocation-free aggregate facts for one item against a goal set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoalEvaluation {
    pub score: f64,
    pub satisfied_count: usize,
    pub required_goal_count: usize,
    pub satisfied_required_count: usize,
}

impl GoalEvaluation {
    /// True when every required want is satisfied. An all-preferred goal set is
    /// complete by definition while its preferences still contribute score.
    pub const fn complete(self) -> bool {
        self.satisfied_required_count == self.required_goal_count
    }
}

/// One structured goal-satisfaction fact for presentation adapters.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalReportEntry {
    pub description: String,
    pub required: bool,
    pub satisfied: bool,
    pub scoring_mode: GoalScoringMode,
    /// Selected numeric value. Per-unit goals report the aggregate across all
    /// matching modifiers; threshold goals report the best matching value for
    /// their direction. Presence goals report `None`.
    pub attained: Option<i64>,
    pub contribution: f64,
}

/// Stable warning classification produced while resolving numeric selectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GoalSelectorWarningCode {
    ImplicitFirstStat,
}

impl GoalSelectorWarningCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ImplicitFirstStat => "implicit_first_stat",
        }
    }
}

/// A non-fatal warning tied to one goal selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalSelectorWarning {
    pub code: GoalSelectorWarningCode,
    pub want_index: usize,
    /// JSON-pointer-compatible path used by machine-readable adapters.
    pub field_path: String,
    /// Deterministically sorted RePoE mod IDs that make the selector ambiguous.
    pub matching_mod_ids: Vec<String>,
    pub message: String,
}

/// Stable reason code for a conservatively proven impossible required goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImpossibleGoalReasonCode {
    ItemNotCraftable,
    NoReachableModifier,
    ModifierGroupConflict,
    AffixCapacity,
}

impl ImpossibleGoalReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ItemNotCraftable => "item_not_craftable",
            Self::NoReachableModifier => "no_reachable_modifier",
            Self::ModifierGroupConflict => "modifier_group_conflict",
            Self::AffixCapacity => "affix_capacity",
        }
    }
}

/// One per-want explanation for a conservatively proven impossible request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpossibleGoalReason {
    pub code: ImpossibleGoalReasonCode,
    pub want_index: usize,
    pub field_path: String,
    pub related_want_indices: Vec<usize>,
    pub message: String,
}

impl<'a> GoalEvaluator<'a> {
    pub const fn new(wants: &'a [WantSpec]) -> Self {
        Self { wants }
    }

    /// Structural validation for goal selectors.
    pub fn validate(&self) -> Result<()> {
        if self.wants.is_empty() {
            bail!("Goal must contain at least one [[wants]] entry");
        }
        let mut representable_score_bound = 0.0_f64;
        for (i, want) in self.wants.iter().enumerate() {
            if want.mod_id.is_none() && want.group.is_none() && want.stat.is_none() {
                bail!("[[wants]] entry {i}: specify at least one of mod_id, group, stat");
            }
            if want.min_value.is_some() && want.max_value.is_some() {
                bail!("[[wants]] entry {i}: min_value and max_value are mutually exclusive");
            }
            if want.weight <= 0.0 || !want.weight.is_finite() {
                bail!("[[wants]] entry {i}: weight must be a positive finite number");
            }
            match want.scoring_mode() {
                GoalScoringMode::Presence => {
                    if want.min_value.is_some() || want.max_value.is_some() {
                        bail!(
                            "[[wants]] entry {i}: presence mode does not accept min_value or max_value"
                        );
                    }
                    if want.cap.is_some() {
                        bail!("[[wants]] entry {i}: presence mode does not accept cap");
                    }
                }
                GoalScoringMode::Threshold => {
                    if want.min_value.is_none() && want.max_value.is_none() {
                        bail!(
                            "[[wants]] entry {i}: threshold mode requires exactly one of min_value or max_value"
                        );
                    }
                    if want.cap.is_some() {
                        bail!("[[wants]] entry {i}: threshold mode does not accept cap");
                    }
                }
                GoalScoringMode::PerUnit => {
                    if want.required && want.min_value.is_none() && want.max_value.is_none() {
                        bail!(
                            "[[wants]] entry {i}: a required per_unit goal needs min_value or max_value"
                        );
                    }
                    if want.max_value.is_some() && want.cap.is_none() {
                        bail!("[[wants]] entry {i}: lower-is-better per_unit scoring requires cap");
                    }
                }
            }

            let maximum_representable_units = match want.scoring_mode() {
                GoalScoringMode::Presence | GoalScoringMode::Threshold => 1.0,
                GoalScoringMode::PerUnit if want.max_value.is_none() && want.cap.is_some() => {
                    f64::from(want.cap.expect("checked above").max(0))
                }
                GoalScoringMode::PerUnit => i64::MAX as f64,
            };
            let contribution_bound = want.weight * maximum_representable_units;
            if !contribution_bound.is_finite() {
                bail!(
                    "[[wants]] entry {i}: weight and per-unit range can overflow the finite score representation"
                );
            }
            representable_score_bound += contribution_bound;
            if !representable_score_bound.is_finite() {
                bail!("Goal weights can overflow the finite aggregate score representation");
            }
        }
        Ok(())
    }

    /// Validate goal selectors against the loaded RePoE export so typos fail
    /// before an expensive search silently chases an impossible target.
    pub fn validate_against_db(&self, db: &GameData) -> Result<()> {
        for (i, want) in self.wants.iter().enumerate() {
            if let Some(mod_id) = &want.mod_id {
                if !db.mods.contains_key(mod_id) {
                    bail!("[[wants]] entry {i}: mod_id '{mod_id}' not found in mods.json");
                }
            }
            if let Some(group) = &want.group {
                if !db
                    .mods
                    .values()
                    .any(|candidate| candidate.groups.iter().any(|g| g == group))
                {
                    bail!("[[wants]] entry {i}: group '{group}' not found in mods.json");
                }
            }
            if let Some(stat) = &want.stat {
                if !db
                    .mods
                    .values()
                    .any(|candidate| candidate.stats.iter().any(|s| &s.id == stat))
                {
                    bail!("[[wants]] entry {i}: stat '{stat}' not found in mods.json");
                }
            }
            if want.scoring_mode() != GoalScoringMode::Presence
                && !matching_db_mods(want, db)
                    .any(|candidate| selected_db_stat_id(want, candidate).is_some())
            {
                bail!(
                    "[[wants]] entry {i}: numeric goal matches no modifier with a selectable stat"
                );
            }
        }
        Ok(())
    }

    /// Warn when a numeric selector relies on RePoE's first-stat ordering for
    /// one or more multi-stat modifiers.
    pub fn selector_warnings(&self, db: &GameData) -> Vec<GoalSelectorWarning> {
        self.wants
            .iter()
            .enumerate()
            .filter(|(_, want)| {
                want.scoring_mode() != GoalScoringMode::Presence && want.stat.is_none()
            })
            .filter_map(|(want_index, want)| {
                let mut matching_mod_ids = db
                    .mods
                    .iter()
                    .filter(|(_, candidate)| candidate.stats.len() > 1)
                    .filter(|(mod_id, candidate)| db_mod_matches_selectors(want, mod_id, candidate))
                    .map(|(mod_id, _)| mod_id.clone())
                    .collect::<Vec<_>>();
                matching_mod_ids.sort();
                matching_mod_ids.dedup();
                if matching_mod_ids.is_empty() {
                    return None;
                }
                Some(GoalSelectorWarning {
                    code: GoalSelectorWarningCode::ImplicitFirstStat,
                    want_index,
                    field_path: format!("/goals/{want_index}/stat"),
                    message: format!(
                        "numeric goal {want_index} omits stat and matches multi-stat modifiers; \
                         using each modifier's first RePoE-declared stat (set stat explicitly to \
                         avoid declaration-order dependence)"
                    ),
                    matching_mod_ids,
                })
            })
            .collect()
    }

    /// Evaluate score, total satisfaction, and required-goal completion in one
    /// allocation-free pass over the wants.
    pub fn evaluate(&self, state: &ItemState, db: &GameData) -> GoalEvaluation {
        let mut evaluation = GoalEvaluation {
            score: 0.0,
            satisfied_count: 0,
            required_goal_count: 0,
            satisfied_required_count: 0,
        };

        for want in self.wants {
            if want.required {
                evaluation.required_goal_count += 1;
            }
            let assessment = assess_want(want, state, db);
            evaluation.score += assessment.contribution;
            if assessment.satisfied {
                evaluation.satisfied_count += 1;
                if want.required {
                    evaluation.satisfied_required_count += 1;
                }
            }
        }
        evaluation
    }

    /// Score an item state: sum of weights over all satisfied wants.
    pub fn score(&self, state: &ItemState, db: &GameData) -> f64 {
        self.evaluate(state, db).score
    }

    /// Maximum raw goal score when every requested condition is satisfied.
    ///
    /// This compatibility accessor returns infinity when an uncapped numeric
    /// goal has no request-only ceiling. New callers should prefer
    /// [`Self::maximum_score`].
    pub fn max_score(&self) -> f64 {
        self.maximum_score().unwrap_or(f64::INFINITY)
    }

    /// Request-only upper score bound, when one exists.
    ///
    /// Uncapped higher-is-better and lower-is-better per-unit goals require
    /// item/data-aware analysis for a sound ceiling, so they return `None`.
    pub fn maximum_score(&self) -> Option<f64> {
        self.wants.iter().try_fold(0.0, |total, want| {
            let contribution = match want.scoring_mode() {
                GoalScoringMode::Presence | GoalScoringMode::Threshold => want.weight,
                GoalScoringMode::PerUnit if want.max_value.is_some() => return None,
                GoalScoringMode::PerUnit => want.weight * f64::from(want.cap?.max(0)),
            };
            Some(total + contribution)
        })
    }

    /// Number of requested conditions satisfied by `state`.
    pub fn satisfied_count(&self, state: &ItemState, db: &GameData) -> usize {
        self.evaluate(state, db).satisfied_count
    }

    pub fn required_goal_count(&self) -> usize {
        self.wants.iter().filter(|want| want.required).count()
    }

    pub fn satisfied_required_count(&self, state: &ItemState, db: &GameData) -> usize {
        self.evaluate(state, db).satisfied_required_count
    }

    /// True when every required condition is present on the same item.
    pub fn is_complete(&self, state: &ItemState, db: &GameData) -> bool {
        self.evaluate(state, db).complete()
    }

    /// One structured satisfaction entry per want.
    pub fn report_entries(&self, state: &ItemState, db: &GameData) -> Vec<GoalReportEntry> {
        self.wants
            .iter()
            .map(|want| {
                let assessment = assess_want(want, state, db);
                GoalReportEntry {
                    description: describe_want(want),
                    required: want.required,
                    satisfied: assessment.satisfied,
                    scoring_mode: want.scoring_mode(),
                    attained: assessment.attained,
                    contribution: assessment.contribution,
                }
            })
            .collect()
    }

    /// Backward-compatible human-readable `(description, satisfied)` pairs.
    pub fn report(&self, state: &ItemState, db: &GameData) -> Vec<(String, bool)> {
        self.report_entries(state, db)
            .into_iter()
            .map(|entry| (entry.description, entry.satisfied))
            .collect()
    }
}

#[derive(Debug, Clone)]
struct GoalCandidate {
    mod_id: String,
    generation_type: GenerationType,
    groups: Vec<String>,
    crafted: bool,
}

/// Conservatively prove required-goal impossibility before search.
///
/// The analysis deliberately returns no reason when a broad selector or
/// combinatorial case cannot be proven cheaply. A false negative merely allows
/// the normal search to return incomplete; false positives are forbidden.
pub fn analyze_impossible_goals(
    wants: &[WantSpec],
    state: &ItemState,
    db: &GameData,
    provided_mod_ids: &HashSet<String>,
) -> Vec<ImpossibleGoalReason> {
    let evaluator = GoalEvaluator::new(wants);
    if evaluator.is_complete(state, db) {
        return Vec::new();
    }

    if !state.is_craftable() {
        return wants
            .iter()
            .enumerate()
            .filter(|(_, want)| want.required)
            .filter(|(_, want)| !assess_want(want, state, db).satisfied)
            .map(|(want_index, _)| ImpossibleGoalReason {
                code: ImpossibleGoalReasonCode::ItemNotCraftable,
                want_index,
                field_path: format!("/goals/{want_index}"),
                related_want_indices: Vec::new(),
                message: "the starting item is corrupted or mirrored and cannot be modified"
                    .to_string(),
            })
            .collect();
    }

    let existing_mod_ids = scorable_mods(state)
        .map(|modifier| modifier.mod_id.as_str())
        .collect::<HashSet<_>>();
    let mut analyzable = Vec::new();
    let mut reasons = Vec::new();

    for (want_index, want) in wants.iter().enumerate() {
        if !want.required || assess_want(want, state, db).satisfied {
            continue;
        }

        let matching_candidates = matching_db_mod_entries(want, db).collect::<Vec<_>>();
        if matching_candidates.iter().any(|(_, candidate)| {
            !matches!(
                candidate.generation_type,
                GenerationType::Prefix | GenerationType::Suffix
            )
        }) {
            // Implicits and enchantments are slot-free and may have separate
            // acquisition rules. Leave mixed/non-affix selectors to search
            // rather than proving impossibility from explicit-affix rules.
            continue;
        }

        let per_unit = want.scoring_mode() == GoalScoringMode::PerUnit;
        let mut candidates = matching_candidates
            .into_iter()
            .filter(|(_, candidate)| {
                matches!(
                    candidate.generation_type,
                    GenerationType::Prefix | GenerationType::Suffix
                )
            })
            .filter(|(_, candidate)| per_unit || db_mod_can_meet_threshold(want, candidate))
            .filter(|(mod_id, candidate)| {
                let existing = existing_mod_ids.contains(mod_id.as_str());
                let provided = provided_mod_ids.contains(*mod_id);
                let random_pool_reachable = candidate.domain == Domain::Item
                    && !candidate.is_essence_only
                    && candidate.required_level <= state.item_level
                    && candidate
                        .spawn_weights
                        .iter()
                        .any(|weight| weight.weight > 0);
                existing || provided || random_pool_reachable
            })
            .map(|(mod_id, candidate)| GoalCandidate {
                mod_id: mod_id.clone(),
                generation_type: candidate.generation_type.clone(),
                groups: candidate.groups.clone(),
                crafted: candidate.domain == Domain::Crafted,
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.mod_id.cmp(&right.mod_id));
        candidates.dedup_by(|left, right| left.mod_id == right.mod_id);

        if candidates.is_empty() {
            reasons.push(ImpossibleGoalReason {
                code: ImpossibleGoalReasonCode::NoReachableModifier,
                want_index,
                field_path: format!("/goals/{want_index}"),
                related_want_indices: Vec::new(),
                message: format!(
                    "required goal {want_index} has no level-valid prefix or suffix with a \
                     positive normal weight or enabled deterministic source"
                ),
            });
        } else if !per_unit {
            analyzable.push((want_index, candidates));
        }
    }

    if !reasons.is_empty()
        || analyzable.len() < 2
        || analyzable.len() > 6
        || analyzable
            .iter()
            .any(|(_, candidates)| candidates.len() > 64)
    {
        return reasons;
    }

    if goal_combination_possible(&analyzable, state, db, true) {
        return reasons;
    }

    let code = if goal_combination_possible(&analyzable, state, db, false) {
        ImpossibleGoalReasonCode::ModifierGroupConflict
    } else {
        ImpossibleGoalReasonCode::AffixCapacity
    };
    let all_indices = analyzable
        .iter()
        .map(|(want_index, _)| *want_index)
        .collect::<Vec<_>>();
    for (want_index, _) in analyzable {
        reasons.push(ImpossibleGoalReason {
            code,
            want_index,
            field_path: format!("/goals/{want_index}"),
            related_want_indices: all_indices
                .iter()
                .copied()
                .filter(|related| *related != want_index)
                .collect(),
            message: match code {
                ImpossibleGoalReasonCode::ModifierGroupConflict => format!(
                    "required goal {want_index} cannot coexist with the related required goals \
                     because every candidate assignment has a modifier-group conflict"
                ),
                ImpossibleGoalReasonCode::AffixCapacity => format!(
                    "required goal {want_index} cannot coexist with the related required goals \
                     within three prefix, three suffix, and one crafted-mod slots"
                ),
                ImpossibleGoalReasonCode::ItemNotCraftable
                | ImpossibleGoalReasonCode::NoReachableModifier => unreachable!(),
            },
        });
    }
    reasons
}

fn db_mod_can_meet_threshold(want: &WantSpec, candidate: &Mod) -> bool {
    if want.scoring_mode() == GoalScoringMode::Presence {
        return true;
    }
    let Some(stat_id) = selected_db_stat_id(want, candidate) else {
        return false;
    };
    let Some(stat) = candidate.stats.iter().find(|stat| stat.id == stat_id) else {
        return false;
    };
    want.min_value.is_none_or(|minimum| stat.max >= minimum)
        && want.max_value.is_none_or(|maximum| stat.min <= maximum)
}

fn goal_combination_possible(
    analyzable: &[(usize, Vec<GoalCandidate>)],
    state: &ItemState,
    db: &GameData,
    enforce_groups: bool,
) -> bool {
    let mut ordered = analyzable.to_vec();
    ordered.sort_by_key(|(_, candidates)| candidates.len());

    let mut chosen_mod_ids = HashSet::new();
    let mut occupied_groups = HashSet::new();
    let mut prefix_count = 0_usize;
    let mut suffix_count = 0_usize;
    let mut crafted_count = 0_usize;
    for modifier in &state.fractured {
        chosen_mod_ids.insert(modifier.mod_id.clone());
        match modifier.generation_type {
            GenerationType::Prefix => prefix_count += 1,
            GenerationType::Suffix => suffix_count += 1,
            _ => {}
        }
        if let Some(candidate) = db.mods.get(&modifier.mod_id) {
            occupied_groups.extend(candidate.groups.iter().cloned());
            crafted_count += usize::from(candidate.domain == Domain::Crafted);
        }
    }

    assign_goal_candidates(
        &ordered,
        0,
        &mut chosen_mod_ids,
        &mut occupied_groups,
        prefix_count,
        suffix_count,
        crafted_count,
        enforce_groups,
    )
}

#[allow(clippy::too_many_arguments)]
fn assign_goal_candidates(
    wants: &[(usize, Vec<GoalCandidate>)],
    position: usize,
    chosen_mod_ids: &mut HashSet<String>,
    occupied_groups: &mut HashSet<String>,
    prefix_count: usize,
    suffix_count: usize,
    crafted_count: usize,
    enforce_groups: bool,
) -> bool {
    if position == wants.len() {
        return true;
    }
    for candidate in &wants[position].1 {
        if chosen_mod_ids.contains(&candidate.mod_id) {
            if assign_goal_candidates(
                wants,
                position + 1,
                chosen_mod_ids,
                occupied_groups,
                prefix_count,
                suffix_count,
                crafted_count,
                enforce_groups,
            ) {
                return true;
            }
            continue;
        }

        let (next_prefixes, next_suffixes) = match candidate.generation_type {
            GenerationType::Prefix => (prefix_count + 1, suffix_count),
            GenerationType::Suffix => (prefix_count, suffix_count + 1),
            _ => continue,
        };
        let next_crafted = crafted_count + usize::from(candidate.crafted);
        if next_prefixes > 3 || next_suffixes > 3 || next_crafted > 1 {
            continue;
        }
        if enforce_groups
            && candidate
                .groups
                .iter()
                .any(|group| occupied_groups.contains(group))
        {
            continue;
        }

        chosen_mod_ids.insert(candidate.mod_id.clone());
        let inserted_groups = candidate
            .groups
            .iter()
            .filter(|group| occupied_groups.insert((*group).clone()))
            .cloned()
            .collect::<Vec<_>>();
        let possible = assign_goal_candidates(
            wants,
            position + 1,
            chosen_mod_ids,
            occupied_groups,
            next_prefixes,
            next_suffixes,
            next_crafted,
            enforce_groups,
        );
        chosen_mod_ids.remove(&candidate.mod_id);
        for group in inserted_groups {
            occupied_groups.remove(&group);
        }
        if possible {
            return true;
        }
    }
    false
}

impl GoalSpec {
    /// Parse a goal spec from TOML text and validate it.
    pub fn from_toml_str(text: &str) -> Result<Self> {
        let spec: GoalSpec = toml::from_str(text).context("Failed to parse goal TOML")?;
        spec.validate()?;
        Ok(spec)
    }

    /// Load and validate a goal spec from a TOML file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read goal file {}", path.display()))?;
        Self::from_toml_str(&text).with_context(|| format!("Invalid goal file {}", path.display()))
    }

    /// Structural validation beyond what serde enforces.
    fn validate(&self) -> Result<()> {
        if self.item.base.trim().is_empty() {
            bail!("[item] base must not be empty");
        }
        if self.item.item_level == 0 || self.item.item_level > 100 {
            bail!("[item] item_level must be between 1 and 100");
        }
        GoalEvaluator::new(&self.wants).validate()?;
        for (name, price) in &self.prices {
            if *price <= 0.0 || !price.is_finite() {
                bail!("[prices] \"{name}\": price must be a positive finite number");
            }
        }
        validate_method_specs(&self.methods)?;
        if self.search.beam_width == Some(0) {
            bail!("[search] beam_width must be greater than 0");
        }
        if self.search.max_steps == Some(0) {
            bail!("[search] max_steps must be greater than 0");
        }
        if self.search.top == Some(0) {
            bail!("[search] top must be greater than 0");
        }
        if self
            .search
            .cost_weight
            .is_some_and(|weight| weight < 0.0 || !weight.is_finite())
        {
            bail!("[search] cost_weight must be a non-negative finite number");
        }
        if self
            .search
            .restart_cost
            .is_some_and(|cost| cost < 0.0 || !cost.is_finite())
        {
            bail!("[search] restart_cost must be a non-negative finite number");
        }
        Ok(())
    }

    /// Validate goal selectors against the loaded RePoE export so typos fail
    /// before an expensive search silently chases an impossible target.
    pub fn validate_against_db(&self, db: &GameData) -> Result<()> {
        GoalEvaluator::new(&self.wants).validate_against_db(db)
    }

    /// Score an item state: sum of weights over satisfied wants.
    /// Called once per candidate node in the beam search — kept allocation-free.
    pub fn score(&self, state: &ItemState, db: &GameData) -> f64 {
        GoalEvaluator::new(&self.wants).score(state, db)
    }

    /// Maximum raw goal score when every requested condition is satisfied.
    pub fn max_score(&self) -> f64 {
        GoalEvaluator::new(&self.wants).max_score()
    }

    /// Request-only maximum score, or `None` when numeric goals require a
    /// data/item-aware ceiling.
    pub fn maximum_score(&self) -> Option<f64> {
        GoalEvaluator::new(&self.wants).maximum_score()
    }

    /// Number of requested conditions satisfied by `state`.
    pub fn satisfied_count(&self, state: &ItemState, db: &GameData) -> usize {
        GoalEvaluator::new(&self.wants).satisfied_count(state, db)
    }

    pub fn required_goal_count(&self) -> usize {
        GoalEvaluator::new(&self.wants).required_goal_count()
    }

    pub fn satisfied_required_count(&self, state: &ItemState, db: &GameData) -> usize {
        GoalEvaluator::new(&self.wants).satisfied_required_count(state, db)
    }

    /// True only when every required condition is present on the same item.
    pub fn is_complete(&self, state: &ItemState, db: &GameData) -> bool {
        GoalEvaluator::new(&self.wants).is_complete(state, db)
    }

    /// Structured satisfaction report for application adapters.
    pub fn report_entries(&self, state: &ItemState, db: &GameData) -> Vec<GoalReportEntry> {
        GoalEvaluator::new(&self.wants).report_entries(state, db)
    }

    /// Backward-compatible human-readable `(description, satisfied)` pairs.
    pub fn report(&self, state: &ItemState, db: &GameData) -> Vec<(String, bool)> {
        GoalEvaluator::new(&self.wants).report(state, db)
    }
}

/// Every mod a want can be satisfied by: affix-slot mods (prefixes, suffixes,
/// fractured, crafted) plus eldritch implicits, generic implicits, and
/// enchantments — implicits and enchants don't occupy affix slots or
/// participate in explicit group conflicts, but absolutely count toward goals.
fn scorable_mods(state: &ItemState) -> impl Iterator<Item = &Modifier> {
    state
        .all_mods_for_conflict()
        .chain(state.exarch_implicit.iter())
        .chain(state.eater_implicit.iter())
        .chain(state.implicits.iter())
        .chain(state.enchants.iter())
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WantAssessment {
    satisfied: bool,
    attained: Option<i64>,
    contribution: f64,
}

fn assess_want(want: &WantSpec, state: &ItemState, db: &GameData) -> WantAssessment {
    match want.scoring_mode() {
        GoalScoringMode::Presence => {
            let satisfied =
                scorable_mods(state).any(|modifier| modifier_matches_selectors(want, modifier, db));
            WantAssessment {
                satisfied,
                attained: None,
                contribution: if satisfied { want.weight } else { 0.0 },
            }
        }
        GoalScoringMode::Threshold => {
            let values = scorable_mods(state)
                .filter_map(|modifier| selected_numeric_value(want, modifier, db));
            if let Some(minimum) = want.min_value {
                let attained = values.max();
                let satisfied = attained.is_some_and(|value| value >= i64::from(minimum));
                WantAssessment {
                    satisfied,
                    attained,
                    contribution: if satisfied { want.weight } else { 0.0 },
                }
            } else {
                let attained = values.min();
                let maximum = want
                    .max_value
                    .expect("validated threshold goals have exactly one bound");
                let satisfied = attained.is_some_and(|value| value <= i64::from(maximum));
                WantAssessment {
                    satisfied,
                    attained,
                    contribution: if satisfied { want.weight } else { 0.0 },
                }
            }
        }
        GoalScoringMode::PerUnit => {
            let mut matched = false;
            let attained = scorable_mods(state)
                .filter_map(|modifier| selected_numeric_value(want, modifier, db))
                .fold(0_i64, |total, value| {
                    matched = true;
                    total.saturating_add(value)
                });
            let satisfied = matched
                && want
                    .min_value
                    .is_none_or(|minimum| attained >= i64::from(minimum))
                && want
                    .max_value
                    .is_none_or(|maximum| attained <= i64::from(maximum));
            let scaled_units = if !matched {
                0
            } else if want.max_value.is_some() {
                i64::from(
                    want.cap
                        .expect("validated lower-is-better per_unit goals have a cap"),
                )
                .saturating_sub(attained)
                .max(0)
            } else {
                want.cap
                    .map_or(attained, |cap| attained.min(i64::from(cap)))
                    .max(0)
            };
            WantAssessment {
                satisfied,
                attained: matched.then_some(attained),
                contribution: want.weight * scaled_units as f64,
            }
        }
    }
}

/// True if `modifier` satisfies every non-numeric selector `want` specifies.
fn modifier_matches_selectors(want: &WantSpec, modifier: &Modifier, db: &GameData) -> bool {
    if let Some(id) = &want.mod_id {
        if &modifier.mod_id != id {
            return false;
        }
    }
    if let Some(group) = &want.group {
        let in_group = db
            .mods
            .get(&modifier.mod_id)
            .is_some_and(|m| m.groups.iter().any(|g| g == group));
        if !in_group {
            return false;
        }
    }
    if let Some(stat) = &want.stat {
        let has_stat = modifier.rolls.iter().any(|roll| &roll.stat_id == stat);
        if !has_stat {
            return false;
        }
    }
    true
}

fn selected_numeric_value(want: &WantSpec, modifier: &Modifier, db: &GameData) -> Option<i64> {
    if !modifier_matches_selectors(want, modifier, db) {
        return None;
    }
    let stat_id = want.stat.as_deref().or_else(|| {
        db.mods
            .get(&modifier.mod_id)
            .and_then(|candidate| candidate.stats.first())
            .map(|stat| stat.id.as_str())
    })?;
    modifier
        .rolls
        .iter()
        .find(|roll| roll.stat_id == stat_id)
        .map(|roll| i64::from(roll.value))
}

fn matching_db_mods<'a>(
    want: &'a WantSpec,
    db: &'a GameData,
) -> impl Iterator<Item = &'a Mod> + 'a {
    matching_db_mod_entries(want, db).map(|(_, candidate)| candidate)
}

fn matching_db_mod_entries<'a>(
    want: &'a WantSpec,
    db: &'a GameData,
) -> impl Iterator<Item = (&'a String, &'a Mod)> + 'a {
    db.mods
        .iter()
        .filter(move |(mod_id, candidate)| db_mod_matches_selectors(want, mod_id, candidate))
}

fn db_mod_matches_selectors(want: &WantSpec, mod_id: &str, candidate: &Mod) -> bool {
    want.mod_id.as_ref().is_none_or(|wanted| wanted == mod_id)
        && want
            .group
            .as_ref()
            .is_none_or(|group| candidate.groups.iter().any(|entry| entry == group))
        && want
            .stat
            .as_ref()
            .is_none_or(|stat| candidate.stats.iter().any(|entry| &entry.id == stat))
}

fn selected_db_stat_id<'a>(want: &'a WantSpec, candidate: &'a Mod) -> Option<&'a str> {
    match &want.stat {
        Some(stat) => candidate
            .stats
            .iter()
            .find(|entry| &entry.id == stat)
            .map(|entry| entry.id.as_str()),
        None => candidate.stats.first().map(|entry| entry.id.as_str()),
    }
}

fn describe_want(w: &WantSpec) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(id) = &w.mod_id {
        parts.push(format!("mod {id}"));
    }
    if let Some(g) = &w.group {
        parts.push(format!("group {g}"));
    }
    if let Some(s) = &w.stat {
        match (w.min_value, w.max_value) {
            (Some(value), None) => parts.push(format!("stat {s} >= {value}")),
            (None, Some(value)) => parts.push(format!("stat {s} <= {value}")),
            _ => parts.push(format!("stat {s}")),
        }
    } else {
        if let Some(value) = w.min_value {
            parts.push(format!("first stat >= {value}"));
        }
        if let Some(value) = w.max_value {
            parts.push(format!("first stat <= {value}"));
        }
    }
    if let Some(cap) = w.cap {
        parts.push(format!("cap {cap}"));
    }
    format!("{} (weight {})", parts.join(" + "), w.weight)
}

// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::data::crafting_catalogs::{parse_essences, parse_fossils};
    use crate::data::mods::{Domain, GenerationType, Mod, ModStat, SpawnWeight};
    use crate::item::modifier::StatRoll;
    use crate::item::state::Rarity;

    fn life_mod() -> Mod {
        Mod {
            name: "Prodigious".to_string(),
            generation_type: GenerationType::Prefix,
            required_level: 30,
            stats: vec![ModStat {
                id: "base_maximum_life".to_string(),
                min: 60,
                max: 79,
            }],
            spawn_weights: vec![SpawnWeight {
                tag: "default".to_string(),
                weight: 1000,
            }],
            generation_weights: vec![],
            adds_tags: vec![],
            tags: vec!["life".to_string()],
            domain: Domain::Item,
            mod_type: "IncreasedLife".to_string(),
            groups: vec!["IncreasedLife".to_string()],
            is_essence_only: false,
            text: None,
        }
    }

    fn db_with_life() -> GameData {
        let mut mods = HashMap::new();
        mods.insert("IncreasedLife5".to_string(), life_mod());
        GameData::new(mods, HashMap::new())
    }

    #[test]
    fn parses_methods_section_and_builds() {
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"

            [[wants]]
            group = "IncreasedLife"

            [[methods]]
            type = "essence"
            mod_id = "IncreasedLife5"
            name = "Essence of Greed"
            cost = 5.0

            [[methods]]
            type = "harvest"
            op = "augment"
            target = "life"
            cost = 30.0

            [[methods]]
            type = "eldritch_chaos"
            god = "exarch"

            [search]
            seed = 42
            "#,
        )
        .unwrap();
        assert_eq!(spec.methods.len(), 3);
        assert_eq!(spec.search.seed, Some(42));

        let db = db_with_life();
        let built: Vec<_> = spec.methods.iter().map(|m| m.build(&db).unwrap()).collect();
        assert_eq!(built[0].name(), "Essence of Greed");
        assert_eq!(built[1].name(), "Harvest augment life");
        assert_eq!(built[2].name(), "Eldritch Chaos Orb (Exarch)");
    }

    #[test]
    fn named_catalog_methods_resolve_for_the_selected_item_class() {
        let mut mods = HashMap::new();
        mods.insert("ChestEssenceLife".to_string(), life_mod());
        let essences = parse_essences(
            r#"{
                "EssenceGreed7": {
                    "name": "Deafening Essence of Greed",
                    "level": 7,
                    "item_level_restriction": null,
                    "type": {"tier": 1, "is_corruption_only": false},
                    "mods": {"Body Armour": "ChestEssenceLife"}
                }
            }"#,
        )
        .unwrap();
        let fossils = parse_fossils(
            r#"{
                "FossilLife": {
                    "name": "Pristine Fossil",
                    "positive_mod_weights": [{"tag": "life", "weight": 1000}],
                    "negative_mod_weights": [{"tag": "defences", "weight": 0}]
                }
            }"#,
        )
        .unwrap();
        let db = GameData::new(mods, HashMap::new()).with_crafting_catalogs(
            None,
            Some(essences),
            Some(fossils),
        );
        let base = BaseItem {
            name: "Test Plate".to_string(),
            item_class: "Body Armour".to_string(),
            tags: vec!["body_armour".to_string()],
            implicits: vec![],
            drop_level: 1,
            inventory_height: 3,
            inventory_width: 2,
        };
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Test Plate"
            item_level = 86
            [[wants]]
            group = "IncreasedLife"
            [[methods]]
            type = "essence"
            essence = "Deafening Essence of Greed"
            cost = 5.0
            [[methods]]
            type = "fossil"
            fossil = "Pristine Fossil"
            cost = 10.0
            [[methods]]
            type = "bestiary_swap"
            add = "prefix"
            beast_level = 83
            cost = 12.0
            "#,
        )
        .unwrap();

        let built = spec
            .methods
            .iter()
            .map(|method| method.build_for_item(&db, &base, 86).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(built[0].name(), "Deafening Essence of Greed");
        assert_eq!(built[1].name(), "Pristine Fossil");
        assert_eq!(built[2].name(), "Add a Prefix, Remove a Random Suffix");
    }

    #[test]
    fn method_build_rejects_unknown_mod_id() {
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            [[methods]]
            type = "essence"
            mod_id = "NoSuchMod"
            cost = 5.0
            "#,
        )
        .unwrap();
        let err = spec.methods[0]
            .build(&db_with_life())
            .err()
            .expect("build must fail");
        assert!(err.to_string().contains("NoSuchMod"), "got: {err}");
    }

    #[test]
    fn method_build_rejects_bench_on_non_crafted_mod() {
        // IncreasedLife5 has domain=item, not crafted — bench must refuse it.
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            [[methods]]
            type = "bench"
            mod_id = "IncreasedLife5"
            "#,
        )
        .unwrap();
        let err = spec.methods[0]
            .build(&db_with_life())
            .err()
            .expect("build must fail");
        assert!(err.to_string().contains("expected crafted"), "got: {err}");
    }

    #[test]
    fn method_build_rejects_bad_harvest_op() {
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            [[methods]]
            type = "harvest"
            op = "explode"
            target = "life"
            cost = 30.0
            "#,
        )
        .unwrap();
        let err = spec.methods[0]
            .build(&db_with_life())
            .err()
            .expect("build must fail");
        assert!(err.to_string().contains("unknown op"), "got: {err}");
    }

    #[test]
    fn build_state_places_starting_mods() {
        let mut mods = HashMap::new();
        mods.insert("IncreasedLife5".to_string(), life_mod());
        let mut fire = life_mod();
        fire.generation_type = GenerationType::Suffix;
        fire.groups = vec!["FireResistance".to_string()];
        fire.stats = vec![ModStat {
            id: "base_fire_damage_resistance_%".to_string(),
            min: 30,
            max: 35,
        }];
        mods.insert("FireResist5".to_string(), fire);
        let mut crafted = life_mod();
        crafted.domain = Domain::Crafted;
        crafted.groups = vec!["CraftedMana".to_string()];
        crafted.generation_type = GenerationType::Suffix;
        mods.insert("CraftedMana1".to_string(), crafted);
        let db = GameData::new(mods, HashMap::new());

        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            item_level = 86
            rarity = "rare"

            [[item.mods]]
            mod_id = "IncreasedLife5"
            values = [75]
            fractured = true

            [[item.mods]]
            mod_id = "FireResist5"

            [[item.mods]]
            mod_id = "CraftedMana1"
            crafted = true

            [[wants]]
            group = "IncreasedLife"
            "#,
        )
        .unwrap();
        let item = spec
            .item
            .build_state("chest".to_string(), vec![], &db)
            .unwrap();

        assert_eq!(item.rarity, Rarity::Rare);
        assert_eq!(item.fractured.len(), 1);
        assert_eq!(
            item.fractured[0].rolls[0].value, 75,
            "explicit value must be kept"
        );
        assert_eq!(item.suffixes.len(), 1);
        // 30..=35 midpoint = 32 when values are omitted.
        assert_eq!(item.suffixes[0].rolls[0].value, 32);
        assert!(item.crafted_mod.is_some());
        // The fractured life mod satisfies the want immediately.
        assert_eq!(spec.score(&item, &db), 1.0);
    }

    #[test]
    fn build_state_rejects_bad_starting_items() {
        let db = db_with_life();
        // Value outside the mod's roll range.
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "x"
            rarity = "rare"
            [[item.mods]]
            mod_id = "IncreasedLife5"
            values = [999]
            [[wants]]
            group = "IncreasedLife"
            "#,
        )
        .unwrap();
        let err = spec
            .item
            .build_state("chest".to_string(), vec![], &db)
            .unwrap_err();
        assert!(err.to_string().contains("outside"), "got: {err}");

        // Mods on a Normal-rarity item have no slots.
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "x"
            [[item.mods]]
            mod_id = "IncreasedLife5"
            [[wants]]
            group = "IncreasedLife"
            "#,
        )
        .unwrap();
        let err = spec
            .item
            .build_state("chest".to_string(), vec![], &db)
            .unwrap_err();
        assert!(
            err.to_string().contains("no open prefix slot"),
            "got: {err}"
        );

        // crafted = true on a non-crafted-domain mod.
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "x"
            rarity = "rare"
            [[item.mods]]
            mod_id = "IncreasedLife5"
            crafted = true
            [[wants]]
            group = "IncreasedLife"
            "#,
        )
        .unwrap();
        let err = spec
            .item
            .build_state("chest".to_string(), vec![], &db)
            .unwrap_err();
        assert!(err.to_string().contains("domain"), "got: {err}");
    }

    #[test]
    fn wants_match_eldritch_implicits() {
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            weight = 10.0
            "#,
        )
        .unwrap();
        let db = db_with_life();
        let mut item = ItemState::new_base("chest", vec![], 86);
        item.rarity = Rarity::Rare;
        item.exarch_implicit = Some(Modifier {
            mod_id: "IncreasedLife5".to_string(),
            generation_type: GenerationType::ExarchImplicit,
            rolls: vec![],
        });
        assert_eq!(
            spec.score(&item, &db),
            10.0,
            "eldritch implicit mods must satisfy wants"
        );
    }

    #[test]
    fn parses_prices_and_rejects_nonpositive() {
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            [prices]
            "Divine Orb" = 220.0
            "#,
        )
        .unwrap();
        assert_eq!(spec.prices.get("Divine Orb"), Some(&220.0));

        let err = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            [prices]
            "Chaos Orb" = -1.0
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("positive"), "got: {err}");
    }

    #[test]
    fn parses_multi_fossil_resonator() {
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            [[methods]]
            type = "fossil"
            name = "Pristine + Dense"
            cost = 25.0
            boosted_tags = ["life"]
            [[methods.fossils]]
            boosted_tags = ["defences"]
            "#,
        )
        .unwrap();
        match &spec.methods[0] {
            MethodSpec::Fossil {
                fossils,
                boosted_tags,
                ..
            } => {
                assert_eq!(boosted_tags, &["life".to_string()]);
                assert_eq!(fossils.len(), 1, "one [[methods.fossils]] sub-table");
                assert_eq!(fossils[0].boosted_tags, vec!["defences".to_string()]);
            }
            other => panic!("expected fossil method, got {other:?}"),
        }
        // Builds into a two-fossil resonator without error.
        spec.methods[0].build(&db_with_life()).unwrap();
    }

    fn item_with_life(value: i32) -> ItemState {
        let mut item = ItemState::new_base("chest", vec!["body_armour".to_string()], 86);
        item.rarity = Rarity::Rare;
        item.prefixes.push(Modifier {
            mod_id: "IncreasedLife5".to_string(),
            generation_type: GenerationType::Prefix,
            rolls: vec![StatRoll {
                stat_id: "base_maximum_life".to_string(),
                value,
            }],
        });
        item
    }

    const VALID_TOML: &str = r#"
        [item]
        base = "Astral Plate"
        item_level = 86

        [[wants]]
        group = "IncreasedLife"
        weight = 10.0

        [[wants]]
        stat = "base_maximum_life"
        min_value = 70
        weight = 5.0

        [search]
        beam_width = 20
        max_steps = 8
        cost_weight = 0.05
    "#;

    #[test]
    fn parses_valid_toml() {
        let spec = GoalSpec::from_toml_str(VALID_TOML).unwrap();
        assert_eq!(spec.item.base, "Astral Plate");
        assert_eq!(spec.item.item_level, 86);
        assert_eq!(spec.wants.len(), 2);
        assert!(
            spec.wants.iter().all(|want| want.required),
            "omitted required flags must preserve legacy all-required behavior"
        );
        assert_eq!(spec.search.beam_width, Some(20));
        assert_eq!(spec.search.cost_weight, Some(0.05));
    }

    #[test]
    fn preferred_wants_score_without_blocking_required_completion() {
        let text = VALID_TOML.replacen(
            "min_value = 70",
            "min_value = 70\n        required = false",
            1,
        );
        let spec = GoalSpec::from_toml_str(&text).unwrap();
        let db = db_with_life();
        let state = item_with_life(65);
        let evaluation = GoalEvaluator::new(&spec.wants).evaluate(&state, &db);

        assert!(spec.wants[0].required);
        assert!(!spec.wants[1].required);
        assert_eq!(evaluation.score, 10.0);
        assert_eq!(evaluation.satisfied_count, 1);
        assert_eq!(evaluation.required_goal_count, 1);
        assert_eq!(evaluation.satisfied_required_count, 1);
        assert!(evaluation.complete());

        let report = spec.report_entries(&state, &db);
        assert!(report[0].required);
        assert!(report[0].satisfied);
        assert!(!report[1].required);
        assert!(!report[1].satisfied);
    }

    #[test]
    fn preference_weight_cannot_mask_an_unsatisfied_required_want() {
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"

            [[wants]]
            group = "IncreasedLife"
            weight = 100.0
            required = false

            [[wants]]
            stat = "base_maximum_life"
            min_value = 70
            weight = 1.0
            "#,
        )
        .unwrap();
        let db = db_with_life();
        let evaluation = GoalEvaluator::new(&spec.wants).evaluate(&item_with_life(65), &db);

        assert_eq!(evaluation.score, 100.0);
        assert_eq!(spec.max_score(), 101.0);
        assert_eq!(evaluation.satisfied_required_count, 0);
        assert_eq!(evaluation.required_goal_count, 1);
        assert!(!evaluation.complete());
    }

    #[test]
    fn all_preferred_goal_set_is_complete_but_keeps_its_score_target() {
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"

            [[wants]]
            group = "IncreasedLife"
            weight = 10.0
            required = false
            "#,
        )
        .unwrap();
        let db = db_with_life();
        let empty = ItemState::new_base("chest", vec![], 86);
        let evaluation = GoalEvaluator::new(&spec.wants).evaluate(&empty, &db);

        assert_eq!(evaluation.score, 0.0);
        assert_eq!(spec.max_score(), 10.0);
        assert_eq!(evaluation.required_goal_count, 0);
        assert!(evaluation.complete());
    }

    #[test]
    fn goal_evaluator_exposes_the_goal_spec_evaluation_contract() {
        let spec = GoalSpec::from_toml_str(VALID_TOML).unwrap();
        let db = db_with_life();
        let state = item_with_life(75);
        let evaluator = GoalEvaluator::new(&spec.wants);

        evaluator.validate().unwrap();
        evaluator.validate_against_db(&db).unwrap();
        assert_eq!(evaluator.score(&state, &db), spec.score(&state, &db));
        assert_eq!(evaluator.max_score(), spec.max_score());
        assert_eq!(
            evaluator.satisfied_count(&state, &db),
            spec.satisfied_count(&state, &db)
        );
        assert_eq!(
            evaluator.is_complete(&state, &db),
            spec.is_complete(&state, &db)
        );
        assert_eq!(evaluator.report(&state, &db), spec.report(&state, &db));

        let no_wants: [WantSpec; 0] = [];
        assert_eq!(
            GoalEvaluator::new(&no_wants)
                .validate()
                .unwrap_err()
                .to_string(),
            "Goal must contain at least one [[wants]] entry"
        );
        let empty_db = GameData::new(HashMap::new(), HashMap::new());
        assert_eq!(
            evaluator
                .validate_against_db(&empty_db)
                .unwrap_err()
                .to_string(),
            "[[wants]] entry 0: group 'IncreasedLife' not found in mods.json"
        );
    }

    #[test]
    fn item_level_defaults_when_omitted() {
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            "#,
        )
        .unwrap();
        assert_eq!(spec.item.item_level, 84);
        assert_eq!(spec.wants[0].weight, 1.0, "weight must default to 1.0");
    }

    #[test]
    fn rejects_want_with_no_criteria() {
        let err = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            weight = 5.0
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("at least one of"), "got: {err}");
    }

    #[test]
    fn numeric_group_selector_uses_the_first_declared_stat() {
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            min_value = 70
            "#,
        )
        .unwrap();
        let db = db_with_life();
        spec.validate_against_db(&db).unwrap();

        let report = spec.report_entries(&item_with_life(75), &db);
        assert!(report[0].satisfied);
        assert_eq!(report[0].attained, Some(75));
        assert_eq!(report[0].scoring_mode, GoalScoringMode::Threshold);
    }

    #[test]
    fn implicit_first_stat_on_multi_stat_mod_emits_typed_warning() {
        let mut multi_stat = life_mod();
        multi_stat.stats.push(ModStat {
            id: "second_stat".to_string(),
            min: 1,
            max: 2,
        });
        let db = GameData::new(
            HashMap::from([("MultiStatLife".to_string(), multi_stat)]),
            HashMap::new(),
        );
        let implicit = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            min_value = 60
            "#,
        )
        .unwrap();

        let warnings = GoalEvaluator::new(&implicit.wants).selector_warnings(&db);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, GoalSelectorWarningCode::ImplicitFirstStat);
        assert_eq!(warnings[0].field_path, "/goals/0/stat");
        assert_eq!(warnings[0].matching_mod_ids, ["MultiStatLife"]);

        let explicit = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            stat = "base_maximum_life"
            min_value = 60
            "#,
        )
        .unwrap();
        assert!(
            GoalEvaluator::new(&explicit.wants)
                .selector_warnings(&db)
                .is_empty(),
            "an explicit stat selector removes declaration-order ambiguity"
        );
    }

    #[test]
    fn per_unit_sums_every_matching_modifier_and_applies_cap() {
        let mut second = life_mod();
        second.name = "Healthy".to_string();
        second.generation_type = GenerationType::Suffix;
        second.groups = vec!["SecondLifeSource".to_string()];
        let mut mods = HashMap::new();
        mods.insert("LifePrefix".to_string(), life_mod());
        mods.insert("LifeSuffix".to_string(), second);
        let db = GameData::new(mods, HashMap::new());

        let mut item = ItemState::new_base("chest", vec!["body_armour".to_string()], 86);
        item.rarity = Rarity::Rare;
        item.prefixes.push(Modifier {
            mod_id: "LifePrefix".to_string(),
            generation_type: GenerationType::Prefix,
            rolls: vec![StatRoll {
                stat_id: "base_maximum_life".to_string(),
                value: 30,
            }],
        });
        item.suffixes.push(Modifier {
            mod_id: "LifeSuffix".to_string(),
            generation_type: GenerationType::Suffix,
            rolls: vec![StatRoll {
                stat_id: "base_maximum_life".to_string(),
                value: 20,
            }],
        });

        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"

            [[wants]]
            stat = "base_maximum_life"
            mode = "per_unit"
            min_value = 40
            cap = 45
            weight = 2.0
            "#,
        )
        .unwrap();
        spec.validate_against_db(&db).unwrap();

        let evaluation = GoalEvaluator::new(&spec.wants).evaluate(&item, &db);
        let report = spec.report_entries(&item, &db);
        assert!(evaluation.complete());
        assert_eq!(evaluation.score, 90.0);
        assert_eq!(spec.maximum_score(), Some(90.0));
        assert_eq!(report[0].attained, Some(50));
        assert_eq!(report[0].contribution, 90.0);
    }

    #[test]
    fn uncapped_preferred_per_unit_defaults_higher_and_has_no_known_maximum() {
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"

            [[wants]]
            group = "IncreasedLife"
            mode = "per_unit"
            required = false
            weight = 0.5
            "#,
        )
        .unwrap();
        let db = db_with_life();
        spec.validate_against_db(&db).unwrap();

        let evaluation = GoalEvaluator::new(&spec.wants).evaluate(&item_with_life(70), &db);
        assert!(evaluation.complete(), "all-preferred sets remain complete");
        assert_eq!(evaluation.score, 35.0);
        assert_eq!(spec.maximum_score(), None);
        assert!(spec.max_score().is_infinite());
    }

    #[test]
    fn lower_is_better_uses_aggregate_threshold_and_nonnegative_gap_score() {
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"

            [[wants]]
            stat = "base_maximum_life"
            mode = "per_unit"
            max_value = 70
            cap = 100
            weight = 1.5
            "#,
        )
        .unwrap();
        let db = db_with_life();
        spec.validate_against_db(&db).unwrap();

        let low = GoalEvaluator::new(&spec.wants).evaluate(&item_with_life(65), &db);
        let high = GoalEvaluator::new(&spec.wants).evaluate(&item_with_life(110), &db);
        assert!(low.complete());
        assert_eq!(low.score, 52.5);
        assert!(!high.complete());
        assert_eq!(high.score, 0.0);
        assert_eq!(spec.maximum_score(), None);
    }

    #[test]
    fn higher_per_unit_never_contributes_a_negative_score() {
        let mut negative = life_mod();
        negative.stats[0].min = -20;
        negative.stats[0].max = -1;
        let db = GameData::new(
            HashMap::from([("NegativeLife".to_string(), negative)]),
            HashMap::new(),
        );
        let mut item = ItemState::new_base("chest", vec![], 86);
        item.rarity = Rarity::Rare;
        item.prefixes.push(Modifier {
            mod_id: "NegativeLife".to_string(),
            generation_type: GenerationType::Prefix,
            rolls: vec![StatRoll {
                stat_id: "base_maximum_life".to_string(),
                value: -10,
            }],
        });
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"

            [[wants]]
            stat = "base_maximum_life"
            mode = "per_unit"
            required = false
            "#,
        )
        .unwrap();

        assert_eq!(spec.score(&item, &db), 0.0);
        assert_eq!(spec.report_entries(&item, &db)[0].attained, Some(-10));
    }

    #[test]
    fn scoring_mode_field_matrix_is_strict() {
        let invalid_wants = [
            (
                "mode = \"presence\"\nmin_value = 1",
                "presence mode does not accept",
            ),
            (
                "mode = \"presence\"\ncap = 10",
                "presence mode does not accept cap",
            ),
            (
                "mode = \"threshold\"",
                "threshold mode requires exactly one",
            ),
            (
                "mode = \"threshold\"\nmin_value = 1\ncap = 10",
                "threshold mode does not accept cap",
            ),
            (
                "mode = \"per_unit\"",
                "required per_unit goal needs min_value or max_value",
            ),
            (
                "mode = \"per_unit\"\nmax_value = 10",
                "lower-is-better per_unit scoring requires cap",
            ),
            (
                "min_value = 1\nmax_value = 2",
                "min_value and max_value are mutually exclusive",
            ),
        ];

        for (fields, expected) in invalid_wants {
            let text = format!(
                r#"
                [item]
                base = "Astral Plate"

                [[wants]]
                group = "IncreasedLife"
                {fields}
                "#
            );
            let error = GoalSpec::from_toml_str(&text).expect_err(fields);
            assert!(
                error.to_string().contains(expected),
                "{fields:?} produced {error:#}"
            );
        }
    }

    #[test]
    fn impossible_analysis_detects_zero_weight_without_deterministic_source() {
        let mut unreachable = life_mod();
        unreachable.spawn_weights[0].weight = 0;
        unreachable.is_essence_only = true;
        let db = GameData::new(
            HashMap::from([("EssenceOnlyLife".to_string(), unreachable)]),
            HashMap::new(),
        );
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            mod_id = "EssenceOnlyLife"
            "#,
        )
        .unwrap();
        let mut item = ItemState::new_base("chest", vec![], 86);
        item.rarity = Rarity::Rare;

        let reasons = analyze_impossible_goals(&spec.wants, &item, &db, &HashSet::new());
        assert_eq!(reasons.len(), 1);
        assert_eq!(
            reasons[0].code,
            ImpossibleGoalReasonCode::NoReachableModifier
        );
        assert_eq!(reasons[0].want_index, 0);

        assert!(
            analyze_impossible_goals(
                &spec.wants,
                &item,
                &db,
                &HashSet::from(["EssenceOnlyLife".to_string()])
            )
            .is_empty(),
            "an enabled deterministic provider prevents a false impossibility"
        );

        let per_unit = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            mod_id = "EssenceOnlyLife"
            mode = "per_unit"
            min_value = 1
            cap = 100
            "#,
        )
        .unwrap();
        let reasons = analyze_impossible_goals(&per_unit.wants, &item, &db, &HashSet::new());
        assert_eq!(reasons.len(), 1);
        assert_eq!(
            reasons[0].code,
            ImpossibleGoalReasonCode::NoReachableModifier
        );
    }

    #[test]
    fn impossible_analysis_skips_satisfied_and_non_affix_wants() {
        let mut enchantment = life_mod();
        enchantment.generation_type = GenerationType::Enchantment;
        enchantment.groups = vec!["EnchantLife".to_string()];
        let db = GameData::new(
            HashMap::from([
                ("LifePrefix".to_string(), life_mod()),
                ("LifeEnchant".to_string(), enchantment.clone()),
            ]),
            HashMap::new(),
        );
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            mod_id = "LifeEnchant"
            [[wants]]
            mod_id = "LifePrefix"
            "#,
        )
        .unwrap();
        let mut item = ItemState::new_base("chest", vec![], 86);
        item.rarity = Rarity::Rare;
        item.enchants.push(Modifier::from_min_rolls(
            "LifeEnchant",
            GenerationType::Enchantment,
            &enchantment.stats,
        ));

        assert!(
            analyze_impossible_goals(&spec.wants, &item, &db, &HashSet::new()).is_empty(),
            "an already-satisfied slot-free want must not consume an affix candidate"
        );

        item.enchants.clear();
        assert!(
            analyze_impossible_goals(&spec.wants, &item, &db, &HashSet::new()).is_empty(),
            "non-affix acquisition is left to search instead of producing a false proof"
        );
    }

    #[test]
    fn impossible_analysis_detects_group_conflicts_without_rejecting_reuse() {
        let mut first = life_mod();
        first.groups = vec!["SharedExclusiveGroup".to_string()];
        let mut second = life_mod();
        second.name = "Second".to_string();
        second.groups = vec!["SharedExclusiveGroup".to_string()];
        let db = GameData::new(
            HashMap::from([
                ("FirstLife".to_string(), first),
                ("SecondLife".to_string(), second),
            ]),
            HashMap::new(),
        );
        let mut item = ItemState::new_base("chest", vec![], 86);
        item.rarity = Rarity::Rare;
        let conflicting = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            mod_id = "FirstLife"
            [[wants]]
            mod_id = "SecondLife"
            "#,
        )
        .unwrap();
        let reasons = analyze_impossible_goals(&conflicting.wants, &item, &db, &HashSet::new());
        assert_eq!(reasons.len(), 2);
        assert!(reasons
            .iter()
            .all(|reason| reason.code == ImpossibleGoalReasonCode::ModifierGroupConflict));

        let reusable = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            mod_id = "FirstLife"
            [[wants]]
            group = "SharedExclusiveGroup"
            "#,
        )
        .unwrap();
        assert!(
            analyze_impossible_goals(&reusable.wants, &item, &db, &HashSet::new()).is_empty(),
            "one modifier may satisfy multiple required wants"
        );
    }

    #[test]
    fn impossible_analysis_detects_affix_slot_overflow() {
        let mut mods = HashMap::new();
        let mut toml = String::from("[item]\nbase = \"Astral Plate\"\n");
        for index in 0..4 {
            let mod_id = format!("Prefix{index}");
            let mut modifier = life_mod();
            modifier.name = mod_id.clone();
            modifier.groups = vec![format!("Group{index}")];
            mods.insert(mod_id.clone(), modifier);
            toml.push_str(&format!("[[wants]]\nmod_id = \"{mod_id}\"\n"));
        }
        let db = GameData::new(mods, HashMap::new());
        let spec = GoalSpec::from_toml_str(&toml).unwrap();
        let mut item = ItemState::new_base("chest", vec![], 86);
        item.rarity = Rarity::Rare;

        let reasons = analyze_impossible_goals(&spec.wants, &item, &db, &HashSet::new());
        assert_eq!(reasons.len(), 4);
        assert!(reasons
            .iter()
            .all(|reason| reason.code == ImpossibleGoalReasonCode::AffixCapacity));
    }

    #[test]
    fn impossible_analysis_marks_unsatisfied_uncraftable_items() {
        let db = db_with_life();
        let spec = GoalSpec::from_toml_str(VALID_TOML).unwrap();
        let mut item = ItemState::new_base("chest", vec![], 86);
        item.corrupted = true;

        let reasons = analyze_impossible_goals(&spec.wants, &item, &db, &HashSet::new());
        assert_eq!(reasons.len(), 2);
        assert!(reasons
            .iter()
            .all(|reason| reason.code == ImpossibleGoalReasonCode::ItemNotCraftable));
    }

    #[test]
    fn rejects_nonpositive_weight() {
        let err = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            weight = 0.0
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("positive"), "got: {err}");
    }

    #[test]
    fn rejects_goal_scores_that_can_overflow_finite_representation() {
        let aggregate = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            mod_id = "First"
            weight = 1.0e308
            [[wants]]
            mod_id = "Second"
            weight = 1.0e308
            "#,
        )
        .unwrap_err();
        assert!(
            aggregate
                .to_string()
                .contains("overflow the finite aggregate score"),
            "got: {aggregate:#}"
        );

        let per_unit = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            stat = "base_maximum_life"
            mode = "per_unit"
            required = false
            weight = 1.0e308
            "#,
        )
        .unwrap_err();
        assert!(
            per_unit
                .to_string()
                .contains("overflow the finite score representation"),
            "got: {per_unit:#}"
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        let err = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            typo_field = 3
            [[wants]]
            group = "IncreasedLife"
            "#,
        )
        .unwrap_err();
        // Typos in field names must fail loudly, not be silently ignored.
        assert!(err.to_string().contains("Failed to parse"), "got: {err}");

        let err = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"

            [[wants]]
            group = "IncreasedLife"

            [[methods]]
            type = "harvest"
            op = "reforge"
            target = "life"
            cost = 1.0
            typo_field = 3
            "#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("Failed to parse"),
            "method-field typo should fail loudly: {err}"
        );
    }

    #[test]
    fn score_counts_satisfied_wants_once() {
        let spec = GoalSpec::from_toml_str(VALID_TOML).unwrap();
        let db = db_with_life();

        // Roll 75: satisfies both the group want (10) and the min_value 70 want (5).
        assert_eq!(spec.score(&item_with_life(75), &db), 15.0);

        // Roll 65: group want satisfied, min_value 70 not reached.
        assert_eq!(spec.score(&item_with_life(65), &db), 10.0);

        // Empty item: nothing satisfied.
        let empty = ItemState::new_base("chest", vec![], 86);
        assert_eq!(spec.score(&empty, &db), 0.0);
    }

    #[test]
    fn score_sees_fractured_and_crafted_mods() {
        let spec = GoalSpec::from_toml_str(VALID_TOML).unwrap();
        let db = db_with_life();

        // Move the life mod into the fractured list — must still count.
        let mut item = item_with_life(75);
        let m = item.prefixes.pop().unwrap();
        item.fractured.push(m);
        assert_eq!(
            spec.score(&item, &db),
            15.0,
            "fractured mods must satisfy wants"
        );
    }

    #[test]
    fn all_criteria_must_hold_on_same_mod() {
        // Want requires group IncreasedLife AND stat base_maximum_energy_shield —
        // no single mod has both, so the want must not be satisfied.
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            stat = "base_maximum_energy_shield"
            "#,
        )
        .unwrap();
        let db = db_with_life();
        assert_eq!(spec.score(&item_with_life(75), &db), 0.0);
    }

    // ─── build_imported_state ────────────────────────────────────────────────

    use crate::import::{ImportedItem, ImportedModifier};

    /// A mod with an explicit generation type, group, domain, and default
    /// spawn weight; one stat `stat_<group>` rolling 1-10.
    fn imported_db_mod(gen: GenerationType, group: &str, domain: Domain, weight: u32) -> Mod {
        Mod {
            name: format!("Test {group}"),
            generation_type: gen,
            required_level: 1,
            stats: vec![ModStat {
                id: format!("stat_{group}"),
                min: 1,
                max: 10,
            }],
            spawn_weights: vec![SpawnWeight {
                tag: "default".to_string(),
                weight,
            }],
            generation_weights: vec![],
            adds_tags: vec![],
            tags: vec![],
            domain,
            mod_type: group.to_string(),
            groups: vec![group.to_string()],
            is_essence_only: false,
            text: None,
        }
    }

    /// A DB shaped like the acceptance fixture: a zero-spawn Delve fractured
    /// prefix, two normal ES prefixes, three suffixes, two enchants, and two
    /// generic implicits.
    fn fixture_db() -> GameData {
        let mut mods = HashMap::new();
        mods.insert(
            "MaximumMinionCountSpectreDelve".to_string(),
            imported_db_mod(GenerationType::Prefix, "MaximumSpectres", Domain::Delve, 0),
        );
        mods.insert(
            "FlatES".to_string(),
            imported_db_mod(
                GenerationType::Prefix,
                "BaseLocalDefences",
                Domain::Item,
                1000,
            ),
        );
        mods.insert(
            "PctES".to_string(),
            imported_db_mod(
                GenerationType::Prefix,
                "DefencesPercent",
                Domain::Item,
                1000,
            ),
        );
        mods.insert(
            "FlatLife".to_string(),
            imported_db_mod(GenerationType::Prefix, "BaseLife", Domain::Item, 1000),
        );
        mods.insert(
            "StrGems".to_string(),
            imported_db_mod(
                GenerationType::Suffix,
                "StrengthGemLevel",
                Domain::Item,
                500,
            ),
        );
        mods.insert(
            "IntGems".to_string(),
            imported_db_mod(
                GenerationType::Suffix,
                "IntelligenceGemLevel",
                Domain::Item,
                500,
            ),
        );
        mods.insert(
            "ESRegenNearby".to_string(),
            imported_db_mod(
                GenerationType::Suffix,
                "EnergyShieldRegenNearby",
                Domain::Item,
                200,
            ),
        );
        mods.insert(
            "EnchantDefences".to_string(),
            imported_db_mod(
                GenerationType::Enchantment,
                "EnchantDefences",
                Domain::Item,
                0,
            ),
        );
        mods.insert(
            "EnchantResists".to_string(),
            imported_db_mod(
                GenerationType::Enchantment,
                "EnchantResists",
                Domain::Item,
                0,
            ),
        );
        mods.insert(
            "PhysAsChaosImpl".to_string(),
            imported_db_mod(GenerationType::Corrupted, "PhysAsChaos", Domain::Item, 0),
        );
        mods.insert(
            "FlaskEffectImpl".to_string(),
            imported_db_mod(GenerationType::Unique, "FlaskEffect", Domain::Item, 0),
        );
        GameData::new(mods, HashMap::new())
    }

    fn imported_mod(mod_id: &str, values: Vec<i32>) -> ImportedModifier {
        ImportedModifier {
            mod_id: mod_id.to_string(),
            values,
            fractured: false,
            crafted: false,
            displayed_lines: vec![],
        }
    }

    fn fractured_mod(mod_id: &str, values: Vec<i32>) -> ImportedModifier {
        ImportedModifier {
            fractured: true,
            ..imported_mod(mod_id, values)
        }
    }

    /// The Damnation Wrap acceptance fixture as Sol's importer would emit it.
    fn fixture_import() -> ImportedItem {
        ImportedItem {
            item_name: Some("Damnation Wrap".to_string()),
            base_name: "Twilight Regalia".to_string(),
            rarity: Rarity::Rare,
            item_level: Some(86),
            quality: Some(30),
            sockets: Some("W-W-W-W-W-W".to_string()),
            displayed_energy_shield: Some(1200),
            explicit_mods: vec![
                fractured_mod("MaximumMinionCountSpectreDelve", vec![1]),
                imported_mod("StrGems", vec![1]),
                imported_mod("IntGems", vec![1]),
                imported_mod("FlatES", vec![5]),
                imported_mod("PctES", vec![7]),
                imported_mod("ESRegenNearby", vec![9]),
            ],
            implicit_mods: vec![
                imported_mod("PhysAsChaosImpl", vec![10]),
                imported_mod("FlaskEffectImpl", vec![10]),
            ],
            enchantments: vec![
                imported_mod("EnchantDefences", vec![10]),
                imported_mod("EnchantResists", vec![10]),
            ],
            corrupted: false,
            mirrored: false,
            warnings: vec![],
        }
    }

    fn build_fixture_state() -> ItemState {
        build_imported_state(
            &fixture_import(),
            "twilight_regalia".to_string(),
            vec!["body_armour".to_string()],
            &fixture_db(),
        )
        .expect("acceptance fixture must validate")
    }

    #[test]
    fn imported_fixture_builds_six_affix_state_with_zero_spawn_fracture() {
        let item = build_fixture_state();
        assert_eq!(item.rarity, Rarity::Rare);
        assert_eq!(item.item_level, 86);
        // Explicit layout: fractured Spectre prefix + 2 rolled prefixes, 3 suffixes.
        assert_eq!(item.fractured.len(), 1);
        assert_eq!(item.fractured[0].mod_id, "MaximumMinionCountSpectreDelve");
        assert_eq!(item.prefixes.len(), 2);
        assert_eq!(item.suffixes.len(), 3);
        assert_eq!(item.prefix_count(), 3);
        assert_eq!(item.suffix_count(), 3);
        assert!(item.is_full(), "six affixes must fill a rare item");
        // Craft-invariant metadata.
        assert_eq!(item.implicits.len(), 2);
        assert_eq!(item.enchants.len(), 2);
        assert_eq!(item.quality, 30);
        assert_eq!(item.sockets.as_deref(), Some("W-W-W-W-W-W"));
        assert_eq!(item.displayed_energy_shield, Some(1200));
        assert!(!item.corrupted && !item.mirrored);
        assert!(item.is_craftable());
        // Imported roll values survive verbatim.
        assert_eq!(item.prefixes[0].rolls[0].value, 5, "FlatES raw value");
        assert_eq!(item.prefixes[1].rolls[0].value, 7, "PctES raw value");
        assert_eq!(item.suffixes[2].rolls[0].value, 9, "ES regen raw value");
    }

    #[test]
    fn imported_zero_spawn_mod_stays_out_of_normal_roll_pools() {
        let db = fixture_db();
        let delve = &db.mods["MaximumMinionCountSpectreDelve"];
        assert!(
            !delve.is_craftable(),
            "Delve-domain mods must never enter the craftable index"
        );

        // Even on an empty rare item, no random pool may offer the Delve mod;
        // ordinary item-domain mods remain available.
        let empty = ItemState::new_base("twilight_regalia", vec!["body_armour".to_string()], 86);
        let mut rare = empty;
        rare.rarity = Rarity::Rare;
        let pool = crate::engine::mod_pool::eligible_mods(&rare, &[], &db);
        assert!(
            pool.iter()
                .all(|(id, _, _)| *id != "MaximumMinionCountSpectreDelve"),
            "zero-spawn Delve mod must be excluded from random rolling"
        );
        assert!(
            pool.iter().any(|(id, _, _)| *id == "FlatES"),
            "normal mods must still roll"
        );

        // The same exclusion holds on the imported state itself.
        let item = build_fixture_state();
        let pool = crate::engine::mod_pool::eligible_mods(&item, &[], &db);
        assert!(pool.is_empty(), "a full rare offers no roll targets at all");
    }

    #[test]
    fn imported_implicits_and_enchants_are_scorable_but_slot_free() {
        let db = fixture_db();
        let item = build_fixture_state();

        // Implicits and enchants must not occupy explicit capacity...
        assert_eq!(item.mod_count(), 6, "only explicit affixes count");
        // ...but goal scoring must see them.
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Twilight Regalia"
            [[wants]]
            group = "EnchantDefences"
            weight = 3.0
            [[wants]]
            group = "FlaskEffect"
            weight = 2.0
            [[wants]]
            group = "MaximumSpectres"
            weight = 1.0
            "#,
        )
        .unwrap();
        assert_eq!(spec.score(&item, &db), 6.0);
        assert!(spec.is_complete(&item, &db));
        assert!(spec
            .report(&item, &db)
            .iter()
            .all(|(_, satisfied)| *satisfied));
    }

    #[test]
    fn imported_enchant_groups_do_not_conflict_with_explicits() {
        // An enchantment sharing a group with an explicit prefix must not
        // trigger the explicit group-conflict check.
        let mut import = fixture_import();
        import.explicit_mods = vec![imported_mod("FlatES", vec![5])];
        import.enchantments = vec![imported_mod("EnchantShared", vec![10])];
        import.implicit_mods.clear();

        let mut db_mods = fixture_db().mods;
        db_mods.insert(
            "EnchantShared".to_string(),
            imported_db_mod(
                GenerationType::Enchantment,
                "BaseLocalDefences",
                Domain::Item,
                0,
            ),
        );
        let db = GameData::new(db_mods, HashMap::new());

        let item = build_imported_state(&import, "x".to_string(), vec![], &db)
            .expect("enchant group overlap with an explicit must be allowed");
        assert_eq!(item.prefixes.len(), 1);
        assert_eq!(item.enchants.len(), 1);
    }

    #[test]
    fn imported_state_rejects_invalid_layouts_and_rolls() {
        let db = fixture_db();
        let build = |mutate: fn(&mut ImportedItem)| {
            let mut import = fixture_import();
            mutate(&mut import);
            build_imported_state(&import, "x".to_string(), vec![], &db)
        };

        // Roll value outside the mod's range.
        let err = build(|i| i.explicit_mods[3].values = vec![999]).unwrap_err();
        assert!(err.to_string().contains("outside"), "got: {err}");

        // Wrong number of imported values.
        let err = build(|i| i.explicit_mods[3].values = vec![1, 2]).unwrap_err();
        assert!(err.to_string().contains("values"), "got: {err}");

        // Prefix capacity overflow (4th distinct-group prefix on a rare).
        let err = build(|i| {
            i.explicit_mods[1] = imported_mod("FlatLife", vec![5]);
        })
        .unwrap_err();
        assert!(
            err.to_string().contains("no open prefix slot"),
            "got: {err}"
        );

        // Group conflict between two explicit mods.
        let err = build(|i| {
            i.explicit_mods[1] = imported_mod("StrGems", vec![1]);
            i.explicit_mods[2] = imported_mod("StrGems", vec![2]);
        })
        .unwrap_err();
        assert!(err.to_string().contains("shares a mod group"), "got: {err}");

        // Fractured and crafted at once.
        let err = build(|i| {
            i.explicit_mods[0].crafted = true;
        })
        .unwrap_err();
        assert!(
            err.to_string().contains("both fractured and crafted"),
            "got: {err}"
        );

        // Crafted flag on a mod whose domain is not crafted.
        let err = build(|i| {
            i.explicit_mods[1].crafted = true;
        })
        .unwrap_err();
        assert!(err.to_string().contains("expected crafted"), "got: {err}");

        // An enchantment that is not an enchantment-type mod.
        let err = build(|i| {
            i.enchantments = vec![imported_mod("FlatES", vec![5])];
            i.explicit_mods.clear();
        })
        .unwrap_err();
        assert!(
            err.to_string().contains("expected enchantment"),
            "got: {err}"
        );

        // An implicit that is really an explicit affix.
        let err = build(|i| {
            i.implicit_mods = vec![imported_mod("PctES", vec![7])];
            i.explicit_mods.clear();
        })
        .unwrap_err();
        assert!(err.to_string().contains("explicit affix"), "got: {err}");

        // Unique rarity, missing item level, negative displayed ES, silly quality.
        let err = build(|i| i.rarity = Rarity::Unique).unwrap_err();
        assert!(err.to_string().contains("unique"), "got: {err}");
        let err = build(|i| i.item_level = None).unwrap_err();
        assert!(err.to_string().contains("item level"), "got: {err}");
        let err = build(|i| i.displayed_energy_shield = Some(-5)).unwrap_err();
        assert!(err.to_string().contains("negative"), "got: {err}");
        let err = build(|i| i.quality = Some(400)).unwrap_err();
        assert!(err.to_string().contains("quality"), "got: {err}");

        // Item level bounds stay strict.
        let err = build(|i| i.item_level = Some(0)).unwrap_err();
        assert!(err.to_string().contains("between 1 and 100"), "got: {err}");

        // required_level gating stays strict for explicits.
        let mut db_mods = fixture_db().mods;
        if let Some(m) = db_mods.get_mut("FlatES") {
            m.required_level = 60;
        }
        let gated_db = GameData::new(db_mods, HashMap::new());
        let mut import = fixture_import();
        import.item_level = Some(50);
        let err = build_imported_state(&import, "x".to_string(), vec![], &gated_db).unwrap_err();
        assert!(
            err.to_string().contains("requires item level"),
            "got: {err}"
        );

        // Two crafted mods.
        let mut db_mods = fixture_db().mods;
        let mut crafted = imported_db_mod(GenerationType::Suffix, "CraftA", Domain::Crafted, 0);
        crafted.required_level = 1;
        db_mods.insert("CraftA".to_string(), crafted);
        let crafted_b = imported_db_mod(GenerationType::Suffix, "CraftB", Domain::Crafted, 0);
        db_mods.insert("CraftB".to_string(), crafted_b);
        let db2 = GameData::new(db_mods, HashMap::new());
        let mut import = fixture_import();
        import.explicit_mods = vec![
            ImportedModifier {
                crafted: true,
                ..imported_mod("CraftA", vec![1])
            },
            ImportedModifier {
                crafted: true,
                ..imported_mod("CraftB", vec![1])
            },
        ];
        let err = build_imported_state(&import, "x".to_string(), vec![], &db2).unwrap_err();
        assert!(
            err.to_string().contains("only one crafted mod"),
            "got: {err}"
        );
    }

    #[test]
    fn imported_corrupted_and_mirrored_flags_block_crafting() {
        let db = fixture_db();
        let mut import = fixture_import();
        import.corrupted = true;
        let item = build_imported_state(&import, "x".to_string(), vec![], &db).unwrap();
        assert!(item.corrupted);
        assert!(!item.is_craftable(), "corrupted items cannot be crafted on");

        let mut import = fixture_import();
        import.mirrored = true;
        let item = build_imported_state(&import, "x".to_string(), vec![], &db).unwrap();
        assert!(item.mirrored);
        assert!(!item.is_craftable(), "mirrored items cannot be crafted on");
    }

    #[test]
    fn imported_eldritch_implicits_route_to_their_slots() {
        let mut db_mods = fixture_db().mods;
        db_mods.insert(
            "ExarchImpl".to_string(),
            imported_db_mod(
                GenerationType::ExarchImplicit,
                "ExarchGroup",
                Domain::Item,
                100,
            ),
        );
        let db = GameData::new(db_mods, HashMap::new());
        let mut import = fixture_import();
        import
            .implicit_mods
            .push(imported_mod("ExarchImpl", vec![4]));

        let item = build_imported_state(&import, "x".to_string(), vec![], &db).unwrap();
        assert_eq!(item.implicits.len(), 2, "generic implicits stay generic");
        assert_eq!(
            item.exarch_implicit.as_ref().map(|m| m.mod_id.as_str()),
            Some("ExarchImpl")
        );
    }

    #[test]
    fn toml_starting_items_still_require_positive_spawn_weight() {
        // The import relaxation must NOT leak into TOML validation: a
        // zero-spawn mod in [[item.mods]] keeps failing exactly as before.
        let db = fixture_db();
        let spec = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "x"
            item_level = 86
            rarity = "rare"
            [[item.mods]]
            mod_id = "MaximumMinionCountSpectreDelve"
            fractured = true
            [[wants]]
            group = "MaximumSpectres"
            "#,
        )
        .unwrap();
        let err = spec
            .item
            .build_state("x".to_string(), vec![], &db)
            .unwrap_err();
        assert!(err.to_string().contains("cannot appear"), "got: {err}");
    }

    #[test]
    fn report_flags_each_want() {
        let spec = GoalSpec::from_toml_str(VALID_TOML).unwrap();
        let db = db_with_life();
        let report = spec.report_entries(&item_with_life(65), &db);
        assert_eq!(report.len(), 2);
        assert!(report[0].satisfied, "group want should be satisfied");
        assert!(
            !report[1].satisfied,
            "min_value 70 want should not be satisfied at roll 65"
        );
        assert!(report.iter().all(|entry| entry.required));
    }
}
