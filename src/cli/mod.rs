//! Command-line interface: argument parsing, goal loading, and the top-level
//! `run()` that wires `GameData` + `GoalSpec` into `BeamSearch` and prints the
//! resulting crafting plan.

use std::sync::Arc;

use anyhow::{bail, Result};
use clap::Parser;

use crate::currency::{
    bench::RemoveCraftedMods,
    fracturing::FracturingOrb,
    orbs::{
        ChaosOrb, DivineOrb, ExaltedOrb, OrbOfAlchemy, OrbOfAlteration, OrbOfAnnulment,
        OrbOfAugmentation, OrbOfScouring, OrbOfTransmutation, RegalOrb,
    },
    CraftingMethod, Repriced,
};
use crate::data::GameData;
use crate::goal::GoalSpec;
use crate::search::beam::{BeamConfig, BeamSearch, SearchResult};

// Built-in defaults, lowest precedence (CLI flag > goal [search] > these).
const DEFAULT_BEAM_WIDTH: usize = 50;
const DEFAULT_MAX_STEPS: usize = 10;
const DEFAULT_COST_WEIGHT: f64 = 0.0;
const DEFAULT_RESTART_COST: f64 = 1.0;

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

    /// Cost penalty per expected chaos in node ranking (see BeamConfig::cost_weight).
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
}

pub fn run(args: Args) -> Result<()> {
    println!("POE1 HTC — Crafting Path Optimizer");

    let db = crate::data::loader::load_all(&args.data_dir)?;
    println!(
        "Loaded {} mods, {} base items from {}",
        db.mods.len(),
        db.base_items.len(),
        args.data_dir
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
    goal.validate_against_db(&db)?;

    // CLI --base-item overrides the goal file's [item] base.
    let base_query = args.base_item.as_deref().unwrap_or(&goal.item.base);
    let (base_id, base) = resolve_base_item(&db, base_query)?;
    println!(
        "Base item: {} ({}), item level {}",
        base.name, base_id, goal.item.item_level
    );

    let config = BeamConfig {
        beam_width: args
            .beam_width
            .or(goal.search.beam_width)
            .unwrap_or(DEFAULT_BEAM_WIDTH),
        max_steps: args
            .max_steps
            .or(goal.search.max_steps)
            .unwrap_or(DEFAULT_MAX_STEPS),
        cost_weight: args
            .cost_weight
            .or(goal.search.cost_weight)
            .unwrap_or(DEFAULT_COST_WEIGHT),
        restart_cost: args
            .restart_cost
            .or(goal.search.restart_cost)
            .unwrap_or(DEFAULT_RESTART_COST),
        seed: args.seed.or(goal.search.seed),
    };
    if config.beam_width == 0 {
        bail!("beam_width must be greater than 0");
    }
    if config.max_steps == 0 {
        bail!("max_steps must be greater than 0");
    }
    if config.cost_weight < 0.0 || !config.cost_weight.is_finite() {
        bail!("cost_weight must be a non-negative finite number");
    }
    if config.restart_cost < 0.0 || !config.restart_cost.is_finite() {
        bail!("restart_cost must be a non-negative finite number");
    }
    println!(
        "Search: beam_width={}, max_steps={}, cost_weight={}, restart_cost={}c{}",
        config.beam_width,
        config.max_steps,
        config.cost_weight,
        config.restart_cost,
        match config.seed {
            Some(s) => format!(", seed={s}"),
            None => String::new(),
        }
    );

    // Default orbs + any extra methods declared in the goal file.
    let mut methods = default_methods();
    for spec in &goal.methods {
        methods.push(spec.build_for_item(&db, base, goal.item.item_level)?);
    }
    let mut method_names = std::collections::HashSet::new();
    for method in &methods {
        if !method_names.insert(method.name()) {
            bail!(
                "Duplicate crafting method name '{}'; give configured methods unique names",
                method.name()
            );
        }
    }
    if !goal.methods.is_empty() {
        let names: Vec<&str> = methods[methods.len() - goal.methods.len()..]
            .iter()
            .map(|m| m.name())
            .collect();
        println!("Goal methods: {}", names.join(", "));
    }

    // Apply [prices] cost overrides by display name.
    let mut unmatched_prices: Vec<&String> = goal.prices.keys().collect();
    let methods: Vec<Arc<dyn CraftingMethod>> = methods
        .into_iter()
        .map(|m| match goal.prices.get(m.name()) {
            Some(&cost) => {
                unmatched_prices.retain(|n| n.as_str() != m.name());
                println!("Price override: {} = {cost} chaos", m.name());
                Arc::new(Repriced { inner: m, cost }) as Arc<dyn CraftingMethod>
            }
            None => m,
        })
        .collect();
    for name in unmatched_prices {
        println!("Warning: [prices] \"{name}\" matches no method name — ignored");
    }

    let initial = goal
        .item
        .build_state(base_id.clone(), base.tags.clone(), &db)?;
    if !goal.item.mods.is_empty() {
        println!(
            "Starting item: {:?} with {} existing mod(s) ({} fractured, crafted: {})",
            initial.rarity,
            initial.prefixes.len() + initial.suffixes.len() + initial.fractured.len(),
            initial.fractured.len(),
            initial.crafted_mod.is_some()
        );
    }
    let starting_score = goal.score(&initial, &db);

    let top = args.top.or(goal.search.top).unwrap_or(1);
    if top == 0 {
        bail!("top must be greater than 0");
    }
    let search = BeamSearch::new(config, &db, methods);
    let results = search.run_k_to_target(initial, |s| goal.score(s, &db), top, goal.max_score());

    match results.first() {
        Some(best) => {
            if !best.warnings.is_empty() {
                println!("\nSearch warnings (affected branches were skipped):");
                for warning in &best.warnings {
                    println!("  - {warning}");
                }
            }
            print_result(best, &goal, &db);
            for (i, alt) in results.iter().enumerate().skip(1) {
                let raw_score = goal.score(&alt.state, &db);
                let satisfied = goal.satisfied_count(&alt.state, &db);
                let risk = if alt.steps.iter().any(|step| !step.repeatable) {
                    format!("one-shot odds {}", fmt_prob(alt.success_prob))
                } else {
                    "rerolls only".to_string()
                };
                println!(
                    "\n--- Alternative pathway #{} (goal {:.1}/{:.1}, {}/{} wants, ranking {:.3}, retry ~{:.1}c, restart ~{:.1}c, {}) ---",
                    i + 1,
                    raw_score,
                    goal.max_score(),
                    satisfied,
                    goal.wants.len(),
                    alt.score,
                    alt.expected_cost,
                    alt.restart_cost,
                    risk
                );
                let names: Vec<&str> = alt.steps.iter().map(|s| s.method.as_str()).collect();
                println!("  {}", names.join(", then "));
            }
            let best_raw_score = goal.score(&best.state, &db);
            if starting_score >= best_raw_score {
                println!(
                    "\nNote: the starting item already scores {starting_score:.1}; \
                     no found path improves its raw goal score."
                );
            }
        }
        None => println!("\nNo crafting path found — no method was applicable to the base item."),
    }
    Ok(())
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

/// The default orb set offered to every search. Essences, fossils, harvest,
/// bench, and eldritch methods need per-instance configuration and are added
/// via the goal file's [[methods] ] entries.
fn default_methods() -> Vec<Arc<dyn CraftingMethod>> {
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

/// Resolve `query` to a base item: first as an exact RePoE metadata ID,
/// then as a case-insensitive display-name match. Ambiguous names resolve to
/// the lexicographically smallest ID (deterministic) with a warning.
fn resolve_base_item<'db>(
    db: &'db GameData,
    query: &str,
) -> Result<(String, &'db crate::data::base_items::BaseItem)> {
    if let Some(base) = db.base_items.get(query) {
        return Ok((query.to_string(), base));
    }

    let query_lower = query.to_lowercase();
    let mut matches: Vec<(&String, &crate::data::base_items::BaseItem)> = db
        .base_items
        .iter()
        .filter(|(_, b)| b.name.to_lowercase() == query_lower)
        .collect();
    matches.sort_by_key(|(id, _)| id.as_str().to_string());

    match matches.len() {
        0 => bail!(
            "Base item '{query}' not found (tried exact ID match and case-insensitive name match)"
        ),
        1 => {}
        n => println!(
            "Warning: {n} base items share the name '{query}'; using {} (pass the metadata ID to disambiguate)",
            matches[0].0
        ),
    }
    let (id, base) = matches[0];
    Ok((id.clone(), base))
}

/// Pretty-print the winning path with retry economics, the final item, and
/// goal satisfaction.
fn print_result(result: &SearchResult, goal: &GoalSpec, db: &GameData) {
    let raw_score = goal.score(&result.state, db);
    let satisfied = goal.satisfied_count(&result.state, db);
    let status = if goal.is_complete(&result.state, db) {
        "COMPLETE"
    } else {
        "INCOMPLETE"
    };
    println!(
        "\n=== Best crafting path: target {status} ({satisfied}/{} wants, goal score {:.1}/{:.1}, ranking score {:.3}) ===",
        goal.wants.len(),
        raw_score,
        goal.max_score(),
        result.score
    );
    if result.steps.is_empty() {
        println!("(the unmodified base item already scores best)");
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
            "(~ marks estimated probabilities from sampled rolls; full rerolls \
             use {} Monte Carlo samples)",
            crate::currency::MONTE_CARLO_SAMPLES
        );
    }

    println!("\n--- Final item ({:?}) ---", result.state.rarity);
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
    for (desc, satisfied) in goal.report(&result.state, db) {
        let mark = if satisfied { "[x]" } else { "[ ]" };
        println!("  {mark} {desc}");
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
