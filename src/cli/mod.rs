//! Command-line adapter: argument parsing, file/stdin loading, precedence
//! resolution, and human-readable output around the reusable application
//! service and search engine.

use std::io::Read;

use anyhow::{bail, Context, Result};
use clap::Parser;

use crate::app::{
    AppErrorCode, AppWarning, BudgetPolicy, EvaluatedSearchResult, GoalSetRequest,
    MethodAccessPolicy, MethodSetRequest, MethodSummary, OptimizeOutcome, OptimizeRequest,
    OptimizerService, PathStatus, PreparationSummary, PriceBook, SearchRequest,
    StartingItemRequest,
};
use crate::data::GameData;
use crate::goal::{GoalSpec, SearchSpec};

#[derive(Parser, Debug)]
#[command(name = "poe1_htc", about = "Path of Exile 1 crafting path optimizer")]
pub struct Args {
    /// Path to the RePoE data directory
    #[arg(long, default_value = "data")]
    pub data_dir: String,

    /// Goal specification TOML file (see goals/example_life_chest.toml).
    /// Without this the binary only verifies that the data files load.
    #[arg(long)]
    pub goal: Option<String>,

    /// Path to Path of Exile clipboard item text, or "-" to read it from stdin.
    /// Requires --goal; the imported item replaces the goal's starting item.
    #[arg(
        long,
        value_name = "PATH",
        requires = "goal",
        conflicts_with = "base_item"
    )]
    pub item_file: Option<String>,

    /// Override the goal file's base item (display name or RePoE metadata ID)
    #[arg(long)]
    pub base_item: Option<String>,

    /// Beam width for the search (higher = more thorough, slower).
    /// Overrides the goal file's [search] beam_width.
    #[arg(long)]
    pub beam_width: Option<usize>,

    /// Maximum number of crafting steps to simulate.
    /// Overrides the goal file's [search] max_steps.
    #[arg(long)]
    pub max_steps: Option<usize>,

    /// Cost penalty per restart-adjusted expected chaos in node ranking
    /// (see BeamConfig::cost_weight).
    /// Overrides the goal file's [search] cost_weight.
    #[arg(long)]
    pub cost_weight: Option<f64>,

    /// Cost to restore or replace the starting base after a failed one-shot path.
    #[arg(long)]
    pub restart_cost: Option<f64>,

    /// RNG seed for a reproducible search.
    /// Overrides the goal file's [search] seed.
    #[arg(long)]
    pub seed: Option<u64>,

    /// Number of distinct crafting pathways to report (best first).
    /// Overrides the goal file's [search] top.
    #[arg(long)]
    pub top: Option<usize>,

    /// Maximum concrete successor states generated before returning partial
    /// results from the last completed depth.
    #[arg(long)]
    pub expansion_limit: Option<u64>,

    /// Wall-clock search limit in milliseconds.
    #[arg(long)]
    pub timeout_ms: Option<u64>,
}

pub fn run(args: Args) -> Result<()> {
    println!("POE1 HTC — Crafting Path Optimizer");

    if args.item_file.is_some() && args.goal.is_none() {
        bail!("--item-file requires --goal");
    }
    if args.item_file.is_some() && args.base_item.is_some() {
        bail!("--item-file cannot be combined with --base-item");
    }

    let service = OptimizerService::from_loaded(crate::data::loader::load_all_with_provenance(
        &args.data_dir,
    )?);
    let db = service.game_data();
    println!(
        "Loaded {} mods, {} base items from {}",
        db.mods.len(),
        db.base_items.len(),
        args.data_dir
    );
    let provenance = service.data_provenance();
    println!(
        "Data provenance: RePoE version {}, fingerprint {}",
        provenance.repoe_version().unwrap_or("unknown"),
        provenance.fingerprint()
    );
    let available_catalogs = [
        db.crafting_bench
            .as_ref()
            .map(|catalog| format!("{} bench recipes", catalog.0.len())),
        db.essences
            .as_ref()
            .map(|catalog| format!("{} essences", catalog.0.len())),
        db.fossils
            .as_ref()
            .map(|catalog| format!("{} fossils", catalog.0.len())),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if available_catalogs.is_empty() {
        println!(
            "Warning: optional crafting catalogs are absent; named craft validation is unavailable"
        );
    } else {
        println!("Crafting catalogs: {}", available_catalogs.join(", "));
    }

    let Some(goal_path) = &args.goal else {
        println!("\nNo --goal file given; data check complete.");
        println!("Run with --goal <file.toml> to compute a crafting path.");
        println!("See goals/example_life_chest.toml for the format.");
        return Ok(());
    };

    let goal = GoalSpec::load(goal_path)?;
    let resolved_search = resolve_search_request(&args, &goal.search);
    let GoalSpec {
        mut item,
        wants,
        methods,
        prices,
        search: _,
    } = goal;

    let item_path = args.item_file.as_deref();
    let starting_item = if let Some(item_path) = item_path {
        StartingItemRequest::ImportedText {
            text: read_item_text(item_path)?,
            fallback_item_level: Some(item.item_level),
        }
    } else {
        if let Some(base_override) = &args.base_item {
            item.base.clone_from(base_override);
        }
        StartingItemRequest::Described(item)
    };
    let request = OptimizeRequest {
        starting_item,
        goals: GoalSetRequest { wants },
        methods: MethodSetRequest {
            configured: methods,
            access: MethodAccessPolicy::LegacyDefaultsAndConfigured,
            price_overrides: PriceBook::new(),
        },
        budget: BudgetPolicy::default(),
        search: resolved_search,
    };
    let prepared = service.prepare(request).map_err(|error| {
        if error.code() == AppErrorCode::ItemImportFailed {
            if let Some(item_path) = item_path {
                return anyhow::Error::new(error)
                    .context(format!("Failed to import item from {item_path}"));
            }
        }
        anyhow::Error::new(error)
    })?;
    let (price_overrides, price_warnings) =
        resolve_legacy_price_overrides(&prices, &prepared.summary().effective_methods)?;
    let prepared = service.reprice_prepared(prepared, price_overrides)?;
    print_preparation(
        prepared.summary(),
        prepared.initial_state(),
        &price_warnings,
    );

    let response = service.optimize_prepared(prepared)?;
    println!(
        "\nSearch finished: {} after {} generation(s), {} ms; resolved seed {}",
        response.termination.reason.as_str(),
        response.termination.snapshot.completed_generations,
        response.termination.snapshot.elapsed_ms,
        response.resolved_seed
    );
    if response.outcome == OptimizeOutcome::Impossible {
        println!("Required goals are provably impossible:");
        for reason in &response.impossible_reasons {
            println!(
                "  - [{} at {}] {}",
                reason.code.as_str(),
                reason.field_path,
                reason.message
            );
        }
        return Ok(());
    }
    match response.results.first() {
        Some(best) => {
            if !best.result.warnings.is_empty() {
                println!("\nSearch warnings (affected branches were skipped):");
                for warning in &best.result.warnings {
                    println!("  - {warning}");
                }
            }
            print_result(best, db);
            for (i, alt) in response.results.iter().enumerate().skip(1) {
                let result = &alt.result;
                let risk = if result.steps.iter().any(|step| !step.repeatable) {
                    format!("one-shot odds {}", fmt_prob(result.success_prob))
                } else {
                    "rerolls only".to_string()
                };
                let goal_score = fmt_goal_score(alt.raw_score, alt.max_score);
                println!(
                    "\n--- Alternative pathway #{} ({}, goal {}, {}/{} required, {}/{} total wants, ranking {:.3}, retry ~{:.1}c, restart ~{:.1}c, {}) ---",
                    i + 1,
                    alt.status.as_str(),
                    goal_score,
                    alt.satisfied_required_count,
                    alt.required_goal_count,
                    alt.satisfied_count,
                    alt.goal_count,
                    result.score,
                    result.expected_cost,
                    result.restart_cost,
                    risk
                );
                let names: Vec<&str> = result
                    .steps
                    .iter()
                    .map(|step| step.method.as_str())
                    .collect();
                println!("  {}", names.join(", then "));
            }
            let starting_complete = response.starting_evaluation.complete();
            if goal_evaluation_at_least(
                starting_complete,
                response.starting_score,
                best.complete,
                best.raw_score,
            ) {
                println!(
                    "\nNote: the starting item already scores {:.1}; \
                     no found path improves its required-completion and raw-score objective.",
                    response.starting_score
                );
            }
        }
        None => {
            println!("\nNo crafting path found — no method was applicable to the starting item.")
        }
    }
    Ok(())
}

fn goal_evaluation_at_least(
    complete: bool,
    raw_score: f64,
    other_complete: bool,
    other_raw_score: f64,
) -> bool {
    match (complete, other_complete) {
        (true, false) => true,
        (false, true) => false,
        _ => raw_score >= other_raw_score,
    }
}

fn read_item_text(path: &str) -> Result<String> {
    if path == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .context("Failed to read clipboard item text from stdin")?;
        Ok(text)
    } else {
        std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read clipboard item text from {path}"))
    }
}

fn resolve_search_request(args: &Args, goal: &SearchSpec) -> SearchRequest {
    let defaults = SearchRequest::default();
    SearchRequest {
        beam_width: args
            .beam_width
            .or(goal.beam_width)
            .unwrap_or(defaults.beam_width),
        max_steps: args
            .max_steps
            .or(goal.max_steps)
            .unwrap_or(defaults.max_steps),
        cost_weight: args
            .cost_weight
            .or(goal.cost_weight)
            .unwrap_or(defaults.cost_weight),
        restart_cost: args
            .restart_cost
            .or(goal.restart_cost)
            .unwrap_or(defaults.restart_cost),
        seed: args.seed.or(goal.seed),
        top: args.top.or(goal.top).unwrap_or(defaults.top),
        expansion_limit: args.expansion_limit.or(goal.expansion_limit),
        timeout_ms: args.timeout_ms.or(goal.timeout_ms),
    }
}

fn resolve_legacy_price_overrides(
    prices: &std::collections::HashMap<String, f64>,
    methods: &[MethodSummary],
) -> Result<(PriceBook, Vec<AppWarning>)> {
    let mut price_book = PriceBook::new();
    let mut unmatched_names = prices.keys().cloned().collect::<Vec<_>>();

    for method in methods {
        if let Some(&cost) = prices.get(&method.display_name) {
            price_book.set(method.id.clone(), cost)?;
            unmatched_names.retain(|name| name != &method.display_name);
        }
    }

    unmatched_names.sort_unstable();
    let warnings = unmatched_names
        .into_iter()
        .map(|name| AppWarning::unmatched_price_override(&name))
        .collect();

    Ok((price_book, warnings))
}

fn print_preparation(
    summary: &PreparationSummary,
    initial: &crate::item::ItemState,
    price_warnings: &[AppWarning],
) {
    if !summary.import_warnings.is_empty() {
        println!("\nImport warnings:");
        for warning in &summary.import_warnings {
            println!("  - [{}] {}", warning.code, warning.message);
        }
    }
    for warning in &summary.base_warnings {
        println!("Warning: {}", warning.message());
    }
    for warning in &summary.goal_warnings {
        println!(
            "Warning: [{} at {}] {}",
            warning.code.as_str(),
            warning.field_path,
            warning.message
        );
    }
    println!(
        "Base item: {} ({}), item level {}",
        summary.base_name, summary.base_id, summary.item_level
    );
    println!(
        "Search: beam_width={}, max_steps={}, cost_weight={}, restart_cost={}c{}",
        summary.search.beam_width,
        summary.search.max_steps,
        summary.search.cost_weight,
        summary.search.restart_cost,
        match summary.search.seed {
            Some(seed) => format!(", seed={seed}"),
            None => String::new(),
        }
    );
    match summary.budget.hard_cap_chaos() {
        Some(cap) => println!(
            "Budget: {cap}c hard cap on {} cost",
            summary.budget.metric().as_str()
        ),
        None => println!(
            "Budget: unbounded ({} cost shown for comparison)",
            summary.budget.metric().as_str()
        ),
    }
    if !summary.configured_methods.is_empty() {
        println!(
            "Goal methods: {}",
            summary
                .configured_methods
                .iter()
                .map(|method| method.display_name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for price in &summary.applied_price_overrides {
        println!(
            "Price override: {} = {} chaos",
            price.method_name, price.cost
        );
    }
    for warning in price_warnings {
        println!("Warning: {}", warning.message());
    }
    if summary.report_starting_item {
        println!(
            "Starting item: {:?} with {} existing mod(s) ({} fractured, crafted: {})",
            initial.rarity,
            initial.mod_count(),
            initial.fractured.len(),
            initial.crafted_mod.is_some()
        );
    }
}

/// Human-readable probability: percentages down to 0.1%, then "~1 in N" so
/// mirror-tier lottery odds don't collapse to "0.0%".
fn fmt_prob(p: f64) -> String {
    if p >= 0.999_999 {
        "certain".to_string()
    } else if p >= 0.001 {
        format!("{:.1}%", p * 100.0)
    } else {
        format!("~1 in {:.0}", 1.0 / p)
    }
}

fn fmt_goal_score(score: f64, maximum: Option<f64>) -> String {
    match maximum {
        Some(maximum) => format!("{score:.1}/{maximum:.1}"),
        None => format!("{score:.1}/unbounded"),
    }
}

/// Pretty-print the winning path with retry economics, the final item, and
/// goal satisfaction.
fn print_result(evaluated: &EvaluatedSearchResult, db: &GameData) {
    let result = &evaluated.result;
    let status = match evaluated.status {
        PathStatus::Complete => "COMPLETE",
        PathStatus::Incomplete => "INCOMPLETE",
        PathStatus::OverBudget => "COMPLETE, OVER BUDGET",
    };
    let goal_score = fmt_goal_score(evaluated.raw_score, evaluated.max_score);
    println!(
        "\n=== Best crafting path: target {status} ({}/{} required, {}/{} total wants, goal score {}, ranking score {:.3}) ===",
        evaluated.satisfied_required_count,
        evaluated.required_goal_count,
        evaluated.satisfied_count,
        evaluated.goal_count,
        goal_score,
        result.score
    );
    if result.steps.is_empty() {
        println!("(the starting item already scores best)");
    }
    if let Some(excess) = result.budget_excess {
        let excess = excess
            .amount_chaos()
            .map_or_else(|| "unbounded".to_string(), |value| format!("{value:.1}c"));
        println!("Budget excess: {excess}");
    }
    let mut any_estimate = false;
    for (i, step) in result.steps.iter().enumerate() {
        any_estimate |= step.probability_estimate;
        let odds = fmt_prob(step.p_at_least);
        let est = if step.probability_estimate { "~" } else { "" };
        let economics = if step.repeatable {
            format!(
                "{est}{odds} per try, reroll until hit -> ~{:.1}c expected",
                step.expected_cost()
            )
        } else if step.probability_estimate && step.p_at_least >= 0.999_999 {
            "sampled roll; true hit chance unresolved".to_string()
        } else if step.p_at_least >= 0.999_999 {
            "deterministic".to_string()
        } else {
            format!("one-shot, {est}{odds} chance of >= this result")
        };
        println!(
            "  {}. {} — {:.2}c per application; {economics}",
            i + 1,
            step.method,
            step.cost
        );
    }

    println!(
        "\nCost if every step hits first try: {:.1} chaos",
        result.total_cost
    );
    println!(
        "Expected cost (rerolling repeatable steps until they hit): ~{:.1} chaos",
        result.expected_cost
    );
    if result.success_prob < 0.999_999 {
        println!(
            "Chance all one-shot steps land at least this well: {}",
            fmt_prob(result.success_prob)
        );
        println!(
            "Expected cost if a one-shot miss scraps the item and you restart: \
             ~{:.1} chaos (includes configured reset cost)",
            result.restart_cost
        );
    }
    if any_estimate {
        println!(
            "(Sampled step probabilities are estimates; full rerolls use {} \
             Monte Carlo samples. ~ on costs and 1-in-N odds denotes approximation.)",
            crate::currency::MONTE_CARLO_SAMPLES
        );
    }

    println!("\n--- Final item ({:?}) ---", result.state.rarity);
    if result.state.quality != 0 {
        println!("Quality: +{}%", result.state.quality);
    }
    if let Some(sockets) = &result.state.sockets {
        println!("Sockets: {sockets}");
    }
    if let Some(energy_shield) = result.state.displayed_energy_shield {
        println!("Imported displayed Energy Shield: {energy_shield}");
    }
    if result.state.corrupted {
        println!("Corrupted");
    }
    if result.state.mirrored {
        println!("Mirrored");
    }
    print_mod_list("Prefixes", &result.state.prefixes, db);
    print_mod_list("Suffixes", &result.state.suffixes, db);
    if !result.state.fractured.is_empty() {
        print_mod_list("Fractured", &result.state.fractured, db);
    }
    if let Some(c) = &result.state.crafted_mod {
        print_mod_list("Crafted", std::slice::from_ref(c), db);
    }
    let exarch_tier = result
        .state
        .exarch_implicit
        .as_ref()
        .and_then(|modifier| db.mods.get(&modifier.mod_id))
        .and_then(|modifier| modifier.eldritch_tier());
    let eater_tier = result
        .state
        .eater_implicit
        .as_ref()
        .and_then(|modifier| db.mods.get(&modifier.mod_id))
        .and_then(|modifier| modifier.eldritch_tier());
    if let Some(implicit) = &result.state.exarch_implicit {
        print_mod_list(
            &format!(
                "Searing Exarch implicit{}",
                exarch_tier.map_or_else(String::new, |tier| format!(" (tier {tier})"))
            ),
            std::slice::from_ref(implicit),
            db,
        );
    }
    if let Some(implicit) = &result.state.eater_implicit {
        print_mod_list(
            &format!(
                "Eater of Worlds implicit{}",
                eater_tier.map_or_else(String::new, |tier| format!(" (tier {tier})"))
            ),
            std::slice::from_ref(implicit),
            db,
        );
    }
    print_mod_list("Implicits", &result.state.implicits, db);
    print_mod_list("Enchantments", &result.state.enchants, db);
    let dominance = match (exarch_tier, eater_tier) {
        (Some(exarch), Some(eater)) if exarch < eater => Some("Searing Exarch"),
        (Some(exarch), Some(eater)) if eater < exarch => Some("Eater of Worlds"),
        (Some(_), Some(_)) => Some("equal (no dominant side)"),
        (Some(_), None) => Some("Searing Exarch"),
        (None, Some(_)) => Some("Eater of Worlds"),
        (None, None) => None,
    };
    if let Some(dominance) = dominance {
        println!("Eldritch dominance: {dominance}");
    }

    println!("\n--- Goal satisfaction ---");
    for entry in &evaluated.report {
        let mark = if entry.satisfied { "[x]" } else { "[ ]" };
        let kind = if entry.required {
            "required"
        } else {
            "preferred"
        };
        let attained = entry
            .attained
            .map_or_else(|| "n/a".to_string(), |value| value.to_string());
        println!(
            "  {mark} [{kind}, {}, attained={attained}, contribution={:.3}] {}",
            entry.scoring_mode.as_str(),
            entry.contribution,
            entry.description
        );
    }
}

fn print_mod_list(label: &str, mods: &[crate::item::Modifier], db: &GameData) {
    if mods.is_empty() {
        return;
    }
    println!("{label}:");
    for m in mods {
        let display_name = db
            .mods
            .get(&m.mod_id)
            .map(|md| md.name.as_str())
            .unwrap_or("<unknown>");
        let rolls: Vec<String> = m
            .rolls
            .iter()
            .map(|r| format!("{} = {}", r.stat_id, r.value))
            .collect();
        println!("  {display_name} [{}]  {}", m.mod_id, rolls.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn method_summary(id: &str, display_name: &str) -> MethodSummary {
        MethodSummary {
            id: crate::currency::MethodId::parse(id).expect("test method ID should be valid"),
            display_name: display_name.to_string(),
            family: if id.starts_with("bench/") {
                crate::currency::MethodFamily::Bench
            } else {
                crate::currency::MethodFamily::Currency
            },
            description: "Legacy pricing test method.".to_string(),
            default_price_chaos: Some(1.0),
            setup: crate::currency::MethodSetup::BuiltIn,
            item_class_support: crate::currency::ItemClassSupport::AnyCraftable,
            probability_model: crate::currency::ProbabilityModel::Exact,
        }
    }

    #[test]
    fn item_file_requires_goal() {
        let error = Args::try_parse_from(["poe1_htc", "--item-file", "item.txt"])
            .expect_err("--item-file without --goal must be rejected");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn item_file_accepts_stdin_marker() {
        let args = Args::try_parse_from(["poe1_htc", "--goal", "goal.toml", "--item-file", "-"])
            .expect("a lone dash should be accepted as the item input path");
        assert_eq!(args.item_file.as_deref(), Some("-"));
    }

    #[test]
    fn item_file_conflicts_with_base_override() {
        let error = Args::try_parse_from([
            "poe1_htc",
            "--goal",
            "goal.toml",
            "--item-file",
            "item.txt",
            "--base-item",
            "Astral Plate",
        ])
        .expect_err("an imported item has an authoritative base");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn search_settings_keep_cli_then_goal_then_default_precedence() {
        let defaults = Args::try_parse_from(["poe1_htc", "--goal", "goal.toml"])
            .expect("minimal goal arguments should parse");
        assert_eq!(
            resolve_search_request(&defaults, &SearchSpec::default()),
            SearchRequest::default()
        );

        let goal_search = SearchSpec {
            beam_width: Some(20),
            max_steps: Some(7),
            cost_weight: Some(0.25),
            restart_cost: Some(3.0),
            seed: Some(11),
            top: Some(4),
            expansion_limit: Some(1_000),
            timeout_ms: Some(5_000),
        };
        assert_eq!(
            resolve_search_request(&defaults, &goal_search),
            SearchRequest {
                beam_width: 20,
                max_steps: 7,
                cost_weight: 0.25,
                restart_cost: 3.0,
                seed: Some(11),
                top: 4,
                expansion_limit: Some(1_000),
                timeout_ms: Some(5_000),
            }
        );

        let cli = Args::try_parse_from([
            "poe1_htc",
            "--goal",
            "goal.toml",
            "--beam-width",
            "30",
            "--max-steps",
            "9",
            "--cost-weight",
            "0.5",
            "--restart-cost",
            "8",
            "--seed",
            "42",
            "--top",
            "6",
        ])
        .expect("search overrides should parse");
        assert_eq!(
            resolve_search_request(&cli, &goal_search),
            SearchRequest {
                beam_width: 30,
                max_steps: 9,
                cost_weight: 0.5,
                restart_cost: 8.0,
                seed: Some(42),
                top: 6,
                expansion_limit: Some(1_000),
                timeout_ms: Some(5_000),
            }
        );
    }

    #[test]
    fn legacy_prices_match_effective_methods() {
        let chaos = method_summary("currency/chaos", "Chaos Orb");
        let bench = method_summary("bench/craft/life", "Bench: Maximum Life");
        let prices = HashMap::from([
            ("Chaos Orb".to_string(), 2.5),
            ("Bench: Maximum Life".to_string(), 8.0),
        ]);

        let (price_book, warnings) =
            resolve_legacy_price_overrides(&prices, &[bench.clone(), chaos.clone()])
                .expect("valid legacy prices should adapt");

        assert_eq!(price_book.get(&chaos.id), Some(2.5));
        assert_eq!(price_book.get(&bench.id), Some(8.0));
        assert!(warnings.is_empty());
    }

    #[test]
    fn legacy_price_warnings_are_sorted_by_unmatched_name() {
        let prices = HashMap::from([("Zulu".to_string(), 3.0), ("Alpha".to_string(), 2.0)]);

        let (price_book, warnings) =
            resolve_legacy_price_overrides(&prices, &[]).expect("valid prices should adapt");

        assert!(price_book.is_empty());
        assert!(warnings.iter().all(|warning| {
            warning.code() == crate::app::AppWarningCode::UnmatchedPriceOverride
        }));
        assert_eq!(
            warnings.iter().map(AppWarning::message).collect::<Vec<_>>(),
            [
                "[prices] \"Alpha\" matches no method name — ignored",
                "[prices] \"Zulu\" matches no method name — ignored",
            ]
        );
    }

    #[test]
    fn legacy_prices_use_case_sensitive_display_names_not_method_ids() {
        let chaos = method_summary("currency/chaos", "Chaos Orb");
        let prices = HashMap::from([
            ("Chaos Orb".to_string(), 2.5),
            ("chaos orb".to_string(), 3.0),
            ("currency/chaos".to_string(), 4.0),
        ]);

        let (price_book, warnings) =
            resolve_legacy_price_overrides(&prices, std::slice::from_ref(&chaos))
                .expect("valid prices should adapt");

        assert_eq!(price_book.get(&chaos.id), Some(2.5));
        assert!(warnings.iter().all(|warning| {
            warning.code() == crate::app::AppWarningCode::UnmatchedPriceOverride
        }));
        assert_eq!(
            warnings.iter().map(AppWarning::message).collect::<Vec<_>>(),
            [
                "[prices] \"chaos orb\" matches no method name — ignored",
                "[prices] \"currency/chaos\" matches no method name — ignored",
            ]
        );
    }

    #[test]
    fn no_improvement_note_uses_completion_before_preference_score() {
        assert!(goal_evaluation_at_least(true, 1.0, false, 100.0));
        assert!(!goal_evaluation_at_least(false, 100.0, true, 1.0));
        assert!(goal_evaluation_at_least(true, 5.0, true, 5.0));
        assert!(!goal_evaluation_at_least(true, 4.0, true, 5.0));
    }
}
