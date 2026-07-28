use anyhow::{Context, Result};
use std::path::Path;

use super::base_items::BaseItem;
use super::crafting_catalogs::{parse_crafting_bench_options, parse_essences, parse_fossils};
use super::mods::Mod;
use super::provenance::{fingerprint_repoe_bundle, DataProvenance};
use super::GameData;

/// Load all RePoE JSON files from `data_dir` into a `GameData` instance.
pub fn load_all(data_dir: &str) -> Result<GameData> {
    Ok(load_all_with_provenance(data_dir)?.game_data)
}

/// Immutable game data and the exact source-bundle identity it was loaded from.
#[derive(Debug)]
pub struct LoadedGameData {
    pub game_data: GameData,
    pub provenance: DataProvenance,
}

/// Load the five fixed RePoE JSON inputs and report their exact byte provenance.
///
/// `mods.json` is required. The other four inputs retain their existing
/// optional behavior. A present `repoe-version.txt` supplies descriptive
/// version metadata but never contributes to the source fingerprint.
pub fn load_all_with_provenance(data_dir: impl AsRef<Path>) -> Result<LoadedGameData> {
    let dir = data_dir.as_ref();
    let bundle = RePoeBundleBytes::read(dir)?;
    let fingerprint = bundle.fingerprint();
    let game_data = bundle.parse(dir)?;
    let provenance = match read_repoe_version(dir)? {
        Some(version) => DataProvenance::versioned(fingerprint, &version).with_context(|| {
            format!(
                "Invalid RePoE version in {}",
                dir.join("repoe-version.txt").display()
            )
        })?,
        None => DataProvenance::unversioned(fingerprint),
    };

    Ok(LoadedGameData {
        game_data,
        provenance,
    })
}

#[derive(Debug)]
struct RePoeBundleBytes {
    mods: Vec<u8>,
    base_items: Option<Vec<u8>>,
    crafting_bench: Option<Vec<u8>>,
    essences: Option<Vec<u8>>,
    fossils: Option<Vec<u8>>,
}

impl RePoeBundleBytes {
    fn read(dir: &Path) -> Result<Self> {
        Ok(Self {
            mods: read_required_file(dir, "mods.json")?,
            base_items: read_optional_file(dir, "base_items.json")?,
            crafting_bench: read_optional_file(dir, "crafting_bench_options.json")?,
            essences: read_optional_file(dir, "essences.json")?,
            fossils: read_optional_file(dir, "fossils.json")?,
        })
    }

    fn fingerprint(&self) -> super::provenance::DataFingerprint {
        fingerprint_repoe_bundle([
            Some(self.mods.as_slice()),
            self.base_items.as_deref(),
            self.crafting_bench.as_deref(),
            self.essences.as_deref(),
            self.fossils.as_deref(),
        ])
    }

    fn parse(self, dir: &Path) -> Result<GameData> {
        let mods: std::collections::HashMap<String, Mod> =
            parse_json_file(&self.mods, &dir.join("mods.json"))?;
        let base_items: std::collections::HashMap<String, BaseItem> = match self.base_items {
            Some(raw) => parse_json_file(&raw, &dir.join("base_items.json"))?,
            None => std::collections::HashMap::new(),
        };
        let crafting_bench = parse_optional_catalog(
            self.crafting_bench,
            &dir.join("crafting_bench_options.json"),
            parse_crafting_bench_options,
        )?;
        let essences =
            parse_optional_catalog(self.essences, &dir.join("essences.json"), parse_essences)?;
        let fossils =
            parse_optional_catalog(self.fossils, &dir.join("fossils.json"), parse_fossils)?;

        Ok(GameData::new(mods, base_items).with_crafting_catalogs(
            crafting_bench,
            essences,
            fossils,
        ))
    }
}

fn read_required_file(dir: &Path, filename: &str) -> Result<Vec<u8>> {
    let path = dir.join(filename);
    std::fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))
}

fn read_optional_file(dir: &Path, filename: &str) -> Result<Option<Vec<u8>>> {
    let path = dir.join(filename);
    match std::fs::read(&path) {
        Ok(raw) => Ok(Some(raw)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Failed to read {}", path.display())),
    }
}

fn parse_json_file<T>(raw: &[u8], path: &Path) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let raw =
        std::str::from_utf8(raw).with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_str(raw).with_context(|| format!("Failed to parse {}", path.display()))
}

fn parse_optional_catalog<T>(
    raw: Option<Vec<u8>>,
    path: &Path,
    parse: impl FnOnce(&str) -> Result<T>,
) -> Result<Option<T>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let raw =
        std::str::from_utf8(&raw).with_context(|| format!("Failed to read {}", path.display()))?;
    parse(raw)
        .with_context(|| format!("Failed to load {}", path.display()))
        .map(Some)
}

fn read_repoe_version(dir: &Path) -> Result<Option<String>> {
    let path = dir.join("repoe-version.txt");
    let Some(raw) = read_optional_file(dir, "repoe-version.txt")? else {
        return Ok(None);
    };
    String::from_utf8(raw)
        .with_context(|| format!("Failed to read {}", path.display()))
        .map(Some)
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
        let db = load_all("data")
            .expect("data/mods.json must exist — run the curl commands from the README");
        assert!(
            db.mods.len() > 10_000,
            "expected 10k+ mods, got {}",
            db.mods.len()
        );
        assert!(
            db.base_items.len() > 100,
            "expected 100+ base items, got {}",
            db.base_items.len()
        );
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
                    matches!(
                        m.generation_type,
                        GenerationType::Prefix | GenerationType::Suffix
                    ),
                    "is_craftable() returned true for non-prefix/suffix mod {id}"
                );
                assert_eq!(
                    m.domain,
                    Domain::Item,
                    "craftable mod {id} has wrong domain"
                );
                assert!(
                    !m.is_essence_only,
                    "is_craftable() returned true for essence-only mod {id}"
                );
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
            m.spawn_weights
                .iter()
                .any(|sw| sw.tag == "default" && sw.weight > 0)
        });
        if let Some(m) = mod_with_default {
            let w = m.spawn_weight_for_tags(&["this_tag_does_not_exist"]);
            let default_w = m
                .spawn_weights
                .iter()
                .find(|sw| sw.tag == "default")
                .unwrap()
                .weight;
            assert_eq!(
                w, default_w,
                "spawn_weight_for_tags should fall back to 'default'"
            );
        }
    }
}
