//! Checked compatibility fixtures for the frozen v1 machine-readable contract.

use poe1_htc::app::dto::{
    BudgetComparisonV1, OptimizeOutcomeV1, PathStatusV1, ProbabilityModelV1,
    SavedOptimizeRequestV1, SavedOptimizeResponseV1, SchemaVersionV1,
    SAVED_OPTIMIZE_REQUEST_SCHEMA_VERSION, SAVED_OPTIMIZE_RESPONSE_SCHEMA_VERSION,
};
use poe1_htc::app::{MethodId, OptimizeRequest};
use serde_json::Value;

const REQUEST: &str = include_str!("fixtures/contracts/v1/request.json");
const COMPLETE: &str = include_str!("fixtures/contracts/v1/response_complete.json");
const INCOMPLETE: &str = include_str!("fixtures/contracts/v1/response_incomplete.json");
const SAMPLED: &str = include_str!("fixtures/contracts/v1/response_sampled.json");
const IMPOSSIBLE: &str = include_str!("fixtures/contracts/v1/response_impossible.json");
const OVER_BUDGET: &str = include_str!("fixtures/contracts/v1/response_over_budget.json");
const REQUEST_SCHEMA: &str = include_str!("../schemas/v1/saved-optimize-request.schema.json");

#[test]
fn saved_request_fixture_round_trips_through_json_and_domain() {
    let dto: SavedOptimizeRequestV1 =
        serde_json::from_str(REQUEST).expect("checked request fixture must parse");
    assert_eq!(dto.schema_version, SchemaVersionV1);

    let domain =
        OptimizeRequest::try_from(dto.clone()).expect("checked request must map to the domain");
    let canonical =
        SavedOptimizeRequestV1::try_from(&domain).expect("domain request must map back to v1");
    assert_eq!(canonical, dto);

    let json = serde_json::to_string_pretty(&canonical).expect("request DTO must serialize");
    let reparsed: SavedOptimizeRequestV1 =
        serde_json::from_str(&json).expect("serialized request DTO must parse");
    assert_eq!(reparsed, canonical);
}

#[test]
fn response_golden_fixtures_cover_every_ratified_result_class() {
    let cases = [
        ("complete", COMPLETE, OptimizeOutcomeV1::Complete),
        ("incomplete", INCOMPLETE, OptimizeOutcomeV1::Incomplete),
        ("sampled", SAMPLED, OptimizeOutcomeV1::Complete),
        ("impossible", IMPOSSIBLE, OptimizeOutcomeV1::Impossible),
        ("over_budget", OVER_BUDGET, OptimizeOutcomeV1::Complete),
    ];

    for (name, json, expected_outcome) in cases {
        let dto: SavedOptimizeResponseV1 = serde_json::from_str(json)
            .unwrap_or_else(|error| panic!("{name} response fixture did not parse: {error}"));
        assert_eq!(dto.schema_version, SchemaVersionV1, "{name}");
        assert_eq!(dto.outcome, expected_outcome, "{name}");

        let serialized = serde_json::to_string_pretty(&dto)
            .unwrap_or_else(|error| panic!("{name} response did not serialize: {error}"));
        let expected_value: Value = serde_json::from_str(json).unwrap();
        let serialized_value: Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(
            serialized_value, expected_value,
            "{name} must remain a canonical semantic golden"
        );
        let reparsed: SavedOptimizeResponseV1 = serde_json::from_str(&serialized)
            .unwrap_or_else(|error| panic!("{name} serialized response did not parse: {error}"));
        assert_eq!(reparsed, dto, "{name}");
    }

    let sampled: SavedOptimizeResponseV1 = serde_json::from_str(SAMPLED).unwrap();
    assert!(matches!(
        sampled.methods[0].probability_model,
        ProbabilityModelV1::MonteCarlo { samples } if samples.get() == 50
    ));
    assert!(sampled.paths[0].steps[0].probability_is_estimate);

    let over_budget: SavedOptimizeResponseV1 = serde_json::from_str(OVER_BUDGET).unwrap();
    assert_eq!(over_budget.paths[0].status, PathStatusV1::OverBudget);
    assert_eq!(
        over_budget.paths[0]
            .costs
            .restart_adjusted_expected
            .comparison,
        BudgetComparisonV1::Over
    );

    let impossible: SavedOptimizeResponseV1 = serde_json::from_str(IMPOSSIBLE).unwrap();
    assert!(impossible.paths.is_empty());
    assert_eq!(impossible.impossible_reasons.len(), 1);
}

#[test]
fn response_contract_rejects_wrong_versions_unknown_fields_and_invalid_identity_or_sampling() {
    let mut wrong_version: Value = serde_json::from_str(COMPLETE).unwrap();
    wrong_version["schema_version"] = Value::from(2);
    assert!(serde_json::from_value::<SavedOptimizeResponseV1>(wrong_version).is_err());

    let mut unknown: Value = serde_json::from_str(COMPLETE).unwrap();
    unknown["future_field"] = Value::Bool(true);
    assert!(serde_json::from_value::<SavedOptimizeResponseV1>(unknown).is_err());

    let mut empty_version: Value = serde_json::from_str(COMPLETE).unwrap();
    empty_version["engine"]["version"] = Value::String(String::new());
    assert!(serde_json::from_value::<SavedOptimizeResponseV1>(empty_version).is_err());

    let mut zero_samples: Value = serde_json::from_str(SAMPLED).unwrap();
    zero_samples["methods"][0]["probability_model"]["samples"] = Value::from(0);
    assert!(serde_json::from_value::<SavedOptimizeResponseV1>(zero_samples).is_err());

    let mut contradictory_outcome: Value = serde_json::from_str(COMPLETE).unwrap();
    contradictory_outcome["outcome"] = Value::String("incomplete".to_string());
    assert!(serde_json::from_value::<SavedOptimizeResponseV1>(contradictory_outcome).is_err());

    let mut false_budget_status: Value = serde_json::from_str(OVER_BUDGET).unwrap();
    false_budget_status["paths"][0]["status"] = Value::String("complete".to_string());
    assert!(serde_json::from_value::<SavedOptimizeResponseV1>(false_budget_status).is_err());

    let mut false_excess: Value = serde_json::from_str(OVER_BUDGET).unwrap();
    false_excess["paths"][0]["costs"]["first_try"]["excess"]["amount_chaos"] = Value::from(4);
    assert!(serde_json::from_value::<SavedOptimizeResponseV1>(false_excess).is_err());

    let mut target_without_maximum: Value = serde_json::from_str(COMPLETE).unwrap();
    target_without_maximum["starting_goal"]["maximum_score"] = Value::Null;
    target_without_maximum["paths"][0]["goal"]["maximum_score"] = Value::Null;
    assert!(serde_json::from_value::<SavedOptimizeResponseV1>(target_without_maximum).is_err());

    let mut completion_snapshot_disagrees: Value = serde_json::from_str(COMPLETE).unwrap();
    completion_snapshot_disagrees["termination"]["snapshot"]["complete_result_existed"] =
        Value::Bool(false);
    assert!(
        serde_json::from_value::<SavedOptimizeResponseV1>(completion_snapshot_disagrees).is_err()
    );

    let mut snapshot_uses_other_step_limit: Value = serde_json::from_str(COMPLETE).unwrap();
    snapshot_uses_other_step_limit["termination"]["snapshot"]["max_steps"] = Value::from(3);
    assert!(
        serde_json::from_value::<SavedOptimizeResponseV1>(snapshot_uses_other_step_limit).is_err()
    );

    let mut impossible_but_starting_complete: Value = serde_json::from_str(IMPOSSIBLE).unwrap();
    impossible_but_starting_complete["starting_goal"]["score"] = Value::from(1);
    impossible_but_starting_complete["starting_goal"]["satisfied_count"] = Value::from(1);
    impossible_but_starting_complete["starting_goal"]["satisfied_required_count"] = Value::from(1);
    impossible_but_starting_complete["starting_goal"]["complete"] = Value::Bool(true);
    impossible_but_starting_complete["starting_goal"]["report"][0]["satisfied"] = Value::Bool(true);
    impossible_but_starting_complete["starting_goal"]["report"][0]["contribution"] = Value::from(1);
    assert!(
        serde_json::from_value::<SavedOptimizeResponseV1>(impossible_but_starting_complete)
            .is_err()
    );
}

#[test]
fn checked_request_schema_tracks_the_literal_v1_contract_and_all_method_variants() {
    let schema: Value = serde_json::from_str(REQUEST_SCHEMA).expect("schema must be valid JSON");
    jsonschema::meta::validate(&schema).expect("schema must satisfy its Draft 2020-12 meta-schema");
    let validator = jsonschema::draft202012::options()
        .with_format("poe1-htc-method-id-v1", |value| {
            MethodId::parse(value).is_ok()
        })
        .with_format("poe1-htc-decimal-u64-v1", |value| {
            value
                .parse::<u64>()
                .is_ok_and(|parsed| parsed.to_string() == value)
        })
        .should_validate_formats(true)
        .should_ignore_unknown_formats(false)
        .build(&schema)
        .expect("checked request schema must compile");

    let fixture: Value = serde_json::from_str(REQUEST).expect("request fixture must be JSON");
    assert!(
        validator.is_valid(&fixture),
        "the checked request fixture must satisfy the published schema"
    );

    let mut invalid_seed = fixture.clone();
    invalid_seed["search"]["seed"] = Value::String("18446744073709551616".to_string());
    assert!(!validator.is_valid(&invalid_seed));
    assert!(serde_json::from_value::<SavedOptimizeRequestV1>(invalid_seed).is_err());

    let mut invalid_method_id = fixture.clone();
    invalid_method_id["methods"]["access"]["selection"]["enabled_methods"][0] =
        Value::String("Currency Chaos".to_string());
    assert!(!validator.is_valid(&invalid_method_id));
    assert!(serde_json::from_value::<SavedOptimizeRequestV1>(invalid_method_id).is_err());

    let mut presence_with_bound = fixture.clone();
    presence_with_bound["goals"][0]["mode"] = Value::String("presence".to_string());
    assert!(!validator.is_valid(&presence_with_bound));
    let dto: SavedOptimizeRequestV1 = serde_json::from_value(presence_with_bound)
        .expect("goal shape validates during conversion");
    assert!(OptimizeRequest::try_from(dto).is_err());

    let mut both_bounds = fixture.clone();
    both_bounds["goals"][0]["max_value"] = Value::from(100);
    assert!(!validator.is_valid(&both_bounds));
    let dto: SavedOptimizeRequestV1 =
        serde_json::from_value(both_bounds).expect("goal shape validates during conversion");
    assert!(OptimizeRequest::try_from(dto).is_err());

    let mut required_unbounded_per_unit = fixture.clone();
    required_unbounded_per_unit["goals"][1]["required"] = Value::Bool(true);
    assert!(!validator.is_valid(&required_unbounded_per_unit));
    let dto: SavedOptimizeRequestV1 = serde_json::from_value(required_unbounded_per_unit)
        .expect("goal shape validates during conversion");
    assert!(OptimizeRequest::try_from(dto).is_err());

    let mut outside_i32 = fixture.clone();
    outside_i32["goals"][0]["min_value"] = Value::from(i64::from(i32::MAX) + 1);
    assert!(!validator.is_valid(&outside_i32));
    let dto: SavedOptimizeRequestV1 =
        serde_json::from_value(outside_i32).expect("integer width validates during conversion");
    assert!(OptimizeRequest::try_from(dto).is_err());

    assert_eq!(
        schema["properties"]["schema_version"]["const"],
        Value::from(1)
    );
    assert_eq!(schema["additionalProperties"], Value::Bool(false));
    assert_eq!(
        schema["$defs"]["configured_method"]["oneOf"]
            .as_array()
            .map(Vec::len),
        Some(9)
    );
    assert_eq!(
        schema["$defs"]["described_item"]["properties"]["item_level"]["minimum"],
        Value::from(1)
    );
    assert_eq!(
        schema["$defs"]["described_item"]["properties"]["item_level"]["maximum"],
        Value::from(100)
    );
    assert_eq!(
        schema["$defs"]["bestiary_swap_method"]["properties"]["beast_level"]["minimum"],
        Value::from(1)
    );
    assert_eq!(
        schema["$defs"]["bestiary_swap_method"]["properties"]["beast_level"]["maximum"],
        Value::from(100)
    );
    assert_eq!(SAVED_OPTIMIZE_REQUEST_SCHEMA_VERSION, 1);
    assert_eq!(
        SAVED_OPTIMIZE_RESPONSE_SCHEMA_VERSION,
        SAVED_OPTIMIZE_REQUEST_SCHEMA_VERSION
    );
}
