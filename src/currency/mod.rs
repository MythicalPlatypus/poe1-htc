/// Sample count for stochastic full-reroll currencies (Alchemy, Chaos, Essence, Fossil).
/// These currencies have too many possible outcomes for exact enumeration, so we draw
/// N independent samples. Each sample is returned with weight 1/N.
///
/// IMPORTANT: 1/N is a sample weight, not a true in-game probability. The actual
/// probability of any specific outcome depends on the full mod pool and is not
/// computed here. Success probabilities derived from these weights are Monte Carlo
/// estimates with resolution 1/N and must be labelled as such in reporting.
pub const MONTE_CARLO_SAMPLES: usize = 50;

/// Explicit-affix reroll families whose output distribution ignores the
/// current removable affixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RerollKind {
    MagicExplicit,
    RareExplicit,
}

/// Rare items roll 4/5/6 explicit modifiers with a 7:4:1 distribution.
pub fn random_rare_affix_count(rng: &mut dyn RngCore) -> usize {
    rare_affix_count_from_roll(rand::Rng::random_range(rng, 0..12))
}

fn rare_affix_count_from_roll(roll: u32) -> usize {
    match roll {
        0..=6 => 4,
        7..=10 => 5,
        _ => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::rare_affix_count_from_roll;

    #[test]
    fn rare_affix_count_uses_seven_four_one_distribution() {
        let counts =
            (0..12)
                .map(rare_affix_count_from_roll)
                .fold([0_usize; 3], |mut counts, affixes| {
                    counts[affixes - 4] += 1;
                    counts
                });
        assert_eq!(counts, [7, 4, 1]);
    }
}

pub mod beastcraft;
pub mod bench;
pub mod eldritch;
pub mod essences;
pub mod fossils;
pub mod fracturing;
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

    /// Whether the weighted concrete states returned by `apply` fully enumerate
    /// their in-game probabilities, including numeric modifier rolls.
    ///
    /// Deterministic methods and exact removals return true (the default).
    /// Monte Carlo rerolls and methods that enumerate mod identities while
    /// sampling numeric rolls must return false. Reporting layers use this to
    /// avoid presenting derived probabilities as exact.
    fn weights_are_probabilities(&self) -> bool {
        true
    }

    /// Whether a disappointing application can be retried with the same outcome
    /// distribution at the same cost ("reroll until it hits").
    ///
    /// True for reroll methods where applying the same method again is an
    /// independent draw from the same distribution (Chaos, Alteration, Essence,
    /// Fossil, Divine, Harvest Reforge, Eldritch Chaos). False (the default) for additive or
    /// destructive one-shot methods (Exalted, Regal, Augmentation, Annulment,
    /// Harvest): a miss changes the item, so a retry is a different problem.
    ///
    /// The expected-cost model in the beam search prices repeatable steps at
    /// `cost / P(at least this good)` and one-shot steps at `cost` (with the
    /// hit probability reported separately).
    ///
    fn repeatable_on_failure(&self) -> bool {
        false
    }

    /// Identifies actions that replace every removable explicit modifier.
    /// Search uses this to reject paths that later discard the same setup.
    fn reroll_kind(&self) -> Option<RerollKind> {
        None
    }

    /// Identifies rarity-upgrade actions whose random explicit result can be
    /// discarded by an immediately following reroll of the same family. The
    /// setup action and its cost remain in the path, but its roll quality no
    /// longer contributes a false one-shot failure probability.
    fn reroll_initializer_kind(&self) -> Option<RerollKind> {
        None
    }

    /// Whether this reroll requires the rarity established by a matching
    /// initializer. Rerolls that already accept Normal items must leave the
    /// initializer path dominated by applying the reroll directly.
    fn consumes_reroll_initializer(&self) -> bool {
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
    fn reroll_kind(&self) -> Option<RerollKind> {
        self.inner.reroll_kind()
    }
    fn reroll_initializer_kind(&self) -> Option<RerollKind> {
        self.inner.reroll_initializer_kind()
    }
    fn consumes_reroll_initializer(&self) -> bool {
        self.inner.consumes_reroll_initializer()
    }
}
