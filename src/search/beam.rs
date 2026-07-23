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
    /// One-shot failures are costed as full-path restarts for ranking.
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
    /// Ranking score using restart-adjusted expected cost.
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
    total
}

fn valid_probability(probability: f64) -> bool {
    probability.is_finite() && probability > 0.0 && probability <= 1.0
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
            score: initial_score,
        };
        let mut beam: Vec<BeamNode> = vec![initial_node.clone()];

        // Best node seen per distinct method sequence, across all steps.
        let mut best_by_path: std::collections::HashMap<String, BeamNode> =
            std::collections::HashMap::new();

        for step in 0..self.config.max_steps {
            if beam.is_empty() || self.config.beam_width == 0 {
                break;
            }

            // Expand: for each node x each method, generate successors in parallel.
            // Collect one ordered Vec per input node before flattening. Rayon's
            // flat_map is unindexed and can otherwise perturb tie order.
            let per_node: Vec<Vec<BeamNode>> = beam
                .par_iter()
                .enumerate()
                .map(|(node_idx, node)| {
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
            let mut candidates: Vec<BeamNode> = per_node.into_iter().flatten().collect();

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

        if best_by_path.is_empty() {
            return vec![SearchResult {
                state: initial_node.state,
                steps: initial_node.steps,
                total_cost: initial_node.cumulative_cost,
                expected_cost: initial_node.expected_cost,
                success_prob: initial_node.success_prob,
                score: initial_node.score,
            }];
        }

        let mut all: Vec<(String, BeamNode)> = best_by_path.into_iter().collect();
        all.sort_by(|(sig_a, a), (sig_b, b)| {
            OrderedFloat(b.score)
                .cmp(&OrderedFloat(a.score))
                .then_with(|| sig_a.cmp(sig_b))
        });
        all.truncate(k);
        all.into_iter()
            .map(|(_, n)| SearchResult {
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
                mc_estimate,
            };
            let expected_cost = node.expected_cost + step_info.expected_cost();
            let success_prob = node.success_prob * if repeatable { 1.0 } else { p };
            let mut steps = node.steps.clone();
            steps.push(step_info);
            let restart_cost = expected_cost_with_restarts(&steps);
            let score = if cost_weight == 0.0 {
                raw
            } else {
                raw - cost_weight * restart_cost
            };
            local.push(BeamNode {
                state: next_state,
                steps,
                cumulative_cost: node.cumulative_cost + cost,
                expected_cost,
                success_prob,
                score,
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
                outcomes: vec![("rare-good", 0.01), ("rare-bad", 0.99)],
            }),
            Arc::new(StaticMethod {
                name: "steady",
                cost: 1.0,
                repeatable: false,
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
            2,
        );

        assert_eq!(results[0].steps[0].method, "steady");
        let rare = results
            .iter()
            .find(|result| result.steps[0].method == "rare")
            .expect("rare path should still be reported");
        assert!((rare.steps[0].p_at_least - 0.01).abs() < 1e-12);
        assert_eq!(rare.expected_cost, 1.0);
        assert!((expected_cost_with_restarts(&rare.steps) - 100.0).abs() < 1e-9);
        assert_eq!(rare.score, 0.0);
    }

    #[test]
    fn invalid_and_zero_weights_are_not_reachable() {
        let db = empty_db();
        let method = StaticMethod {
            name: "mixed",
            cost: 0.0,
            repeatable: false,
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
    fn restart_cost_rejects_impossible_or_invalid_probabilities() {
        for probability in [0.0, -0.1, 1.1, f64::NAN, f64::INFINITY] {
            let steps = [PathStep {
                method: "invalid".to_string(),
                cost: 1.0,
                p_at_least: probability,
                repeatable: false,
                mc_estimate: false,
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
                    outcomes: vec![("same-score", 1.0)],
                }) as Arc<dyn CraftingMethod>
            })
            .collect();
        let search = BeamSearch::new(config(8, 1, 0.0), &db, methods);

        let results = search.run_k(initial(), |_| 1.0, 2);

        assert_eq!(results[0].steps[0].method, "alpha");
        assert_eq!(results[1].steps[0].method, "zeta");
    }
}
