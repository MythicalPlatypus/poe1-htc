//! Version 1 machine-readable optimization-response contract.
//!
//! The application service deliberately returns domain objects. This module
//! freezes their public wire representation without adding persistence fields
//! to `ItemState`, goal evaluation, or beam-search nodes.

use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::num::NonZeroU64;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::app::{
    AppWarning, EvaluatedSearchResult, GoalEvaluation, GoalReportEntry, GoalScoringMode,
    ImpossibleGoalReason, MethodSummary, OptimizeOutcome, OptimizeResponse, PathStatus,
};
use crate::currency::{
    ItemClassSupport, MethodCatalog, MethodFamily, MethodSetup, ProbabilityApproximation,
    ProbabilityModel,
};
use crate::data::mods::GenerationType;
use crate::item::modifier::StatRoll;
use crate::item::state::Rarity;
use crate::item::{ItemState, Modifier};
use crate::search::{
    BudgetComparison, BudgetMetric, BudgetPolicy, CostBudgetAssessment, CostValue, PathCosts,
    SearchProgress, SearchTermination, SearchTerminationReason,
};

use super::request_v1::{
    BudgetMetricV1, BudgetV1, DecimalU64V1, DtoConversionError, ItemInfluenceV1, MethodFamilyV1,
    NonNegativeFiniteV1, PositiveFiniteV1, SchemaVersionV1, SearchV1,
    SAVED_OPTIMIZE_REQUEST_SCHEMA_VERSION,
};

/// Numeric schema version emitted by [`SavedOptimizeResponseV1`].
pub const SAVED_OPTIMIZE_RESPONSE_SCHEMA_VERSION: u32 = SAVED_OPTIMIZE_REQUEST_SCHEMA_VERSION;

/// Stable v1 optimization response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SavedOptimizeResponseV1 {
    pub schema_version: SchemaVersionV1,
    pub engine: EngineIdentityV1,
    pub data: DataIdentityV1,
    pub search: SearchV1,
    pub effective_limits: SearchLimitsV1,
    pub budget: BudgetV1,
    pub resolved_seed: DecimalU64V1,
    pub outcome: OptimizeOutcomeV1,
    pub termination: SearchTerminationV1,
    pub methods: Vec<MethodMetadataV1>,
    pub initial_item: ItemStateV1,
    pub starting_goal: GoalEvaluationV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<PathResultV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub impossible_reasons: Vec<ImpossibleGoalReasonV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<DiagnosticV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedOptimizeResponseWireV1 {
    schema_version: SchemaVersionV1,
    engine: EngineIdentityV1,
    data: DataIdentityV1,
    search: SearchV1,
    effective_limits: SearchLimitsV1,
    budget: BudgetV1,
    resolved_seed: DecimalU64V1,
    outcome: OptimizeOutcomeV1,
    termination: SearchTerminationV1,
    methods: Vec<MethodMetadataV1>,
    initial_item: ItemStateV1,
    starting_goal: GoalEvaluationV1,
    #[serde(default)]
    paths: Vec<PathResultV1>,
    #[serde(default)]
    impossible_reasons: Vec<ImpossibleGoalReasonV1>,
    #[serde(default)]
    diagnostics: Vec<DiagnosticV1>,
}

impl<'de> Deserialize<'de> for SavedOptimizeResponseV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = SavedOptimizeResponseWireV1::deserialize(deserializer)?;
        let response = Self {
            schema_version: wire.schema_version,
            engine: wire.engine,
            data: wire.data,
            search: wire.search,
            effective_limits: wire.effective_limits,
            budget: wire.budget,
            resolved_seed: wire.resolved_seed,
            outcome: wire.outcome,
            termination: wire.termination,
            methods: wire.methods,
            initial_item: wire.initial_item,
            starting_goal: wire.starting_goal,
            paths: wire.paths,
            impossible_reasons: wire.impossible_reasons,
            diagnostics: wire.diagnostics,
        };
        response.validate().map_err(de::Error::custom)?;
        Ok(response)
    }
}

impl SavedOptimizeResponseV1 {
    /// Validate relationships that cannot be expressed by derived field-level
    /// deserialization, such as outcome/path consistency and cost-versus-cap
    /// comparisons.
    pub fn validate(&self) -> Result<(), DtoConversionError> {
        validate_goal_evaluation(&self.starting_goal, "/starting_goal")?;
        let mut method_ids = HashSet::new();
        for (index, method) in self.methods.iter().enumerate() {
            if !method_ids.insert(method.id.clone()) {
                return Err(DtoConversionError::new(
                    format!("/methods/{index}/id"),
                    "effective method IDs must be unique",
                ));
            }
        }
        for (index, path) in self.paths.iter().enumerate() {
            let base = format!("/paths/{index}");
            validate_goal_evaluation(&path.goal, &format!("{base}/goal"))?;
            if path.goal.maximum_score != self.starting_goal.maximum_score {
                return Err(DtoConversionError::new(
                    format!("{base}/goal/maximum_score"),
                    "all evaluations in one response must use the same maximum score",
                ));
            }
            validate_path_costs(&path.costs, self.budget, &format!("{base}/costs"))?;
            let selected = selected_cost_metric(&path.costs, self.budget);
            match path.status {
                PathStatusV1::Complete => {
                    if !path.goal.complete {
                        return Err(DtoConversionError::new(
                            format!("{base}/status"),
                            "complete path status requires a complete goal evaluation",
                        ));
                    }
                    if matches!(self.budget, BudgetV1::HardCap { .. })
                        && selected.comparison != BudgetComparisonV1::Under
                    {
                        return Err(DtoConversionError::new(
                            format!("{base}/status"),
                            "complete path under a hard cap must be budget-compliant",
                        ));
                    }
                }
                PathStatusV1::Incomplete => {
                    if path.goal.complete {
                        return Err(DtoConversionError::new(
                            format!("{base}/status"),
                            "incomplete path status conflicts with a complete goal evaluation",
                        ));
                    }
                    if selected.comparison == BudgetComparisonV1::Over {
                        return Err(DtoConversionError::new(
                            format!("{base}/status"),
                            "incomplete over-budget paths are not returned by v1",
                        ));
                    }
                }
                PathStatusV1::OverBudget => {
                    if !path.goal.complete || selected.comparison != BudgetComparisonV1::Over {
                        return Err(DtoConversionError::new(
                            format!("{base}/status"),
                            "over_budget requires goal completion and an over-cap binding metric",
                        ));
                    }
                }
            }
            for (step_index, step) in path.steps.iter().enumerate() {
                if !method_ids.contains(&step.method_id) {
                    return Err(DtoConversionError::new(
                        format!("{base}/steps/{step_index}/method_id"),
                        "path step must reference an effective response method",
                    ));
                }
            }
            if path.steps.is_empty()
                && (path.final_item != self.initial_item
                    || path.goal != self.starting_goal
                    || path.one_shot_success_probability.get() != 1.0
                    || path.costs.first_try.value != CostValue::zero()
                    || path.costs.retry_expected.value != CostValue::zero()
                    || path.costs.restart_adjusted_expected.value != CostValue::zero())
            {
                return Err(DtoConversionError::new(
                    &base,
                    "a zero-step path must preserve the starting item, goal evaluation, unit probability, and zero costs",
                ));
            }
        }

        let returned_completion = self.starting_goal.complete
            || self.paths.iter().any(|path| {
                matches!(
                    path.status,
                    PathStatusV1::Complete | PathStatusV1::OverBudget
                )
            });
        match self.outcome {
            OptimizeOutcomeV1::Complete if !returned_completion => {
                return Err(DtoConversionError::new(
                    "/outcome",
                    "complete outcome requires a complete starting item or returned path",
                ));
            }
            OptimizeOutcomeV1::Incomplete if returned_completion => {
                return Err(DtoConversionError::new(
                    "/outcome",
                    "incomplete outcome conflicts with a complete starting item or path",
                ));
            }
            OptimizeOutcomeV1::Impossible => {
                if self.termination.reason != SearchTerminationReasonV1::Impossible
                    || !self.paths.is_empty()
                    || self.impossible_reasons.is_empty()
                    || self.starting_goal.complete
                {
                    return Err(DtoConversionError::new(
                        "/outcome",
                        "impossible outcome requires an incomplete starting goal, impossible termination, reasons, and no paths",
                    ));
                }
            }
            OptimizeOutcomeV1::Complete | OptimizeOutcomeV1::Incomplete => {}
        }
        if self.termination.reason == SearchTerminationReasonV1::Impossible
            && self.outcome != OptimizeOutcomeV1::Impossible
        {
            return Err(DtoConversionError::new(
                "/termination/reason",
                "impossible termination requires outcome = impossible",
            ));
        }
        self.validate_search_termination(returned_completion)?;
        Ok(())
    }

    fn validate_search_termination(
        &self,
        returned_completion: bool,
    ) -> Result<(), DtoConversionError> {
        let snapshot = &self.termination.snapshot;
        if self.search.beam_width == 0 || self.search.max_steps == 0 || self.search.top == 0 {
            return Err(DtoConversionError::new(
                "/search",
                "beam_width, max_steps, and top must be greater than zero",
            ));
        }
        if snapshot.max_steps != self.search.max_steps {
            return Err(DtoConversionError::new(
                "/termination/snapshot/max_steps",
                "snapshot max_steps must match the executed search settings",
            ));
        }
        if snapshot.completed_generations > snapshot.max_steps {
            return Err(DtoConversionError::new(
                "/termination/snapshot/completed_generations",
                "completed generations cannot exceed max_steps",
            ));
        }
        if snapshot.complete_result_existed != returned_completion {
            return Err(DtoConversionError::new(
                "/termination/snapshot/complete_result_existed",
                "completion snapshot must agree with response outcome and returned results",
            ));
        }
        validate_effective_limit(
            self.search.expansion_limit,
            self.effective_limits.expansion_limit,
            "/effective_limits/expansion_limit",
        )?;
        validate_effective_limit(
            self.search.timeout_ms,
            self.effective_limits.timeout_ms,
            "/effective_limits/timeout_ms",
        )?;

        match self.termination.reason {
            SearchTerminationReasonV1::TargetReached => {
                let maximum = self.starting_goal.maximum_score.ok_or_else(|| {
                    DtoConversionError::new(
                        "/termination/reason",
                        "target_reached requires a known maximum score",
                    )
                })?;
                if !snapshot.complete_result_existed || snapshot.best_score.get() < maximum.get() {
                    return Err(DtoConversionError::new(
                        "/termination/reason",
                        "target_reached requires a complete result at the known maximum score",
                    ));
                }
            }
            SearchTerminationReasonV1::StepLimit => {
                if snapshot.completed_generations != snapshot.max_steps {
                    return Err(DtoConversionError::new(
                        "/termination/reason",
                        "step_limit requires all configured generations to be completed",
                    ));
                }
            }
            SearchTerminationReasonV1::ExpansionLimit => {
                if self.effective_limits.expansion_limit.is_none() {
                    return Err(DtoConversionError::new(
                        "/termination/reason",
                        "expansion_limit termination requires an effective expansion limit",
                    ));
                }
            }
            SearchTerminationReasonV1::TimedOut => {
                if self.effective_limits.timeout_ms.is_none() {
                    return Err(DtoConversionError::new(
                        "/termination/reason",
                        "timed_out termination requires an effective timeout",
                    ));
                }
            }
            SearchTerminationReasonV1::Impossible => {
                if snapshot.completed_generations != 0
                    || snapshot.states_generated != 0
                    || snapshot.states_retained != 0
                    || snapshot.current_beam_size != 0
                {
                    return Err(DtoConversionError::new(
                        "/termination/snapshot",
                        "pre-search impossible termination must have zero search activity",
                    ));
                }
            }
            SearchTerminationReasonV1::Cancelled | SearchTerminationReasonV1::SearchExhausted => {}
        }
        Ok(())
    }
}

fn validate_effective_limit(
    requested: Option<u64>,
    effective: Option<u64>,
    path: &str,
) -> Result<(), DtoConversionError> {
    if let Some(requested) = requested {
        match effective {
            Some(effective) if effective <= requested => {}
            _ => {
                return Err(DtoConversionError::new(
                    path,
                    "effective limit must retain or tighten the requested limit",
                ));
            }
        }
    }
    Ok(())
}

/// Engine build facts required to interpret a saved result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineIdentityV1 {
    pub version: NonEmptyStringV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_revision: Option<String>,
}

/// Identity of the exact RePoE inputs used by the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataIdentityV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repoe_version: Option<String>,
    pub fingerprint: NonEmptyStringV1,
}

/// Effective runtime limits after request and adapter limits were merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchLimitsV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptimizeOutcomeV1 {
    Complete,
    Incomplete,
    Impossible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathStatusV1 {
    Complete,
    Incomplete,
    OverBudget,
}

/// Search stop reason and its last fully committed snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchTerminationV1 {
    pub reason: SearchTerminationReasonV1,
    pub snapshot: SearchProgressV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchTerminationReasonV1 {
    TargetReached,
    StepLimit,
    ExpansionLimit,
    TimedOut,
    Cancelled,
    SearchExhausted,
    Impossible,
}

/// Progress from the same fully committed generation as returned paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchProgressV1 {
    pub elapsed_ms: u64,
    pub completed_generations: u64,
    pub max_steps: u64,
    pub states_generated: u64,
    pub states_retained: u64,
    pub current_beam_size: u64,
    pub best_score: NonNegativeFiniteV1,
    pub complete_result_existed: bool,
}

/// Effective method metadata, including its sampling model and resolved price.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodMetadataV1 {
    pub id: crate::currency::MethodId,
    pub display_name: String,
    pub family: MethodFamilyV1,
    pub description: String,
    pub effective_price_chaos: PositiveFiniteV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_price_chaos: Option<PositiveFiniteV1>,
    pub price_overridden: bool,
    pub setup: MethodSetupV1,
    pub item_class_support: ItemClassSupportV1,
    pub probability_model: ProbabilityModelV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MethodSetupV1 {
    BuiltIn,
    Configured,
    CatalogOrConfigured { catalog: MethodCatalogV1 },
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodCatalogV1 {
    CraftingBench,
    Essences,
    Fossils,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemClassSupportV1 {
    AnyCraftable,
    CatalogRestricted,
    EldritchArmour,
    InfluenceCompatible,
    Unavailable,
}

/// Exact versus sampled probability behavior for one effective method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProbabilityModelV1 {
    Exact,
    MonteCarlo {
        samples: NonZeroU64,
    },
    MonteCarloWithApproximation {
        samples: NonZeroU64,
        approximation: ProbabilityApproximationV1,
    },
    ExactIdentitySampledRolls,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbabilityApproximationV1 {
    UniformEldritchAffixCount,
}

/// Complete item state suitable for rendering a representative result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemStateV1 {
    pub base_id: String,
    pub base_name: String,
    pub base_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub influences: Vec<ItemInfluenceV1>,
    pub item_level: u32,
    pub rarity: ItemStateRarityV1,
    pub prefixes: Vec<ModifierV1>,
    pub suffixes: Vec<ModifierV1>,
    pub fractured: Vec<ModifierV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crafted_mod: Option<ModifierV1>,
    pub corrupted: bool,
    pub mirrored: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exarch_implicit: Option<ModifierV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eater_implicit: Option<ModifierV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implicits: Vec<ModifierV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enchants: Vec<ModifierV1>,
    pub quality: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sockets: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub displayed_energy_shield: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStateRarityV1 {
    Normal,
    Magic,
    Rare,
    Unique,
}

/// Rolled modifier placed in an explicit, implicit, enchant, or crafted slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModifierV1 {
    pub mod_id: String,
    pub generation_type: GenerationTypeV1,
    pub rolls: Vec<StatRollV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationTypeV1 {
    Prefix,
    Suffix,
    Unique,
    Corrupted,
    Enchantment,
    Blight,
    Monster,
    Tempest,
    SearingExarchImplicit,
    EaterOfWorldsImplicit,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatRollV1 {
    pub stat_id: String,
    pub value: i32,
}

/// Goal aggregate and the individual facts used to explain it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalEvaluationV1 {
    pub score: NonNegativeFiniteV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_score: Option<NonNegativeFiniteV1>,
    pub satisfied_count: u64,
    pub goal_count: u64,
    pub satisfied_required_count: u64,
    pub required_goal_count: u64,
    pub complete: bool,
    pub report: Vec<GoalReportEntryV1>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalReportEntryV1 {
    pub description: String,
    pub required: bool,
    pub satisfied: bool,
    pub scoring_mode: GoalScoringModeV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attained: Option<i64>,
    pub contribution: NonNegativeFiniteV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalScoringModeV1 {
    Presence,
    Threshold,
    PerUnit,
}

/// One returned distinct crafting path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathResultV1 {
    pub status: PathStatusV1,
    pub goal: GoalEvaluationV1,
    pub ranking_score: RankingScoreV1,
    pub final_item: ItemStateV1,
    pub steps: Vec<PathStepV1>,
    pub one_shot_success_probability: ProbabilityV1,
    pub costs: PathCostsV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<DiagnosticV1>,
}

/// One ordered craft application in a returned path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathStepV1 {
    pub method_id: crate::currency::MethodId,
    pub display_name: String,
    pub cost_per_application: CostValue,
    pub probability_at_least_this_good: ProbabilityV1,
    pub repeatable_on_failure: bool,
    pub probability_is_estimate: bool,
}

/// All three expected-cost models and each one's relation to the active cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathCostsV1 {
    pub first_try: CostMetricV1,
    pub retry_expected: CostMetricV1,
    pub restart_adjusted_expected: CostMetricV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostMetricV1 {
    pub value: CostValue,
    pub comparison: BudgetComparisonV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excess: Option<CostValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetComparisonV1 {
    Under,
    Over,
    NotComparable,
}

/// A conservatively proven per-want impossibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpossibleGoalReasonV1 {
    pub code: String,
    pub want_index: u64,
    pub field_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_want_indices: Vec<u64>,
    pub message: String,
}

/// Stable machine-readable warning attached to the response or one path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticV1 {
    pub severity: DiagnosticSeverityV1,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_path: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverityV1 {
    Warning,
}

/// Serialization-safe search ranking score.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RankingScoreV1 {
    Finite { value: FiniteNumberV1 },
    NegativeUnbounded,
}

/// Any finite JSON number, including negative ranking values.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct FiniteNumberV1(f64);

impl FiniteNumberV1 {
    pub fn new(value: f64) -> Option<Self> {
        value
            .is_finite()
            .then_some(Self(if value == 0.0 { 0.0 } else { value }))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Serialize for FiniteNumberV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if !self.0.is_finite() {
            return Err(serde::ser::Error::custom("expected a finite number"));
        }
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for FiniteNumberV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("expected a finite number"))
    }
}

/// Non-empty identity string with surrounding whitespace preserved.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NonEmptyStringV1(String);

impl NonEmptyStringV1 {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.is_empty()).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NonEmptyStringV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for NonEmptyStringV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.0.is_empty() {
            return Err(serde::ser::Error::custom(
                "expected a non-empty identity string",
            ));
        }
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for NonEmptyStringV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("expected a non-empty identity string"))
    }
}

/// Strict finite probability in the closed interval `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ProbabilityV1(f64);

impl ProbabilityV1 {
    pub fn new(value: f64) -> Option<Self> {
        (value.is_finite() && (0.0..=1.0).contains(&value) && !value.is_sign_negative())
            .then_some(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Serialize for ProbabilityV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if Self::new(self.0).is_none() {
            return Err(serde::ser::Error::custom(
                "expected a finite probability between 0 and 1",
            ));
        }
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for ProbabilityV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| {
            de::Error::custom("expected a finite probability between 0 and 1 inclusive")
        })
    }
}

impl TryFrom<&OptimizeResponse> for SavedOptimizeResponseV1 {
    type Error = DtoConversionError;

    fn try_from(response: &OptimizeResponse) -> Result<Self, Self::Error> {
        let search = search_v1(&response.summary.search)?;
        let budget = budget_v1(response.summary.budget)?;
        let overridden_ids = response
            .summary
            .applied_price_overrides
            .iter()
            .map(|price| price.method_id.clone())
            .collect::<HashSet<_>>();
        let methods = response
            .summary
            .effective_methods
            .iter()
            .enumerate()
            .map(|(index, method)| {
                method_metadata_v1(
                    method,
                    response.summary.price_book.get(&method.id),
                    overridden_ids.contains(&method.id),
                    &format!("/methods/{index}"),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let starting_goal = goal_evaluation_v1(
            response.starting_evaluation,
            response.starting_report.len(),
            response.maximum_score,
            &response.starting_report,
            "/starting_goal",
        )?;
        let paths = response
            .results
            .iter()
            .enumerate()
            .map(|(index, result)| {
                path_result_v1(
                    result,
                    &response.summary.base_name,
                    &format!("/paths/{index}"),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let impossible_reasons = response
            .impossible_reasons
            .iter()
            .enumerate()
            .map(|(index, reason)| impossible_reason_v1(reason, index))
            .collect::<Result<Vec<_>, _>>()?;

        let saved = Self {
            schema_version: SchemaVersionV1,
            engine: EngineIdentityV1 {
                version: nonempty("/engine/version", env!("CARGO_PKG_VERSION"))?,
                build_revision: option_env!("POE1_HTC_BUILD_REVISION")
                    .filter(|revision| !revision.is_empty())
                    .map(str::to_owned),
            },
            data: DataIdentityV1 {
                repoe_version: response
                    .summary
                    .data_provenance
                    .repoe_version()
                    .map(str::to_owned),
                fingerprint: nonempty(
                    "/data/fingerprint",
                    response.summary.data_provenance.fingerprint().as_str(),
                )?,
            },
            resolved_seed: DecimalU64V1::new(response.resolved_seed),
            search,
            effective_limits: search_limits_v1(response.effective_limits)?,
            budget,
            outcome: response.outcome.into(),
            termination: search_termination_v1(&response.termination)?,
            methods,
            initial_item: item_state_v1(&response.initial_state, &response.summary.base_name),
            starting_goal,
            paths,
            impossible_reasons,
            diagnostics: response_diagnostics(response),
        };
        saved.validate()?;
        Ok(saved)
    }
}

fn search_v1(search: &crate::app::SearchRequest) -> Result<SearchV1, DtoConversionError> {
    Ok(SearchV1 {
        beam_width: usize_u64("/search/beam_width", search.beam_width)?,
        max_steps: usize_u64("/search/max_steps", search.max_steps)?,
        cost_weight: nonnegative("/search/cost_weight", search.cost_weight)?,
        restart_cost: nonnegative("/search/restart_cost", search.restart_cost)?,
        seed: search.seed.map(DecimalU64V1::new),
        top: usize_u64("/search/top", search.top)?,
        expansion_limit: search.expansion_limit,
        timeout_ms: search.timeout_ms,
    })
}

fn search_limits_v1(
    limits: crate::search::SearchLimits,
) -> Result<SearchLimitsV1, DtoConversionError> {
    let timeout_ms = limits
        .timeout
        .map(|timeout| {
            u64::try_from(timeout.as_millis()).map_err(|_| {
                DtoConversionError::new(
                    "/effective_limits/timeout_ms",
                    "timeout is outside the v1 unsigned 64-bit millisecond range",
                )
            })
        })
        .transpose()?;
    Ok(SearchLimitsV1 {
        expansion_limit: limits.expansion_limit,
        timeout_ms,
    })
}

fn budget_v1(budget: BudgetPolicy) -> Result<BudgetV1, DtoConversionError> {
    let metric = budget_metric_v1(budget.metric());
    match budget.hard_cap_chaos() {
        Some(amount) => Ok(BudgetV1::HardCap {
            amount_chaos: nonnegative("/budget/amount_chaos", amount)?,
            metric,
        }),
        None => Ok(BudgetV1::Unbounded { metric }),
    }
}

fn method_metadata_v1(
    method: &MethodSummary,
    effective_price: Option<f64>,
    price_overridden: bool,
    path: &str,
) -> Result<MethodMetadataV1, DtoConversionError> {
    let effective_price = effective_price.ok_or_else(|| {
        DtoConversionError::new(
            format!("{path}/effective_price_chaos"),
            "effective method has no resolved price",
        )
    })?;
    Ok(MethodMetadataV1 {
        id: method.id.clone(),
        display_name: method.display_name.clone(),
        family: method_family_v1(method.family),
        description: method.description.clone(),
        effective_price_chaos: positive(&format!("{path}/effective_price_chaos"), effective_price)?,
        default_price_chaos: method
            .default_price_chaos
            .map(|price| positive(&format!("{path}/default_price_chaos"), price))
            .transpose()?,
        price_overridden,
        setup: method_setup_v1(method.setup),
        item_class_support: item_class_support_v1(method.item_class_support),
        probability_model: probability_model_v1(
            method.probability_model,
            &format!("{path}/probability_model"),
        )?,
    })
}

fn path_result_v1(
    evaluated: &EvaluatedSearchResult,
    base_name: &str,
    path: &str,
) -> Result<PathResultV1, DtoConversionError> {
    let result = &evaluated.result;
    let goal = goal_evaluation_v1(
        GoalEvaluation {
            score: evaluated.raw_score,
            satisfied_count: evaluated.satisfied_count,
            required_goal_count: evaluated.required_goal_count,
            satisfied_required_count: evaluated.satisfied_required_count,
        },
        evaluated.goal_count,
        evaluated.max_score,
        &evaluated.report,
        &format!("{path}/goal"),
    )?;
    let steps = result
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            Ok(PathStepV1 {
                method_id: step.method_id.clone(),
                display_name: step.method.clone(),
                cost_per_application: CostValue::from_computed(step.cost).map_err(|error| {
                    DtoConversionError::new(
                        format!("{path}/steps/{index}/cost_per_application"),
                        error.to_string(),
                    )
                })?,
                probability_at_least_this_good: probability(
                    &format!("{path}/steps/{index}/probability_at_least_this_good"),
                    step.p_at_least,
                )?,
                repeatable_on_failure: step.repeatable,
                probability_is_estimate: step.probability_estimate,
            })
        })
        .collect::<Result<Vec<_>, DtoConversionError>>()?;

    Ok(PathResultV1 {
        status: evaluated.status.into(),
        goal,
        ranking_score: ranking_score_v1(&format!("{path}/ranking_score"), result.score)?,
        final_item: item_state_v1(&result.state, base_name),
        steps,
        one_shot_success_probability: probability(
            &format!("{path}/one_shot_success_probability"),
            result.success_prob,
        )?,
        costs: path_costs_v1(result.costs, result.budget_assessments),
        diagnostics: Vec::new(),
    })
}

fn goal_evaluation_v1(
    evaluation: GoalEvaluation,
    goal_count: usize,
    maximum_score: Option<f64>,
    report: &[GoalReportEntry],
    path: &str,
) -> Result<GoalEvaluationV1, DtoConversionError> {
    Ok(GoalEvaluationV1 {
        score: nonnegative(&format!("{path}/score"), evaluation.score)?,
        maximum_score: maximum_score
            .map(|score| nonnegative(&format!("{path}/maximum_score"), score))
            .transpose()?,
        satisfied_count: usize_u64(
            &format!("{path}/satisfied_count"),
            evaluation.satisfied_count,
        )?,
        goal_count: usize_u64(&format!("{path}/goal_count"), goal_count)?,
        satisfied_required_count: usize_u64(
            &format!("{path}/satisfied_required_count"),
            evaluation.satisfied_required_count,
        )?,
        required_goal_count: usize_u64(
            &format!("{path}/required_goal_count"),
            evaluation.required_goal_count,
        )?,
        complete: evaluation.complete(),
        report: report
            .iter()
            .enumerate()
            .map(|(index, entry)| goal_report_entry_v1(entry, &format!("{path}/report/{index}")))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn goal_report_entry_v1(
    entry: &GoalReportEntry,
    path: &str,
) -> Result<GoalReportEntryV1, DtoConversionError> {
    Ok(GoalReportEntryV1 {
        description: entry.description.clone(),
        required: entry.required,
        satisfied: entry.satisfied,
        scoring_mode: entry.scoring_mode.into(),
        attained: entry.attained,
        contribution: nonnegative(&format!("{path}/contribution"), entry.contribution)?,
    })
}

fn search_termination_v1(
    termination: &SearchTermination,
) -> Result<SearchTerminationV1, DtoConversionError> {
    Ok(SearchTerminationV1 {
        reason: termination.reason.into(),
        snapshot: search_progress_v1(&termination.snapshot)?,
    })
}

fn search_progress_v1(progress: &SearchProgress) -> Result<SearchProgressV1, DtoConversionError> {
    Ok(SearchProgressV1 {
        elapsed_ms: progress.elapsed_ms,
        completed_generations: usize_u64(
            "/termination/snapshot/completed_generations",
            progress.completed_generations,
        )?,
        max_steps: usize_u64("/termination/snapshot/max_steps", progress.max_steps)?,
        states_generated: progress.states_generated,
        states_retained: progress.states_retained,
        current_beam_size: usize_u64(
            "/termination/snapshot/current_beam_size",
            progress.current_beam_size,
        )?,
        best_score: nonnegative("/termination/snapshot/best_score", progress.best_score)?,
        complete_result_existed: progress.complete_result_existed,
    })
}

fn path_costs_v1(
    costs: PathCosts,
    assessments: crate::search::PathBudgetAssessments,
) -> PathCostsV1 {
    PathCostsV1 {
        first_try: cost_metric_v1(costs.first_try, assessments.first_try),
        retry_expected: cost_metric_v1(costs.retry_expected, assessments.retry_expected),
        restart_adjusted_expected: cost_metric_v1(
            costs.restart_adjusted_expected,
            assessments.restart_adjusted_expected,
        ),
    }
}

fn cost_metric_v1(value: CostValue, assessment: CostBudgetAssessment) -> CostMetricV1 {
    CostMetricV1 {
        value,
        comparison: assessment.comparison.into(),
        excess: assessment.excess,
    }
}

fn validate_goal_evaluation(
    evaluation: &GoalEvaluationV1,
    path: &str,
) -> Result<(), DtoConversionError> {
    let goal_count = usize_u64(&format!("{path}/goal_count"), evaluation.report.len())?;
    let satisfied_count = usize_u64(
        &format!("{path}/satisfied_count"),
        evaluation
            .report
            .iter()
            .filter(|entry| entry.satisfied)
            .count(),
    )?;
    let required_goal_count = usize_u64(
        &format!("{path}/required_goal_count"),
        evaluation
            .report
            .iter()
            .filter(|entry| entry.required)
            .count(),
    )?;
    let satisfied_required_count = usize_u64(
        &format!("{path}/satisfied_required_count"),
        evaluation
            .report
            .iter()
            .filter(|entry| entry.required && entry.satisfied)
            .count(),
    )?;
    if evaluation.goal_count != goal_count {
        return Err(DtoConversionError::new(
            format!("{path}/goal_count"),
            "goal_count does not match the report length",
        ));
    }
    if evaluation.satisfied_count != satisfied_count {
        return Err(DtoConversionError::new(
            format!("{path}/satisfied_count"),
            "satisfied_count does not match the report",
        ));
    }
    if evaluation.required_goal_count != required_goal_count {
        return Err(DtoConversionError::new(
            format!("{path}/required_goal_count"),
            "required_goal_count does not match the report",
        ));
    }
    if evaluation.satisfied_required_count != satisfied_required_count {
        return Err(DtoConversionError::new(
            format!("{path}/satisfied_required_count"),
            "satisfied_required_count does not match the report",
        ));
    }
    if evaluation.complete != (satisfied_required_count == required_goal_count) {
        return Err(DtoConversionError::new(
            format!("{path}/complete"),
            "complete does not match required-goal satisfaction",
        ));
    }
    if evaluation
        .maximum_score
        .is_some_and(|maximum| evaluation.score.get() > maximum.get())
    {
        return Err(DtoConversionError::new(
            format!("{path}/maximum_score"),
            "maximum_score cannot be lower than the attained score",
        ));
    }
    Ok(())
}

fn validate_path_costs(
    costs: &PathCostsV1,
    budget: BudgetV1,
    path: &str,
) -> Result<(), DtoConversionError> {
    let cap = match budget {
        BudgetV1::Unbounded { .. } => None,
        BudgetV1::HardCap { amount_chaos, .. } => Some(amount_chaos.get()),
    };
    for (name, metric) in [
        ("first_try", &costs.first_try),
        ("retry_expected", &costs.retry_expected),
        (
            "restart_adjusted_expected",
            &costs.restart_adjusted_expected,
        ),
    ] {
        let expected_comparison = metric
            .value
            .compare_to_budget(cap)
            .map(BudgetComparisonV1::from)
            .map_err(|error| {
                DtoConversionError::new(format!("{path}/{name}/comparison"), error.to_string())
            })?;
        let expected_excess = metric.value.budget_excess(cap).map_err(|error| {
            DtoConversionError::new(format!("{path}/{name}/excess"), error.to_string())
        })?;
        if metric.comparison != expected_comparison {
            return Err(DtoConversionError::new(
                format!("{path}/{name}/comparison"),
                "comparison does not match the active hard cap",
            ));
        }
        if metric.excess != expected_excess {
            return Err(DtoConversionError::new(
                format!("{path}/{name}/excess"),
                "excess does not match the value and active hard cap",
            ));
        }
    }
    Ok(())
}

fn selected_cost_metric(costs: &PathCostsV1, budget: BudgetV1) -> &CostMetricV1 {
    let metric = match budget {
        BudgetV1::Unbounded { metric } | BudgetV1::HardCap { metric, .. } => metric,
    };
    match metric {
        BudgetMetricV1::FirstTry => &costs.first_try,
        BudgetMetricV1::RetryExpected => &costs.retry_expected,
        BudgetMetricV1::RestartAdjustedExpected => &costs.restart_adjusted_expected,
    }
}

fn item_state_v1(state: &ItemState, base_name: &str) -> ItemStateV1 {
    ItemStateV1 {
        base_id: state.base_id.clone(),
        base_name: base_name.to_string(),
        base_tags: state.base_tags.clone(),
        influences: item_influences_v1(&state.base_tags),
        item_level: state.item_level,
        rarity: (&state.rarity).into(),
        prefixes: state.prefixes.iter().map(ModifierV1::from).collect(),
        suffixes: state.suffixes.iter().map(ModifierV1::from).collect(),
        fractured: state.fractured.iter().map(ModifierV1::from).collect(),
        crafted_mod: state.crafted_mod.as_ref().map(ModifierV1::from),
        corrupted: state.corrupted,
        mirrored: state.mirrored,
        exarch_implicit: state.exarch_implicit.as_ref().map(ModifierV1::from),
        eater_implicit: state.eater_implicit.as_ref().map(ModifierV1::from),
        implicits: state.implicits.iter().map(ModifierV1::from).collect(),
        enchants: state.enchants.iter().map(ModifierV1::from).collect(),
        quality: state.quality,
        sockets: state.sockets.clone(),
        displayed_energy_shield: state.displayed_energy_shield,
    }
}

fn impossible_reason_v1(
    reason: &ImpossibleGoalReason,
    reason_index: usize,
) -> Result<ImpossibleGoalReasonV1, DtoConversionError> {
    let base = format!("/impossible_reasons/{reason_index}");
    Ok(ImpossibleGoalReasonV1 {
        code: reason.code.as_str().to_string(),
        want_index: usize_u64(&format!("{base}/want_index"), reason.want_index)?,
        field_path: reason.field_path.clone(),
        related_want_indices: reason
            .related_want_indices
            .iter()
            .enumerate()
            .map(|(index, want_index)| {
                usize_u64(&format!("{base}/related_want_indices/{index}"), *want_index)
            })
            .collect::<Result<Vec<_>, _>>()?,
        message: reason.message.clone(),
    })
}

fn response_diagnostics(response: &OptimizeResponse) -> Vec<DiagnosticV1> {
    let mut diagnostics = Vec::new();
    diagnostics.extend(
        response
            .summary
            .import_warnings
            .iter()
            .map(|warning| DiagnosticV1 {
                severity: DiagnosticSeverityV1::Warning,
                code: warning.code.clone(),
                field_path: Some("/starting_item/text".to_string()),
                message: warning.message.clone(),
                related_ids: Vec::new(),
            }),
    );
    diagnostics.extend(
        response
            .summary
            .base_warnings
            .iter()
            .map(|warning| app_warning_v1(warning, "/starting_item")),
    );
    diagnostics.extend(
        response
            .summary
            .goal_warnings
            .iter()
            .map(|warning| DiagnosticV1 {
                severity: DiagnosticSeverityV1::Warning,
                code: warning.code.as_str().to_string(),
                field_path: Some(warning.field_path.clone()),
                message: warning.message.clone(),
                related_ids: warning.matching_mod_ids.clone(),
            }),
    );
    let search_warnings = response
        .results
        .iter()
        .flat_map(|result| result.result.warnings.iter())
        .cloned()
        .collect::<BTreeSet<_>>();
    diagnostics.extend(search_warnings.into_iter().map(|warning| DiagnosticV1 {
        severity: DiagnosticSeverityV1::Warning,
        code: "craft_branch_error".to_string(),
        field_path: None,
        message: warning,
        related_ids: Vec::new(),
    }));
    diagnostics
}

fn item_influences_v1(base_tags: &[String]) -> Vec<ItemInfluenceV1> {
    [
        ("shaper_item", ItemInfluenceV1::Shaper),
        ("elder_item", ItemInfluenceV1::Elder),
        ("crusader_item", ItemInfluenceV1::Crusader),
        ("hunter_item", ItemInfluenceV1::Hunter),
        ("redeemer_item", ItemInfluenceV1::Redeemer),
        ("warlord_item", ItemInfluenceV1::Warlord),
    ]
    .into_iter()
    .filter(|(tag, _)| base_tags.iter().any(|candidate| candidate == tag))
    .map(|(_, influence)| influence)
    .collect()
}

fn app_warning_v1(warning: &AppWarning, field_path: &str) -> DiagnosticV1 {
    DiagnosticV1 {
        severity: DiagnosticSeverityV1::Warning,
        code: warning.code().as_str().to_string(),
        field_path: Some(field_path.to_string()),
        message: warning.message().to_string(),
        related_ids: Vec::new(),
    }
}

fn method_setup_v1(setup: MethodSetup) -> MethodSetupV1 {
    match setup {
        MethodSetup::BuiltIn => MethodSetupV1::BuiltIn,
        MethodSetup::Configured => MethodSetupV1::Configured,
        MethodSetup::CatalogOrConfigured(catalog) => MethodSetupV1::CatalogOrConfigured {
            catalog: method_catalog_v1(catalog),
        },
        MethodSetup::Unsupported => MethodSetupV1::Unsupported,
    }
}

fn method_catalog_v1(catalog: MethodCatalog) -> MethodCatalogV1 {
    match catalog {
        MethodCatalog::CraftingBench => MethodCatalogV1::CraftingBench,
        MethodCatalog::Essences => MethodCatalogV1::Essences,
        MethodCatalog::Fossils => MethodCatalogV1::Fossils,
    }
}

fn item_class_support_v1(support: ItemClassSupport) -> ItemClassSupportV1 {
    match support {
        ItemClassSupport::AnyCraftable => ItemClassSupportV1::AnyCraftable,
        ItemClassSupport::CatalogRestricted => ItemClassSupportV1::CatalogRestricted,
        ItemClassSupport::EldritchArmour => ItemClassSupportV1::EldritchArmour,
        ItemClassSupport::InfluenceCompatible => ItemClassSupportV1::InfluenceCompatible,
        ItemClassSupport::Unavailable => ItemClassSupportV1::Unavailable,
    }
}

fn probability_model_v1(
    model: ProbabilityModel,
    path: &str,
) -> Result<ProbabilityModelV1, DtoConversionError> {
    Ok(match model {
        ProbabilityModel::Exact => ProbabilityModelV1::Exact,
        ProbabilityModel::MonteCarlo { samples } => ProbabilityModelV1::MonteCarlo {
            samples: positive_samples(&format!("{path}/samples"), samples)?,
        },
        ProbabilityModel::MonteCarloWithApproximation {
            samples,
            approximation,
        } => ProbabilityModelV1::MonteCarloWithApproximation {
            samples: positive_samples(&format!("{path}/samples"), samples)?,
            approximation: match approximation {
                ProbabilityApproximation::UniformEldritchAffixCount => {
                    ProbabilityApproximationV1::UniformEldritchAffixCount
                }
            },
        },
        ProbabilityModel::ExactIdentitySampledRolls => {
            ProbabilityModelV1::ExactIdentitySampledRolls
        }
        ProbabilityModel::Unavailable => ProbabilityModelV1::Unavailable,
    })
}

fn method_family_v1(family: MethodFamily) -> MethodFamilyV1 {
    match family {
        MethodFamily::Currency => MethodFamilyV1::Currency,
        MethodFamily::Bench => MethodFamilyV1::Bench,
        MethodFamily::Essence => MethodFamilyV1::Essence,
        MethodFamily::Fossil => MethodFamilyV1::Fossil,
        MethodFamily::Harvest => MethodFamilyV1::Harvest,
        MethodFamily::Eldritch => MethodFamilyV1::Eldritch,
        MethodFamily::Influence => MethodFamilyV1::Influence,
        MethodFamily::Bestiary => MethodFamilyV1::Bestiary,
    }
}

fn budget_metric_v1(metric: BudgetMetric) -> BudgetMetricV1 {
    match metric {
        BudgetMetric::FirstTry => BudgetMetricV1::FirstTry,
        BudgetMetric::RetryExpected => BudgetMetricV1::RetryExpected,
        BudgetMetric::RestartAdjustedExpected => BudgetMetricV1::RestartAdjustedExpected,
    }
}

fn usize_u64(path: &str, value: usize) -> Result<u64, DtoConversionError> {
    u64::try_from(value)
        .map_err(|_| DtoConversionError::new(path, "value is outside the v1 unsigned 64-bit range"))
}

fn positive_samples(path: &str, value: usize) -> Result<NonZeroU64, DtoConversionError> {
    let value = usize_u64(path, value)?;
    NonZeroU64::new(value)
        .ok_or_else(|| DtoConversionError::new(path, "sample count must be greater than zero"))
}

fn nonempty(path: &str, value: &str) -> Result<NonEmptyStringV1, DtoConversionError> {
    NonEmptyStringV1::new(value.to_string())
        .ok_or_else(|| DtoConversionError::new(path, "identity string must not be empty"))
}

fn nonnegative(path: &str, value: f64) -> Result<NonNegativeFiniteV1, DtoConversionError> {
    let value = if value == 0.0 { 0.0 } else { value };
    NonNegativeFiniteV1::new(value)
        .ok_or_else(|| DtoConversionError::new(path, "expected a non-negative finite number"))
}

fn positive(path: &str, value: f64) -> Result<PositiveFiniteV1, DtoConversionError> {
    PositiveFiniteV1::new(value)
        .ok_or_else(|| DtoConversionError::new(path, "expected a positive finite number"))
}

fn probability(path: &str, value: f64) -> Result<ProbabilityV1, DtoConversionError> {
    let value = if value == 0.0 { 0.0 } else { value };
    ProbabilityV1::new(value).ok_or_else(|| {
        DtoConversionError::new(path, "expected a finite probability between 0 and 1")
    })
}

fn ranking_score_v1(path: &str, value: f64) -> Result<RankingScoreV1, DtoConversionError> {
    if let Some(value) = FiniteNumberV1::new(value) {
        Ok(RankingScoreV1::Finite { value })
    } else if value == f64::NEG_INFINITY {
        Ok(RankingScoreV1::NegativeUnbounded)
    } else {
        Err(DtoConversionError::new(
            path,
            "ranking score must be finite or negative-unbounded",
        ))
    }
}

impl From<OptimizeOutcome> for OptimizeOutcomeV1 {
    fn from(value: OptimizeOutcome) -> Self {
        match value {
            OptimizeOutcome::Complete => Self::Complete,
            OptimizeOutcome::Incomplete => Self::Incomplete,
            OptimizeOutcome::Impossible => Self::Impossible,
        }
    }
}

impl From<PathStatus> for PathStatusV1 {
    fn from(value: PathStatus) -> Self {
        match value {
            PathStatus::Complete => Self::Complete,
            PathStatus::Incomplete => Self::Incomplete,
            PathStatus::OverBudget => Self::OverBudget,
        }
    }
}

impl From<SearchTerminationReason> for SearchTerminationReasonV1 {
    fn from(value: SearchTerminationReason) -> Self {
        match value {
            SearchTerminationReason::TargetReached => Self::TargetReached,
            SearchTerminationReason::StepLimit => Self::StepLimit,
            SearchTerminationReason::ExpansionLimit => Self::ExpansionLimit,
            SearchTerminationReason::TimedOut => Self::TimedOut,
            SearchTerminationReason::Cancelled => Self::Cancelled,
            SearchTerminationReason::SearchExhausted => Self::SearchExhausted,
            SearchTerminationReason::Impossible => Self::Impossible,
        }
    }
}

impl From<BudgetComparison> for BudgetComparisonV1 {
    fn from(value: BudgetComparison) -> Self {
        match value {
            BudgetComparison::Under => Self::Under,
            BudgetComparison::Over => Self::Over,
            BudgetComparison::NotComparable => Self::NotComparable,
        }
    }
}

impl From<GoalScoringMode> for GoalScoringModeV1 {
    fn from(value: GoalScoringMode) -> Self {
        match value {
            GoalScoringMode::Presence => Self::Presence,
            GoalScoringMode::Threshold => Self::Threshold,
            GoalScoringMode::PerUnit => Self::PerUnit,
        }
    }
}

impl From<&Rarity> for ItemStateRarityV1 {
    fn from(value: &Rarity) -> Self {
        match value {
            Rarity::Normal => Self::Normal,
            Rarity::Magic => Self::Magic,
            Rarity::Rare => Self::Rare,
            Rarity::Unique => Self::Unique,
        }
    }
}

impl From<&Modifier> for ModifierV1 {
    fn from(value: &Modifier) -> Self {
        Self {
            mod_id: value.mod_id.clone(),
            generation_type: (&value.generation_type).into(),
            rolls: value.rolls.iter().map(StatRollV1::from).collect(),
        }
    }
}

impl From<&StatRoll> for StatRollV1 {
    fn from(value: &StatRoll) -> Self {
        Self {
            stat_id: value.stat_id.clone(),
            value: value.value,
        }
    }
}

impl From<&GenerationType> for GenerationTypeV1 {
    fn from(value: &GenerationType) -> Self {
        match value {
            GenerationType::Prefix => Self::Prefix,
            GenerationType::Suffix => Self::Suffix,
            GenerationType::Unique => Self::Unique,
            GenerationType::Corrupted => Self::Corrupted,
            GenerationType::Enchantment => Self::Enchantment,
            GenerationType::Blight => Self::Blight,
            GenerationType::Monster => Self::Monster,
            GenerationType::Tempest => Self::Tempest,
            GenerationType::ExarchImplicit => Self::SearingExarchImplicit,
            GenerationType::EaterImplicit => Self::EaterOfWorldsImplicit,
            GenerationType::Unknown => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        EvaluatedSearchResult, PathStatus, PreparationSummary, PriceBook, SearchRequest,
    };
    use crate::currency::{
        ItemClassSupport, MethodFamily, MethodId, MethodMetadata, MethodSetup, ProbabilityModel,
    };
    use crate::data::{DataFingerprint, DataProvenance};
    use crate::goal::{ImpossibleGoalReason, ImpossibleGoalReasonCode};
    use crate::search::beam::PathStep;
    use crate::search::{
        PathBudgetAssessments, SearchResult, SearchTermination, SearchTerminationReason,
    };

    #[test]
    fn probability_rejects_non_finite_and_out_of_range_values() {
        for invalid in [
            -0.0,
            -0.01,
            1.01,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            assert!(ProbabilityV1::new(invalid).is_none());
        }
        assert_eq!(ProbabilityV1::new(0.0).map(ProbabilityV1::get), Some(0.0));
        assert_eq!(ProbabilityV1::new(1.0).map(ProbabilityV1::get), Some(1.0));
    }

    #[test]
    fn strict_probability_json_round_trips_and_rejects_invalid_numbers() {
        let probability = ProbabilityV1::new(0.25).expect("valid probability");
        assert_eq!(serde_json::to_string(&probability).unwrap(), "0.25");
        assert_eq!(
            serde_json::from_str::<ProbabilityV1>("0.25").unwrap().get(),
            0.25
        );
        assert!(serde_json::from_str::<ProbabilityV1>("-0.0").is_err());
        assert!(serde_json::from_str::<ProbabilityV1>("1.1").is_err());
        assert!(serde_json::from_str::<ProbabilityV1>("null").is_err());
    }

    #[test]
    fn domain_response_maps_to_strict_replayable_v1_json() {
        let mut state = ItemState::new_base(
            "Metadata/Items/TestPlate",
            vec!["body_armour".to_string()],
            86,
        );
        state.rarity = Rarity::Rare;
        let report = vec![GoalReportEntry {
            description: "exact test modifier".to_string(),
            required: true,
            satisfied: true,
            scoring_mode: GoalScoringMode::Presence,
            attained: None,
            contribution: 1.0,
        }];
        let evaluation = GoalEvaluation {
            score: 1.0,
            satisfied_count: 1,
            required_goal_count: 1,
            satisfied_required_count: 1,
        };
        let costs = PathCosts {
            first_try: CostValue::finite(0.0).expect("zero is a finite cost"),
            retry_expected: CostValue::finite(0.0).expect("zero is a finite cost"),
            restart_adjusted_expected: CostValue::finite(0.0).expect("zero is a finite cost"),
        };
        let under = CostBudgetAssessment {
            comparison: BudgetComparison::Under,
            excess: None,
        };
        let result = EvaluatedSearchResult {
            result: SearchResult {
                state: state.clone(),
                steps: Vec::new(),
                total_cost: 0.0,
                expected_cost: 0.0,
                success_prob: 1.0,
                score: 1.0,
                restart_cost: 0.0,
                costs,
                budget_assessments: PathBudgetAssessments {
                    first_try: under,
                    retry_expected: under,
                    restart_adjusted_expected: under,
                },
                budget_comparison: BudgetComparison::Under,
                budget_excess: None,
                warnings: Vec::new(),
            },
            raw_score: 1.0,
            max_score: Some(1.0),
            satisfied_count: 1,
            goal_count: 1,
            satisfied_required_count: 1,
            required_goal_count: 1,
            complete: true,
            status: PathStatus::Complete,
            report: report.clone(),
        };
        let search = SearchRequest {
            seed: Some(u64::MAX),
            ..SearchRequest::default()
        };
        let mut response = OptimizeResponse {
            outcome: OptimizeOutcome::Complete,
            resolved_seed: u64::MAX,
            effective_limits: crate::search::SearchLimits {
                expansion_limit: Some(99),
                timeout: Some(std::time::Duration::from_millis(250)),
            },
            termination: SearchTermination {
                reason: SearchTerminationReason::TargetReached,
                snapshot: SearchProgress {
                    elapsed_ms: 7,
                    completed_generations: 0,
                    max_steps: search.max_steps,
                    states_generated: 0,
                    states_retained: 0,
                    current_beam_size: 1,
                    best_score: 1.0,
                    complete_result_existed: true,
                },
            },
            impossible_reasons: Vec::new(),
            summary: PreparationSummary {
                data_provenance: DataProvenance::unversioned(DataFingerprint::sha256_of_bytes(
                    b"response-contract-v1",
                )),
                base_id: state.base_id.clone(),
                base_name: "Test Plate".to_string(),
                item_level: state.item_level,
                imported_start: false,
                import_warnings: Vec::new(),
                base_warnings: Vec::new(),
                goal_warnings: Vec::new(),
                impossible_reasons: Vec::new(),
                search,
                budget: BudgetPolicy::hard_cap(5.0, BudgetMetric::RestartAdjustedExpected)
                    .expect("test cap is valid"),
                effective_methods: Vec::new(),
                configured_methods: Vec::new(),
                price_book: PriceBook::new(),
                applied_price_overrides: Vec::new(),
                report_starting_item: true,
            },
            initial_state: state,
            starting_evaluation: evaluation,
            starting_report: report,
            starting_score: 1.0,
            maximum_score: Some(1.0),
            results: vec![result],
        };

        let dto = SavedOptimizeResponseV1::try_from(&response)
            .expect("valid domain response should map to v1");
        assert_eq!(dto.schema_version, SchemaVersionV1);
        assert_eq!(dto.search.seed.map(DecimalU64V1::get), Some(u64::MAX));
        assert_eq!(dto.resolved_seed.get(), u64::MAX);
        assert_eq!(dto.effective_limits.expansion_limit, Some(99));
        assert_eq!(dto.effective_limits.timeout_ms, Some(250));
        assert_eq!(dto.paths[0].status, PathStatusV1::Complete);
        assert!(matches!(
            dto.paths[0].ranking_score,
            RankingScoreV1::Finite { value } if value.get() == 1.0
        ));
        assert_eq!(
            dto.paths[0].costs.restart_adjusted_expected.comparison,
            BudgetComparisonV1::Under
        );

        let json = serde_json::to_string_pretty(&dto).expect("v1 response should serialize");
        assert!(json.contains(&format!("\"{}\"", u64::MAX)));
        let reparsed: SavedOptimizeResponseV1 =
            serde_json::from_str(&json).expect("serialized v1 response should parse");
        assert_eq!(reparsed, dto);

        let chaos_id = MethodId::parse("currency/chaos").unwrap();
        response.summary.effective_methods = vec![MethodMetadata {
            id: chaos_id.clone(),
            display_name: "Chaos Orb".to_string(),
            family: MethodFamily::Currency,
            description: "Reroll a Rare item".to_string(),
            default_price_chaos: Some(1.0),
            setup: MethodSetup::BuiltIn,
            item_class_support: ItemClassSupport::AnyCraftable,
            probability_model: ProbabilityModel::MonteCarlo { samples: 50 },
        }];
        response
            .summary
            .price_book
            .set(chaos_id.clone(), 1.0)
            .unwrap();
        response.results[0].result.steps = vec![PathStep {
            method_id: chaos_id,
            method: "Chaos Orb".to_string(),
            cost: 1.0,
            p_at_least: 0.25,
            repeatable: true,
            probability_estimate: true,
        }];
        let sampled = SavedOptimizeResponseV1::try_from(&response)
            .expect("sampled domain response should map to v1");
        assert!(matches!(
            sampled.methods[0].probability_model,
            ProbabilityModelV1::MonteCarlo { samples } if samples.get() == 50
        ));
        assert!(sampled.paths[0].steps[0].probability_is_estimate);

        response.summary.budget =
            BudgetPolicy::hard_cap(0.5, BudgetMetric::RestartAdjustedExpected).unwrap();
        response.results[0].status = PathStatus::OverBudget;
        response.results[0].result.costs = PathCosts {
            first_try: CostValue::finite(1.0).unwrap(),
            retry_expected: CostValue::finite(4.0).unwrap(),
            restart_adjusted_expected: CostValue::finite(10.0).unwrap(),
        };
        response.results[0].result.budget_assessments = PathBudgetAssessments {
            first_try: CostBudgetAssessment {
                comparison: BudgetComparison::Over,
                excess: Some(CostValue::finite(0.5).unwrap()),
            },
            retry_expected: CostBudgetAssessment {
                comparison: BudgetComparison::Over,
                excess: Some(CostValue::finite(3.5).unwrap()),
            },
            restart_adjusted_expected: CostBudgetAssessment {
                comparison: BudgetComparison::Over,
                excess: Some(CostValue::finite(9.5).unwrap()),
            },
        };
        response.results[0].result.budget_comparison = BudgetComparison::Over;
        response.results[0].result.budget_excess = Some(CostValue::finite(9.5).unwrap());
        let over_budget = SavedOptimizeResponseV1::try_from(&response)
            .expect("over-budget domain response should map to v1");
        assert_eq!(over_budget.paths[0].status, PathStatusV1::OverBudget);

        response.outcome = OptimizeOutcome::Impossible;
        response.results.clear();
        response.starting_evaluation = GoalEvaluation {
            score: 0.0,
            satisfied_count: 0,
            required_goal_count: 1,
            satisfied_required_count: 0,
        };
        response.starting_report[0].satisfied = false;
        response.starting_report[0].contribution = 0.0;
        response.starting_score = 0.0;
        response.termination = SearchTermination {
            reason: SearchTerminationReason::Impossible,
            snapshot: SearchProgress {
                elapsed_ms: 0,
                completed_generations: 0,
                max_steps: search.max_steps,
                states_generated: 0,
                states_retained: 0,
                current_beam_size: 0,
                best_score: 0.0,
                complete_result_existed: false,
            },
        };
        response.impossible_reasons = vec![ImpossibleGoalReason {
            code: ImpossibleGoalReasonCode::NoReachableModifier,
            want_index: 0,
            field_path: "/goals/0".to_string(),
            related_want_indices: Vec::new(),
            message: "no reachable modifier".to_string(),
        }];
        let impossible = SavedOptimizeResponseV1::try_from(&response)
            .expect("impossible domain response should map to v1");
        assert_eq!(impossible.outcome, OptimizeOutcomeV1::Impossible);
        assert!(impossible.paths.is_empty());
        assert_eq!(impossible.impossible_reasons.len(), 1);
    }
}
