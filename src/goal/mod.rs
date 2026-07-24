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
//! `GoalSpec::score` returns the sum of `weight` over all *satisfied* wants.
//! A want is satisfied when at least one mod on the item (prefix, suffix,
//! fractured, crafted, or eldritch implicit) matches **all** criteria the want
//! specifies:
//!
//! - `mod_id` — the mod's RePoE ID equals this string exactly.
//! - `group`  — the mod's DB entry lists this group in `groups` (the same
//!   field used for conflict checks — never display names).
//! - `stat`   — the mod has a rolled stat with this RePoE stat ID.
//! - `min_value` — requires `stat`; the matching stat's rolled value must be
//!   `>= min_value` *on the same mod*.
//!
//! Each want contributes its weight at most once, no matter how many mods
//! match it. Scoring is binary per want (no partial credit for low rolls
//! except via the `min_value` threshold).

use std::collections::HashMap;
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
use crate::data::mods::{Domain, GenerationType};
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
    /// Desired mods. At least one required.
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
    /// Score contribution when satisfied. Defaults to 1.0; must be > 0.
    #[serde(default = "default_weight")]
    pub weight: f64,
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
#[serde(tag = "type", rename_all = "snake_case")]
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
        if self.wants.is_empty() {
            bail!("Goal must contain at least one [[wants]] entry");
        }
        for (i, w) in self.wants.iter().enumerate() {
            if w.mod_id.is_none() && w.group.is_none() && w.stat.is_none() {
                bail!("[[wants]] entry {i}: specify at least one of mod_id, group, stat");
            }
            if w.min_value.is_some() && w.stat.is_none() {
                bail!("[[wants]] entry {i}: min_value requires stat to be set");
            }
            if w.weight <= 0.0 || !w.weight.is_finite() {
                bail!("[[wants]] entry {i}: weight must be a positive finite number");
            }
        }
        for (name, price) in &self.prices {
            if *price <= 0.0 || !price.is_finite() {
                bail!("[prices] \"{name}\": price must be a positive finite number");
            }
        }
        for (i, method) in self.methods.iter().enumerate() {
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
                    bail!(
                        "[[methods]] entry {i}: named fossil cannot also use manual tag/mod fields"
                    );
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
                    bail!(
                        "[[methods]] entry {i}: fossil mod '{id}' cannot be both blocked and forced"
                    );
                }
            }
            if let MethodSpec::BestiarySwap { beast_level, .. } = method {
                if !(1..=100).contains(beast_level) {
                    bail!("[[methods]] entry {i}: beast_level must be between 1 and 100");
                }
            }
        }
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
        }
        Ok(())
    }

    /// Score an item state: sum of weights over satisfied wants.
    /// Called once per candidate node in the beam search — kept allocation-free.
    pub fn score(&self, state: &ItemState, db: &GameData) -> f64 {
        self.wants
            .iter()
            .filter(|w| scorable_mods(state).any(|m| want_matches(w, m, db)))
            .map(|w| w.weight)
            .sum()
    }

    /// Maximum raw goal score when every requested condition is satisfied.
    pub fn max_score(&self) -> f64 {
        self.wants.iter().map(|want| want.weight).sum()
    }

    /// Number of requested conditions satisfied by `state`.
    pub fn satisfied_count(&self, state: &ItemState, db: &GameData) -> usize {
        self.wants
            .iter()
            .filter(|want| scorable_mods(state).any(|m| want_matches(want, m, db)))
            .count()
    }

    /// True only when every requested condition is present on the same item.
    pub fn is_complete(&self, state: &ItemState, db: &GameData) -> bool {
        self.satisfied_count(state, db) == self.wants.len()
    }

    /// Human-readable satisfaction report for the final CLI output:
    /// one `(description, satisfied)` pair per want.
    pub fn report(&self, state: &ItemState, db: &GameData) -> Vec<(String, bool)> {
        self.wants
            .iter()
            .map(|w| {
                let satisfied = scorable_mods(state).any(|m| want_matches(w, m, db));
                (describe_want(w), satisfied)
            })
            .collect()
    }
}

/// Every mod a want can be satisfied by: affix-slot mods (prefixes, suffixes,
/// fractured, crafted) plus eldritch implicits — implicits don't participate in
/// affix conflicts but absolutely count toward goals.
fn scorable_mods(state: &ItemState) -> impl Iterator<Item = &Modifier> {
    state
        .all_mods_for_conflict()
        .chain(state.exarch_implicit.iter())
        .chain(state.eater_implicit.iter())
}

/// True if `modifier` satisfies every criterion `want` specifies.
fn want_matches(want: &WantSpec, modifier: &Modifier, db: &GameData) -> bool {
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
        let threshold = want.min_value.unwrap_or(i32::MIN);
        let has_stat = modifier
            .rolls
            .iter()
            .any(|r| &r.stat_id == stat && r.value >= threshold);
        if !has_stat {
            return false;
        }
    }
    true
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
        match w.min_value {
            Some(v) => parts.push(format!("stat {s} >= {v}")),
            None => parts.push(format!("stat {s}")),
        }
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
        assert_eq!(spec.search.beam_width, Some(20));
        assert_eq!(spec.search.cost_weight, Some(0.05));
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
    fn rejects_min_value_without_stat() {
        let err = GoalSpec::from_toml_str(
            r#"
            [item]
            base = "Astral Plate"
            [[wants]]
            group = "IncreasedLife"
            min_value = 70
            "#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("min_value requires stat"),
            "got: {err}"
        );
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

    #[test]
    fn report_flags_each_want() {
        let spec = GoalSpec::from_toml_str(VALID_TOML).unwrap();
        let db = db_with_life();
        let report = spec.report(&item_with_life(65), &db);
        assert_eq!(report.len(), 2);
        assert!(report[0].1, "group want should be satisfied");
        assert!(
            !report[1].1,
            "min_value 70 want should not be satisfied at roll 65"
        );
    }
}
