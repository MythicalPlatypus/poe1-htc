use std::fmt;
use std::sync::Arc;

use crate::currency::{
    bench::RemoveCraftedMods,
    fracturing::FracturingOrb,
    orbs::{
        ChaosOrb, DivineOrb, ExaltedOrb, OrbOfAlchemy, OrbOfAlteration, OrbOfAnnulment,
        OrbOfAugmentation, OrbOfScouring, OrbOfTransmutation, RegalOrb,
    },
    CraftingMethod, MethodMetadata,
};
use crate::data::{base_items::BaseItem, loader::LoadedGameData, DataProvenance, GameData};

/// Stable machine-readable application error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AppErrorCode {
    BaseItemNotFound,
    InvalidGoal,
    InvalidSearch,
    ItemImportFailed,
    InvalidStartingItem,
    MethodConfigurationInvalid,
    DuplicateMethod,
    InvalidMethodAccess,
    UnknownMethodPrice,
    PreparedDataMismatch,
}

impl AppErrorCode {
    /// The stable string representation exposed to application adapters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BaseItemNotFound => "base_item_not_found",
            Self::InvalidGoal => "invalid_goal",
            Self::InvalidSearch => "invalid_search",
            Self::ItemImportFailed => "item_import_failed",
            Self::InvalidStartingItem => "invalid_starting_item",
            Self::MethodConfigurationInvalid => "method_configuration_invalid",
            Self::DuplicateMethod => "duplicate_method",
            Self::InvalidMethodAccess => "invalid_method_access",
            Self::UnknownMethodPrice => "unknown_method_price",
            Self::PreparedDataMismatch => "prepared_data_mismatch",
        }
    }
}

impl fmt::Display for AppErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// An application-service failure with a stable code and readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    code: AppErrorCode,
    message: String,
}

impl AppError {
    pub(super) fn new(code: AppErrorCode, message: String) -> Self {
        Self { code, message }
    }

    pub const fn code(&self) -> AppErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

/// Stable machine-readable application warning codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AppWarningCode {
    AmbiguousBaseItem,
    UnmatchedPriceOverride,
}

impl AppWarningCode {
    /// The stable string representation exposed to application adapters.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AmbiguousBaseItem => "ambiguous_base_item",
            Self::UnmatchedPriceOverride => "unmatched_price_override",
        }
    }
}

impl fmt::Display for AppWarningCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A non-fatal application diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppWarning {
    code: AppWarningCode,
    message: String,
}

impl AppWarning {
    pub(super) fn new(code: AppWarningCode, message: String) -> Self {
        Self { code, message }
    }

    /// Build the compatibility warning used when a legacy display-name price
    /// does not resolve to an effective semantic method.
    pub fn unmatched_price_override(display_name: &str) -> Self {
        Self::new(
            AppWarningCode::UnmatchedPriceOverride,
            format!("[prices] \"{display_name}\" matches no method name — ignored"),
        )
    }

    pub const fn code(&self) -> AppWarningCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AppWarning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// A resolved RePoE base and any non-fatal diagnostics from resolution.
#[derive(Debug)]
pub struct BaseItemResolution<'data> {
    pub id: &'data str,
    pub base_item: &'data BaseItem,
    pub warnings: Vec<AppWarning>,
}

impl BaseItemResolution<'_> {
    pub fn warnings(&self) -> &[AppWarning] {
        &self.warnings
    }
}

/// One immutable pairing of game data and the provenance that identifies it.
///
/// Prepared work retains this exact allocation, so two services cannot exchange
/// jobs merely because their fingerprints happen to match.
#[derive(Debug)]
pub(super) struct ServiceDataset {
    game_data: Arc<GameData>,
    data_provenance: DataProvenance,
}

impl ServiceDataset {
    pub(super) fn game_data(&self) -> &GameData {
        &self.game_data
    }

    pub(super) fn data_provenance(&self) -> &DataProvenance {
        &self.data_provenance
    }
}

/// Reusable, side-effect-free entry point to optimizer application behavior.
#[derive(Debug, Clone)]
pub struct OptimizerService {
    dataset: Arc<ServiceDataset>,
}

impl OptimizerService {
    /// Construct a service from already loaded data and its authoritative
    /// source provenance. This constructor performs no file I/O.
    pub fn new(game_data: GameData, data_provenance: DataProvenance) -> Self {
        Self::from_shared(Arc::new(game_data), data_provenance)
    }

    /// Construct a service directly from the pure data-loader result.
    pub fn from_loaded(loaded: LoadedGameData) -> Self {
        Self::new(loaded.game_data, loaded.provenance)
    }

    pub fn from_shared(game_data: Arc<GameData>, data_provenance: DataProvenance) -> Self {
        Self {
            dataset: Arc::new(ServiceDataset {
                game_data,
                data_provenance,
            }),
        }
    }

    pub fn game_data(&self) -> &GameData {
        self.dataset.game_data()
    }

    pub fn data_provenance(&self) -> &DataProvenance {
        self.dataset.data_provenance()
    }

    pub(super) fn shared_dataset(&self) -> Arc<ServiceDataset> {
        Arc::clone(&self.dataset)
    }

    /// Resolve an exact RePoE metadata ID or a case-insensitive display name.
    ///
    /// Exact IDs take precedence. Duplicate display names deterministically
    /// select the lexicographically smallest metadata ID and return a warning.
    pub fn resolve_base_item(&self, query: &str) -> Result<BaseItemResolution<'_>, AppError> {
        if let Some((id, base_item)) = self.game_data().base_items.get_key_value(query) {
            return Ok(BaseItemResolution {
                id,
                base_item,
                warnings: Vec::new(),
            });
        }

        let normalized_query = query.to_lowercase();
        let mut matches: Vec<(&String, &BaseItem)> = self
            .game_data()
            .base_items
            .iter()
            .filter(|(_, base_item)| base_item.name.to_lowercase() == normalized_query)
            .collect();
        matches.sort_unstable_by_key(|(id, _)| *id);

        let Some(&(id, base_item)) = matches.first() else {
            return Err(AppError::new(
                AppErrorCode::BaseItemNotFound,
                format!(
                    "Base item '{query}' not found (tried exact ID match and case-insensitive name match)"
                ),
            ));
        };

        let warnings = if matches.len() > 1 {
            vec![AppWarning::new(
                AppWarningCode::AmbiguousBaseItem,
                format!(
                    "{} base items share the name '{query}'; using {id} (pass the metadata ID to disambiguate)",
                    matches.len()
                ),
            )]
        } else {
            Vec::new()
        };

        Ok(BaseItemResolution {
            id,
            base_item,
            warnings,
        })
    }

    /// The legacy CLI method set in its established search order.
    pub fn legacy_default_methods(&self) -> Vec<Arc<dyn CraftingMethod>> {
        vec![
            Arc::new(OrbOfScouring),
            Arc::new(OrbOfTransmutation),
            Arc::new(OrbOfAlteration),
            Arc::new(OrbOfAugmentation),
            Arc::new(RegalOrb),
            Arc::new(OrbOfAlchemy),
            Arc::new(ChaosOrb),
            Arc::new(ExaltedOrb),
            Arc::new(OrbOfAnnulment),
            Arc::new(DivineOrb),
            Arc::new(FracturingOrb),
            Arc::new(RemoveCraftedMods),
        ]
    }

    /// Structured registry metadata for the legacy built-in method set.
    ///
    /// The returned order is the exact deterministic search-registration order.
    pub fn legacy_method_registry(&self) -> Vec<MethodMetadata> {
        self.legacy_default_methods()
            .into_iter()
            .map(|method| method.metadata())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::data::DataFingerprint;

    fn base_item(name: &str) -> BaseItem {
        BaseItem {
            name: name.to_string(),
            item_class: "Body Armours".to_string(),
            tags: Vec::new(),
            implicits: Vec::new(),
            drop_level: 1,
            inventory_height: 3,
            inventory_width: 2,
        }
    }

    fn service_with_bases(entries: &[(&str, &str)]) -> OptimizerService {
        let bases = entries
            .iter()
            .map(|(id, name)| ((*id).to_string(), base_item(name)))
            .collect::<HashMap<_, _>>();
        OptimizerService::new(
            GameData::new(HashMap::new(), bases),
            DataProvenance::unversioned(DataFingerprint::sha256_of_bytes(
                b"app-service-base-resolution-fixture-v1",
            )),
        )
    }

    #[test]
    fn exact_metadata_id_takes_precedence_over_display_name() {
        let service = service_with_bases(&[
            ("Astral Plate", "Exact ID Base"),
            ("Metadata/Items/Armours/AstralPlate", "Astral Plate"),
        ]);

        let resolution = service
            .resolve_base_item("Astral Plate")
            .expect("exact metadata ID should resolve");

        assert_eq!(resolution.id, "Astral Plate");
        assert_eq!(resolution.base_item.name, "Exact ID Base");
        assert!(resolution.warnings().is_empty());
    }

    #[test]
    fn display_name_matching_is_unicode_case_insensitive() {
        let service = service_with_bases(&[("Metadata/Items/Armours/Epee", "Épée Astrale")]);

        let resolution = service
            .resolve_base_item("éPÉE aStRaLe")
            .expect("Unicode case variants should resolve");

        assert_eq!(resolution.id, "Metadata/Items/Armours/Epee");
        assert_eq!(resolution.base_item.name, "Épée Astrale");
        assert!(resolution.warnings().is_empty());
    }

    #[test]
    fn duplicate_names_select_smallest_id_and_return_stable_warning() {
        let service = service_with_bases(&[
            ("Metadata/Z", "Astral Plate"),
            ("Metadata/A", "Astral Plate"),
        ]);

        let resolution = service
            .resolve_base_item("ASTRAL PLATE")
            .expect("duplicate display names should resolve deterministically");

        assert_eq!(resolution.id, "Metadata/A");
        assert_eq!(resolution.warnings().len(), 1);
        let warning = &resolution.warnings()[0];
        assert_eq!(warning.code(), AppWarningCode::AmbiguousBaseItem);
        assert_eq!(warning.code().as_str(), "ambiguous_base_item");
        assert_eq!(
            warning.message(),
            "2 base items share the name 'ASTRAL PLATE'; using Metadata/A (pass the metadata ID to disambiguate)"
        );
    }

    #[test]
    fn missing_base_returns_stable_code_and_existing_message() {
        let service = service_with_bases(&[]);

        let error = service
            .resolve_base_item("Missing Plate")
            .expect_err("unknown base should fail");

        assert_eq!(error.code(), AppErrorCode::BaseItemNotFound);
        assert_eq!(error.code().as_str(), "base_item_not_found");
        assert_eq!(
            error.message(),
            "Base item 'Missing Plate' not found (tried exact ID match and case-insensitive name match)"
        );
        assert_eq!(error.to_string(), error.message());
    }

    #[test]
    fn legacy_default_methods_keep_exact_names_and_order() {
        let service = service_with_bases(&[]);
        let methods = service.legacy_default_methods();
        let identities = methods
            .iter()
            .map(|method| (method.id(), method.name()))
            .collect::<Vec<_>>();

        assert_eq!(
            identities
                .iter()
                .map(|(id, name)| (id.as_str(), *name))
                .collect::<Vec<_>>(),
            [
                ("currency/scour", "Orb of Scouring"),
                ("currency/transmute", "Orb of Transmutation"),
                ("currency/alteration", "Orb of Alteration"),
                ("currency/augmentation", "Orb of Augmentation"),
                ("currency/regal", "Regal Orb"),
                ("currency/alchemy", "Orb of Alchemy"),
                ("currency/chaos", "Chaos Orb"),
                ("currency/exalted", "Exalted Orb"),
                ("currency/annulment", "Orb of Annulment"),
                ("currency/divine", "Divine Orb"),
                ("currency/fracturing", "Fracturing Orb"),
                ("bench/remove-crafted", "Remove Crafted Mods"),
            ]
        );
    }
}
