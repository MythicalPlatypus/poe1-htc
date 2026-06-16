//! Beam Search engine.
//!
//! At each step the engine:
//!   1. Expands the current beam by applying every available `CraftingMethod`
//!      to every `ItemState` in the beam.
//!   2. Scores each resulting state using the user-supplied `score_fn`.
//!   3. Keeps the top `beam_width` states by score (ties broken arbitrarily).
//!
//! The search terminates when `max_steps` is reached or the beam is empty.

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
}

/// A node in the beam — the current item state plus its ancestry for path reconstruction.
#[derive(Clone)]
pub struct BeamNode {
    pub state: ItemState,
    /// Names of crafting operations applied to reach this state.
    pub path: Vec<String>,
    /// Cumulative cost in chaos orbs along this path.
    pub cumulative_cost: f64,
    /// Product of outcome probabilities along this path (1.0 = fully certain).
    pub cumulative_prob: f64,
    /// Probability-weighted score: `score_fn(state) * cumulative_prob`.
    /// Lower-probability paths score lower even if the item itself is better.
    pub score: f64,
}

pub struct SearchResult {
    /// The best-scoring item state found.
    pub state: ItemState,
    /// The sequence of crafting operations that produced it.
    pub path: Vec<String>,
    /// Total cost in chaos orbs along the winning path.
    pub total_cost: f64,
    /// Probability of this specific outcome sequence occurring.
    pub path_probability: f64,
    /// Probability-weighted score at the winning node.
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
        let initial_score = score_fn(&initial);
        let mut beam: Vec<BeamNode> = vec![BeamNode {
            state: initial,
            path: Vec::new(),
            cumulative_cost: 0.0,
            cumulative_prob: 1.0,
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
                            let next_prob = node.cumulative_prob * prob;
                            let mut path = node.path.clone();
                            path.push(method.name().to_string());
                            local.push(BeamNode {
                                state: next_state,
                                path,
                                cumulative_cost: node.cumulative_cost + method.cost_chaos(),
                                cumulative_prob: next_prob,
                                score: raw * next_prob,
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
            path_probability: n.cumulative_prob,
            score: n.score,
        })
    }
}
