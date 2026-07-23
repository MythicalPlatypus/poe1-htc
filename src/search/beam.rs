//! Beam Search engine.
//!
//! At each step the engine:
//!   1. Expands the current beam by applying every available `CraftingMethod`
//!      to every `ItemState` in the beam.
//!   2. Scores each resulting state: `score_fn(state) - cost_weight * expected_cost`.
//!   3. Keeps the top `beam_width` states by score (ties broken arbitrarily).
//!
//! The search terminates when `max_steps` is reached or the beam is empty.
//!
//! ## Expected-cost model
//! For every successor we compute `p_at_least`: the probability that a single
//! application of the method produces a state scoring at least as well as this
//! successor (the sum of sibling outcome weights with `raw score >= this raw
//! score`). For exact-enumeration methods this probability is exact; for Monte
//! Carlo methods it is an empirical estimate with resolution 1/N (see
//! `MONTE_CARLO_SAMPLES`) and is flagged `mc_estimate`.
//!
//! Steps are then priced by their retry semantics
//! (`CraftingMethod::repeatable_on_failure`):
//!   - **Repeatable** (reroll methods): a miss can be retried i.i.d., so the
//!     step's expected cost is `cost / p_at_least` and it never "fails".
//!   - **One-shot** (additive/destructive methods): a miss changes the item, so
//!     the step costs `cost` once and contributes `p_at_least` to the path's
//!     `success_prob` — the chance all one-shot steps land at least this well.
//!
//! Node ranking uses `expected_cost`, so a Chaos-spam plan is correctly priced
//! at roughly `cost x expected attempts` rather than one lucky application.
//!
//! ## Reproducibility
//! With `BeamConfig::seed` set, each (step, node, method) triple derives its own
//! `StdRng`, making the whole search deterministic — including under rayon,
//! because candidate collection preserves iteration order and mod pools iterate
//! in sorted order.

use std::cmp::Reverse;
use std::sync::Arc;

use ordered_float::OrderedFloat;
use rand::rngs::StdRng;
use rand::SeedableRng;
use rayon::prelude::*;

use crate::currency::CraftingMethod;
use crate::data::GameData;
use crate::item::ItemState;

pub struct BeamConfig {
    /// Number of states to keep after each expansion step.
    pub beam_width: usize,
    /// Maximum number of crafting steps to simulate.
    pub max_steps: usize,
    /// Cost penalty per expected chaos orb applied to node ranking.
    /// `node.score = score_fn(state) - cost_weight * expected_cost`
    /// Tune relative to the scale of your `score_fn`. Use 0.0 to ignore cost in ranking.
    pub cost_weight: f64,
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
    /// estimate (resolution 1/N) when `mc_estimate` is true.
    pub p_at_least: f64,
    /// Whether a miss can be retried i.i.d. (reroll methods).
    pub repeatable: bool,
    /// Whether `p_at_least` comes from Monte Carlo sampling.
    pub mc_estimate: bool,
}

impl PathStep {
    /// Expected chaos spent on this step: `cost / p` when the step can be
    /// repeated until it hits, otherwise a single application's cost.
    pub fn expected_cost(&self) -> f64 {
        if self.repeatable {
            self.cost / self.p_at_least
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
    /// Ranking score: `score_fn(state) - cost_weight * expected_cost`.
    pub score: f64,
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
/// `R / P`. Reset costs (scouring, re-buying the base) are not included.
pub fn expected_cost_with_restarts(steps: &[PathStep]) -> f64 {
    let mut run_cost = 0.0; // R: expected cost of one run (to completion or first miss)
    let mut reach = 1.0; // Π p_j over one-shot steps before the current one
    let mut p_all = 1.0; // P: probability a run completes
    for step in steps {
        run_cost += reach * step.expected_cost();
        if !step.repeatable {
            reach *= step.p_at_least;
            p_all *= step.p_at_least;
        }
    }
    run_cost / p_all
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
    /// Ranking score at the winning node.
    pub score: f64,
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
        let initial_score = score_fn(&initial);
        let mut beam: Vec<BeamNode> = vec![BeamNode {
            state: initial,
            steps: Vec::new(),
            cumulative_cost: 0.0,
            expected_cost: 0.0,
            success_prob: 1.0,
            score: initial_score,
        }];

        // Best node seen per distinct method sequence, across all steps.
        let mut best_by_path: std::collections::HashMap<String, BeamNode> =
            std::collections::HashMap::new();

        for step in 0..self.config.max_steps {
            if beam.is_empty() {
                break;
            }

            // Expand: for each node x each method, generate successors in parallel.
            let mut candidates: Vec<BeamNode> = beam
                .par_iter()
                .enumerate()
                .flat_map(|(node_idx, node)| {
                    let mut local: Vec<BeamNode> = Vec::new();
                    for (method_idx, method) in self.methods.iter().enumerate() {
                        if !method.can_apply(&node.state, self.db) {
                            continue;
                        }
                        let mut rng = self.make_rng(step, node_idx, method_idx);
                        let outcomes = match method.apply(&node.state, self.db, &mut rng) {
                            Ok(o) => o,
                            Err(_) => continue,
                        };
                        self.push_successors(
                            node,
                            method.as_ref(),
                            outcomes,
                            &score_fn,
                            &mut local,
                        );
                    }
                    local
                })
                .collect();

            if candidates.is_empty() {
                break;
            }

            // Record the best node per method sequence BEFORE truncation, so
            // alternative pathways survive even if the beam drops them.
            for node in &candidates {
                let sig = node
                    .steps
                    .iter()
                    .map(|s| s.method.as_str())
                    .collect::<Vec<_>>()
                    .join("\u{1f}");
                match best_by_path.entry(sig) {
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        if node.score > e.get().score {
                            e.insert(node.clone());
                        }
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(node.clone());
                    }
                }
            }

            // Sort descending by score, keep beam_width best.
            candidates.sort_by_key(|n| Reverse(OrderedFloat(n.score)));
            candidates.truncate(self.config.beam_width);
            beam = candidates;
        }

        let mut all: Vec<BeamNode> = best_by_path.into_values().collect();
        all.sort_by_key(|n| Reverse(OrderedFloat(n.score)));
        all.truncate(k);
        all.into_iter()
            .map(|n| SearchResult {
                state: n.state,
                steps: n.steps,
                total_cost: n.cumulative_cost,
                expected_cost: n.expected_cost,
                success_prob: n.success_prob,
                score: n.score,
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
        local: &mut Vec<BeamNode>,
    ) where
        F: Fn(&ItemState) -> f64 + Send + Sync,
    {
        let cost = method.cost_chaos();
        let repeatable = method.repeatable_on_failure();
        let mc_estimate = !method.weights_are_probabilities();
        let cost_weight = self.config.cost_weight;

        let scored: Vec<(ItemState, f64, f64)> = outcomes
            .into_iter()
            .map(|(state, prob)| {
                let raw = score_fn(&state);
                (state, prob, raw)
            })
            .collect();

        // p_at_least per outcome: total sibling weight with raw >= this raw.
        // Sort indices by raw descending, prefix-sum weights, and give tied
        // outcomes the cumulative weight through the end of their tie group.
        let mut order: Vec<usize> = (0..scored.len()).collect();
        order.sort_by(|&a, &b| {
            scored[b]
                .2
                .partial_cmp(&scored[a].2)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
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
            // Guard against degenerate zero weights; caps p at 1.0 against
            // float accumulation drift.
            let p = p_at_least[idx].clamp(1e-12, 1.0);
            let step_info = PathStep {
                method: method.name().to_string(),
                cost,
                p_at_least: p,
                repeatable,
                mc_estimate,
            };
            let expected_cost = node.expected_cost + step_info.expected_cost();
            let success_prob = node.success_prob * if repeatable { 1.0 } else { p };
            let mut steps = node.steps.clone();
            steps.push(step_info);
            local.push(BeamNode {
                state: next_state,
                steps,
                cumulative_cost: node.cumulative_cost + cost,
                expected_cost,
                success_prob,
                score: raw - cost_weight * expected_cost,
            });
        }
    }
}
