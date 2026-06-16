/// Monte Carlo sample count for stochastic full-reroll currencies (Alchemy, Chaos,
/// Essence, Fossil). These have too many possible outcomes for exact enumeration,
/// so we draw this many independent samples, each returned with probability 1/N.
/// This gives the beam search representative diversity while correctly discounting
/// the probability of each specific outcome.
pub const MONTE_CARLO_SAMPLES: usize = 50;

pub mod orbs;
pub mod essences;
pub mod fossils;
pub mod harvest;
pub mod eldritch;
pub mod influence;

use anyhow::Result;
use crate::data::GameData;
use crate::item::ItemState;

/// Every crafting method implements this trait.
/// The beam search engine calls `apply` to generate successor states.
pub trait CraftingMethod: Send + Sync {
    /// Human-readable name of this crafting operation (e.g. "Chaos Orb").
    fn name(&self) -> &str;

    /// Relative cost of one application of this method in chaos orbs.
    /// Used by the beam search heuristic.
    fn cost_chaos(&self) -> f64;

    /// Returns true if this method can be applied to `item` in its current state.
    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool;

    /// Apply this crafting method to `item` and return all possible successor states
    /// with their associated probabilities (prob sums to 1.0 within each group).
    ///
    /// For deterministic operations (e.g. Annulment when only one mod exists)
    /// the Vec contains exactly one entry with probability 1.0.
    ///
    /// For probabilistic operations (e.g. Chaos Orb) the Vec contains one entry
    /// per distinct outcome; callers sample or enumerate as needed.
    fn apply(&self, item: &ItemState, db: &GameData) -> Result<Vec<(ItemState, f64)>>;
}
