/// Sample count for stochastic full-reroll currencies (Alchemy, Chaos, Essence, Fossil).
/// These currencies have too many possible outcomes for exact enumeration, so we draw
/// N independent samples. Each sample is returned with weight 1/N.
///
/// IMPORTANT: 1/N is a sample weight, not a true in-game probability. The actual
/// probability of any specific outcome depends on the full mod pool and is not
/// computed here. `path_weight` values that include Monte Carlo steps are therefore
/// NOT true probabilities and must not be treated as such in scoring or reporting.
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
