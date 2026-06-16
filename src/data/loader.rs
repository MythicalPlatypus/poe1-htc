use anyhow::{Context, Result};
use std::path::Path;

use super::base_items::BaseItem;
use super::mods::Mod;
use super::GameData;

/// Load all RePoE JSON files from `data_dir` into a `GameData` instance.
pub fn load_all(data_dir: &str) -> Result<GameData> {
    let dir = Path::new(data_dir);

    let mods = load_mods(dir)?;
    let base_items = load_base_items(dir)?;

    Ok(GameData { mods, base_items })
}

fn load_mods(dir: &Path) -> Result<std::collections::HashMap<String, Mod>> {
    let path = dir.join("mods.json");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let parsed: std::collections::HashMap<String, Mod> = serde_json::from_str(&raw)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(parsed)
}

fn load_base_items(dir: &Path) -> Result<std::collections::HashMap<String, BaseItem>> {
    let path = dir.join("base_items.json");
    if !path.exists() {
        // base_items is optional for now; return empty map
        return Ok(std::collections::HashMap::new());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let parsed: std::collections::HashMap<String, BaseItem> = serde_json::from_str(&raw)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(parsed)
}

// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::mods::{Domain, GenerationType};

    /// Verifies that mods.json and base_items.json parse without error and
    /// contain a plausible number of entries. Requires `data/` to be populated.
    #[test]
    fn loads_repoe_data() {
        let db = load_all("data").expect("data/mods.json must exist — run the curl commands from the README");
        assert!(db.mods.len() > 10_000, "expected 10k+ mods, got {}", db.mods.len());
        assert!(db.base_items.len() > 100, "expected 100+ base items, got {}", db.base_items.len());
    }

    /// Spot-checks that every mod in the loaded data has a non-empty mod_type,
    /// and that craftable mods are only Prefix or Suffix.
    #[test]
    fn craftable_mods_are_prefix_or_suffix() {
        let db = load_all("data").expect("data/ must be populated");
        for (id, m) in &db.mods {
            assert!(!m.mod_type.is_empty(), "mod {id} has empty mod_type");
            if m.is_craftable() {
                assert!(
                    matches!(m.generation_type, GenerationType::Prefix | GenerationType::Suffix),
                    "is_craftable() returned true for non-prefix/suffix mod {id}"
                );
                assert_eq!(m.domain, Domain::Item, "craftable mod {id} has wrong domain");
                assert!(!m.is_essence_only, "is_craftable() returned true for essence-only mod {id}");
            }
        }
    }

    /// Checks that spawn_weight_for_tags returns 0 for a tag with no entry
    /// and a non-zero value for the "default" tag when one exists.
    #[test]
    fn spawn_weight_fallback_to_default() {
        let db = load_all("data").expect("data/ must be populated");
        // Find any mod that has a "default" spawn weight > 0.
        let mod_with_default = db.mods.values().find(|m| {
            m.spawn_weights.iter().any(|sw| sw.tag == "default" && sw.weight > 0)
        });
        if let Some(m) = mod_with_default {
            let w = m.spawn_weight_for_tags(&["this_tag_does_not_exist"]);
            let default_w = m.spawn_weights.iter().find(|sw| sw.tag == "default").unwrap().weight;
            assert_eq!(w, default_w, "spawn_weight_for_tags should fall back to 'default'");
        }
    }
}
