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
    use std::collections::HashSet;
    use std::sync::Arc;

    use super::beastcraft::{BestiaryAffixSwapCraft, BestiaryAffixSwapKind};
    use super::bench::{BenchCraft, RemoveCraftedMods};
    use super::eldritch::{
        EldritchChaosOrb, EldritchExaltedOrb, EldritchGod, EldritchOrbOfAnnulment,
    };
    use super::essences::Essence;
    use super::fossils::FossilCraft;
    use super::fracturing::FracturingOrb;
    use super::harvest::{HarvestCraft, HarvestOp, HarvestTarget};
    use super::influence::{ApplyInfluence, AwakenersOrb, ConquerorExaltedOrb, Influence};
    use super::orbs::{
        ChaosOrb, DivineOrb, ExaltedOrb, OrbOfAlchemy, OrbOfAlteration, OrbOfAnnulment,
        OrbOfAugmentation, OrbOfScouring, OrbOfTransmutation, RegalOrb,
    };
    use super::{
        rare_affix_count_from_roll, CraftingMethod, MethodSetup, ProbabilityModel, Repriced,
    };

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

    #[test]
    fn every_concrete_method_has_a_distinct_canonical_semantic_id() {
        let bestiary =
            BestiaryAffixSwapCraft::new(BestiaryAffixSwapKind::AddPrefixRemoveSuffix, 83, 12.0)
                .expect("test beast level should be valid");
        let methods: Vec<(Box<dyn CraftingMethod>, &str)> = vec![
            (Box::new(OrbOfScouring), "currency/scour"),
            (Box::new(OrbOfTransmutation), "currency/transmute"),
            (Box::new(OrbOfAlteration), "currency/alteration"),
            (Box::new(OrbOfAugmentation), "currency/augmentation"),
            (Box::new(RegalOrb), "currency/regal"),
            (Box::new(OrbOfAlchemy), "currency/alchemy"),
            (Box::new(ChaosOrb), "currency/chaos"),
            (Box::new(ExaltedOrb), "currency/exalted"),
            (Box::new(OrbOfAnnulment), "currency/annulment"),
            (Box::new(DivineOrb), "currency/divine"),
            (Box::new(FracturingOrb), "currency/fracturing"),
            (Box::new(RemoveCraftedMods), "bench/remove-crafted"),
            (
                Box::new(BenchCraft {
                    display_name: "Display-only bench name".to_string(),
                    mod_id: "Metadata/Mods/Élite".to_string(),
                    cost_chaos: 99.0,
                }),
                "bench/add-explicit/Metadata%2FMods%2F%C3%89lite",
            ),
            (
                Box::new(Essence {
                    display_name: "Display-only essence name".to_string(),
                    guaranteed_mod_id: "Metadata/Mods/Life%".to_string(),
                    cost_chaos: 99.0,
                    max_item_level: Some(82),
                    can_reforge_rare: true,
                }),
                "essence/apply/Metadata%2FMods%2FLife%25/max-item-level/82/reforge-rare/true",
            ),
            (
                Box::new(FossilCraft {
                    display_name: "Display-only fossil name".to_string(),
                    cost_chaos: 99.0,
                    fossils: vec![],
                }),
                "fossil/resonator/config/%7B%22parts%22%3A%5B%5D%7D",
            ),
            (
                Box::new(HarvestCraft {
                    display_name: "Display-only Harvest name".to_string(),
                    cost_chaos: 99.0,
                    target: HarvestTarget::Defence,
                    op: HarvestOp::Reforge,
                }),
                "harvest/reforge/defence",
            ),
            (
                Box::new(EldritchChaosOrb {
                    god: EldritchGod::SearingExarch,
                }),
                "eldritch/chaos/exarch",
            ),
            (
                Box::new(EldritchExaltedOrb {
                    god: EldritchGod::EaterOfWorlds,
                }),
                "eldritch/exalted/eater",
            ),
            (
                Box::new(EldritchOrbOfAnnulment {
                    god: EldritchGod::SearingExarch,
                }),
                "eldritch/annulment/exarch",
            ),
            (
                Box::new(ConquerorExaltedOrb {
                    influence: Influence::Hunter,
                }),
                "influence/conqueror-exalt/hunter",
            ),
            (
                Box::new(ApplyInfluence {
                    influence: Influence::Shaper,
                }),
                "unsupported/apply-influence/shaper",
            ),
            (
                Box::new(AwakenersOrb {
                    source_influence_a: Influence::Elder,
                    source_influence_b: Influence::Warlord,
                }),
                "unsupported/awakeners-orb/elder/warlord",
            ),
            (
                Box::new(bestiary),
                "bestiary/affix-swap/add-prefix-remove-suffix/beast-level/83",
            ),
        ];

        assert_eq!(methods.len(), 23);
        let mut unique = HashSet::new();
        for (method, expected) in methods {
            let id = method.id();
            assert_eq!(id.as_str(), expected);
            assert!(unique.insert(id.clone()), "duplicate MethodId: {expected}");

            let metadata = method.metadata();
            assert_eq!(metadata.id, id);
            assert_eq!(metadata.display_name, method.name());
            assert!(
                !metadata.description.trim().is_empty(),
                "{expected} needs a user-facing description"
            );
            match metadata.probability_model {
                ProbabilityModel::Exact => assert!(
                    method.weights_are_probabilities(),
                    "{expected} labels sampled outcomes as exact"
                ),
                ProbabilityModel::MonteCarlo { .. }
                | ProbabilityModel::MonteCarloWithApproximation { .. }
                | ProbabilityModel::ExactIdentitySampledRolls => assert!(
                    !method.weights_are_probabilities(),
                    "{expected} labels exact outcomes as sampled"
                ),
                ProbabilityModel::Unavailable => assert_eq!(
                    metadata.setup,
                    MethodSetup::Unsupported,
                    "{expected} labels an available method's probabilities unavailable"
                ),
            }

            if metadata.setup == MethodSetup::Unsupported {
                assert_eq!(
                    metadata.default_price_chaos, None,
                    "{expected} exposes a price despite being unavailable"
                );
            } else {
                let price = metadata
                    .default_price_chaos
                    .expect("available production methods need a positive default price");
                assert!(
                    price.is_finite() && price > 0.0,
                    "{expected} has invalid default price {price}"
                );
            }
        }
    }

    #[test]
    fn repricing_delegates_semantic_identity_and_intrinsic_metadata() {
        let original: Arc<dyn CraftingMethod> = Arc::new(ChaosOrb);
        let repriced = Repriced {
            inner: Arc::clone(&original),
            cost: 37.5,
        };

        assert_eq!(repriced.id(), original.id());
        assert_eq!(repriced.id().as_str(), "currency/chaos");
        assert_eq!(repriced.cost_chaos(), 37.5);
        assert_eq!(
            repriced.default_price_chaos(),
            original.default_price_chaos()
        );
        assert_ne!(repriced.default_price_chaos(), Some(37.5));
        assert_eq!(repriced.metadata(), original.metadata());
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
mod metadata;
mod method_id;
pub mod orbs;

pub use metadata::{
    ItemClassSupport, MethodCatalog, MethodFamily, MethodMetadata, MethodSetup,
    ProbabilityApproximation, ProbabilityModel,
};
pub use method_id::{InvalidMethodId, MethodId};

use crate::data::GameData;
use crate::item::ItemState;
use anyhow::Result;
use rand::RngCore;

/// Every crafting method implements this trait.
/// The beam search engine calls `apply` to generate successor states.
pub trait CraftingMethod: Send + Sync {
    /// Stable semantic identity, independent of display name and price.
    ///
    /// The returned value must not change during this method instance's
    /// lifetime; search engines may cache it when the method is registered.
    fn id(&self) -> MethodId;

    /// Broad crafting-system grouping used by registry selectors.
    fn family(&self) -> MethodFamily;

    /// Human-readable name of this crafting operation (e.g. "Chaos Orb").
    fn name(&self) -> &str;

    /// Concise user-facing explanation of the operation.
    fn description(&self) -> &str;

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

    /// Modifier IDs this method can introduce without relying on the normal
    /// positive-weight affix pool.
    ///
    /// This preparation-time capability supports conservative impossible-goal
    /// detection. Returning an ID means the method may provide it, not that the
    /// method is always applicable or guarantees a completed goal.
    fn provided_mod_ids(&self) -> Vec<&str> {
        Vec::new()
    }

    /// How this method becomes available to an optimizer request.
    fn setup(&self) -> MethodSetup {
        MethodSetup::BuiltIn
    }

    /// Coarse item-class support exposed by the method registry.
    fn item_class_support(&self) -> ItemClassSupport {
        ItemClassSupport::AnyCraftable
    }

    /// Quality of the outcome probabilities returned by this method.
    fn probability_model(&self) -> ProbabilityModel {
        if self.weights_are_probabilities() {
            ProbabilityModel::Exact
        } else {
            ProbabilityModel::MonteCarlo {
                samples: MONTE_CARLO_SAMPLES,
            }
        }
    }

    /// Built-in or configured baseline price before request-level repricing.
    fn default_price_chaos(&self) -> Option<f64> {
        let cost = self.cost_chaos();
        (cost.is_finite() && cost > 0.0).then_some(cost)
    }

    /// Build an owned description suitable for adapters and registry clients.
    fn metadata(&self) -> MethodMetadata {
        MethodMetadata {
            id: self.id(),
            display_name: self.name().to_string(),
            family: self.family(),
            description: self.description().to_string(),
            default_price_chaos: self.default_price_chaos(),
            setup: self.setup(),
            item_class_support: self.item_class_support(),
            probability_model: self.probability_model(),
        }
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
    fn id(&self) -> MethodId {
        self.inner.id()
    }

    fn family(&self) -> MethodFamily {
        self.inner.family()
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        self.inner.description()
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
    fn provided_mod_ids(&self) -> Vec<&str> {
        self.inner.provided_mod_ids()
    }
    fn setup(&self) -> MethodSetup {
        self.inner.setup()
    }
    fn item_class_support(&self) -> ItemClassSupport {
        self.inner.item_class_support()
    }
    fn probability_model(&self) -> ProbabilityModel {
        self.inner.probability_model()
    }
    fn default_price_chaos(&self) -> Option<f64> {
        self.inner.default_price_chaos()
    }
    fn metadata(&self) -> MethodMetadata {
        self.inner.metadata()
    }
}
