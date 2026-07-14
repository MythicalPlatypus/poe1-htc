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
//! fractured, or crafted) matches **all** criteria the want specifies:
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

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::data::GameData;
use crate::item::{ItemState, Modifier};

fn default_item_level() -> u32 {
    84
}

fn default_weight() -> f64 {
    1.0
}

/// Top-level goal file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalSpec {
    pub item: ItemSpec,
    /// Desired mods. At least one required.
    pub wants: Vec<WantSpec>,
    /// Optional search-parameter overrides.
    #[serde(default)]
    pub search: SearchSpec,
}

/// The base item to start crafting from.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemSpec {
    /// Base item display name (e.g. "Astral Plate") or RePoE metadata ID
    /// (e.g. "Metadata/Items/Armours/BodyArmours/BodyStr15").
    pub base: String,
    /// Item level — gates which mods can roll (`required_level <= item_level`).
    #[serde(default = "default_item_level")]
    pub item_level: u32,
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
    /// Cost penalty per chaos orb in node ranking; tune relative to the sum of
    /// want weights. 0.0 ignores cost entirely (the search will happily exalt-spam).
    pub cost_weight: Option<f64>,
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
        Ok(())
    }

    /// Score an item state: sum of weights over satisfied wants.
    /// Called once per candidate node in the beam search — kept allocation-free.
    pub fn score(&self, state: &ItemState, db: &GameData) -> f64 {
        self.wants
            .iter()
            .filter(|w| {
                state
                    .all_mods_for_conflict()
                    .any(|m| want_matches(w, m, db))
            })
            .map(|w| w.weight)
            .sum()
    }

    /// Human-readable satisfaction report for the final CLI output:
    /// one `(description, satisfied)` pair per want.
    pub fn report(&self, state: &ItemState, db: &GameData) -> Vec<(String, bool)> {
        self.wants
            .iter()
            .map(|w| {
                let satisfied = state
                    .all_mods_for_conflict()
                    .any(|m| want_matches(w, m, db));
                (describe_want(w), satisfied)
            })
            .collect()
    }
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
        GameData {
            mods,
            base_items: HashMap::new(),
        }
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
