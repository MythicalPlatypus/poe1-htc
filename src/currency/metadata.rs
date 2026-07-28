use std::fmt;

use super::MethodId;

/// Broad crafting-system grouping used by method selectors and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MethodFamily {
    Currency,
    Bench,
    Essence,
    Fossil,
    Harvest,
    Eldritch,
    Influence,
    Bestiary,
}

impl MethodFamily {
    /// Stable machine-readable code for saved selections and diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Currency => "currency",
            Self::Bench => "bench",
            Self::Essence => "essence",
            Self::Fossil => "fossil",
            Self::Harvest => "harvest",
            Self::Eldritch => "eldritch",
            Self::Influence => "influence",
            Self::Bestiary => "bestiary",
        }
    }

    /// Short human-readable family name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Currency => "Currency",
            Self::Bench => "Crafting Bench",
            Self::Essence => "Essence",
            Self::Fossil => "Fossil",
            Self::Harvest => "Harvest",
            Self::Eldritch => "Eldritch",
            Self::Influence => "Influence",
            Self::Bestiary => "Bestiary",
        }
    }
}

impl fmt::Display for MethodFamily {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Optional RePoE catalog that can supply a configured crafting method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MethodCatalog {
    CraftingBench,
    Essences,
    Fossils,
}

impl MethodCatalog {
    /// Stable machine-readable catalog code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CraftingBench => "crafting_bench",
            Self::Essences => "essences",
            Self::Fossils => "fossils",
        }
    }

    /// Human-readable catalog name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::CraftingBench => "Crafting Bench",
            Self::Essences => "Essences",
            Self::Fossils => "Fossils",
        }
    }
}

impl fmt::Display for MethodCatalog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a method becomes available to an optimizer request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MethodSetup {
    BuiltIn,
    Configured,
    CatalogOrConfigured(MethodCatalog),
    Unsupported,
}

impl MethodSetup {
    /// Stable machine-readable setup classification.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BuiltIn => "built_in",
            Self::Configured => "configured",
            Self::CatalogOrConfigured(_) => "catalog_or_configured",
            Self::Unsupported => "unsupported",
        }
    }

    /// Human-readable setup classification.
    pub const fn label(self) -> &'static str {
        match self {
            Self::BuiltIn => "Built in",
            Self::Configured => "Configured",
            Self::CatalogOrConfigured(_) => "Catalog or configured",
            Self::Unsupported => "Unsupported",
        }
    }

    /// Catalog that can supply this method, when one exists.
    pub const fn catalog(self) -> Option<MethodCatalog> {
        match self {
            Self::CatalogOrConfigured(catalog) => Some(catalog),
            Self::BuiltIn | Self::Configured | Self::Unsupported => None,
        }
    }
}

/// Coarse item-class applicability surfaced before state-specific validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemClassSupport {
    AnyCraftable,
    CatalogRestricted,
    EldritchArmour,
    InfluenceCompatible,
    Unavailable,
}

impl ItemClassSupport {
    /// Stable machine-readable support classification.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AnyCraftable => "any_craftable",
            Self::CatalogRestricted => "catalog_restricted",
            Self::EldritchArmour => "eldritch_armour",
            Self::InfluenceCompatible => "influence_compatible",
            Self::Unavailable => "unavailable",
        }
    }

    /// Human-readable support classification.
    pub const fn label(self) -> &'static str {
        match self {
            Self::AnyCraftable => "Any craftable item",
            Self::CatalogRestricted => "Restricted by catalog entry",
            Self::EldritchArmour => "Eldritch-compatible armour",
            Self::InfluenceCompatible => "Influence-compatible item",
            Self::Unavailable => "Unavailable",
        }
    }
}

/// A known mechanic-level approximation beyond finite outcome sampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProbabilityApproximation {
    UniformEldritchAffixCount,
}

impl ProbabilityApproximation {
    /// Stable machine-readable approximation code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UniformEldritchAffixCount => "uniform_eldritch_affix_count",
        }
    }

    /// Human-readable explanation of the approximation.
    pub const fn label(self) -> &'static str {
        match self {
            Self::UniformEldritchAffixCount => "Uniform legal Eldritch affix-count distribution",
        }
    }
}

/// Quality of the outcome probabilities exposed by a crafting method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProbabilityModel {
    Exact,
    MonteCarlo {
        samples: usize,
    },
    MonteCarloWithApproximation {
        samples: usize,
        approximation: ProbabilityApproximation,
    },
    ExactIdentitySampledRolls,
    Unavailable,
}

impl ProbabilityModel {
    /// Stable machine-readable probability classification.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::MonteCarlo { .. } => "monte_carlo",
            Self::MonteCarloWithApproximation { .. } => "monte_carlo_with_approximation",
            Self::ExactIdentitySampledRolls => "exact_identity_sampled_rolls",
            Self::Unavailable => "unavailable",
        }
    }

    /// Human-readable probability classification.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Exact => "Exact",
            Self::MonteCarlo { .. } => "Monte Carlo estimate",
            Self::MonteCarloWithApproximation { .. } => {
                "Monte Carlo estimate with mechanic approximation"
            }
            Self::ExactIdentitySampledRolls => "Exact identities with sampled numeric rolls",
            Self::Unavailable => "Unavailable",
        }
    }

    /// Whether result probabilities must be presented as estimates.
    pub const fn is_estimate(self) -> bool {
        matches!(
            self,
            Self::MonteCarlo { .. }
                | Self::MonteCarloWithApproximation { .. }
                | Self::ExactIdentitySampledRolls
        )
    }
}

/// Owned, adapter-independent description of one registered crafting method.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodMetadata {
    pub id: MethodId,
    pub display_name: String,
    pub family: MethodFamily,
    pub description: String,
    pub default_price_chaos: Option<f64>,
    pub setup: MethodSetup,
    pub item_class_support: ItemClassSupport,
    pub probability_model: ProbabilityModel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_codes_and_labels_are_stable_and_distinct() {
        let expected = [
            (MethodFamily::Currency, "currency", "Currency"),
            (MethodFamily::Bench, "bench", "Crafting Bench"),
            (MethodFamily::Essence, "essence", "Essence"),
            (MethodFamily::Fossil, "fossil", "Fossil"),
            (MethodFamily::Harvest, "harvest", "Harvest"),
            (MethodFamily::Eldritch, "eldritch", "Eldritch"),
            (MethodFamily::Influence, "influence", "Influence"),
            (MethodFamily::Bestiary, "bestiary", "Bestiary"),
        ];

        for (family, code, label) in expected {
            assert_eq!(family.as_str(), code);
            assert_eq!(family.to_string(), code);
            assert_eq!(family.label(), label);
        }
    }

    #[test]
    fn setup_exposes_only_its_actual_catalog_dependency() {
        let setup = MethodSetup::CatalogOrConfigured(MethodCatalog::Essences);
        assert_eq!(setup.as_str(), "catalog_or_configured");
        assert_eq!(setup.catalog(), Some(MethodCatalog::Essences));
        assert_eq!(MethodSetup::BuiltIn.catalog(), None);
        assert_eq!(MethodSetup::Configured.catalog(), None);
        assert_eq!(MethodSetup::Unsupported.catalog(), None);
    }

    #[test]
    fn probability_quality_marks_both_sampled_models_as_estimates() {
        assert!(!ProbabilityModel::Exact.is_estimate());
        assert!(ProbabilityModel::MonteCarlo { samples: 50 }.is_estimate());
        assert!(ProbabilityModel::MonteCarloWithApproximation {
            samples: 50,
            approximation: ProbabilityApproximation::UniformEldritchAffixCount,
        }
        .is_estimate());
        assert!(ProbabilityModel::ExactIdentitySampledRolls.is_estimate());
        assert!(!ProbabilityModel::Unavailable.is_estimate());
    }

    #[test]
    fn auxiliary_codes_and_labels_are_stable() {
        let catalogs = [
            (
                MethodCatalog::CraftingBench,
                "crafting_bench",
                "Crafting Bench",
            ),
            (MethodCatalog::Essences, "essences", "Essences"),
            (MethodCatalog::Fossils, "fossils", "Fossils"),
        ];
        for (catalog, code, label) in catalogs {
            assert_eq!(catalog.as_str(), code);
            assert_eq!(catalog.to_string(), code);
            assert_eq!(catalog.label(), label);
        }

        let setup_kinds = [
            (MethodSetup::BuiltIn, "built_in", "Built in"),
            (MethodSetup::Configured, "configured", "Configured"),
            (
                MethodSetup::CatalogOrConfigured(MethodCatalog::Fossils),
                "catalog_or_configured",
                "Catalog or configured",
            ),
            (MethodSetup::Unsupported, "unsupported", "Unsupported"),
        ];
        for (setup, code, label) in setup_kinds {
            assert_eq!(setup.as_str(), code);
            assert_eq!(setup.label(), label);
        }

        let support_kinds = [
            (
                ItemClassSupport::AnyCraftable,
                "any_craftable",
                "Any craftable item",
            ),
            (
                ItemClassSupport::CatalogRestricted,
                "catalog_restricted",
                "Restricted by catalog entry",
            ),
            (
                ItemClassSupport::EldritchArmour,
                "eldritch_armour",
                "Eldritch-compatible armour",
            ),
            (
                ItemClassSupport::InfluenceCompatible,
                "influence_compatible",
                "Influence-compatible item",
            ),
            (ItemClassSupport::Unavailable, "unavailable", "Unavailable"),
        ];
        for (support, code, label) in support_kinds {
            assert_eq!(support.as_str(), code);
            assert_eq!(support.label(), label);
        }

        let approximate = ProbabilityModel::MonteCarloWithApproximation {
            samples: 50,
            approximation: ProbabilityApproximation::UniformEldritchAffixCount,
        };
        let probability_models = [
            (ProbabilityModel::Exact, "exact", "Exact"),
            (
                ProbabilityModel::MonteCarlo { samples: 50 },
                "monte_carlo",
                "Monte Carlo estimate",
            ),
            (
                approximate,
                "monte_carlo_with_approximation",
                "Monte Carlo estimate with mechanic approximation",
            ),
            (
                ProbabilityModel::ExactIdentitySampledRolls,
                "exact_identity_sampled_rolls",
                "Exact identities with sampled numeric rolls",
            ),
            (ProbabilityModel::Unavailable, "unavailable", "Unavailable"),
        ];
        for (model, code, label) in probability_models {
            assert_eq!(model.as_str(), code);
            assert_eq!(model.label(), label);
        }

        let approximation = ProbabilityApproximation::UniformEldritchAffixCount;
        assert_eq!(approximation.as_str(), "uniform_eldritch_affix_count");
        assert_eq!(
            approximation.label(),
            "Uniform legal Eldritch affix-count distribution"
        );
        assert_eq!(
            approximate,
            ProbabilityModel::MonteCarloWithApproximation {
                samples: 50,
                approximation: ProbabilityApproximation::UniformEldritchAffixCount,
            }
        );
    }
}
