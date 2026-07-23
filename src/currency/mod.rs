/// Sample count for stochastic full-reroll currencies (Alchemy, Chaos, Essence, Fossil).
/// These currencies have too many possible outcomes for exact enumeration, so we draw
/// N independent samples. Each sample is returned with weight 1/N.
///
/// IMPORTANT: 1/N is a sample weight, not a true in-game probability. The actual
/// probability of any specific outcome depends on the full mod pool and is not
/// computed here. Success probabilities derived from these weights are Monte Carlo
/// estimates with resolution 1/N and must be labelled as such in reporting.
pub const MONTE_CARLO_SAMPLES: usize = 50;

pub mod bench;
pub mod eldritch;
pub mod essences;
pub mod fossils;
pub mod harvest;
pub mod influence;
pub mod orbs;

use crate::data::GameData;
use crate::item::ItemState;
use anyhow::Result;
use rand::RngCore;

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
    ///
    /// All randomness MUST come from `rng` — never from thread-local RNGs — so
    /// that seeded searches are reproducible.
    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>>;

    /// Whether the f64 weights returned by `apply` are true in-game probabilities.
    ///
    /// Exact-enumeration methods (Exalted, Annulment, Harvest, Eldritch, Scouring)
    /// return true (the default). Monte Carlo methods (Chaos, Alchemy, Essence,
    /// Fossil, Divine, Transmutation, Alteration) MUST override this to return
    /// false — their weights are 1/N sample weights. Reporting layers use this to
    /// decide whether derived probabilities may be presented as exact.
    fn weights_are_probabilities(&self) -> bool {
        true
    }

    /// Whether a disappointing application can be retried with the same outcome
    /// distribution at the same cost ("reroll until it hits").
    ///
    /// True for full/partial reroll methods where the post-application state does
    /// not depend on the pre-application state (Chaos, Alchemy, Alteration,
    /// Essence, Fossil, Divine, Transmutation*): rolling again is an independent
    /// draw from the same distribution. False (the default) for additive or
    /// destructive one-shot methods (Exalted, Regal, Augmentation, Annulment,
    /// Harvest): a miss changes the item, so a retry is a different problem.
    ///
    /// The expected-cost model in the beam search prices repeatable steps at
    /// `cost / P(at least this good)` and one-shot steps at `cost` (with the
    /// hit probability reported separately).
    ///
    /// *Transmutation retries strictly need a Scouring Orb in between; the
    /// approximation ignores that ~1c overhead.
    fn repeatable_on_failure(&self) -> bool {
        false
    }
}

/// Decorator that overrides a method's chaos cost — used by the goal file's
/// `[prices]` table so league-accurate prices don't require code changes.
/// Delegates everything else to the wrapped method.
pub struct Repriced {
    pub inner: std::sync::Arc<dyn CraftingMethod>,
    pub cost: f64,
}

impl CraftingMethod for Repriced {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn cost_chaos(&self) -> f64 {
        self.cost
    }
    fn can_apply(&self, item: &ItemState, db: &GameData) -> bool {
        self.inner.can_apply(item, db)
    }
    fn apply(
        &self,
        item: &ItemState,
        db: &GameData,
        rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        self.inner.apply(item, db, rng)
    }
    fn weights_are_probabilities(&self) -> bool {
        self.inner.weights_are_probabilities()
    }
    fn repeatable_on_failure(&self) -> bool {
        self.inner.repeatable_on_failure()
    }
}
