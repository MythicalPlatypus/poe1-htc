//! Reusable application services shared by user-facing adapters.

pub mod dto;

pub use crate::currency::{
    ItemClassSupport, MethodCatalog, MethodFamily, MethodId, MethodMetadata, MethodSetup,
    ProbabilityApproximation, ProbabilityModel,
};
pub use crate::data::{DataFingerprint, DataProvenance};
pub use crate::goal::{
    GoalEvaluation, GoalReportEntry, GoalScoringMode, GoalSelectorWarning, GoalSelectorWarningCode,
    ImpossibleGoalReason, ImpossibleGoalReasonCode,
};
pub use crate::search::{
    BudgetComparison, BudgetMetric, BudgetPolicy, CancellationToken, CostValue, SearchLimits,
    SearchObserver, SearchProgress, SearchRuntime, SearchTermination, SearchTerminationReason,
};

mod catalog;
mod pricing;
mod request;
mod runner;
mod service;

pub use catalog::{
    compatible_clean_base_affixes, search_base_items, AffixKind, AffixStatSummary, AffixSummary,
    BaseItemSummary, CatalogError, CleanBaseAffixCatalog, CleanBaseAffixQuery,
};
pub use pricing::{PriceBook, PriceBookError};
pub use request::{
    GoalSetRequest, MethodAccessPolicy, MethodSelection, MethodSetRequest, OptimizeRequest,
    SearchRequest, StartingItemRequest,
};
pub use runner::{
    AppliedPriceOverride, EvaluatedSearchResult, MethodSummary, OptimizeOutcome, OptimizeResponse,
    PathStatus, PreparationSummary, PreparedOptimization,
};
pub use service::{
    AppError, AppErrorCode, AppWarning, AppWarningCode, BaseItemResolution, OptimizerService,
};
