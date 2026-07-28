use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::currency::{CraftingMethod, MethodId, MethodMetadata, Repriced};
use crate::data::DataProvenance;
use crate::goal::{
    analyze_impossible_goals, build_imported_state, validate_method_specs, GoalEvaluation,
    GoalEvaluator, GoalReportEntry, GoalSelectorWarning, ImpossibleGoalReason, WantSpec,
};
use crate::import::{import_item_text, ImportOptions, ImportWarning};
use crate::item::ItemState;
use crate::search::beam::{BeamConfig, BeamSearch, SearchEvaluation, SearchResult};
use crate::search::{
    BudgetComparison, BudgetPolicy, SearchLimits, SearchProgress, SearchRuntime, SearchTermination,
    SearchTerminationReason,
};
use rand::RngCore;

use super::pricing::PriceBook;
use super::request::{
    GoalSetRequest, MethodAccessPolicy, MethodSelection, MethodSetRequest, OptimizeRequest,
    SearchRequest, StartingItemRequest,
};
use super::service::{AppError, AppErrorCode, AppWarning, OptimizerService, ServiceDataset};

/// One semantic price override applied to an effective crafting method.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedPriceOverride {
    pub method_id: MethodId,
    pub method_name: String,
    pub cost: f64,
}

/// Registry metadata for one effective method.
pub type MethodSummary = MethodMetadata;

/// Owned facts produced while validating and preparing an optimization.
#[derive(Debug, Clone)]
pub struct PreparationSummary {
    /// Immutable identity of the RePoE inputs used to prepare this job.
    pub data_provenance: DataProvenance,
    pub base_id: String,
    pub base_name: String,
    pub item_level: u32,
    pub imported_start: bool,
    pub import_warnings: Vec<ImportWarning>,
    pub base_warnings: Vec<AppWarning>,
    pub goal_warnings: Vec<GoalSelectorWarning>,
    pub impossible_reasons: Vec<ImpossibleGoalReason>,
    pub search: SearchRequest,
    pub budget: BudgetPolicy,
    /// Every effective method in the exact order used by beam search.
    pub effective_methods: Vec<MethodSummary>,
    /// Configured methods in request order, excluding legacy defaults.
    pub configured_methods: Vec<MethodSummary>,
    /// Fully resolved price of every effective method.
    pub price_book: PriceBook,
    pub applied_price_overrides: Vec<AppliedPriceOverride>,
    pub report_starting_item: bool,
}

/// A validated, fully owned optimization ready to run.
///
/// The prepared request retains the exact immutable data set against which its
/// item and crafting methods were built. This keeps it safe to move to a
/// background worker without exposing service-borrowing lifetimes.
pub struct PreparedOptimization {
    dataset: Arc<ServiceDataset>,
    initial_state: ItemState,
    wants: Vec<WantSpec>,
    /// Effective methods before any semantic price overrides are applied.
    ///
    /// Retaining these shared method implementations lets adapter repricing
    /// replace an earlier override book instead of stacking `Repriced`
    /// wrappers or leaving stale costs on omitted IDs.
    unpriced_methods: Vec<Arc<dyn CraftingMethod>>,
    methods: Vec<Arc<dyn CraftingMethod>>,
    summary: PreparationSummary,
}

impl PreparedOptimization {
    pub fn summary(&self) -> &PreparationSummary {
        &self.summary
    }

    pub fn initial_state(&self) -> &ItemState {
        &self.initial_state
    }
}

impl fmt::Debug for PreparedOptimization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let method_summaries = self
            .methods
            .iter()
            .map(|method| method.metadata())
            .collect::<Vec<_>>();
        formatter
            .debug_struct("PreparedOptimization")
            .field("dataset", &Arc::as_ptr(&self.dataset))
            .field("initial_state", &self.initial_state)
            .field("wants", &self.wants)
            .field("methods", &method_summaries)
            .field("summary", &self.summary)
            .finish()
    }
}

/// A search result plus goal evaluation facts needed by presentation adapters.
#[derive(Debug)]
pub struct EvaluatedSearchResult {
    pub result: SearchResult,
    pub raw_score: f64,
    /// Request-only score ceiling. `None` means an uncapped numeric goal needs
    /// item/data-aware analysis before a sound maximum can be reported.
    pub max_score: Option<f64>,
    pub satisfied_count: usize,
    pub goal_count: usize,
    pub satisfied_required_count: usize,
    pub required_goal_count: usize,
    pub complete: bool,
    pub status: PathStatus,
    pub report: Vec<GoalReportEntry>,
}

/// Goal-level outcome of an optimization response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OptimizeOutcome {
    Complete,
    Incomplete,
    Impossible,
}

impl OptimizeOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::Impossible => "impossible",
        }
    }
}

/// Compliance and completion status of one returned path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathStatus {
    Complete,
    Incomplete,
    OverBudget,
}

impl PathStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::OverBudget => "over_budget",
        }
    }
}

/// Fully owned output from one optimization.
#[derive(Debug)]
pub struct OptimizeResponse {
    pub outcome: OptimizeOutcome,
    pub resolved_seed: u64,
    /// Strictest work limits actually applied after merging serialized request
    /// settings with non-serialized adapter/runtime safety limits.
    pub effective_limits: SearchLimits,
    pub termination: SearchTermination,
    pub impossible_reasons: Vec<ImpossibleGoalReason>,
    pub summary: PreparationSummary,
    pub initial_state: ItemState,
    pub starting_evaluation: GoalEvaluation,
    pub starting_report: Vec<GoalReportEntry>,
    /// Backward-compatible shorthand for `starting_evaluation.score`.
    pub starting_score: f64,
    /// Request-level preference-score ceiling. `None` means no sound finite
    /// maximum is currently known (for example an uncapped per-unit goal).
    pub maximum_score: Option<f64>,
    pub results: Vec<EvaluatedSearchResult>,
}

#[derive(Debug)]
struct OwnedBaseResolution {
    id: String,
    name: String,
    tags: Vec<String>,
    warnings: Vec<AppWarning>,
}

struct PricedMethods {
    methods: Vec<Arc<dyn CraftingMethod>>,
    applied_overrides: Vec<AppliedPriceOverride>,
    resolved_prices: PriceBook,
}

impl OptimizerService {
    /// Validate and resolve a request without running the CPU-heavy beam search.
    pub fn prepare(&self, request: OptimizeRequest) -> Result<PreparedOptimization, AppError> {
        let OptimizeRequest {
            starting_item,
            goals,
            methods,
            budget,
            search,
        } = request;
        let GoalSetRequest { wants } = goals;
        let MethodSetRequest {
            configured,
            access,
            price_overrides,
        } = methods;

        let dataset = self.shared_dataset();
        let db = dataset.game_data();
        let evaluator = GoalEvaluator::new(&wants);
        evaluator
            .validate()
            .map_err(|error| AppError::new(AppErrorCode::InvalidGoal, format!("{error:#}")))?;
        validate_method_specs(&configured).map_err(|error| {
            AppError::new(
                AppErrorCode::MethodConfigurationInvalid,
                format!("{error:#}"),
            )
        })?;
        evaluator
            .validate_against_db(db)
            .map_err(|error| AppError::new(AppErrorCode::InvalidGoal, format!("{error:#}")))?;
        let goal_warnings = evaluator.selector_warnings(db);

        let imported_start = matches!(&starting_item, StartingItemRequest::ImportedText { .. });
        let (
            resolved_base,
            imported_initial,
            described_item,
            import_warnings,
            report_starting_item,
        ) = match starting_item {
            StartingItemRequest::ImportedText {
                text,
                fallback_item_level,
            } => {
                let imported = import_item_text(
                    &text,
                    db,
                    ImportOptions {
                        fallback_item_level,
                        strict: true,
                    },
                )
                .map_err(|error| {
                    AppError::new(AppErrorCode::ItemImportFailed, format!("{error:#}"))
                })?;
                let resolved_base = self.resolve_owned_base_for_prepare(&imported.base_name)?;
                let initial = build_imported_state(
                    &imported,
                    resolved_base.id.clone(),
                    resolved_base.tags.clone(),
                    db,
                )
                .map_err(|error| {
                    AppError::new(AppErrorCode::InvalidStartingItem, format!("{error:#}"))
                })?;
                let import_warnings = imported.warnings.clone();
                (resolved_base, Some(initial), None, import_warnings, true)
            }
            StartingItemRequest::Described(item) => {
                if item.base.trim().is_empty() {
                    return Err(AppError::new(
                        AppErrorCode::InvalidStartingItem,
                        "[item] base must not be empty".to_string(),
                    ));
                }
                if !(1..=100).contains(&item.item_level) {
                    return Err(AppError::new(
                        AppErrorCode::InvalidStartingItem,
                        "[item] item_level must be between 1 and 100".to_string(),
                    ));
                }
                let resolved_base = self.resolve_owned_base_for_prepare(&item.base)?;
                let report_starting_item = !item.mods.is_empty();
                (
                    resolved_base,
                    None,
                    Some(item),
                    Vec::new(),
                    report_starting_item,
                )
            }
        };

        validate_search(search)?;

        let item_level = imported_initial.as_ref().map_or_else(
            || described_item.as_ref().map_or(0, |item| item.item_level),
            |item| item.item_level,
        );
        let base_item = db.base_items.get(&resolved_base.id).ok_or_else(|| {
            AppError::new(
                AppErrorCode::PreparedDataMismatch,
                format!(
                    "Resolved base item '{}' is absent from the preparation data set",
                    resolved_base.id
                ),
            )
        })?;

        let mut effective_methods = self.legacy_default_methods();
        let mut configured_methods = Vec::with_capacity(configured.len());
        for (index, spec) in configured.iter().enumerate() {
            let method = spec
                .build_for_item(db, base_item, item_level)
                .map_err(|error| {
                    AppError::new(
                        AppErrorCode::MethodConfigurationInvalid,
                        format!("{error:#}"),
                    )
                })?;
            if method.cost_chaos() <= 0.0 || !method.cost_chaos().is_finite() {
                return Err(AppError::new(
                    AppErrorCode::MethodConfigurationInvalid,
                    format!("[[methods]] entry {index}: cost must be a positive finite number"),
                ));
            }
            configured_methods.push(method.metadata());
            effective_methods.push(method);
        }

        for method in &effective_methods {
            if method.cost_chaos() <= 0.0 || !method.cost_chaos().is_finite() {
                return Err(AppError::new(
                    AppErrorCode::MethodConfigurationInvalid,
                    format!(
                        "Crafting method '{}' cost must be a positive finite number",
                        method.name()
                    ),
                ));
            }
        }

        let mut method_names = HashSet::with_capacity(effective_methods.len());
        for method in &effective_methods {
            if !method_names.insert(method.name()) {
                return Err(AppError::new(
                    AppErrorCode::DuplicateMethod,
                    format!(
                        "Duplicate crafting method name '{}'; give configured methods unique names",
                        method.name()
                    ),
                ));
            }
        }

        let mut methods_by_id = HashMap::with_capacity(effective_methods.len());
        for method in &effective_methods {
            let id = method.id();
            if let Some(existing_name) = methods_by_id.insert(id.clone(), method.name()) {
                return Err(AppError::new(
                    AppErrorCode::DuplicateMethod,
                    format!(
                        "Duplicate crafting method ID '{id}' for '{existing_name}' and '{}'; configure each semantic operation at most once",
                        method.name()
                    ),
                ));
            }
        }

        let effective_methods =
            apply_method_access(effective_methods, &mut configured_methods, access)?;
        let unpriced_methods = effective_methods.clone();
        let priced = apply_price_overrides(effective_methods, &price_overrides)?;
        let methods = priced.methods;
        let effective_methods = methods.iter().map(|method| method.metadata()).collect();
        // Preserve the CLI's established failure precedence: imported state is
        // built before method configuration, while a described state is built
        // after methods and prices have been resolved.
        let initial_state = match (imported_initial, described_item) {
            (Some(initial), None) => initial,
            (None, Some(item)) => item
                .build_state(resolved_base.id.clone(), resolved_base.tags.clone(), db)
                .map_err(|error| {
                    AppError::new(AppErrorCode::InvalidStartingItem, format!("{error:#}"))
                })?,
            _ => {
                return Err(AppError::new(
                    AppErrorCode::InvalidStartingItem,
                    "Starting-item preparation produced an invalid internal state".to_string(),
                ))
            }
        };

        let summary = PreparationSummary {
            data_provenance: dataset.data_provenance().clone(),
            base_id: resolved_base.id,
            base_name: resolved_base.name,
            item_level: initial_state.item_level,
            imported_start,
            import_warnings,
            base_warnings: resolved_base.warnings,
            goal_warnings,
            impossible_reasons: analyze_impossible_goals(
                &wants,
                &initial_state,
                db,
                &methods
                    .iter()
                    .flat_map(|method| method.provided_mod_ids())
                    .map(str::to_string)
                    .collect(),
            ),
            search,
            budget,
            effective_methods,
            configured_methods,
            price_book: priced.resolved_prices,
            applied_price_overrides: priced.applied_overrides,
            report_starting_item,
        };

        Ok(PreparedOptimization {
            dataset,
            initial_state,
            wants,
            unpriced_methods,
            methods,
            summary,
        })
    }

    /// Prepare and run one optimization.
    pub fn optimize(&self, request: OptimizeRequest) -> Result<OptimizeResponse, AppError> {
        self.optimize_with_runtime(request, &SearchRuntime::default())
    }

    /// Prepare and run one optimization with non-serialized observer and
    /// cancellation controls.
    pub fn optimize_with_runtime(
        &self,
        request: OptimizeRequest,
        runtime: &SearchRuntime<'_>,
    ) -> Result<OptimizeResponse, AppError> {
        let prepared = self.prepare(request)?;
        self.optimize_prepared_with_runtime(prepared, runtime)
    }

    /// Replace the semantic, ID-keyed price overrides on an already prepared job.
    ///
    /// This supports adapters that must first resolve legacy display-name
    /// prices against the prepared method list. Method order and identity are
    /// unchanged. IDs absent from `price_overrides` return to each method's
    /// original configured/default price.
    pub fn reprice_prepared(
        &self,
        mut prepared: PreparedOptimization,
        price_overrides: PriceBook,
    ) -> Result<PreparedOptimization, AppError> {
        self.ensure_prepared_data(&prepared)?;
        let priced = apply_price_overrides(prepared.unpriced_methods.clone(), &price_overrides)?;
        prepared.methods = priced.methods;
        prepared.summary.applied_price_overrides = priced.applied_overrides;
        prepared.summary.price_book = priced.resolved_prices;
        Ok(prepared)
    }

    /// Run a previously prepared optimization against its original data set.
    pub fn optimize_prepared(
        &self,
        prepared: PreparedOptimization,
    ) -> Result<OptimizeResponse, AppError> {
        self.optimize_prepared_with_runtime(prepared, &SearchRuntime::default())
    }

    /// Run prepared work with non-serialized observer, cancellation, and
    /// adapter safety limits.
    pub fn optimize_prepared_with_runtime(
        &self,
        prepared: PreparedOptimization,
        runtime: &SearchRuntime<'_>,
    ) -> Result<OptimizeResponse, AppError> {
        self.ensure_prepared_data(&prepared)?;

        let PreparedOptimization {
            dataset,
            initial_state,
            wants,
            unpriced_methods: _,
            methods,
            summary,
        } = prepared;
        let db = dataset.game_data();
        let evaluator = GoalEvaluator::new(&wants);
        let starting_evaluation = evaluator.evaluate(&initial_state, db);
        let starting_report = evaluator.report_entries(&initial_state, db);
        let starting_score = starting_evaluation.score;
        let maximum_score = evaluator.maximum_score();
        let initial_for_search = initial_state.clone();
        let resolved_seed = summary
            .search
            .seed
            .unwrap_or_else(|| rand::rng().next_u64());
        let request_limits = SearchLimits {
            expansion_limit: summary.search.expansion_limit,
            timeout: summary.search.timeout_ms.map(Duration::from_millis),
        };
        let effective_limits = strictest_search_limits(request_limits, runtime.limits);
        if !summary.impossible_reasons.is_empty() {
            return Ok(OptimizeResponse {
                outcome: OptimizeOutcome::Impossible,
                resolved_seed,
                effective_limits,
                termination: SearchTermination {
                    reason: SearchTerminationReason::Impossible,
                    snapshot: SearchProgress {
                        elapsed_ms: 0,
                        completed_generations: 0,
                        max_steps: summary.search.max_steps,
                        states_generated: 0,
                        states_retained: 0,
                        current_beam_size: 0,
                        best_score: starting_score,
                        complete_result_existed: false,
                    },
                },
                impossible_reasons: summary.impossible_reasons.clone(),
                summary,
                initial_state,
                starting_evaluation,
                starting_report,
                starting_score,
                maximum_score,
                results: Vec::new(),
            });
        }
        let beam_config = BeamConfig {
            beam_width: summary.search.beam_width,
            max_steps: summary.search.max_steps,
            cost_weight: summary.search.cost_weight,
            restart_cost: summary.search.restart_cost,
            seed: Some(resolved_seed),
        };
        let search = BeamSearch::new(beam_config, db, methods).with_budget(summary.budget);
        let effective_runtime = SearchRuntime {
            observer: runtime.observer,
            cancellation: runtime.cancellation,
            limits: effective_limits,
        };
        let search_run = search.run_k_to_goal_controlled(
            initial_for_search,
            |state| {
                let evaluation = evaluator.evaluate(state, db);
                SearchEvaluation {
                    raw_score: evaluation.score,
                    complete: evaluation.complete(),
                }
            },
            summary.search.top,
            maximum_score,
            &effective_runtime,
        );
        let termination = search_run.termination;
        let results: Vec<EvaluatedSearchResult> = search_run
            .results
            .into_iter()
            .map(|result| {
                let evaluation = evaluator.evaluate(&result.state, db);
                let raw_score = evaluation.score;
                let max_score = maximum_score;
                let satisfied_count = evaluation.satisfied_count;
                let goal_count = wants.len();
                let satisfied_required_count = evaluation.satisfied_required_count;
                let required_goal_count = evaluation.required_goal_count;
                let complete = evaluation.complete();
                let status = if result.budget_comparison == BudgetComparison::Over {
                    debug_assert!(complete, "incomplete over-budget paths are pruned");
                    PathStatus::OverBudget
                } else if complete {
                    PathStatus::Complete
                } else {
                    PathStatus::Incomplete
                };
                let report = evaluator.report_entries(&result.state, db);
                EvaluatedSearchResult {
                    result,
                    raw_score,
                    max_score,
                    satisfied_count,
                    goal_count,
                    satisfied_required_count,
                    required_goal_count,
                    complete,
                    status,
                    report,
                }
            })
            .collect();
        let outcome =
            if results.iter().any(|result| result.complete) || starting_evaluation.complete() {
                OptimizeOutcome::Complete
            } else {
                OptimizeOutcome::Incomplete
            };

        Ok(OptimizeResponse {
            outcome,
            resolved_seed,
            effective_limits,
            termination,
            impossible_reasons: summary.impossible_reasons.clone(),
            summary,
            initial_state,
            starting_evaluation,
            starting_report,
            starting_score,
            maximum_score,
            results,
        })
    }

    fn resolve_owned_base_for_prepare(&self, query: &str) -> Result<OwnedBaseResolution, AppError> {
        let resolution = self.resolve_base_item(query)?;
        Ok(OwnedBaseResolution {
            id: resolution.id.to_string(),
            name: resolution.base_item.name.clone(),
            tags: resolution.base_item.tags.clone(),
            warnings: resolution.warnings,
        })
    }

    fn ensure_prepared_data(&self, prepared: &PreparedOptimization) -> Result<(), AppError> {
        let service_dataset = self.shared_dataset();
        if Arc::ptr_eq(&service_dataset, &prepared.dataset) {
            Ok(())
        } else {
            Err(AppError::new(
                AppErrorCode::PreparedDataMismatch,
                "Prepared optimization belongs to a different GameData instance".to_string(),
            ))
        }
    }
}

fn apply_method_access(
    methods: Vec<Arc<dyn CraftingMethod>>,
    configured_methods: &mut Vec<MethodSummary>,
    access: MethodAccessPolicy,
) -> Result<Vec<Arc<dyn CraftingMethod>>, AppError> {
    match access {
        MethodAccessPolicy::LegacyDefaultsAndConfigured => Ok(methods),
        MethodAccessPolicy::Allowlist(requested_ids) => {
            apply_method_allowlist(methods, configured_methods, requested_ids)
        }
        MethodAccessPolicy::Explicit(selection) => {
            apply_explicit_method_selection(methods, configured_methods, selection)
        }
    }
}

fn apply_method_allowlist(
    methods: Vec<Arc<dyn CraftingMethod>>,
    configured_methods: &mut Vec<MethodSummary>,
    requested_ids: Vec<MethodId>,
) -> Result<Vec<Arc<dyn CraftingMethod>>, AppError> {
    let mut requested: HashSet<MethodId> = HashSet::with_capacity(requested_ids.len());
    for method_id in &requested_ids {
        if !requested.insert(method_id.clone()) {
            return Err(AppError::new(
                AppErrorCode::InvalidMethodAccess,
                format!("Method allowlist contains duplicate ID '{method_id}'"),
            ));
        }
    }

    let available = methods
        .iter()
        .map(|method| method.id())
        .collect::<HashSet<_>>();
    for method_id in &requested_ids {
        if !available.contains(method_id) {
            return Err(AppError::new(
                AppErrorCode::InvalidMethodAccess,
                format!("Method allowlist references unavailable method ID '{method_id}'"),
            ));
        }
    }

    configured_methods.retain(|method| requested.contains(&method.id));
    Ok(methods
        .into_iter()
        .filter(|method| requested.contains(&method.id()))
        .collect())
}

fn apply_explicit_method_selection(
    methods: Vec<Arc<dyn CraftingMethod>>,
    configured_methods: &mut Vec<MethodSummary>,
    selection: MethodSelection,
) -> Result<Vec<Arc<dyn CraftingMethod>>, AppError> {
    let MethodSelection {
        enabled_families,
        enabled_methods,
        disabled_methods,
    } = selection;

    let mut families = HashSet::with_capacity(enabled_families.len());
    for family in &enabled_families {
        if !families.insert(*family) {
            return Err(AppError::new(
                AppErrorCode::InvalidMethodAccess,
                format!(
                    "Method selection contains duplicate family '{}'",
                    family.as_str()
                ),
            ));
        }
    }

    let mut enabled = HashSet::with_capacity(enabled_methods.len());
    for method_id in &enabled_methods {
        if !enabled.insert(method_id.clone()) {
            return Err(AppError::new(
                AppErrorCode::InvalidMethodAccess,
                format!("Method selection contains duplicate enabled ID '{method_id}'"),
            ));
        }
    }

    let mut disabled = HashSet::with_capacity(disabled_methods.len());
    for method_id in &disabled_methods {
        if !disabled.insert(method_id.clone()) {
            return Err(AppError::new(
                AppErrorCode::InvalidMethodAccess,
                format!("Method selection contains duplicate disabled ID '{method_id}'"),
            ));
        }
    }

    for method_id in &enabled_methods {
        if disabled.contains(method_id) {
            return Err(AppError::new(
                AppErrorCode::InvalidMethodAccess,
                format!("Method selection both enables and disables method ID '{method_id}'"),
            ));
        }
    }

    let available_ids = methods
        .iter()
        .map(|method| method.id())
        .collect::<HashSet<_>>();
    let available_families = methods
        .iter()
        .map(|method| method.family())
        .collect::<HashSet<_>>();

    for family in &enabled_families {
        if !available_families.contains(family) {
            return Err(AppError::new(
                AppErrorCode::InvalidMethodAccess,
                format!(
                    "Method selection references unavailable family '{}'",
                    family.as_str()
                ),
            ));
        }
    }
    for method_id in &enabled_methods {
        if !available_ids.contains(method_id) {
            return Err(AppError::new(
                AppErrorCode::InvalidMethodAccess,
                format!("Method selection enables unavailable method ID '{method_id}'"),
            ));
        }
    }
    for method_id in &disabled_methods {
        if !available_ids.contains(method_id) {
            return Err(AppError::new(
                AppErrorCode::InvalidMethodAccess,
                format!("Method selection disables unavailable method ID '{method_id}'"),
            ));
        }
    }

    let is_enabled = |method: &dyn CraftingMethod| {
        (families.contains(&method.family()) || enabled.contains(&method.id()))
            && !disabled.contains(&method.id())
    };
    configured_methods.retain(|method| {
        (families.contains(&method.family) || enabled.contains(&method.id))
            && !disabled.contains(&method.id)
    });
    Ok(methods
        .into_iter()
        .filter(|method| is_enabled(method.as_ref()))
        .collect())
}

fn apply_price_overrides(
    methods: Vec<Arc<dyn CraftingMethod>>,
    price_overrides: &PriceBook,
) -> Result<PricedMethods, AppError> {
    let available = methods
        .iter()
        .map(|method| method.id())
        .collect::<HashSet<_>>();
    for (method_id, _) in price_overrides.iter() {
        if !available.contains(method_id) {
            return Err(AppError::new(
                AppErrorCode::UnknownMethodPrice,
                format!("Price book references unavailable method ID '{method_id}'"),
            ));
        }
    }

    let mut applied = Vec::with_capacity(price_overrides.len());
    let mut resolved = PriceBook::new();
    let methods = methods
        .into_iter()
        .map(|method| {
            let method_id = method.id();
            let method_name = method.name().to_string();
            let effective: Arc<dyn CraftingMethod> =
                if let Some(cost) = price_overrides.get(&method_id) {
                    applied.push(AppliedPriceOverride {
                        method_id: method_id.clone(),
                        method_name,
                        cost,
                    });
                    Arc::new(Repriced {
                        inner: method,
                        cost,
                    })
                } else {
                    method
                };
            resolved
                .set(method_id, effective.cost_chaos())
                .map_err(|error| {
                    AppError::new(AppErrorCode::MethodConfigurationInvalid, error.to_string())
                })?;
            Ok(effective)
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    Ok(PricedMethods {
        methods,
        applied_overrides: applied,
        resolved_prices: resolved,
    })
}

fn validate_search(search: SearchRequest) -> Result<(), AppError> {
    if search.beam_width == 0 {
        return Err(AppError::new(
            AppErrorCode::InvalidSearch,
            "beam_width must be greater than 0".to_string(),
        ));
    }
    if search.max_steps == 0 {
        return Err(AppError::new(
            AppErrorCode::InvalidSearch,
            "max_steps must be greater than 0".to_string(),
        ));
    }
    if search.cost_weight < 0.0 || !search.cost_weight.is_finite() {
        return Err(AppError::new(
            AppErrorCode::InvalidSearch,
            "cost_weight must be a non-negative finite number".to_string(),
        ));
    }
    if search.restart_cost < 0.0 || !search.restart_cost.is_finite() {
        return Err(AppError::new(
            AppErrorCode::InvalidSearch,
            "restart_cost must be a non-negative finite number".to_string(),
        ));
    }
    if search.top == 0 {
        return Err(AppError::new(
            AppErrorCode::InvalidSearch,
            "top must be greater than 0".to_string(),
        ));
    }
    Ok(())
}

fn strictest_search_limits(request: SearchLimits, runtime: SearchLimits) -> SearchLimits {
    SearchLimits {
        expansion_limit: strictest_optional_limit(request.expansion_limit, runtime.expansion_limit),
        timeout: strictest_optional_limit(request.timeout, runtime.timeout),
    }
}

fn strictest_optional_limit<T: Copy + Ord>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}
