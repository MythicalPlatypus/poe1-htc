use crate::currency::{MethodFamily, MethodId};
use crate::goal::{ItemSpec, MethodSpec, WantSpec};
use crate::search::BudgetPolicy;

use super::pricing::PriceBook;

/// A complete, owned optimization request independent of files and CLI flags.
#[derive(Debug)]
pub struct OptimizeRequest {
    pub starting_item: StartingItemRequest,
    pub goals: GoalSetRequest,
    pub methods: MethodSetRequest,
    pub budget: BudgetPolicy,
    pub search: SearchRequest,
}

/// The supported ways to construct the item at the root of a search.
#[derive(Debug)]
pub enum StartingItemRequest {
    /// Build and validate a programmatically described starting item.
    Described(ItemSpec),
    /// Parse and validate raw Path of Exile clipboard text.
    ImportedText {
        text: String,
        fallback_item_level: Option<u32>,
    },
}

/// The current goal language, owned separately from the TOML [`crate::goal::GoalSpec`].
#[derive(Debug)]
pub struct GoalSetRequest {
    pub wants: Vec<WantSpec>,
}

/// Legacy default methods plus configured methods and semantic price overrides.
#[derive(Debug)]
pub struct MethodSetRequest {
    pub configured: Vec<MethodSpec>,
    /// Which built-in and configured semantic methods search may use.
    pub access: MethodAccessPolicy,
    /// Optional overrides keyed by stable semantic method identity.
    pub price_overrides: PriceBook,
}

/// Method-selection policy applied without changing deterministic registry order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MethodAccessPolicy {
    /// Compatibility policy: all legacy built-ins followed by every configured
    /// method in request order.
    #[default]
    LegacyDefaultsAndConfigured,
    /// Enable exactly these semantic IDs. The vector's order does not alter
    /// search order; it remains built-ins followed by configured request order.
    Allowlist(Vec<MethodId>),
    /// Compose family-wide selection with per-method additions and exclusions.
    ///
    /// The request-vector order never changes search order.
    Explicit(MethodSelection),
}

/// Explicit method selection used by family and individual UI controls.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MethodSelection {
    /// Enable every available method in these families.
    pub enabled_families: Vec<MethodFamily>,
    /// Enable these methods even when their family is not enabled.
    pub enabled_methods: Vec<MethodId>,
    /// Disable these methods after family and individual inclusion.
    pub disabled_methods: Vec<MethodId>,
}

/// Fully resolved search settings.
///
/// Adapters are responsible for applying their own precedence rules before
/// constructing this value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchRequest {
    pub beam_width: usize,
    pub max_steps: usize,
    pub cost_weight: f64,
    pub restart_cost: f64,
    pub seed: Option<u64>,
    pub top: usize,
    pub expansion_limit: Option<u64>,
    pub timeout_ms: Option<u64>,
}

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            beam_width: 50,
            max_steps: 10,
            cost_weight: 0.0,
            restart_cost: 1.0,
            seed: None,
            top: 1,
            expansion_limit: None,
            timeout_ms: None,
        }
    }
}
