//! Application-service contract tests using only synthetic RePoE data.

use std::collections::HashMap;

use poe1_htc::app::{
    AppErrorCode, BudgetComparison, BudgetMetric, BudgetPolicy, CancellationToken, DataFingerprint,
    DataProvenance, GoalSetRequest, ImpossibleGoalReasonCode, ItemClassSupport, MethodAccessPolicy,
    MethodCatalog, MethodFamily, MethodId, MethodSelection, MethodSetRequest, MethodSetup,
    OptimizeOutcome, OptimizeRequest, OptimizerService, PathStatus, PriceBook, ProbabilityModel,
    SearchRequest, SearchRuntime, SearchTerminationReason, StartingItemRequest,
};
use poe1_htc::data::{
    base_items::BaseItem,
    mods::{Domain, GenerationType, Mod, ModStat, SpawnWeight},
    GameData,
};
use poe1_htc::goal::{GoalEvaluator, ItemSpec, MethodSpec, WantSpec};
use poe1_htc::item::{state::Rarity, ItemState};
use poe1_htc::search::beam::{BeamConfig, BeamSearch, SearchResult};

const PRIMARY_BASE_ID: &str = "Metadata/Items/Armours/BodyArmours/TestPlate";
const IMPORTED_BASE_ID: &str = "Metadata/Items/Armours/BodyArmours/ClipboardPlate";
const CRAFTED_LIFE_ID: &str = "CraftedLife";
const CRAFTED_FIRE_ID: &str = "CraftedFire";

const LEGACY_METHOD_NAMES: [&str; 12] = [
    "Orb of Scouring",
    "Orb of Transmutation",
    "Orb of Alteration",
    "Orb of Augmentation",
    "Regal Orb",
    "Orb of Alchemy",
    "Chaos Orb",
    "Exalted Orb",
    "Orb of Annulment",
    "Divine Orb",
    "Fracturing Orb",
    "Remove Crafted Mods",
];

fn base_item(name: &str) -> BaseItem {
    BaseItem {
        name: name.to_string(),
        item_class: "Body Armour".to_string(),
        tags: vec!["body_armour".to_string(), "default".to_string()],
        implicits: Vec::new(),
        drop_level: 1,
        inventory_height: 3,
        inventory_width: 2,
    }
}

fn modifier(
    name: &str,
    generation_type: GenerationType,
    domain: Domain,
    group: &str,
    stat: &str,
    min: i32,
    max: i32,
) -> Mod {
    Mod {
        name: name.to_string(),
        generation_type,
        required_level: 1,
        stats: vec![ModStat {
            id: stat.to_string(),
            min,
            max,
        }],
        spawn_weights: vec![
            SpawnWeight {
                tag: "body_armour".to_string(),
                weight: 1_000,
            },
            SpawnWeight {
                tag: "default".to_string(),
                weight: 0,
            },
        ],
        generation_weights: Vec::new(),
        adds_tags: Vec::new(),
        tags: Vec::new(),
        domain,
        mod_type: group.to_string(),
        groups: vec![group.to_string()],
        is_essence_only: false,
        text: None,
    }
}

fn synthetic_game_data() -> GameData {
    let base_items = HashMap::from([
        (PRIMARY_BASE_ID.to_string(), base_item("Test Plate")),
        (IMPORTED_BASE_ID.to_string(), base_item("Clipboard Plate")),
    ]);
    let mods = HashMap::from([
        (
            "P1".to_string(),
            modifier(
                "First Prefix",
                GenerationType::Prefix,
                Domain::Item,
                "PrefixOne",
                "stat_p1",
                1,
                10,
            ),
        ),
        (
            "P2".to_string(),
            modifier(
                "Second Prefix",
                GenerationType::Prefix,
                Domain::Item,
                "PrefixTwo",
                "stat_p2",
                11,
                20,
            ),
        ),
        (
            "P3".to_string(),
            modifier(
                "Third Prefix",
                GenerationType::Prefix,
                Domain::Item,
                "PrefixThree",
                "stat_p3",
                21,
                30,
            ),
        ),
        (
            "S1".to_string(),
            modifier(
                "First Suffix",
                GenerationType::Suffix,
                Domain::Item,
                "SuffixOne",
                "stat_s1",
                1,
                10,
            ),
        ),
        (
            "S2".to_string(),
            modifier(
                "Second Suffix",
                GenerationType::Suffix,
                Domain::Item,
                "SuffixTwo",
                "stat_s2",
                11,
                20,
            ),
        ),
        (
            "S3".to_string(),
            modifier(
                "Third Suffix",
                GenerationType::Suffix,
                Domain::Item,
                "SuffixThree",
                "stat_s3",
                21,
                30,
            ),
        ),
        (
            CRAFTED_LIFE_ID.to_string(),
            modifier(
                "Crafted Vitality",
                GenerationType::Prefix,
                Domain::Crafted,
                "CraftedLifeGroup",
                "crafted_life",
                42,
                42,
            ),
        ),
        (
            CRAFTED_FIRE_ID.to_string(),
            modifier(
                "Crafted Embers",
                GenerationType::Suffix,
                Domain::Crafted,
                "CraftedFireGroup",
                "crafted_fire",
                35,
                35,
            ),
        ),
    ]);
    GameData::new(mods, base_items)
}

fn synthetic_provenance() -> DataProvenance {
    DataProvenance::unversioned(DataFingerprint::sha256_of_bytes(
        b"application-service-synthetic-game-data-v1",
    ))
}

fn synthetic_service() -> OptimizerService {
    OptimizerService::new(synthetic_game_data(), synthetic_provenance())
}

fn described_item(base: &str) -> ItemSpec {
    ItemSpec {
        base: base.to_string(),
        item_level: 86,
        rarity: Some("rare".to_string()),
        mods: Vec::new(),
        influences: Vec::new(),
        exarch_implicit: None,
        eater_implicit: None,
    }
}

fn exact_want(mod_id: &str, weight: f64) -> WantSpec {
    WantSpec {
        mod_id: Some(mod_id.to_string()),
        group: None,
        stat: None,
        min_value: None,
        max_value: None,
        mode: None,
        cap: None,
        weight,
        required: true,
    }
}

fn preferred_want(mod_id: &str, weight: f64) -> WantSpec {
    let mut want = exact_want(mod_id, weight);
    want.required = false;
    want
}

fn bench_method(name: &str, mod_id: &str, cost: f64) -> MethodSpec {
    MethodSpec::Bench {
        name: Some(name.to_string()),
        mod_id: mod_id.to_string(),
        cost,
    }
}

fn method_id(value: &str) -> MethodId {
    MethodId::parse(value).expect("test method ID should be valid")
}

fn price_book(entries: &[(&str, f64)]) -> PriceBook {
    PriceBook::try_from_iter(entries.iter().map(|(id, cost)| (method_id(id), *cost)))
        .expect("test prices should be valid")
}

fn fixed_search() -> SearchRequest {
    SearchRequest {
        beam_width: 8,
        max_steps: 2,
        cost_weight: 0.0,
        restart_cost: 1.0,
        seed: Some(7),
        top: 1,
        expansion_limit: None,
        timeout_ms: None,
    }
}

fn valid_request() -> OptimizeRequest {
    OptimizeRequest {
        starting_item: StartingItemRequest::Described(described_item(PRIMARY_BASE_ID)),
        goals: GoalSetRequest {
            wants: vec![exact_want(CRAFTED_LIFE_ID, 4.0)],
        },
        methods: MethodSetRequest {
            configured: vec![bench_method("Bench Life", CRAFTED_LIFE_ID, 2.0)],
            access: MethodAccessPolicy::LegacyDefaultsAndConfigured,
            price_overrides: PriceBook::new(),
        },
        budget: BudgetPolicy::default(),
        search: fixed_search(),
    }
}

fn expected_method_names(configured: &[&str]) -> Vec<String> {
    LEGACY_METHOD_NAMES
        .iter()
        .copied()
        .chain(configured.iter().copied())
        .map(str::to_string)
        .collect()
}

fn assert_state_eq(actual: &ItemState, expected: &ItemState) {
    assert_eq!(actual.base_id, expected.base_id);
    assert_eq!(actual.base_tags, expected.base_tags);
    assert_eq!(actual.item_level, expected.item_level);
    assert_eq!(actual.rarity, expected.rarity);
    assert_eq!(actual.prefixes, expected.prefixes);
    assert_eq!(actual.suffixes, expected.suffixes);
    assert_eq!(actual.fractured, expected.fractured);
    assert_eq!(actual.crafted_mod, expected.crafted_mod);
    assert_eq!(actual.corrupted, expected.corrupted);
    assert_eq!(actual.mirrored, expected.mirrored);
    assert_eq!(actual.exarch_implicit, expected.exarch_implicit);
    assert_eq!(actual.eater_implicit, expected.eater_implicit);
    assert_eq!(actual.implicits, expected.implicits);
    assert_eq!(actual.enchants, expected.enchants);
    assert_eq!(actual.quality, expected.quality);
    assert_eq!(actual.sockets, expected.sockets);
    assert_eq!(
        actual.displayed_energy_shield,
        expected.displayed_energy_shield
    );
}

fn assert_search_result_eq(actual: &SearchResult, expected: &SearchResult) {
    assert_state_eq(&actual.state, &expected.state);
    assert_eq!(actual.steps.len(), expected.steps.len());
    for (actual_step, expected_step) in actual.steps.iter().zip(&expected.steps) {
        assert_eq!(actual_step.method_id, expected_step.method_id);
        assert_eq!(actual_step.method, expected_step.method);
        assert_eq!(actual_step.cost, expected_step.cost);
        assert_eq!(actual_step.p_at_least, expected_step.p_at_least);
        assert_eq!(actual_step.repeatable, expected_step.repeatable);
        assert_eq!(
            actual_step.probability_estimate,
            expected_step.probability_estimate
        );
    }
    assert_eq!(actual.total_cost, expected.total_cost);
    assert_eq!(actual.expected_cost, expected.expected_cost);
    assert_eq!(actual.success_prob, expected.success_prob);
    assert_eq!(actual.score, expected.score);
    assert_eq!(actual.restart_cost, expected.restart_cost);
    assert_eq!(actual.costs, expected.costs);
    assert_eq!(actual.budget_comparison, expected.budget_comparison);
    assert_eq!(actual.budget_excess, expected.budget_excess);
    assert_eq!(actual.warnings, expected.warnings);
}

#[test]
fn described_request_runs_end_to_end_with_evaluated_report() {
    let service = synthetic_service();

    let response = service
        .optimize(valid_request())
        .expect("synthetic request should optimize");

    assert_eq!(response.summary.base_id, PRIMARY_BASE_ID);
    assert_eq!(response.summary.base_name, "Test Plate");
    assert_eq!(response.summary.item_level, 86);
    assert!(!response.summary.imported_start);
    assert_eq!(response.summary.search.seed, Some(7));
    assert_eq!(response.resolved_seed, 7);
    assert_eq!(
        response.termination.reason,
        SearchTerminationReason::TargetReached
    );
    assert_eq!(response.outcome, OptimizeOutcome::Complete);
    assert_eq!(response.starting_score, 0.0);
    assert_eq!(response.results.len(), 1);

    let evaluated = &response.results[0];
    assert_eq!(evaluated.raw_score, 4.0);
    assert_eq!(evaluated.max_score, Some(4.0));
    assert_eq!(evaluated.satisfied_count, 1);
    assert_eq!(evaluated.goal_count, 1);
    assert_eq!(evaluated.satisfied_required_count, 1);
    assert_eq!(evaluated.required_goal_count, 1);
    assert!(evaluated.complete);
    assert_eq!(evaluated.status, PathStatus::Complete);
    assert_eq!(evaluated.report.len(), 1);
    assert_eq!(
        evaluated.report[0].description,
        "mod CraftedLife (weight 4)"
    );
    assert!(evaluated.report[0].required);
    assert!(evaluated.report[0].satisfied);
    assert_eq!(evaluated.result.steps.len(), 1);
    assert_eq!(
        evaluated.result.steps[0].method_id.as_str(),
        "bench/add-explicit/CraftedLife"
    );
    assert_eq!(evaluated.result.steps[0].method, "Bench Life");
    assert_eq!(evaluated.result.steps[0].cost, 2.0);
    let crafted = evaluated
        .result
        .state
        .crafted_mod
        .as_ref()
        .expect("bench result should contain its crafted modifier");
    assert_eq!(crafted.mod_id, CRAFTED_LIFE_ID);
    assert_eq!(crafted.rolls[0].value, 42);
}

#[test]
fn hard_budget_returns_labeled_completion_and_compliant_alternative() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.budget =
        BudgetPolicy::hard_cap(1.0, BudgetMetric::FirstTry).expect("test cap should be valid");

    let response = service
        .optimize(request)
        .expect("budgeted search should run");

    assert_eq!(response.outcome, OptimizeOutcome::Complete);
    assert_eq!(response.summary.budget.hard_cap_chaos(), Some(1.0));
    assert_eq!(response.results.len(), 2);

    let over_budget = response
        .results
        .iter()
        .find(|result| result.status == PathStatus::OverBudget)
        .expect("goal-complete over-budget exemplar should be retained");
    assert!(over_budget.complete);
    assert_eq!(over_budget.result.budget_comparison, BudgetComparison::Over);
    assert_eq!(
        over_budget
            .result
            .budget_excess
            .and_then(|cost| cost.amount_chaos()),
        Some(1.0)
    );

    let compliant = response
        .results
        .iter()
        .find(|result| result.status == PathStatus::Incomplete)
        .expect("best compliant incomplete alternative should be retained");
    assert!(!compliant.complete);
    assert_eq!(compliant.result.budget_comparison, BudgetComparison::Under);
    assert!(compliant.result.steps.is_empty());
}

#[test]
fn exact_hard_cap_is_compliant_and_zero_cap_is_predictable() {
    let service = synthetic_service();

    let mut exact = valid_request();
    exact.budget =
        BudgetPolicy::hard_cap(2.0, BudgetMetric::FirstTry).expect("test cap should be valid");
    let exact_response = service
        .optimize(exact)
        .expect("exact-cap search should run");
    assert_eq!(exact_response.results.len(), 1);
    assert_eq!(exact_response.results[0].status, PathStatus::Complete);
    assert_eq!(
        exact_response.results[0].result.budget_comparison,
        BudgetComparison::Under
    );

    let mut zero = valid_request();
    zero.budget = BudgetPolicy::hard_cap(0.0, BudgetMetric::FirstTry).expect("zero cap is valid");
    let zero_response = service.optimize(zero).expect("zero-cap search should run");
    assert!(zero_response
        .results
        .iter()
        .any(|result| result.status == PathStatus::OverBudget));
    assert!(zero_response
        .results
        .iter()
        .any(|result| result.status == PathStatus::Incomplete));
}

#[test]
fn cancellation_and_expansion_limits_return_normal_service_responses() {
    let service = synthetic_service();
    let token = CancellationToken::new();
    token.cancel();
    let cancelled = service
        .optimize_with_runtime(
            valid_request(),
            &SearchRuntime {
                observer: None,
                cancellation: Some(&token),
                limits: Default::default(),
            },
        )
        .expect("cancellation is a normal response");
    assert_eq!(
        cancelled.termination.reason,
        SearchTerminationReason::Cancelled
    );
    assert_eq!(cancelled.outcome, OptimizeOutcome::Incomplete);
    assert_eq!(cancelled.results.len(), 1);
    assert!(cancelled.results[0].result.steps.is_empty());

    let mut limited_request = valid_request();
    limited_request.search.expansion_limit = Some(0);
    let limited = service
        .optimize(limited_request)
        .expect("expansion limiting is a normal response");
    assert_eq!(
        limited.termination.reason,
        SearchTerminationReason::ExpansionLimit
    );
    assert_eq!(limited.termination.snapshot.completed_generations, 0);
    assert_eq!(limited.termination.snapshot.states_generated, 0);
}

#[test]
fn provably_unreachable_required_goal_returns_impossible_response() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.goals.wants = vec![exact_want(CRAFTED_FIRE_ID, 1.0)];

    let response = service
        .optimize(request)
        .expect("proven impossibility is a normal response");

    assert_eq!(response.outcome, OptimizeOutcome::Impossible);
    assert_eq!(
        response.termination.reason,
        SearchTerminationReason::Impossible
    );
    assert!(response.results.is_empty());
    assert_eq!(response.impossible_reasons.len(), 1);
    assert_eq!(
        response.impossible_reasons[0].code,
        ImpossibleGoalReasonCode::NoReachableModifier
    );
    assert_eq!(response.impossible_reasons[0].want_index, 0);
    assert_eq!(
        response.summary.impossible_reasons,
        response.impossible_reasons
    );
}

#[test]
fn omitted_seed_is_resolved_once_and_replays_exactly() {
    let service = synthetic_service();
    let mut unseeded = valid_request();
    unseeded.search.seed = None;
    let first = service
        .optimize(unseeded)
        .expect("unseeded request should resolve a run seed");

    let mut replay = valid_request();
    replay.search.seed = Some(first.resolved_seed);
    let second = service
        .optimize(replay)
        .expect("reported seed should replay the search");

    assert_eq!(second.resolved_seed, first.resolved_seed);
    assert_eq!(second.outcome, first.outcome);
    assert_eq!(second.termination.reason, first.termination.reason);
    assert_eq!(second.results.len(), first.results.len());
    for (actual, expected) in second.results.iter().zip(&first.results) {
        assert_search_result_eq(&actual.result, &expected.result);
        assert_eq!(actual.status, expected.status);
        assert_eq!(actual.raw_score, expected.raw_score);
    }
}

#[test]
fn data_provenance_survives_preparation_repricing_and_search() {
    let service = synthetic_service();
    let expected = synthetic_provenance();
    assert_eq!(service.data_provenance(), &expected);

    let prepared = service
        .prepare(valid_request())
        .expect("synthetic request should prepare");
    assert_eq!(&prepared.summary().data_provenance, &expected);

    let repriced = service
        .reprice_prepared(
            prepared,
            price_book(&[("bench/add-explicit/CraftedLife", 3.5)]),
        )
        .expect("prepared request should reprice");
    assert_eq!(&repriced.summary().data_provenance, &expected);

    let response = service
        .optimize_prepared(repriced)
        .expect("repriced request should optimize");
    assert_eq!(response.summary.data_provenance, expected);
}

#[test]
fn required_completion_ranks_ahead_of_a_higher_scoring_preference() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.goals.wants = vec![
        exact_want(CRAFTED_LIFE_ID, 1.0),
        preferred_want(CRAFTED_FIRE_ID, 100.0),
    ];
    request.methods.configured = vec![
        bench_method("Bench Life", CRAFTED_LIFE_ID, 2.0),
        bench_method("Bench Fire", CRAFTED_FIRE_ID, 2.0),
    ];
    request.methods.access = MethodAccessPolicy::Allowlist(vec![
        method_id("bench/add-explicit/CraftedLife"),
        method_id("bench/add-explicit/CraftedFire"),
    ]);
    request.search.max_steps = 1;
    request.search.top = 2;

    let response = service
        .optimize(request)
        .expect("mixed required/preferred request should optimize");
    assert_eq!(response.results.len(), 2);
    assert!(!response.starting_evaluation.complete());
    assert_eq!(response.starting_evaluation.score, 0.0);
    assert_eq!(response.starting_evaluation.required_goal_count, 1);
    assert_eq!(response.starting_evaluation.satisfied_required_count, 0);
    assert_eq!(response.starting_report.len(), 2);
    assert!(response.starting_report[0].required);
    assert!(!response.starting_report[0].satisfied);
    assert!(!response.starting_report[1].required);
    assert!(!response.starting_report[1].satisfied);

    let completed = &response.results[0];
    assert!(completed.complete);
    assert_eq!(completed.raw_score, 1.0);
    assert_eq!(completed.satisfied_required_count, 1);
    assert_eq!(completed.required_goal_count, 1);
    assert_eq!(
        completed.result.steps[0].method_id.as_str(),
        "bench/add-explicit/CraftedLife"
    );
    assert!(!completed.report[1].required);
    assert!(!completed.report[1].satisfied);

    let preferred_only = &response.results[1];
    assert!(!preferred_only.complete);
    assert_eq!(preferred_only.raw_score, 100.0);
    assert_eq!(preferred_only.satisfied_required_count, 0);
    assert_eq!(
        preferred_only.result.steps[0].method_id.as_str(),
        "bench/add-explicit/CraftedFire"
    );
}

#[test]
fn mixed_goal_counts_and_report_cover_a_fully_satisfied_result() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.goals.wants = vec![
        exact_want(CRAFTED_LIFE_ID, 1.0),
        preferred_want(CRAFTED_LIFE_ID, 2.0),
    ];
    request.methods.access =
        MethodAccessPolicy::Allowlist(vec![method_id("bench/add-explicit/CraftedLife")]);
    request.search.max_steps = 1;

    let response = service
        .optimize(request)
        .expect("mixed goal request should optimize");
    let best = &response.results[0];

    assert!(best.complete);
    assert_eq!(best.raw_score, 3.0);
    assert_eq!(best.satisfied_required_count, 1);
    assert_eq!(best.required_goal_count, 1);
    assert_eq!(best.satisfied_count, 2);
    assert_eq!(best.goal_count, 2);
    assert_eq!(best.report.len(), 2);
    assert!(best.report[0].required);
    assert!(!best.report[1].required);
    assert!(best.report.iter().all(|entry| entry.satisfied));
}

#[test]
fn all_preferred_goals_still_search_for_an_improvement() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.goals.wants = vec![preferred_want(CRAFTED_LIFE_ID, 4.0)];
    request.methods.access =
        MethodAccessPolicy::Allowlist(vec![method_id("bench/add-explicit/CraftedLife")]);
    request.search.max_steps = 1;

    let response = service
        .optimize(request)
        .expect("all-preferred request should optimize");
    let best = &response.results[0];

    assert!(response.starting_evaluation.complete());
    assert_eq!(response.starting_evaluation.score, 0.0);
    assert!(best.complete);
    assert_eq!(best.required_goal_count, 0);
    assert_eq!(best.satisfied_required_count, 0);
    assert_eq!(best.satisfied_count, 1);
    assert_eq!(best.raw_score, 4.0);
    assert_eq!(best.result.steps.len(), 1);
    assert!(!best.report[0].required);
    assert!(best.report[0].satisfied);
}

#[test]
fn clipboard_text_replaces_the_described_base_and_preserves_metadata_and_warnings() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.starting_item = StartingItemRequest::ImportedText {
        text: "\
Item Class: Body Armours
Rarity: Rare
Imported Shelter
Clipboard Plate
--------
Quality: +20% (augmented)
Armour: 456 (augmented)
Energy Shield: 321 (augmented)
--------
Requirements:
Level: 70
Str: 100
--------
Sockets: R-G-B
--------
Item Level: 77"
            .to_string(),
        fallback_item_level: Some(86),
    };

    let prepared = service
        .prepare(request)
        .expect("clipboard text should prepare");

    assert_eq!(prepared.summary().base_id, IMPORTED_BASE_ID);
    assert_eq!(prepared.summary().base_name, "Clipboard Plate");
    assert_eq!(prepared.summary().item_level, 77);
    assert!(prepared.summary().imported_start);
    assert!(prepared.summary().report_starting_item);
    assert_eq!(prepared.summary().import_warnings.len(), 1);
    assert_eq!(
        prepared.summary().import_warnings[0].code,
        "unsupported_metadata"
    );
    assert!(prepared.summary().import_warnings[0]
        .message
        .contains("Armour: 456 (augmented)"));

    let initial = prepared.initial_state();
    assert_eq!(initial.base_id, IMPORTED_BASE_ID);
    assert_eq!(initial.item_level, 77);
    assert_eq!(initial.rarity, Rarity::Rare);
    assert_eq!(initial.quality, 20);
    assert_eq!(initial.sockets.as_deref(), Some("R-G-B"));
    assert_eq!(initial.displayed_energy_shield, Some(321));
}

#[test]
fn effective_methods_are_legacy_defaults_then_configured_request_order() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.methods.configured = vec![
        bench_method("Bench Life", CRAFTED_LIFE_ID, 2.0),
        bench_method("Bench Fire", CRAFTED_FIRE_ID, 3.0),
    ];

    let prepared = service.prepare(request).expect("methods should prepare");

    assert_eq!(
        prepared
            .summary()
            .effective_methods
            .iter()
            .map(|method| method.display_name.clone())
            .collect::<Vec<_>>(),
        expected_method_names(&["Bench Life", "Bench Fire"])
    );
    assert_eq!(
        prepared
            .summary()
            .configured_methods
            .iter()
            .map(|method| method.display_name.as_str())
            .collect::<Vec<_>>(),
        ["Bench Life", "Bench Fire"],
    );
    assert_eq!(
        prepared
            .summary()
            .configured_methods
            .iter()
            .map(|method| method.id.as_str())
            .collect::<Vec<_>>(),
        [
            "bench/add-explicit/CraftedLife",
            "bench/add-explicit/CraftedFire"
        ],
    );
}

#[test]
fn method_registry_exposes_typed_metadata_in_search_order() {
    let service = synthetic_service();
    let registry = service.legacy_method_registry();

    assert_eq!(registry.len(), LEGACY_METHOD_NAMES.len());
    assert_eq!(
        registry
            .iter()
            .map(|method| method.display_name.as_str())
            .collect::<Vec<_>>(),
        LEGACY_METHOD_NAMES
    );
    assert!(registry[..11]
        .iter()
        .all(|method| method.family == MethodFamily::Currency));
    assert_eq!(registry[11].family, MethodFamily::Bench);
    assert!(registry
        .iter()
        .all(|method| !method.description.trim().is_empty()));
    assert!(registry
        .iter()
        .all(|method| method.default_price_chaos.is_some()));
    assert!(registry
        .iter()
        .all(|method| method.setup == MethodSetup::BuiltIn));

    let chaos = registry
        .iter()
        .find(|method| method.id.as_str() == "currency/chaos")
        .expect("Chaos should be registered");
    assert_eq!(
        chaos.probability_model,
        ProbabilityModel::MonteCarlo { samples: 50 }
    );
    let exalted = registry
        .iter()
        .find(|method| method.id.as_str() == "currency/exalted")
        .expect("Exalted should be registered");
    assert_eq!(
        exalted.probability_model,
        ProbabilityModel::ExactIdentitySampledRolls
    );
    let annulment = registry
        .iter()
        .find(|method| method.id.as_str() == "currency/annulment")
        .expect("Annulment should be registered");
    assert_eq!(annulment.probability_model, ProbabilityModel::Exact);
}

#[test]
fn configured_registry_metadata_keeps_intrinsic_price_and_requirements() {
    let service = synthetic_service();
    let prepared = service
        .prepare(valid_request())
        .expect("configured Bench method should prepare");
    let bench = prepared
        .summary()
        .configured_methods
        .first()
        .expect("configured Bench metadata should be present");

    assert_eq!(bench.id.as_str(), "bench/add-explicit/CraftedLife");
    assert_eq!(bench.family, MethodFamily::Bench);
    assert_eq!(bench.default_price_chaos, Some(2.0));
    assert_eq!(
        bench.setup,
        MethodSetup::CatalogOrConfigured(MethodCatalog::CraftingBench)
    );
    assert_eq!(
        bench.item_class_support,
        ItemClassSupport::CatalogRestricted
    );
    assert_eq!(
        bench.probability_model,
        ProbabilityModel::ExactIdentitySampledRolls
    );
}

#[test]
fn semantic_allowlist_filters_without_reordering_registry_methods() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.methods.configured = vec![
        bench_method("Bench Life", CRAFTED_LIFE_ID, 2.0),
        bench_method("Bench Fire", CRAFTED_FIRE_ID, 3.0),
    ];
    request.methods.access = MethodAccessPolicy::Allowlist(vec![
        method_id("bench/add-explicit/CraftedFire"),
        method_id("currency/chaos"),
        method_id("currency/scour"),
    ]);

    let prepared = service.prepare(request).expect("allowlist should prepare");

    assert_eq!(
        prepared
            .summary()
            .effective_methods
            .iter()
            .map(|method| method.id.as_str())
            .collect::<Vec<_>>(),
        [
            "currency/scour",
            "currency/chaos",
            "bench/add-explicit/CraftedFire",
        ]
    );
    assert_eq!(
        prepared
            .summary()
            .configured_methods
            .iter()
            .map(|method| method.id.as_str())
            .collect::<Vec<_>>(),
        ["bench/add-explicit/CraftedFire"]
    );
    assert_eq!(prepared.summary().price_book.len(), 3);
}

#[test]
fn family_selection_composes_inclusions_and_exclusions_without_reordering() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.methods.configured = vec![
        bench_method("Bench Life", CRAFTED_LIFE_ID, 2.0),
        bench_method("Bench Fire", CRAFTED_FIRE_ID, 3.0),
    ];
    request.methods.access = MethodAccessPolicy::Explicit(MethodSelection {
        enabled_families: vec![MethodFamily::Currency],
        enabled_methods: vec![method_id("bench/add-explicit/CraftedFire")],
        disabled_methods: vec![method_id("currency/divine")],
    });

    let prepared = service
        .prepare(request)
        .expect("family selection should prepare");

    assert_eq!(
        prepared
            .summary()
            .effective_methods
            .iter()
            .map(|method| method.id.as_str())
            .collect::<Vec<_>>(),
        [
            "currency/scour",
            "currency/transmute",
            "currency/alteration",
            "currency/augmentation",
            "currency/regal",
            "currency/alchemy",
            "currency/chaos",
            "currency/exalted",
            "currency/annulment",
            "currency/fracturing",
            "bench/add-explicit/CraftedFire",
        ]
    );
    assert_eq!(
        prepared
            .summary()
            .configured_methods
            .iter()
            .map(|method| method.id.as_str())
            .collect::<Vec<_>>(),
        ["bench/add-explicit/CraftedFire"]
    );
}

#[test]
fn family_selection_can_disable_one_configured_method() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.methods.configured = vec![
        bench_method("Bench Life", CRAFTED_LIFE_ID, 2.0),
        bench_method("Bench Fire", CRAFTED_FIRE_ID, 3.0),
    ];
    request.methods.access = MethodAccessPolicy::Explicit(MethodSelection {
        enabled_families: vec![MethodFamily::Bench],
        enabled_methods: Vec::new(),
        disabled_methods: vec![method_id("bench/add-explicit/CraftedLife")],
    });

    let prepared = service
        .prepare(request)
        .expect("Bench family selection should prepare");

    assert_eq!(
        prepared
            .summary()
            .effective_methods
            .iter()
            .map(|method| method.id.as_str())
            .collect::<Vec<_>>(),
        ["bench/remove-crafted", "bench/add-explicit/CraftedFire"]
    );
}

#[test]
fn explicit_selection_vector_order_does_not_change_registry_order() {
    let service = synthetic_service();
    let build_request = |enabled_families, enabled_methods, disabled_methods| {
        let mut request = valid_request();
        request.methods.configured = vec![
            bench_method("Bench Life", CRAFTED_LIFE_ID, 2.0),
            bench_method("Bench Fire", CRAFTED_FIRE_ID, 3.0),
        ];
        request.methods.access = MethodAccessPolicy::Explicit(MethodSelection {
            enabled_families,
            enabled_methods,
            disabled_methods,
        });
        request
    };

    let forward = service
        .prepare(build_request(
            vec![MethodFamily::Currency, MethodFamily::Bench],
            vec![
                method_id("currency/chaos"),
                method_id("bench/add-explicit/CraftedFire"),
            ],
            vec![
                method_id("currency/divine"),
                method_id("bench/add-explicit/CraftedLife"),
            ],
        ))
        .expect("forward selection should prepare");
    let reverse = service
        .prepare(build_request(
            vec![MethodFamily::Bench, MethodFamily::Currency],
            vec![
                method_id("bench/add-explicit/CraftedFire"),
                method_id("currency/chaos"),
            ],
            vec![
                method_id("bench/add-explicit/CraftedLife"),
                method_id("currency/divine"),
            ],
        ))
        .expect("reverse selection should prepare");

    assert_eq!(
        forward.summary().effective_methods,
        reverse.summary().effective_methods
    );
    assert_eq!(
        forward.summary().configured_methods,
        reverse.summary().configured_methods
    );
}

#[test]
fn disabling_the_only_source_returns_impossible_without_a_search_path() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.methods.access = MethodAccessPolicy::Allowlist(Vec::new());

    let response = service
        .optimize(request)
        .expect("an empty allowlist should yield the starting state");

    assert!(response.summary.effective_methods.is_empty());
    assert!(response.summary.price_book.is_empty());
    assert_eq!(response.outcome, OptimizeOutcome::Impossible);
    assert_eq!(
        response.termination.reason,
        SearchTerminationReason::Impossible
    );
    assert!(response.results.is_empty());
    assert_eq!(response.impossible_reasons.len(), 1);
}

#[test]
fn empty_explicit_selection_proves_the_required_craft_impossible() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.methods.access = MethodAccessPolicy::Explicit(MethodSelection::default());

    let response = service
        .optimize(request)
        .expect("an empty explicit selection should remain a valid no-method search");

    assert!(response.summary.effective_methods.is_empty());
    assert!(response.summary.configured_methods.is_empty());
    assert!(response.summary.price_book.is_empty());
    assert_eq!(response.outcome, OptimizeOutcome::Impossible);
    assert_eq!(
        response.termination.reason,
        SearchTerminationReason::Impossible
    );
    assert!(response.results.is_empty());
    assert_eq!(response.impossible_reasons.len(), 1);
}

#[test]
fn invalid_method_allowlist_has_stable_diagnostics() {
    let service = synthetic_service();

    let mut duplicate = valid_request();
    duplicate.methods.access = MethodAccessPolicy::Allowlist(vec![
        method_id("currency/chaos"),
        method_id("currency/chaos"),
    ]);
    let error = service
        .prepare(duplicate)
        .expect_err("duplicate allowlist IDs should fail");
    assert_eq!(error.code(), AppErrorCode::InvalidMethodAccess);
    assert_eq!(error.code().as_str(), "invalid_method_access");
    assert_eq!(
        error.message(),
        "Method allowlist contains duplicate ID 'currency/chaos'"
    );

    let mut unknown = valid_request();
    unknown.methods.access =
        MethodAccessPolicy::Allowlist(vec![method_id("currency/not-implemented")]);
    let error = service
        .prepare(unknown)
        .expect_err("unknown allowlist IDs should fail");
    assert_eq!(error.code(), AppErrorCode::InvalidMethodAccess);
    assert_eq!(
        error.message(),
        "Method allowlist references unavailable method ID 'currency/not-implemented'"
    );
}

#[test]
fn invalid_explicit_method_selection_has_stable_diagnostic_precedence() {
    let service = synthetic_service();

    let mut duplicate_family = valid_request();
    duplicate_family.methods.access = MethodAccessPolicy::Explicit(MethodSelection {
        enabled_families: vec![MethodFamily::Currency, MethodFamily::Currency],
        enabled_methods: vec![method_id("currency/chaos"), method_id("currency/chaos")],
        disabled_methods: Vec::new(),
    });
    let error = service
        .prepare(duplicate_family)
        .expect_err("duplicate family should win diagnostic precedence");
    assert_eq!(error.code(), AppErrorCode::InvalidMethodAccess);
    assert_eq!(
        error.message(),
        "Method selection contains duplicate family 'currency'"
    );

    let mut duplicate_enabled = valid_request();
    duplicate_enabled.methods.access = MethodAccessPolicy::Explicit(MethodSelection {
        enabled_families: Vec::new(),
        enabled_methods: vec![method_id("currency/chaos"), method_id("currency/chaos")],
        disabled_methods: Vec::new(),
    });
    let error = service
        .prepare(duplicate_enabled)
        .expect_err("duplicate enabled ID should fail");
    assert_eq!(
        error.message(),
        "Method selection contains duplicate enabled ID 'currency/chaos'"
    );

    let mut duplicate_disabled = valid_request();
    duplicate_disabled.methods.access = MethodAccessPolicy::Explicit(MethodSelection {
        enabled_families: Vec::new(),
        enabled_methods: Vec::new(),
        disabled_methods: vec![method_id("currency/divine"), method_id("currency/divine")],
    });
    let error = service
        .prepare(duplicate_disabled)
        .expect_err("duplicate disabled ID should fail");
    assert_eq!(
        error.message(),
        "Method selection contains duplicate disabled ID 'currency/divine'"
    );

    let mut conflicting = valid_request();
    conflicting.methods.access = MethodAccessPolicy::Explicit(MethodSelection {
        enabled_families: Vec::new(),
        enabled_methods: vec![method_id("currency/chaos")],
        disabled_methods: vec![method_id("currency/chaos")],
    });
    let error = service
        .prepare(conflicting)
        .expect_err("an explicit enable/disable collision should fail");
    assert_eq!(
        error.message(),
        "Method selection both enables and disables method ID 'currency/chaos'"
    );

    let mut unavailable_family = valid_request();
    unavailable_family.methods.access = MethodAccessPolicy::Explicit(MethodSelection {
        enabled_families: vec![MethodFamily::Harvest],
        enabled_methods: Vec::new(),
        disabled_methods: Vec::new(),
    });
    let error = service
        .prepare(unavailable_family)
        .expect_err("an unavailable family should fail");
    assert_eq!(
        error.message(),
        "Method selection references unavailable family 'harvest'"
    );

    let mut unknown_enabled = valid_request();
    unknown_enabled.methods.access = MethodAccessPolicy::Explicit(MethodSelection {
        enabled_families: Vec::new(),
        enabled_methods: vec![method_id("test/not-enabled")],
        disabled_methods: Vec::new(),
    });
    let error = service
        .prepare(unknown_enabled)
        .expect_err("an unavailable enabled ID should fail");
    assert_eq!(
        error.message(),
        "Method selection enables unavailable method ID 'test/not-enabled'"
    );

    let mut unknown_disabled = valid_request();
    unknown_disabled.methods.access = MethodAccessPolicy::Explicit(MethodSelection {
        enabled_families: Vec::new(),
        enabled_methods: Vec::new(),
        disabled_methods: vec![method_id("test/not-enabled")],
    });
    let error = service
        .prepare(unknown_disabled)
        .expect_err("an unavailable disabled ID should fail");
    assert_eq!(
        error.message(),
        "Method selection disables unavailable method ID 'test/not-enabled'"
    );
}

#[test]
fn pricing_a_family_disabled_method_is_rejected() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.methods.access = MethodAccessPolicy::Explicit(MethodSelection {
        enabled_families: vec![MethodFamily::Currency],
        enabled_methods: Vec::new(),
        disabled_methods: vec![method_id("currency/divine")],
    });
    request.methods.price_overrides = price_book(&[("currency/divine", 75.0)]);

    let error = service
        .prepare(request)
        .expect_err("disabled-method pricing should fail after access filtering");

    assert_eq!(error.code(), AppErrorCode::UnknownMethodPrice);
    assert_eq!(
        error.message(),
        "Price book references unavailable method ID 'currency/divine'"
    );
}

#[test]
fn semantic_repricing_keeps_order_and_changes_step_cost() {
    let service = synthetic_service();
    let prepared = service
        .prepare(valid_request())
        .expect("request should prepare before adapter repricing");
    let prepared = service
        .reprice_prepared(
            prepared,
            price_book(&[
                ("currency/scour", 0.25),
                ("bench/add-explicit/CraftedLife", 7.25),
            ]),
        )
        .expect("semantic repricing should prepare");
    let response = service
        .optimize_prepared(prepared)
        .expect("repriced request should optimize");

    assert_eq!(
        response
            .summary
            .effective_methods
            .iter()
            .map(|method| method.display_name.clone())
            .collect::<Vec<_>>(),
        expected_method_names(&["Bench Life"])
    );
    assert_eq!(
        response
            .summary
            .applied_price_overrides
            .iter()
            .map(|applied| {
                (
                    applied.method_id.as_str(),
                    applied.method_name.as_str(),
                    applied.cost,
                )
            })
            .collect::<Vec<_>>(),
        [
            ("currency/scour", "Orb of Scouring", 0.25),
            ("bench/add-explicit/CraftedLife", "Bench Life", 7.25),
        ]
    );
    assert_eq!(
        response
            .summary
            .price_book
            .get(&method_id("currency/scour")),
        Some(0.25)
    );
    assert_eq!(
        response
            .summary
            .price_book
            .get(&method_id("bench/add-explicit/CraftedLife")),
        Some(7.25)
    );
    assert_eq!(response.summary.price_book.len(), 13);
    assert_eq!(response.results[0].result.steps[0].method, "Bench Life");
    assert_eq!(response.results[0].result.steps[0].cost, 7.25);
}

#[test]
fn semantic_repricing_replaces_prior_overrides_and_restores_defaults() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.methods.price_overrides = price_book(&[
        ("currency/scour", 0.25),
        ("bench/add-explicit/CraftedLife", 4.0),
    ]);
    let prepared = service
        .prepare(request)
        .expect("initial semantic prices should prepare");

    let prepared = service
        .reprice_prepared(
            prepared,
            price_book(&[("bench/add-explicit/CraftedLife", 7.25)]),
        )
        .expect("replacement price book should prepare");
    assert_eq!(
        prepared
            .summary()
            .price_book
            .get(&method_id("currency/scour")),
        Some(1.0),
        "an omitted prior override should return to the method default"
    );
    assert_eq!(
        prepared
            .summary()
            .price_book
            .get(&method_id("bench/add-explicit/CraftedLife")),
        Some(7.25)
    );
    assert_eq!(prepared.summary().applied_price_overrides.len(), 1);
    assert_eq!(
        prepared.summary().applied_price_overrides[0]
            .method_id
            .as_str(),
        "bench/add-explicit/CraftedLife"
    );

    let prepared = service
        .reprice_prepared(prepared, PriceBook::new())
        .expect("an empty replacement book should restore every original price");
    assert!(prepared.summary().applied_price_overrides.is_empty());
    assert_eq!(
        prepared
            .summary()
            .price_book
            .get(&method_id("currency/scour")),
        Some(1.0)
    );
    assert_eq!(
        prepared
            .summary()
            .price_book
            .get(&method_id("bench/add-explicit/CraftedLife")),
        Some(2.0)
    );

    let response = service
        .optimize_prepared(prepared)
        .expect("restored default prices should optimize");
    assert_eq!(response.results[0].result.steps[0].cost, 2.0);
}

#[test]
fn semantic_method_identity_ignores_display_name_and_price() {
    let service = synthetic_service();
    let mut first = valid_request();
    first.methods.configured = vec![bench_method("First Label", CRAFTED_LIFE_ID, 2.0)];
    let mut second = valid_request();
    second.methods.configured = vec![bench_method("Second Label", CRAFTED_LIFE_ID, 99.0)];

    let first = service.prepare(first).expect("first method should prepare");
    let second = service
        .prepare(second)
        .expect("second method should prepare");

    assert_eq!(
        first.summary().configured_methods[0].id,
        second.summary().configured_methods[0].id
    );
    assert_ne!(
        first.summary().configured_methods[0].display_name,
        second.summary().configured_methods[0].display_name
    );
    assert_eq!(
        first.summary().configured_methods[0].id.as_str(),
        "bench/add-explicit/CraftedLife"
    );
}

#[test]
fn unavailable_semantic_price_id_is_rejected_before_search() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.methods.price_overrides = price_book(&[("test/not-enabled", 2.0)]);

    let error = service
        .prepare(request)
        .expect_err("unavailable semantic price should fail");

    assert_eq!(error.code(), AppErrorCode::UnknownMethodPrice);
    assert_eq!(error.code().as_str(), "unknown_method_price");
    assert_eq!(
        error.message(),
        "Price book references unavailable method ID 'test/not-enabled'"
    );
}

#[test]
fn duplicate_semantic_method_id_fails_even_when_labels_differ() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.methods.configured = vec![
        bench_method("First Life Label", CRAFTED_LIFE_ID, 2.0),
        bench_method("Second Life Label", CRAFTED_LIFE_ID, 3.0),
    ];

    let error = service
        .prepare(request)
        .expect_err("one semantic operation may not be configured twice");

    assert_eq!(error.code(), AppErrorCode::DuplicateMethod);
    assert_eq!(
        error.message(),
        "Duplicate crafting method ID 'bench/add-explicit/CraftedLife' for 'First Life Label' and 'Second Life Label'; configure each semantic operation at most once"
    );
}

#[test]
fn duplicate_configured_display_name_fails_before_search() {
    let service = synthetic_service();
    let mut request = valid_request();
    request.methods.configured = vec![bench_method("Chaos Orb", CRAFTED_LIFE_ID, 2.0)];

    let error = service
        .prepare(request)
        .expect_err("a configured method may not shadow a legacy display name");

    assert_eq!(error.code(), AppErrorCode::DuplicateMethod);
    assert_eq!(error.code().as_str(), "duplicate_method");
    assert_eq!(
        error.message(),
        "Duplicate crafting method name 'Chaos Orb'; give configured methods unique names"
    );
}

#[test]
fn invalid_goal_search_and_item_have_stable_diagnostics() {
    let service = synthetic_service();

    let mut invalid_goal = valid_request();
    invalid_goal.goals.wants.clear();
    let error = service
        .prepare(invalid_goal)
        .expect_err("an empty goal should fail");
    assert_eq!(error.code(), AppErrorCode::InvalidGoal);
    assert_eq!(
        error.message(),
        "Goal must contain at least one [[wants]] entry"
    );

    let mut invalid_search = valid_request();
    invalid_search.search.beam_width = 0;
    let error = service
        .prepare(invalid_search)
        .expect_err("a zero beam width should fail");
    assert_eq!(error.code(), AppErrorCode::InvalidSearch);
    assert_eq!(error.message(), "beam_width must be greater than 0");

    let mut invalid_item = valid_request();
    let StartingItemRequest::Described(item) = &mut invalid_item.starting_item else {
        panic!("fixture should use a described item");
    };
    item.item_level = 0;
    let error = service
        .prepare(invalid_item)
        .expect_err("an impossible item level should fail");
    assert_eq!(error.code(), AppErrorCode::InvalidStartingItem);
    assert_eq!(
        error.message(),
        "[item] item_level must be between 1 and 100"
    );
}

#[test]
fn prepared_job_cannot_run_on_a_different_service_data_instance() {
    let preparing_service = synthetic_service();
    let other_service = synthetic_service();
    assert_eq!(
        preparing_service.data_provenance(),
        other_service.data_provenance(),
        "the safety check must not rely on fingerprint inequality"
    );
    let prepared = preparing_service
        .prepare(valid_request())
        .expect("request should prepare");

    let error = other_service
        .optimize_prepared(prepared)
        .expect_err("prepared work must remain tied to its exact GameData instance");

    assert_eq!(error.code(), AppErrorCode::PreparedDataMismatch);
    assert_eq!(error.code().as_str(), "prepared_data_mismatch");
    assert_eq!(
        error.message(),
        "Prepared optimization belongs to a different GameData instance"
    );
}

#[test]
fn seeded_service_result_matches_direct_beam_search() {
    let service = synthetic_service();
    let search_request = SearchRequest {
        beam_width: 8,
        max_steps: 2,
        cost_weight: 0.02,
        restart_cost: 0.5,
        seed: Some(0xC0FFEE),
        top: 3,
        expansion_limit: None,
        timeout_ms: None,
    };
    let request = OptimizeRequest {
        starting_item: StartingItemRequest::Described(described_item(PRIMARY_BASE_ID)),
        goals: GoalSetRequest {
            wants: vec![exact_want("P1", 1.0)],
        },
        methods: MethodSetRequest {
            configured: Vec::new(),
            access: MethodAccessPolicy::LegacyDefaultsAndConfigured,
            price_overrides: PriceBook::new(),
        },
        budget: BudgetPolicy::default(),
        search: search_request,
    };

    let response = service
        .optimize(request)
        .expect("service search should succeed");

    let db = service.game_data();
    let base = db
        .base_items
        .get(PRIMARY_BASE_ID)
        .expect("synthetic base should exist");
    let initial = described_item(PRIMARY_BASE_ID)
        .build_state(PRIMARY_BASE_ID.to_string(), base.tags.clone(), db)
        .expect("direct initial state should build");
    let wants = vec![exact_want("P1", 1.0)];
    let evaluator = GoalEvaluator::new(&wants);
    let direct = BeamSearch::new(
        BeamConfig {
            beam_width: search_request.beam_width,
            max_steps: search_request.max_steps,
            cost_weight: search_request.cost_weight,
            restart_cost: search_request.restart_cost,
            seed: search_request.seed,
        },
        db,
        service.legacy_default_methods(),
    )
    .run_k_to_target(
        initial,
        |state| evaluator.score(state, db),
        search_request.top,
        evaluator.max_score(),
    );

    assert_eq!(response.results.len(), direct.len());
    for (actual, expected) in response.results.iter().zip(&direct) {
        assert_search_result_eq(&actual.result, expected);
        assert_eq!(actual.raw_score, evaluator.score(&expected.state, db));
        assert_eq!(actual.max_score, evaluator.maximum_score());
        assert_eq!(
            actual.satisfied_count,
            evaluator.satisfied_count(&expected.state, db)
        );
        assert_eq!(
            actual.satisfied_required_count,
            evaluator.satisfied_required_count(&expected.state, db)
        );
        assert_eq!(actual.required_goal_count, evaluator.required_goal_count());
        assert_eq!(actual.complete, evaluator.is_complete(&expected.state, db));
        assert_eq!(actual.report, evaluator.report_entries(&expected.state, db));
    }
}
