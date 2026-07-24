//! Beam Search engine.
//!
//! At each step the engine:
//!   1. Expands the current beam by applying every available `CraftingMethod`
//!      to every `ItemState` in the beam.
//!   2. Scores each resulting state: `score_fn(state) - cost_weight *
//!      restart-adjusted expected cost`.
//!   3. Keeps the top `beam_width` states by score (ties preserve deterministic
//!      generation order).
//!
//! The search terminates when `max_steps` is reached or the beam is empty.
//!
//! ## Expected-cost model
//! For every successor we compute `p_at_least`: the probability that a single
//! application of the method produces a state scoring at least as well as this
//! successor (the sum of sibling outcome weights with `raw score >= this raw
//! score`). For exact-enumeration methods this probability is exact; for Monte
//! Carlo methods it is an empirical estimate with resolution 1/N (see
//! `MONTE_CARLO_SAMPLES`) and is flagged as an estimate.
//!
//! Steps are then priced by their retry semantics
//! (`CraftingMethod::repeatable_on_failure`):
//!   - **Repeatable** (reroll methods): a miss can be retried i.i.d., so the
//!     step's expected cost is `cost / p_at_least` and it never "fails".
//!   - **One-shot** (additive/destructive methods): a miss changes the item, so
//!     the step costs `cost` once and contributes `p_at_least` to the path's
//!     `success_prob` — the chance all one-shot steps land at least this well.
//!
//! Node ranking uses restart-adjusted expected cost, so both repeatable spam and
//! low-probability one-shot paths pay for the attempts needed to reproduce the
//! shown result. `BeamNode::expected_cost` remains the optimistic, no-restart
//! figure used by reporting.
//!
//! ## Reproducibility
//! With `BeamConfig::seed` set, each (step, node, method) triple derives its own
//! `StdRng`, making the whole search deterministic — including under rayon,
//! because candidate collection and score ties preserve deterministic order and
//! mod pools iterate in sorted order.

use std::cmp::Ordering;
use std::sync::Arc;

use ordered_float::OrderedFloat;
use rand::rngs::StdRng;
use rand::SeedableRng;
use rayon::prelude::*;

use crate::currency::{CraftingMethod, RerollKind};
use crate::data::GameData;
use crate::item::ItemState;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ModifierKey {
    mod_id: String,
    generation_type: u8,
    rolls: Vec<(String, i32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StateKey {
    base_id: String,
    base_tags: Vec<String>,
    item_level: u32,
    rarity: u8,
    prefixes: Vec<ModifierKey>,
    suffixes: Vec<ModifierKey>,
    fractured: Vec<ModifierKey>,
    crafted: Option<ModifierKey>,
    corrupted: bool,
    mirrored: bool,
    exarch: Option<ModifierKey>,
    eater: Option<ModifierKey>,
    implicits: Vec<ModifierKey>,
    enchants: Vec<ModifierKey>,
    quality: u8,
    sockets: Option<String>,
    displayed_energy_shield: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RerollContextKey {
    kind: RerollKind,
    base_id: String,
    base_tags: Vec<String>,
    item_level: u32,
    fractured: Vec<ModifierKey>,
    corrupted: bool,
    mirrored: bool,
    exarch: Option<ModifierKey>,
    eater: Option<ModifierKey>,
    implicits: Vec<ModifierKey>,
    enchants: Vec<ModifierKey>,
}

struct RerollTransition {
    consumed: Option<RerollContextKey>,
    initialized: Option<RerollContextKey>,
    superseded_initializer: Option<usize>,
}

fn generation_type_key(generation_type: &crate::data::mods::GenerationType) -> u8 {
    use crate::data::mods::GenerationType;
    match generation_type {
        GenerationType::Prefix => 0,
        GenerationType::Suffix => 1,
        GenerationType::Unique => 2,
        GenerationType::Corrupted => 3,
        GenerationType::Enchantment => 4,
        GenerationType::Blight => 5,
        GenerationType::Monster => 6,
        GenerationType::Tempest => 7,
        GenerationType::ExarchImplicit => 8,
        GenerationType::EaterImplicit => 9,
        GenerationType::Unknown => 10,
    }
}

fn modifier_key(modifier: &crate::item::Modifier) -> ModifierKey {
    let mut rolls: Vec<(String, i32)> = modifier
        .rolls
        .iter()
        .map(|roll| (roll.stat_id.clone(), roll.value))
        .collect();
    rolls.sort();
    ModifierKey {
        mod_id: modifier.mod_id.clone(),
        generation_type: generation_type_key(&modifier.generation_type),
        rolls,
    }
}

fn sorted_modifier_keys<'a>(
    modifiers: impl Iterator<Item = &'a crate::item::Modifier>,
) -> Vec<ModifierKey> {
    let mut keys: Vec<ModifierKey> = modifiers.map(modifier_key).collect();
    keys.sort_by(|a, b| {
        a.mod_id
            .cmp(&b.mod_id)
            .then_with(|| a.generation_type.cmp(&b.generation_type))
            .then_with(|| a.rolls.cmp(&b.rolls))
    });
    keys
}

fn state_key(state: &ItemState) -> StateKey {
    let mut base_tags = state.base_tags.clone();
    base_tags.sort();
    base_tags.dedup();
    StateKey {
        base_id: state.base_id.clone(),
        base_tags,
        item_level: state.item_level,
        rarity: match state.rarity {
            crate::item::state::Rarity::Normal => 0,
            crate::item::state::Rarity::Magic => 1,
            crate::item::state::Rarity::Rare => 2,
            crate::item::state::Rarity::Unique => 3,
        },
        prefixes: sorted_modifier_keys(state.prefixes.iter()),
        suffixes: sorted_modifier_keys(state.suffixes.iter()),
        fractured: sorted_modifier_keys(state.fractured.iter()),
        crafted: state.crafted_mod.as_ref().map(modifier_key),
        corrupted: state.corrupted,
        mirrored: state.mirrored,
        exarch: state.exarch_implicit.as_ref().map(modifier_key),
        eater: state.eater_implicit.as_ref().map(modifier_key),
        implicits: sorted_modifier_keys(state.implicits.iter()),
        enchants: sorted_modifier_keys(state.enchants.iter()),
        quality: state.quality,
        sockets: state.sockets.clone(),
        displayed_energy_shield: state.displayed_energy_shield,
    }
}

fn reroll_context_key(state: &ItemState, kind: RerollKind) -> RerollContextKey {
    let mut base_tags = state.base_tags.clone();
    base_tags.sort();
    base_tags.dedup();
    RerollContextKey {
        kind,
        base_id: state.base_id.clone(),
        base_tags,
        item_level: state.item_level,
        fractured: sorted_modifier_keys(state.fractured.iter()),
        corrupted: state.corrupted,
        mirrored: state.mirrored,
        exarch: state.exarch_implicit.as_ref().map(modifier_key),
        eater: state.eater_implicit.as_ref().map(modifier_key),
        implicits: sorted_modifier_keys(state.implicits.iter()),
        enchants: sorted_modifier_keys(state.enchants.iter()),
    }
}

fn deduplicate_candidates(candidates: Vec<BeamNode>) -> Vec<BeamNode> {
    let mut unique: Vec<BeamNode> = Vec::with_capacity(candidates.len());
    let mut positions: std::collections::HashMap<StateKey, usize> =
        std::collections::HashMap::new();
    for candidate in candidates {
        let key = state_key(&candidate.state);
        match positions.get(&key).copied() {
            Some(position) => {
                let current = &unique[position];
                if candidate.score > current.score
                    || (candidate.score == current.score
                        && (candidate.restart_adjusted_cost < current.restart_adjusted_cost
                            || (candidate.restart_adjusted_cost == current.restart_adjusted_cost
                                && candidate.success_prob > current.success_prob)))
                {
                    unique[position] = candidate;
                }
            }
            None => {
                positions.insert(key, unique.len());
                unique.push(candidate);
            }
        }
    }
    unique
}

fn path_signature(steps: &[PathStep]) -> String {
    steps
        .iter()
        .map(|step| step.method.as_str())
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn prune_prefix_dominated_paths(mut paths: Vec<(String, BeamNode)>) -> Vec<(String, BeamNode)> {
    let metrics: std::collections::HashMap<String, (f64, f64)> = paths
        .iter()
        .map(|(signature, node)| {
            (
                signature.clone(),
                (node.raw_score, node.restart_adjusted_cost),
            )
        })
        .collect();

    paths.retain(|(_, node)| {
        !(0..node.steps.len()).any(|prefix_len| {
            let prefix = path_signature(&node.steps[..prefix_len]);
            metrics.get(&prefix).is_some_and(|(score, cost)| {
                *score >= node.raw_score && *cost <= node.restart_adjusted_cost
            })
        })
    });
    paths
}

pub struct BeamConfig {
    /// Number of states to keep after each expansion step.
    pub beam_width: usize,
    /// Maximum number of crafting steps to simulate.
    pub max_steps: usize,
    /// Cost penalty per expected chaos orb applied to node ranking.
    /// One-shot failures are costed as full-path restarts for ranking.
    /// Tune relative to the scale of your `score_fn`. Use 0.0 to ignore cost in ranking.
    pub cost_weight: f64,
    /// Cost paid after a failed one-shot path to restore or replace the base
    /// before trying again.
    pub restart_cost: f64,
    /// RNG seed for reproducible searches. `None` draws OS entropy per expansion.
    pub seed: Option<u64>,
}

/// One applied crafting operation on a path, with its retry economics.
#[derive(Debug, Clone)]
pub struct PathStep {
    /// Method name (e.g. "Chaos Orb").
    pub method: String,
    /// Cost of one application in chaos.
    pub cost: f64,
    /// Probability that a single application scores at least as well as the
    /// outcome this path took. Exact for enumerating methods; a Monte Carlo
    /// estimate when `probability_estimate` is true.
    pub p_at_least: f64,
    /// Whether a miss can be retried i.i.d. (reroll methods).
    pub repeatable: bool,
    /// Whether `p_at_least` comes from Monte Carlo sampling.
    pub probability_estimate: bool,
}

impl PathStep {
    /// Expected chaos spent on this step: `cost / p` when the step can be
    /// repeated until it hits, otherwise a single application's cost.
    /// Invalid costs or repeatable probabilities produce infinity.
    pub fn expected_cost(&self) -> f64 {
        if !self.cost.is_finite() || self.cost < 0.0 {
            return f64::INFINITY;
        }
        if self.repeatable {
            if !valid_probability(self.p_at_least) {
                f64::INFINITY
            } else {
                self.cost / self.p_at_least
            }
        } else {
            self.cost
        }
    }
}

/// A node in the beam — the current item state plus its ancestry for path reconstruction.
#[derive(Clone)]
pub struct BeamNode {
    pub state: ItemState,
    /// Crafting operations applied to reach this state, in order.
    pub steps: Vec<PathStep>,
    /// Sum of single-application costs — what the path costs if every step
    /// hits on the first try.
    pub cumulative_cost: f64,
    /// Expected chaos under the retry model (see module docs).
    pub expected_cost: f64,
    /// Product of `p_at_least` over one-shot steps: the chance that every
    /// non-repeatable step lands at least this well. 1.0 when the path has
    /// no one-shot randomness.
    pub success_prob: f64,
    /// Unpenalized user goal score for completion checks.
    pub raw_score: f64,
    /// Ranking score using restart-adjusted expected cost.
    pub score: f64,
    /// Expected cost under the configured restart-on-miss policy.
    pub restart_adjusted_cost: f64,
    /// Full-reroll contexts already consumed on this path. Repeating any full
    /// explicit reroll in the same context would supersede earlier setup.
    reroll_contexts: std::collections::HashSet<RerollContextKey>,
    /// Rarity-upgrade rolls that may be discarded by a directly following
    /// reroll. The value is the corresponding path-step index.
    reroll_initializers: std::collections::HashMap<RerollContextKey, usize>,
}

/// Expected chaos to complete the path under a **restart-on-miss** policy:
/// every one-shot step that lands worse than shown scraps the item and the
/// whole plan restarts from the base (reroll steps still priced `cost / p`
/// within each run). This is the pessimistic bracket around the optimistic
/// `expected_cost`, which prices one-shot steps only once.
///
/// Classic sequential-Bernoulli-with-restart formula: with per-stage expected
/// cost `c_i` and one-shot success probability `p_i` (1 for repeatable stages),
/// a single run costs `R = Σ c_i · Π_{j<i} p_j` (later stages are only paid
/// when reached) and completes with `P = Π p_i`, so the expected total is
/// `R / P`. `expected_cost_with_restarts_and_reset` additionally charges the
/// configured reset cost after each failed run.
pub fn expected_cost_with_restarts(steps: &[PathStep]) -> f64 {
    expected_cost_with_restarts_and_reset(steps, 0.0)
}

pub fn expected_cost_with_restarts_and_reset(steps: &[PathStep], reset_cost: f64) -> f64 {
    if !reset_cost.is_finite() || reset_cost < 0.0 {
        return f64::INFINITY;
    }
    // R / P = Σ c_i / Π_{j>=i} p_j. Walking backward avoids a final 0/0
    // when a long path's total success probability underflows.
    let mut suffix_success = 1.0;
    let mut total = 0.0;
    for step in steps.iter().rev() {
        if !step.repeatable {
            if !valid_probability(step.p_at_least) {
                return f64::INFINITY;
            }
            suffix_success *= step.p_at_least;
            if suffix_success == 0.0 {
                return f64::INFINITY;
            }
        }

        let stage_cost = step.expected_cost();
        if !stage_cost.is_finite() {
            return f64::INFINITY;
        }
        total += stage_cost / suffix_success;
        if !total.is_finite() {
            return f64::INFINITY;
        }
    }
    let reset_attempts = if suffix_success < 1.0 {
        (1.0 - suffix_success) / suffix_success
    } else {
        0.0
    };
    total + reset_cost * reset_attempts
}

fn valid_probability(probability: f64) -> bool {
    probability.is_finite() && probability > 0.0 && probability <= 1.0
}

fn reaches_target(node: &BeamNode, target_score: Option<f64>) -> bool {
    target_score.is_some_and(|target| node.raw_score >= target)
}

/// Best-first node ordering. Completed targets always beat incomplete states;
/// equal ranking scores prefer the cheaper restart policy.
fn compare_nodes(a: &BeamNode, b: &BeamNode, target_score: Option<f64>) -> Ordering {
    reaches_target(b, target_score)
        .cmp(&reaches_target(a, target_score))
        .then_with(|| OrderedFloat(b.score).cmp(&OrderedFloat(a.score)))
        .then_with(|| {
            OrderedFloat(a.restart_adjusted_cost).cmp(&OrderedFloat(b.restart_adjusted_cost))
        })
}

pub struct SearchResult {
    /// The best-scoring item state found.
    pub state: ItemState,
    /// The sequence of crafting operations that produced it.
    pub steps: Vec<PathStep>,
    /// Cost if every step hits first try (sum of one-application costs).
    pub total_cost: f64,
    /// Expected chaos under the retry model (see module docs).
    pub expected_cost: f64,
    /// Chance that all one-shot steps land at least this well.
    pub success_prob: f64,
    /// Ranking score at the winning node, using restart-adjusted expected cost.
    pub score: f64,
    /// Expected chaos under the configured restart-on-miss policy.
    pub restart_cost: f64,
    /// Craft applications that unexpectedly failed after reporting themselves
    /// applicable. Those branches were skipped during search.
    pub warnings: Vec<String>,
}

pub struct BeamSearch<'db> {
    pub config: BeamConfig,
    pub db: &'db GameData,
    pub methods: Vec<Arc<dyn CraftingMethod>>,
}

impl<'db> BeamSearch<'db> {
    pub fn new(
        config: BeamConfig,
        db: &'db GameData,
        methods: Vec<Arc<dyn CraftingMethod>>,
    ) -> Self {
        Self {
            config,
            db,
            methods,
        }
    }

    /// Derive the RNG for one (step, node, method) expansion. With a seed this
    /// is a pure function of the coordinates, making searches reproducible.
    fn make_rng(&self, step: usize, node_idx: usize, method_idx: usize) -> StdRng {
        match self.config.seed {
            Some(seed) => {
                // Mix the coordinates into one u64 (SplitMix64-style odd constants);
                // seed_from_u64 then diffuses it across the full RNG state.
                let mixed = seed
                    ^ (step as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    ^ (node_idx as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
                    ^ (method_idx as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
                StdRng::seed_from_u64(mixed)
            }
            None => StdRng::from_os_rng(),
        }
    }

    /// Run the beam search starting from `initial`, using `score_fn` to rank states.
    ///
    /// `score_fn` receives the item state and returns a score; higher is better.
    /// The search returns the best `SearchResult` found across all steps.
    pub fn run<F>(&self, initial: ItemState, score_fn: F) -> Option<SearchResult>
    where
        F: Fn(&ItemState) -> f64 + Send + Sync,
    {
        self.run_k(initial, score_fn, 1).into_iter().next()
    }

    /// Like [`run`](Self::run), but returns up to `k` results with **distinct
    /// method sequences**, best first. Distinctness is judged on the ordered
    /// list of method names, so "Transmute, Regal" and "Transmute, Alchemy"
    /// are different pathways even when they reach similar items; the best
    /// outcome per sequence is kept, including sequences that later fell off
    /// the beam.
    pub fn run_k<F>(&self, initial: ItemState, score_fn: F, k: usize) -> Vec<SearchResult>
    where
        F: Fn(&ItemState) -> f64 + Send + Sync,
    {
        self.run_k_internal(initial, score_fn, k, None)
    }

    /// Goal-aware search: completed states are recorded as terminal results and
    /// always rank ahead of incomplete states. Among completed states, normal
    /// score/cost ranking still applies.
    pub fn run_k_to_target<F>(
        &self,
        initial: ItemState,
        score_fn: F,
        k: usize,
        target_score: f64,
    ) -> Vec<SearchResult>
    where
        F: Fn(&ItemState) -> f64 + Send + Sync,
    {
        self.run_k_internal(initial, score_fn, k, Some(target_score))
    }

    fn run_k_internal<F>(
        &self,
        initial: ItemState,
        score_fn: F,
        k: usize,
        target_score: Option<f64>,
    ) -> Vec<SearchResult>
    where
        F: Fn(&ItemState) -> f64 + Send + Sync,
    {
        if k == 0 {
            return Vec::new();
        }

        let initial_score = score_fn(&initial);
        let initial_node = BeamNode {
            state: initial,
            steps: Vec::new(),
            cumulative_cost: 0.0,
            expected_cost: 0.0,
            success_prob: 1.0,
            raw_score: initial_score,
            score: initial_score,
            restart_adjusted_cost: 0.0,
            reroll_contexts: std::collections::HashSet::new(),
            reroll_initializers: std::collections::HashMap::new(),
        };
        let mut beam: Vec<BeamNode> = if reaches_target(&initial_node, target_score) {
            Vec::new()
        } else {
            vec![initial_node.clone()]
        };

        // Best node seen per distinct method sequence, across all steps.
        let mut best_by_path: std::collections::HashMap<String, BeamNode> =
            std::collections::HashMap::from([(String::new(), initial_node.clone())]);
        let mut search_warnings = std::collections::BTreeSet::new();

        for step in 0..self.config.max_steps {
            if beam.is_empty() || self.config.beam_width == 0 {
                break;
            }

            // Expand: for each node x each method, generate successors in parallel.
            // Collect one ordered Vec per input node before flattening. Rayon's
            // flat_map is unindexed and can otherwise perturb tie order.
            let per_node: Vec<(Vec<BeamNode>, Vec<String>)> = beam
                .par_iter()
                .enumerate()
                .map(|(node_idx, node)| {
                    let mut local: Vec<BeamNode> = Vec::new();
                    let mut warnings = Vec::new();
                    for (method_idx, method) in self.methods.iter().enumerate() {
                        let reroll_context = method
                            .reroll_kind()
                            .map(|kind| reroll_context_key(&node.state, kind));
                        let initializer_context = method
                            .reroll_initializer_kind()
                            .map(|kind| reroll_context_key(&node.state, kind));
                        if reroll_context
                            .as_ref()
                            .is_some_and(|context| node.reroll_contexts.contains(context))
                        {
                            continue;
                        }
                        let superseded_initializer = reroll_context
                            .as_ref()
                            .and_then(|context| node.reroll_initializers.get(context).copied());
                        if let Some(index) = superseded_initializer {
                            if !method.consumes_reroll_initializer()
                                || index + 1 != node.steps.len()
                            {
                                continue;
                            }
                        }
                        // A consecutive application of an i.i.d. reroll is not
                        // another crafting stage. It is already represented by
                        // the previous step's retry-until-hit probability.
                        if method.repeatable_on_failure()
                            && node.steps.last().is_some_and(|previous| {
                                previous.repeatable && previous.method == method.name()
                            })
                        {
                            continue;
                        }
                        if !method.can_apply(&node.state, self.db) {
                            continue;
                        }
                        let mut rng = self.make_rng(step, node_idx, method_idx);
                        let outcomes = match method.apply(&node.state, self.db, &mut rng) {
                            Ok(o) => o,
                            Err(error) => {
                                warnings.push(format!("{}: {error:#}", method.name()));
                                continue;
                            }
                        };
                        self.push_successors(
                            node,
                            method.as_ref(),
                            outcomes,
                            &score_fn,
                            RerollTransition {
                                consumed: reroll_context,
                                initialized: initializer_context,
                                superseded_initializer,
                            },
                            &mut local,
                        );
                    }
                    (local, warnings)
                })
                .collect();
            let mut candidates = Vec::new();
            for (local, warnings) in per_node {
                candidates.extend(local);
                search_warnings.extend(warnings);
            }

            if candidates.is_empty() {
                break;
            }

            // Record the best node per method sequence BEFORE truncation, so
            // alternative pathways survive even if the beam drops them.
            for node in &candidates {
                let sig = path_signature(&node.steps);
                match best_by_path.entry(sig) {
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        if compare_nodes(node, e.get(), target_score) == Ordering::Less {
                            e.insert(node.clone());
                        }
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(node.clone());
                    }
                }
            }

            // Monte Carlo methods frequently generate identical concrete
            // items. Keep those paths in best_by_path for reporting, but let
            // only the best-ranked copy consume beam width.
            let mut unique_candidates = deduplicate_candidates(candidates);
            if target_score.is_some() {
                unique_candidates.retain(|node| !reaches_target(node, target_score));
            }

            // Sort descending by score, keep beam_width best.
            unique_candidates.sort_by(|a, b| compare_nodes(a, b, None));
            unique_candidates.truncate(self.config.beam_width);
            beam = unique_candidates;
        }

        let mut all: Vec<(String, BeamNode)> = best_by_path.into_iter().collect();
        if target_score.is_some() {
            all = prune_prefix_dominated_paths(all);
            if all
                .iter()
                .any(|(_, node)| !node.steps.is_empty() && node.raw_score > initial_score)
            {
                all.retain(|(_, node)| !node.steps.is_empty());
            }
        }
        all.sort_by(|(sig_a, a), (sig_b, b)| {
            compare_nodes(a, b, target_score).then_with(|| sig_a.cmp(sig_b))
        });
        all.truncate(k);
        let warning_count = search_warnings.len();
        let mut warnings: Vec<String> = search_warnings.into_iter().take(20).collect();
        if warning_count > warnings.len() {
            warnings.push(format!(
                "{} additional unique craft errors omitted",
                warning_count - warnings.len()
            ));
        }
        all.into_iter()
            .map(|(_, n)| SearchResult {
                state: n.state,
                steps: n.steps,
                total_cost: n.cumulative_cost,
                expected_cost: n.expected_cost,
                success_prob: n.success_prob,
                score: n.score,
                restart_cost: n.restart_adjusted_cost,
                warnings: warnings.clone(),
            })
            .collect()
    }

    /// Turn one method's outcome set into beam candidates, computing each
    /// outcome's `p_at_least` from its siblings (see module docs).
    fn push_successors<F>(
        &self,
        node: &BeamNode,
        method: &dyn CraftingMethod,
        outcomes: Vec<(ItemState, f64)>,
        score_fn: &F,
        reroll: RerollTransition,
        local: &mut Vec<BeamNode>,
    ) where
        F: Fn(&ItemState) -> f64 + Send + Sync,
    {
        let cost = method.cost_chaos();
        let repeatable = method.repeatable_on_failure();
        let probability_estimate = !method.weights_are_probabilities();
        let cost_weight = self.config.cost_weight;

        if !cost.is_finite() || cost < 0.0 {
            return;
        }

        // Zero, negative, NaN, and infinite weights cannot describe reachable
        // outcomes. Also reject non-finite scores before they reach sorting.
        let weighted_scored: Vec<(ItemState, f64, f64)> = outcomes
            .into_iter()
            .filter(|(_, weight)| weight.is_finite() && *weight > 0.0)
            .filter_map(|(state, weight)| {
                let raw = score_fn(&state);
                raw.is_finite().then_some((state, weight, raw))
            })
            .collect();
        let max_weight = weighted_scored
            .iter()
            .map(|(_, weight, _)| *weight)
            .fold(0.0_f64, f64::max);
        if max_weight == 0.0 {
            return;
        }
        let scaled_total: f64 = weighted_scored
            .iter()
            .map(|(_, weight, _)| weight / max_weight)
            .sum();
        if !scaled_total.is_finite() || scaled_total <= 0.0 {
            return;
        }
        let scored: Vec<(ItemState, f64, f64)> = weighted_scored
            .into_iter()
            .filter_map(|(state, weight, raw)| {
                let normalized = (weight / max_weight) / scaled_total;
                (normalized > 0.0).then_some((state, normalized, raw))
            })
            .collect();
        if scored.is_empty() {
            return;
        }

        // p_at_least per outcome: total sibling weight with raw >= this raw.
        // Sort indices by raw descending, prefix-sum weights, and give tied
        // outcomes the cumulative weight through the end of their tie group.
        let mut order: Vec<usize> = (0..scored.len()).collect();
        order.sort_by(|&a, &b| scored[b].2.total_cmp(&scored[a].2));
        let mut p_at_least = vec![0.0_f64; scored.len()];
        let mut cum = 0.0;
        let mut i = 0;
        while i < order.len() {
            let tie_raw = scored[order[i]].2;
            let mut j = i;
            while j < order.len() && scored[order[j]].2 == tie_raw {
                cum += scored[order[j]].1;
                j += 1;
            }
            for &idx in &order[i..j] {
                p_at_least[idx] = cum;
            }
            i = j;
        }

        for (idx, (next_state, _prob, raw)) in scored.into_iter().enumerate() {
            // Cap only upward float drift. A floor would make genuinely rare
            // outcomes look cheaper and more likely than they are.
            let p = p_at_least[idx].min(1.0);
            let step_info = PathStep {
                method: method.name().to_string(),
                cost,
                p_at_least: p,
                repeatable,
                probability_estimate,
            };
            let mut steps = node.steps.clone();
            if let Some(index) = reroll.superseded_initializer {
                steps[index].p_at_least = 1.0;
                steps[index].probability_estimate = false;
            }
            steps.push(step_info);
            let expected_cost = steps.iter().map(PathStep::expected_cost).sum();
            let success_prob = steps
                .iter()
                .filter(|step| !step.repeatable)
                .map(|step| step.p_at_least)
                .product();
            let restart_cost =
                expected_cost_with_restarts_and_reset(&steps, self.config.restart_cost);
            let score = if cost_weight == 0.0 {
                raw
            } else {
                raw - cost_weight * restart_cost
            };
            let mut reroll_contexts = node.reroll_contexts.clone();
            if let Some(context) = &reroll.consumed {
                reroll_contexts.insert(context.clone());
            }
            let mut reroll_initializers = node.reroll_initializers.clone();
            if let Some(context) = &reroll.consumed {
                reroll_initializers.remove(context);
            }
            if let Some(context) = &reroll.initialized {
                reroll_initializers.insert(context.clone(), steps.len() - 1);
            }
            local.push(BeamNode {
                state: next_state,
                steps,
                cumulative_cost: node.cumulative_cost + cost,
                expected_cost,
                success_prob,
                raw_score: raw,
                score,
                restart_adjusted_cost: restart_cost,
                reroll_contexts,
                reroll_initializers,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use anyhow::Result;
    use rand::RngCore;

    use super::*;

    struct StaticMethod {
        name: &'static str,
        cost: f64,
        repeatable: bool,
        reroll_kind: Option<RerollKind>,
        outcomes: Vec<(&'static str, f64)>,
    }

    impl CraftingMethod for StaticMethod {
        fn name(&self) -> &str {
            self.name
        }

        fn cost_chaos(&self) -> f64 {
            self.cost
        }

        fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
            item.base_id == "initial"
        }

        fn apply(
            &self,
            item: &ItemState,
            _db: &GameData,
            _rng: &mut dyn RngCore,
        ) -> Result<Vec<(ItemState, f64)>> {
            Ok(self
                .outcomes
                .iter()
                .map(|(id, weight)| {
                    let mut state = item.clone();
                    state.base_id = (*id).to_string();
                    (state, *weight)
                })
                .collect())
        }

        fn repeatable_on_failure(&self) -> bool {
            self.repeatable
        }

        fn reroll_kind(&self) -> Option<RerollKind> {
            self.reroll_kind
        }
    }

    fn empty_db() -> GameData {
        GameData::new(HashMap::new(), HashMap::new())
    }

    fn initial() -> ItemState {
        ItemState::new_base("initial", Vec::new(), 1)
    }

    fn config(beam_width: usize, max_steps: usize, cost_weight: f64) -> BeamConfig {
        BeamConfig {
            beam_width,
            max_steps,
            cost_weight,
            restart_cost: 0.0,
            seed: Some(7),
        }
    }

    #[test]
    fn one_shot_luck_is_ranked_at_restart_cost() {
        let db = empty_db();
        let methods: Vec<Arc<dyn CraftingMethod>> = vec![
            Arc::new(StaticMethod {
                name: "rare",
                cost: 1.0,
                repeatable: false,
                reroll_kind: None,
                outcomes: vec![("rare-good", 0.01), ("rare-bad", 0.99)],
            }),
            Arc::new(StaticMethod {
                name: "steady",
                cost: 1.0,
                repeatable: false,
                reroll_kind: None,
                outcomes: vec![("steady", 1.0)],
            }),
        ];
        let search = BeamSearch::new(config(8, 1, 1.0), &db, methods);

        let results = search.run_k(
            initial(),
            |state| match state.base_id.as_str() {
                "rare-good" => 100.0,
                "steady" => 20.0,
                _ => 0.0,
            },
            3,
        );

        assert_eq!(results[0].steps[0].method, "steady");
        let rare = results
            .iter()
            .find(|result| {
                result
                    .steps
                    .first()
                    .is_some_and(|step| step.method == "rare")
            })
            .expect("rare path should still be reported");
        assert!((rare.steps[0].p_at_least - 0.01).abs() < 1e-12);
        assert_eq!(rare.expected_cost, 1.0);
        assert!((expected_cost_with_restarts(&rare.steps) - 100.0).abs() < 1e-9);
        assert_eq!(rare.score, 0.0);
    }

    #[test]
    fn completed_target_always_ranks_before_incomplete_state() {
        let db = empty_db();
        let methods: Vec<Arc<dyn CraftingMethod>> = vec![
            Arc::new(StaticMethod {
                name: "complete-expensive",
                cost: 100.0,
                repeatable: false,
                reroll_kind: None,
                outcomes: vec![("complete", 1.0)],
            }),
            Arc::new(StaticMethod {
                name: "incomplete-free",
                cost: 0.0,
                repeatable: false,
                reroll_kind: None,
                outcomes: vec![("incomplete", 1.0)],
            }),
        ];
        let search = BeamSearch::new(config(8, 1, 1.0), &db, methods);
        let score = |state: &ItemState| match state.base_id.as_str() {
            "complete" => 10.0,
            "incomplete" => 9.0,
            _ => 0.0,
        };

        let results = search.run_k_to_target(initial(), score, 3, 10.0);
        assert_eq!(results[0].state.base_id, "complete");
        assert!(results[0].score < results[1].score);
        assert!(results.iter().all(|result| !result.steps.is_empty()));
    }

    #[test]
    fn invalid_and_zero_weights_are_not_reachable() {
        let db = empty_db();
        let method = StaticMethod {
            name: "mixed",
            cost: 0.0,
            repeatable: false,
            reroll_kind: None,
            outcomes: vec![
                ("zero", 0.0),
                ("negative", -1.0),
                ("nan", f64::NAN),
                ("infinite", f64::INFINITY),
                ("good", 2.0),
                ("bad", 6.0),
            ],
        };
        let search = BeamSearch::new(
            config(8, 1, 0.0),
            &db,
            vec![Arc::new(method) as Arc<dyn CraftingMethod>],
        );

        let result = search
            .run(initial(), |state| match state.base_id.as_str() {
                "zero" | "negative" | "nan" | "infinite" => 1_000.0,
                "good" => 10.0,
                _ => 0.0,
            })
            .expect("the valid outcomes should remain searchable");

        assert_eq!(result.state.base_id, "good");
        assert!((result.steps[0].p_at_least - 0.25).abs() < 1e-12);
    }

    #[test]
    fn applicable_method_errors_are_reported_in_results() {
        struct BrokenMethod;
        impl CraftingMethod for BrokenMethod {
            fn name(&self) -> &str {
                "broken"
            }
            fn cost_chaos(&self) -> f64 {
                1.0
            }
            fn can_apply(&self, _item: &ItemState, _db: &GameData) -> bool {
                true
            }
            fn apply(
                &self,
                _item: &ItemState,
                _db: &GameData,
                _rng: &mut dyn RngCore,
            ) -> Result<Vec<(ItemState, f64)>> {
                anyhow::bail!("fixture failure")
            }
        }

        let db = empty_db();
        let search = BeamSearch::new(
            config(4, 1, 0.0),
            &db,
            vec![
                Arc::new(BrokenMethod) as Arc<dyn CraftingMethod>,
                Arc::new(StaticMethod {
                    name: "working",
                    cost: 0.0,
                    repeatable: false,
                    reroll_kind: None,
                    outcomes: vec![("good", 1.0)],
                }),
            ],
        );

        let result = search
            .run(initial(), |state| (state.base_id == "good") as u8 as f64)
            .expect("working method should keep the search alive");

        assert_eq!(result.state.base_id, "good");
        assert_eq!(result.warnings, ["broken: fixture failure"]);
    }

    #[test]
    fn empty_search_returns_the_initial_state() {
        let db = empty_db();
        for config in [config(4, 0, 1.0), config(0, 4, 1.0)] {
            let search = BeamSearch::new(config, &db, Vec::new());
            let result = search
                .run(initial(), |_| 42.0)
                .expect("a no-op search should return its starting state");

            assert!(result.steps.is_empty());
            assert_eq!(result.state.base_id, "initial");
            assert_eq!(result.score, 42.0);
        }
    }

    #[test]
    fn consecutive_identical_rerolls_are_not_reported_as_extra_steps() {
        let db = empty_db();
        let method: Arc<dyn CraftingMethod> = Arc::new(StaticMethod {
            name: "reroll",
            cost: 1.0,
            repeatable: true,
            reroll_kind: Some(RerollKind::RareExplicit),
            outcomes: vec![("initial", 1.0)],
        });
        let search = BeamSearch::new(config(4, 4, 0.0), &db, vec![method]);

        let results = search.run_k(initial(), |_| 1.0, 10);
        assert_eq!(results.len(), 2, "only no-op and one reroll should exist");
        assert!(results.iter().all(|result| result.steps.len() <= 1));
    }

    #[test]
    fn later_full_reroll_cannot_supersede_setup_in_the_same_context() {
        let db = empty_db();
        let methods: Vec<Arc<dyn CraftingMethod>> = vec![
            Arc::new(StaticMethod {
                name: "reroll-a",
                cost: 1.0,
                repeatable: true,
                reroll_kind: Some(RerollKind::RareExplicit),
                outcomes: vec![("initial", 1.0)],
            }),
            Arc::new(StaticMethod {
                name: "setup",
                cost: 1.0,
                repeatable: false,
                reroll_kind: None,
                outcomes: vec![("initial", 1.0)],
            }),
            Arc::new(StaticMethod {
                name: "reroll-b",
                cost: 1.0,
                repeatable: true,
                reroll_kind: Some(RerollKind::RareExplicit),
                outcomes: vec![("initial", 1.0)],
            }),
        ];
        let search = BeamSearch::new(config(32, 3, 0.0), &db, methods);

        let results = search.run_k(initial(), |_| 1.0, 100);
        for result in results {
            let rerolls = result
                .steps
                .iter()
                .filter(|step| step.method.starts_with("reroll-"))
                .count();
            assert!(
                rerolls <= 1,
                "later full reroll superseded an earlier same-context reroll: {:?}",
                result
                    .steps
                    .iter()
                    .map(|step| step.method.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn direct_rarity_setup_keeps_cost_but_not_discarded_roll_probability() {
        struct Setup;
        impl CraftingMethod for Setup {
            fn name(&self) -> &str {
                "setup"
            }
            fn cost_chaos(&self) -> f64 {
                2.0
            }
            fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
                item.rarity == crate::item::state::Rarity::Normal
            }
            fn apply(
                &self,
                item: &ItemState,
                _db: &GameData,
                _rng: &mut dyn RngCore,
            ) -> Result<Vec<(ItemState, f64)>> {
                let outcome = |mod_id: &str| {
                    let mut next = item.clone();
                    next.rarity = crate::item::state::Rarity::Rare;
                    next.prefixes.push(crate::item::Modifier {
                        mod_id: mod_id.to_string(),
                        generation_type: crate::data::mods::GenerationType::Prefix,
                        rolls: Vec::new(),
                    });
                    next
                };
                Ok(vec![
                    (outcome("setup-good"), 0.2),
                    (outcome("setup-bad"), 0.8),
                ])
            }
            fn reroll_initializer_kind(&self) -> Option<RerollKind> {
                Some(RerollKind::RareExplicit)
            }
        }

        struct Finish;
        impl CraftingMethod for Finish {
            fn name(&self) -> &str {
                "finish"
            }
            fn cost_chaos(&self) -> f64 {
                1.0
            }
            fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
                item.rarity == crate::item::state::Rarity::Rare
            }
            fn apply(
                &self,
                item: &ItemState,
                _db: &GameData,
                _rng: &mut dyn RngCore,
            ) -> Result<Vec<(ItemState, f64)>> {
                let outcome = |mod_id: &str| {
                    let mut next = item.clone();
                    next.prefixes.clear();
                    next.prefixes.push(crate::item::Modifier {
                        mod_id: mod_id.to_string(),
                        generation_type: crate::data::mods::GenerationType::Prefix,
                        rolls: Vec::new(),
                    });
                    next
                };
                Ok(vec![
                    (outcome("final-good"), 0.25),
                    (outcome("final-bad"), 0.75),
                ])
            }
            fn repeatable_on_failure(&self) -> bool {
                true
            }
            fn reroll_kind(&self) -> Option<RerollKind> {
                Some(RerollKind::RareExplicit)
            }
            fn consumes_reroll_initializer(&self) -> bool {
                true
            }
        }

        let db = empty_db();
        let search = BeamSearch::new(
            config(16, 2, 0.0),
            &db,
            vec![
                Arc::new(Setup) as Arc<dyn CraftingMethod>,
                Arc::new(Finish) as Arc<dyn CraftingMethod>,
            ],
        );
        let score = |state: &ItemState| {
            state
                .prefixes
                .first()
                .map_or(0.0, |modifier| match modifier.mod_id.as_str() {
                    "setup-good" => 1.0,
                    "final-good" => 2.0,
                    _ => 0.0,
                })
        };

        let result = search
            .run_k_to_target(initial(), score, 1, 2.0)
            .into_iter()
            .next()
            .expect("setup followed by the reroll should complete");

        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[0].p_at_least, 1.0);
        assert!(!result.steps[0].probability_estimate);
        assert!((result.steps[1].p_at_least - 0.25).abs() < 1e-12);
        assert!((result.expected_cost - 6.0).abs() < 1e-12);
        assert_eq!(result.success_prob, 1.0);
    }

    #[test]
    fn restart_cost_rejects_impossible_or_invalid_probabilities() {
        for probability in [0.0, -0.1, 1.1, f64::NAN, f64::INFINITY] {
            let steps = [PathStep {
                method: "invalid".to_string(),
                cost: 1.0,
                p_at_least: probability,
                repeatable: false,
                probability_estimate: false,
            }];
            assert!(expected_cost_with_restarts(&steps).is_infinite());
        }
        assert_eq!(expected_cost_with_restarts(&[]), 0.0);
    }

    #[test]
    fn final_score_ties_have_deterministic_path_order() {
        let db = empty_db();
        let methods: Vec<Arc<dyn CraftingMethod>> = ["zeta", "alpha"]
            .into_iter()
            .map(|name| {
                Arc::new(StaticMethod {
                    name,
                    cost: 0.0,
                    repeatable: false,
                    reroll_kind: None,
                    outcomes: vec![("same-score", 1.0)],
                }) as Arc<dyn CraftingMethod>
            })
            .collect();
        let search = BeamSearch::new(config(8, 1, 0.0), &db, methods);

        let results = search.run_k(initial(), |_| 1.0, 3);

        assert!(results[0].steps.is_empty());
        assert_eq!(results[1].steps[0].method, "alpha");
        assert_eq!(results[2].steps[0].method, "zeta");
    }

    #[test]
    fn goal_search_hides_suffixes_that_add_no_goal_value() {
        let make_node = |methods: &[&str], raw_score: f64, cost: f64| {
            let steps: Vec<PathStep> = methods
                .iter()
                .map(|method| PathStep {
                    method: (*method).to_string(),
                    cost: 1.0,
                    p_at_least: 1.0,
                    repeatable: false,
                    probability_estimate: false,
                })
                .collect();
            BeamNode {
                state: initial(),
                steps,
                cumulative_cost: cost,
                expected_cost: cost,
                success_prob: 1.0,
                raw_score,
                score: raw_score,
                restart_adjusted_cost: cost,
                reroll_contexts: std::collections::HashSet::new(),
                reroll_initializers: std::collections::HashMap::new(),
            }
        };
        let paths = [
            make_node(&[], 0.0, 0.0),
            make_node(&["gain"], 10.0, 5.0),
            make_node(&["gain", "waste"], 10.0, 7.0),
            make_node(&["gain", "improve"], 12.0, 8.0),
            make_node(&["zero"], 0.0, 1.0),
        ]
        .into_iter()
        .map(|node| (path_signature(&node.steps), node))
        .collect();

        let kept = prune_prefix_dominated_paths(paths);
        let signatures: Vec<&str> = kept
            .iter()
            .map(|(signature, _)| signature.as_str())
            .collect();

        assert_eq!(signatures, ["", "gain", "gain\u{1f}improve"]);
    }

    #[test]
    fn duplicate_states_keep_only_the_best_path() {
        let duplicate_state = {
            let mut state = initial();
            state.base_tags = vec!["b".to_string(), "a".to_string()];
            state
        };
        let same_semantic_state = {
            let mut state = initial();
            state.base_tags = vec!["a".to_string(), "b".to_string(), "a".to_string()];
            state
        };
        let make_node = |state, score, success_prob| BeamNode {
            state,
            steps: Vec::new(),
            cumulative_cost: 0.0,
            expected_cost: 0.0,
            success_prob,
            raw_score: score,
            score,
            restart_adjusted_cost: expected_cost_with_restarts(&[]),
            reroll_contexts: std::collections::HashSet::new(),
            reroll_initializers: std::collections::HashMap::new(),
        };

        let unique = deduplicate_candidates(vec![
            make_node(duplicate_state, 4.0, 1.0),
            make_node(same_semantic_state, 5.0, 0.5),
        ]);

        assert_eq!(unique.len(), 1);
        assert_eq!(unique[0].score, 5.0);
    }

    #[test]
    fn semantic_dedup_prefers_cheaper_path_when_score_ignores_cost() {
        struct TransitionMethod {
            name: &'static str,
            from: &'static str,
            to: &'static str,
            cost: f64,
        }

        impl CraftingMethod for TransitionMethod {
            fn name(&self) -> &str {
                self.name
            }

            fn cost_chaos(&self) -> f64 {
                self.cost
            }

            fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
                item.base_id == self.from
            }

            fn apply(
                &self,
                item: &ItemState,
                _db: &GameData,
                _rng: &mut dyn RngCore,
            ) -> Result<Vec<(ItemState, f64)>> {
                let mut next = item.clone();
                next.base_id = self.to.to_string();
                Ok(vec![(next, 1.0)])
            }
        }

        let db = empty_db();
        let methods: Vec<Arc<dyn CraftingMethod>> = vec![
            Arc::new(TransitionMethod {
                name: "expensive",
                from: "initial",
                to: "common",
                cost: 10.0,
            }),
            Arc::new(TransitionMethod {
                name: "cheap",
                from: "initial",
                to: "common",
                cost: 1.0,
            }),
            Arc::new(TransitionMethod {
                name: "finish",
                from: "common",
                to: "complete",
                cost: 1.0,
            }),
        ];
        let search = BeamSearch::new(config(1, 2, 0.0), &db, methods);

        let result = search
            .run(initial(), |state| f64::from(state.base_id == "complete"))
            .expect("the shared successor should reach the final state");
        let method_names: Vec<&str> = result
            .steps
            .iter()
            .map(|step| step.method.as_str())
            .collect();

        assert_eq!(method_names, ["cheap", "finish"]);
        assert_eq!(result.total_cost, 2.0);
    }

    #[test]
    fn imported_metadata_differences_are_not_collapsed() {
        let make_node = |state| BeamNode {
            state,
            steps: Vec::new(),
            cumulative_cost: 0.0,
            expected_cost: 0.0,
            success_prob: 1.0,
            raw_score: 1.0,
            score: 1.0,
            restart_adjusted_cost: 0.0,
            reroll_contexts: std::collections::HashSet::new(),
            reroll_initializers: std::collections::HashMap::new(),
        };
        let with_quality = {
            let mut state = initial();
            state.quality = 30;
            state
        };
        let with_enchant = {
            let mut state = initial();
            state.enchants.push(crate::item::Modifier {
                mod_id: "EnchantDefences".to_string(),
                generation_type: crate::data::mods::GenerationType::Enchantment,
                rolls: Vec::new(),
            });
            state
        };
        let with_implicit = {
            let mut state = initial();
            state.implicits.push(crate::item::Modifier {
                mod_id: "PhysAsChaosImpl".to_string(),
                generation_type: crate::data::mods::GenerationType::Corrupted,
                rolls: Vec::new(),
            });
            state
        };
        let with_sockets = {
            let mut state = initial();
            state.sockets = Some("W-W-W-W-W-W".to_string());
            state
        };
        let with_es = {
            let mut state = initial();
            state.displayed_energy_shield = Some(1200);
            state
        };

        let unique = deduplicate_candidates(vec![
            make_node(initial()),
            make_node(with_quality),
            make_node(with_enchant),
            make_node(with_implicit),
            make_node(with_sockets),
            make_node(with_es),
        ]);
        assert_eq!(
            unique.len(),
            6,
            "states differing only in imported metadata are semantically different"
        );
    }
}
