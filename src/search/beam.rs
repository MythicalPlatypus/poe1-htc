//! Beam Search engine.
//!
//! At each step the engine:
//!   1. Expands the current beam by applying every available `CraftingMethod`
//!      to every `ItemState` in the beam.
//!   2. Scores each resulting state: `score_fn(state) - cost_weight * cumulative_cost`.
//!   3. Keeps the top `beam_width` states by score (ties broken arbitrarily).
//!
//! The search terminates when `max_steps` is reached or the beam is empty.
//!
//! ## Path weight vs. true probability
//! `path_weight` is the product of per-step outcome weights along the path.
//! For methods that enumerate exact outcomes (Exalted, Annulment, Harvest) this
//! equals the true in-game probability of the path. For Monte Carlo methods (Chaos,
//! Alch, Essence, Fossil) each sample carries weight 1/N — a sample weight, NOT a
//! true probability. Mixed paths are therefore not comparable on this field.
//! `path_weight` is tracked for reporting only and does NOT affect node ranking.

use std::cmp::Reverse;
use std::sync::Arc;

use ordered_float::OrderedFloat;
use rayon::prelude::*;

use crate::currency::CraftingMethod;
use crate::data::GameData;
use crate::item::ItemState;

pub struct BeamConfig {
    /// Number of states to keep after each expansion step.
    pub beam_width: usize,
    /// Maximum number of crafting steps to simulate.
    pub max_steps: usize,
    /// Cost penalty per chaos orb applied to node ranking.
    /// `node.score = score_fn(state) - cost_weight * cumulative_cost`
    /// Tune relative to the scale of your `score_fn`. Use 0.0 to ignore cost in ranking.
    pub cost_weight: f64,
}

/// A node in the beam — the current item state plus its ancestry for path reconstruction.
#[derive(Clone)]
pub struct BeamNode {
    pub state: ItemState,
    /// Names of crafting operations applied to reach this state.
    pub path: Vec<String>,
    /// Cumulative cost in chaos orbs along this path.
    pub cumulative_cost: f64,
    /// Product of per-step outcome weights along this path.
    /// Equals true probability only for paths composed entirely of exact-enumeration
    /// methods (Exalted, Annulment, Harvest). For Monte Carlo methods each step
    /// contributes 1/N (a sample weight), making this field a mixed, non-comparable
    /// number on such paths. Used for reporting only; does not affect ranking.
    pub path_weight: f64,
    /// Ranking score: `score_fn(state) - cost_weight * cumulative_cost`.
    pub score: f64,
}

pub struct SearchResult {
    /// The best-scoring item state found.
    pub state: ItemState,
    /// The sequence of crafting operations that produced it.
    pub path: Vec<String>,
    /// Total cost in chaos orbs along the winning path.
    pub total_cost: f64,
    /// Product of per-step outcome weights along the winning path.
    /// True probability only when no Monte Carlo methods appear in the path;
    /// otherwise a sample weight (see module-level doc for details).
    pub path_weight: f64,
    /// Ranking score at the winning node: `score_fn(state) - cost_weight * total_cost`.
    pub score: f64,
}

pub struct BeamSearch<'db> {
    pub config: BeamConfig,
    pub db: &'db GameData,
    pub methods: Vec<Arc<dyn CraftingMethod>>,
}

impl<'db> BeamSearch<'db> {
    pub fn new(config: BeamConfig, db: &'db GameData, methods: Vec<Arc<dyn CraftingMethod>>) -> Self {
        Self { config, db, methods }
    }

    /// Run the beam search starting from `initial`, using `score_fn` to rank states.
    ///
    /// `score_fn` receives the item state and returns a score; higher is better.
    /// The search returns the best `SearchResult` found across all steps.
    pub fn run<F>(&self, initial: ItemState, score_fn: F) -> Option<SearchResult>
    where
        F: Fn(&ItemState) -> f64 + Send + Sync,
    {
        let cost_weight = self.config.cost_weight;
        let initial_score = score_fn(&initial);
        let mut beam: Vec<BeamNode> = vec![BeamNode {
            state: initial,
            path: Vec::new(),
            cumulative_cost: 0.0,
            path_weight: 1.0,
            score: initial_score,
        }];

        let mut best: Option<BeamNode> = None;

        for _step in 0..self.config.max_steps {
            if beam.is_empty() { break; }

            // Expand: for each node × each method, generate successors in parallel.
            let mut candidates: Vec<BeamNode> = beam
                .par_iter()
                .flat_map(|node| {
                    let mut local: Vec<BeamNode> = Vec::new();
                    for method in &self.methods {
                        if !method.can_apply(&node.state, self.db) { continue; }
                        let outcomes = match method.apply(&node.state, self.db) {
                            Ok(o) => o,
                            Err(_) => continue,
                        };
                        for (next_state, prob) in outcomes {
                            let raw = score_fn(&next_state);
                            let new_cost = node.cumulative_cost + method.cost_chaos();
                            let mut path = node.path.clone();
                            path.push(method.name().to_string());
                            local.push(BeamNode {
                                state: next_state,
                                path,
                                cumulative_cost: new_cost,
                                path_weight: node.path_weight * prob,
                                score: raw - cost_weight * new_cost,
                            });
                        }
                    }
                    local
                })
                .collect();

            if candidates.is_empty() { break; }

            // Sort descending by score, keep beam_width best.
            candidates.sort_by_key(|n| Reverse(OrderedFloat(n.score)));
            candidates.truncate(self.config.beam_width);

            // Track global best.
            if let Some(top) = candidates.first() {
                let is_better = best.as_ref().map_or(true, |b| top.score > b.score);
                if is_better {
                    best = Some(top.clone());
                }
            }

            beam = candidates;
        }

        best.map(|n| SearchResult {
            state: n.state,
            path: n.path,
            total_cost: n.cumulative_cost,
            path_weight: n.path_weight,
            score: n.score,
        })
    }
}
