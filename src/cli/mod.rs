//! Command-line interface: argument parsing, goal loading, and the top-level
//! `run()` that wires `GameData` + `GoalSpec` into `BeamSearch` and prints the
//! resulting crafting plan.

use std::sync::Arc;

use anyhow::{bail, Result};
use clap::Parser;

use crate::currency::{
    orbs::{ChaosOrb, ExaltedOrb, OrbOfAlchemy, OrbOfAnnulment, OrbOfScouring},
    CraftingMethod,
};
use crate::data::GameData;
use crate::goal::GoalSpec;
use crate::item::ItemState;
use crate::search::beam::{BeamConfig, BeamSearch, SearchResult};

// Built-in defaults, lowest precedence (CLI flag > goal [search] > these).
const DEFAULT_BEAM_WIDTH: usize = 50;
const DEFAULT_MAX_STEPS: usize = 10;
const DEFAULT_COST_WEIGHT: f64 = 0.0;

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

    /// Cost penalty per chaos orb in node ranking (see BeamConfig::cost_weight).
    /// Overrides the goal file's [search] cost_weight.
    #[arg(long)]
    pub cost_weight: Option<f64>,
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

    let Some(goal_path) = &args.goal else {
        println!("\nNo --goal file given; data check complete.");
        println!("Run with --goal <file.toml> to compute a crafting path.");
        println!("See goals/example_life_chest.toml for the format.");
        return Ok(());
    };

    let goal = GoalSpec::load(goal_path)?;

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
    };
    println!(
        "Search: beam_width={}, max_steps={}, cost_weight={}",
        config.beam_width, config.max_steps, config.cost_weight
    );

    let methods = default_methods();
    // Names of methods whose outcome weights are Monte Carlo sample weights —
    // used below to label the path weight honestly.
    let mc_names: Vec<String> = methods
        .iter()
        .filter(|m| !m.weights_are_probabilities())
        .map(|m| m.name().to_string())
        .collect();

    let initial = ItemState::new_base(base_id.clone(), base.tags.clone(), goal.item.item_level);
    let search = BeamSearch::new(config, &db, methods);
    let result = search.run(initial, |s| goal.score(s, &db));

    match result {
        Some(r) => print_result(&r, &goal, &db, &mc_names),
        None => println!("\nNo crafting path found — no method was applicable to the base item."),
    }
    Ok(())
}

/// The default set of crafting methods offered to the search.
/// Essences, fossils, harvest, and eldritch methods need per-instance
/// configuration (which essence, which fossils, …) and are not yet exposed
/// through the goal file.
fn default_methods() -> Vec<Arc<dyn CraftingMethod>> {
    vec![
        Arc::new(OrbOfScouring),
        Arc::new(OrbOfAlchemy),
        Arc::new(ChaosOrb),
        Arc::new(ExaltedOrb),
        Arc::new(OrbOfAnnulment),
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

/// Pretty-print the winning path, final item, and goal satisfaction.
fn print_result(result: &SearchResult, goal: &GoalSpec, db: &GameData, mc_names: &[String]) {
    println!("\n=== Best crafting path (score {:.1}) ===", result.score);
    if result.path.is_empty() {
        println!("(the unmodified base item already scores best)");
    }
    for (i, step) in result.path.iter().enumerate() {
        println!("  {}. {step}", i + 1);
    }
    println!("Estimated cost: {:.0} chaos", result.total_cost);

    // path_weight is only a true probability when no Monte Carlo step is on the path.
    let has_mc_step = result.path.iter().any(|s| mc_names.contains(s));
    if has_mc_step {
        println!(
            "Path weight: {:.3e} (contains Monte Carlo steps — NOT a true probability; \
             treat this path as one representative outcome)",
            result.path_weight
        );
    } else {
        println!(
            "Path probability: {:.3e} (exact — all steps enumerate true outcomes)",
            result.path_weight
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
