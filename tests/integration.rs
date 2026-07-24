//! Integration tests (milestones M6-M7).
//!
//! Covers, with synthetic `GameData` (no data files needed):
//!   1. Beam-search ranking — `cost_weight` changes which path wins.
//!   2. Multi-step path construction and cost/probability bookkeeping.
//!   3. Probability invariants — every `CraftingMethod::apply` returns weights
//!      that sum to 1.0 (true probabilities for exact methods, 1/N sample
//!      weights for Monte Carlo methods).
//!   4. Forced-mod legality — Essence/Fossil applications are rejected when the
//!      guaranteed/forced mod conflicts with a fractured mod's group or when
//!      fractured mods leave no open affix slot.
//!   5. The basic-orb progression (Transmute/Alter/Augment/Regal/Divine) and
//!      bench crafts.
//!   6. The expected-cost model (repeatable vs one-shot retry semantics) and
//!      seeded-search reproducibility.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use poe1_htc::currency::{
    bench::BenchCraft,
    essences::Essence,
    fossils::{FossilCraft, FossilModifier},
    harvest::{HarvestCraft, HarvestOp, HarvestTarget},
    orbs::{
        ChaosOrb, DivineOrb, ExaltedOrb, OrbOfAlchemy, OrbOfAlteration, OrbOfAnnulment,
        OrbOfAugmentation, OrbOfTransmutation, RegalOrb,
    },
    CraftingMethod, Repriced, MONTE_CARLO_SAMPLES,
};
use poe1_htc::data::{
    mods::{Domain, GenerationType, Mod, ModStat, SpawnWeight},
    GameData,
};
use poe1_htc::item::{
    modifier::{Modifier, StatRoll},
    state::{ItemState, Rarity},
};
use poe1_htc::search::beam::{expected_cost_with_restarts, BeamConfig, BeamSearch, PathStep};

// ─── helpers ─────────────────────────────────────────────────────────────────

/// A craftable test mod. `mod_type` doubles as its (single) group, so two mods
/// built with the same `mod_type` conflict — mirroring real RePoE group data.
fn make_mod(gen_type: GenerationType, mod_type: &str, tag: &str, weight: u32) -> Mod {
    Mod {
        name: format!("Test {mod_type}"),
        generation_type: gen_type,
        required_level: 1,
        stats: vec![ModStat {
            id: format!("stat_{mod_type}"),
            min: 1,
            max: 10,
        }],
        spawn_weights: vec![SpawnWeight {
            tag: tag.to_string(),
            weight,
        }],
        generation_weights: vec![],
        adds_tags: vec![],
        tags: vec![],
        domain: Domain::Item,
        mod_type: mod_type.to_string(),
        groups: vec![mod_type.to_string()],
        is_essence_only: false,
    }
}

fn db_from(mods: Vec<(&str, Mod)>) -> GameData {
    GameData::new(
        mods.into_iter()
            .map(|(id, m)| (id.to_string(), m))
            .collect(),
        HashMap::new(),
    )
}

fn empty_db() -> &'static GameData {
    static EMPTY_DB: std::sync::OnceLock<GameData> = std::sync::OnceLock::new();
    EMPTY_DB.get_or_init(|| GameData::new(HashMap::new(), HashMap::new()))
}

/// A varied pool: 4 prefixes and 4 suffixes with distinct groups and weights.
fn varied_db() -> GameData {
    let mut mods = HashMap::new();
    for i in 0..4u32 {
        mods.insert(
            format!("P{i}"),
            make_mod(
                GenerationType::Prefix,
                &format!("PT{i}"),
                "sword",
                100 * (i + 1),
            ),
        );
        mods.insert(
            format!("S{i}"),
            make_mod(
                GenerationType::Suffix,
                &format!("ST{i}"),
                "sword",
                50 * (i + 1),
            ),
        );
    }
    GameData::new(mods, HashMap::new())
}

/// `varied_db` plus extra entries, rebuilt so the craftable index stays fresh.
fn varied_db_with(extra: Vec<(&str, Mod)>) -> GameData {
    let base = varied_db();
    let mut mods = base.mods;
    for (id, m) in extra {
        mods.insert(id.to_string(), m);
    }
    GameData::new(mods, HashMap::new())
}

fn sword(rarity: Rarity) -> ItemState {
    let mut item = ItemState::new_base("sword", vec!["sword".to_string()], 84);
    item.rarity = rarity;
    item
}

fn rare_sword() -> ItemState {
    sword(Rarity::Rare)
}

fn marker(mod_id: &str, gen_type: GenerationType) -> Modifier {
    Modifier {
        mod_id: mod_id.to_string(),
        generation_type: gen_type,
        rolls: vec![StatRoll {
            stat_id: "marker".to_string(),
            value: 1,
        }],
    }
}

fn test_rng() -> StdRng {
    StdRng::seed_from_u64(0xC0FFEE)
}

fn assert_weights_sum_to_one(outcomes: &[(ItemState, f64)], context: &str) {
    assert!(!outcomes.is_empty(), "{context}: no outcomes returned");
    let sum: f64 = outcomes.iter().map(|(_, p)| p).sum();
    assert!(
        (sum - 1.0).abs() < 1e-9,
        "{context}: weights sum to {sum}, expected 1.0"
    );
    for (_, p) in outcomes {
        assert!(
            *p > 0.0 && *p <= 1.0,
            "{context}: weight {p} outside (0, 1]"
        );
    }
}

fn step_names(result: &poe1_htc::search::beam::SearchResult) -> Vec<&str> {
    result.steps.iter().map(|s| s.method.as_str()).collect()
}

// ─── 1 + 2: beam-search ranking ─────────────────────────────────────────────

/// One-shot synthetic method: applies once to an unmodified item and stamps a
/// marker prefix carrying its own name, so the score function can tell paths apart.
struct StampMethod {
    method_name: &'static str,
    cost: f64,
}

impl CraftingMethod for StampMethod {
    fn name(&self) -> &str {
        self.method_name
    }
    fn cost_chaos(&self) -> f64 {
        self.cost
    }
    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.prefixes.is_empty()
    }
    fn apply(
        &self,
        item: &ItemState,
        _db: &GameData,
        _rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        let mut next = item.clone();
        next.prefixes
            .push(marker(self.method_name, GenerationType::Prefix));
        Ok(vec![(next, 1.0)])
    }
}

/// Scores a state stamped by `StampMethod`: "pricey" beats "cheap" on raw score.
fn stamp_score(state: &ItemState) -> f64 {
    match state.prefixes.first().map(|m| m.mod_id.as_str()) {
        Some("cheap") => 5.0,
        Some("pricey") => 6.0,
        _ => 0.0,
    }
}

fn stamp_search(cost_weight: f64) -> BeamSearch<'static> {
    let methods: Vec<Arc<dyn CraftingMethod>> = vec![
        Arc::new(StampMethod {
            method_name: "cheap",
            cost: 1.0,
        }),
        Arc::new(StampMethod {
            method_name: "pricey",
            cost: 10.0,
        }),
    ];
    BeamSearch::new(
        BeamConfig {
            beam_width: 10,
            max_steps: 3,
            cost_weight,
            seed: None,
        },
        empty_db(),
        methods,
    )
}

#[test]
fn cost_weight_zero_picks_highest_raw_score() {
    let result = stamp_search(0.0).run(rare_sword(), stamp_score).unwrap();
    assert_eq!(
        step_names(&result),
        vec!["pricey"],
        "with cost ignored, the higher-scoring method must win"
    );
    assert_eq!(result.total_cost, 10.0);
    assert_eq!(result.score, 6.0);
}

#[test]
fn cost_weight_changes_winner() {
    // cheap: 5 - 1.0*1 = 4.0  |  pricey: 6 - 1.0*10 = -4.0
    let result = stamp_search(1.0).run(rare_sword(), stamp_score).unwrap();
    assert_eq!(
        step_names(&result),
        vec!["cheap"],
        "with cost_weight=1.0 the cheap method must win"
    );
    assert_eq!(result.total_cost, 1.0);
    assert_eq!(result.score, 4.0);
}

/// Deterministic method that adds one marker prefix per application (up to 3).
struct AddOne;

impl CraftingMethod for AddOne {
    fn name(&self) -> &str {
        "Add One"
    }
    fn cost_chaos(&self) -> f64 {
        1.0
    }
    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.prefixes.len() < 3
    }
    fn apply(
        &self,
        item: &ItemState,
        _db: &GameData,
        _rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        let mut next = item.clone();
        let id = format!("m{}", next.prefixes.len());
        next.prefixes.push(marker(&id, GenerationType::Prefix));
        Ok(vec![(next, 1.0)])
    }
}

#[test]
fn beam_search_builds_multistep_path() {
    let search = BeamSearch::new(
        BeamConfig {
            beam_width: 4,
            max_steps: 5,
            cost_weight: 0.0,
            seed: None,
        },
        empty_db(),
        vec![Arc::new(AddOne) as Arc<dyn CraftingMethod>],
    );
    let result = search
        .run(rare_sword(), |s| s.prefixes.len() as f64)
        .unwrap();

    assert_eq!(
        step_names(&result),
        vec!["Add One"; 3],
        "must chain exactly 3 applications"
    );
    assert_eq!(result.total_cost, 3.0);
    assert_eq!(
        result.expected_cost, 3.0,
        "deterministic steps cost exactly once"
    );
    assert_eq!(result.score, 3.0);
    // All steps are deterministic (p = 1.0) → certain success.
    assert!((result.success_prob - 1.0).abs() < 1e-12);
}

// ─── 3: probability invariants ───────────────────────────────────────────────

#[test]
fn exalted_orb_probabilities_sum_to_one() {
    let db = varied_db();
    let outcomes = ExaltedOrb
        .apply(&rare_sword(), &db, &mut test_rng())
        .unwrap();
    // One outcome per eligible mod (8 in the pool, empty item → all eligible).
    assert_eq!(outcomes.len(), 8);
    assert_weights_sum_to_one(&outcomes, "Exalted Orb");
    assert!(
        !ExaltedOrb.weights_are_probabilities(),
        "Exalted mod selection is exact, but numeric rolls are sampled"
    );
    assert!(
        !ExaltedOrb.repeatable_on_failure(),
        "a bad exalt slam cannot be rerolled"
    );
}

#[test]
fn annulment_probabilities_sum_to_one_and_are_uniform() {
    let db = varied_db();
    let mut item = rare_sword();
    item.prefixes.push(marker("P0", GenerationType::Prefix));
    item.prefixes.push(marker("P1", GenerationType::Prefix));
    item.suffixes.push(marker("S0", GenerationType::Suffix));
    item.crafted_mod = Some(marker("C0", GenerationType::Prefix));

    let outcomes = OrbOfAnnulment.apply(&item, &db, &mut test_rng()).unwrap();
    assert_eq!(
        outcomes.len(),
        4,
        "2 prefixes + 1 suffix + 1 crafted = 4 removal targets"
    );
    assert_weights_sum_to_one(&outcomes, "Orb of Annulment");
    for (_, p) in &outcomes {
        assert!(
            (p - 0.25).abs() < 1e-12,
            "each removal must be equally likely, got {p}"
        );
    }
}

#[test]
fn chaos_orb_sample_weights_sum_to_one() {
    let db = varied_db();
    let outcomes = ChaosOrb.apply(&rare_sword(), &db, &mut test_rng()).unwrap();
    assert_eq!(outcomes.len(), MONTE_CARLO_SAMPLES);
    assert_weights_sum_to_one(&outcomes, "Chaos Orb");
    assert!(
        !ChaosOrb.weights_are_probabilities(),
        "Chaos Orb must report its weights as Monte Carlo samples"
    );
    assert!(
        ChaosOrb.repeatable_on_failure(),
        "a bad chaos roll can simply be rerolled"
    );
}

#[test]
fn alchemy_produces_valid_rares() {
    let db = varied_db();
    let normal = sword(Rarity::Normal);
    let outcomes = OrbOfAlchemy.apply(&normal, &db, &mut test_rng()).unwrap();
    assert_weights_sum_to_one(&outcomes, "Orb of Alchemy");
    for (state, _) in &outcomes {
        assert_eq!(state.rarity, Rarity::Rare);
        let n = state.prefixes.len() + state.suffixes.len();
        assert!(
            (4..=6).contains(&n),
            "alchemy rare must have 4-6 mods, got {n}"
        );
        assert!(state.prefixes.len() <= 3 && state.suffixes.len() <= 3);
    }
}

#[test]
fn harvest_augment_probabilities_sum_to_one() {
    // Two life-tagged suffixes with different weights.
    let mut m1 = make_mod(GenerationType::Suffix, "LifeA", "sword", 100);
    m1.tags = vec!["life".to_string()];
    let mut m2 = make_mod(GenerationType::Suffix, "LifeB", "sword", 300);
    m2.tags = vec!["life".to_string()];
    let db = db_from(vec![("LA", m1), ("LB", m2)]);

    let craft = HarvestCraft {
        display_name: "Augment Life".to_string(),
        cost_chaos: 30.0,
        target: HarvestTarget::Life,
        op: HarvestOp::Augment,
    };
    let mut item = rare_sword();
    item.prefixes.push(marker("junk", GenerationType::Prefix));
    let outcomes = craft.apply(&item, &db, &mut test_rng()).unwrap();
    assert_eq!(outcomes.len(), 2);
    assert_weights_sum_to_one(&outcomes, "Harvest Augment");
    // Weighted 100 vs 300 → probabilities 0.25 and 0.75.
    let mut probs: Vec<f64> = outcomes.iter().map(|(_, p)| *p).collect();
    probs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!((probs[0] - 0.25).abs() < 1e-12 && (probs[1] - 0.75).abs() < 1e-12);
}

// ─── 4: forced-mod legality ──────────────────────────────────────────────────

/// An item whose fractured mod occupies the "PT0" group.
fn item_with_fractured_pt0() -> ItemState {
    let mut item = rare_sword();
    item.fractured.push(marker("P0", GenerationType::Prefix));
    item
}

#[test]
fn essence_blocked_by_fractured_group_conflict() {
    // Essence guarantees "P0alt", which shares group "PT0" with the fractured "P0".
    let db = varied_db_with(vec![(
        "P0alt",
        make_mod(GenerationType::Prefix, "PT0", "sword", 100),
    )]);
    let essence = Essence {
        display_name: "Essence of Conflict".to_string(),
        guaranteed_mod_id: "P0alt".to_string(),
        cost_chaos: 5.0,
    };
    let err = essence
        .apply(&item_with_fractured_pt0(), &db, &mut test_rng())
        .unwrap_err();
    assert!(
        err.to_string().contains("shares a mod group"),
        "expected group-conflict error, got: {err}"
    );
}

#[test]
fn essence_blocked_when_fractured_mods_fill_slots() {
    let db = varied_db_with(vec![(
        "NewPrefix",
        make_mod(GenerationType::Prefix, "PTnew", "sword", 100),
    )]);
    let mut item = rare_sword();
    for i in 0..3 {
        item.fractured
            .push(marker(&format!("P{i}"), GenerationType::Prefix));
    }
    let essence = Essence {
        display_name: "Essence of Overflow".to_string(),
        guaranteed_mod_id: "NewPrefix".to_string(),
        cost_chaos: 5.0,
    };
    let err = essence.apply(&item, &db, &mut test_rng()).unwrap_err();
    assert!(
        err.to_string().contains("no open prefix slot"),
        "expected slot-overflow error, got: {err}"
    );
}

#[test]
fn essence_places_guaranteed_mod_in_every_outcome() {
    let db = varied_db();
    let essence = Essence {
        display_name: "Essence of Testing".to_string(),
        guaranteed_mod_id: "P2".to_string(),
        cost_chaos: 5.0,
    };
    let outcomes = essence.apply(&rare_sword(), &db, &mut test_rng()).unwrap();
    assert_weights_sum_to_one(&outcomes, "Essence");
    for (state, _) in &outcomes {
        assert!(
            state.prefixes.iter().any(|m| m.mod_id == "P2"),
            "guaranteed mod P2 missing from an essence outcome"
        );
    }
}

#[test]
fn fossil_forced_mod_blocked_by_fractured_group_conflict() {
    let db = varied_db_with(vec![(
        "P0alt",
        make_mod(GenerationType::Prefix, "PT0", "sword", 100),
    )]);
    let craft = FossilCraft {
        display_name: "Conflicting Resonator".to_string(),
        cost_chaos: 10.0,
        fossils: vec![FossilModifier {
            boosted_tags: vec![],
            reduced_tags: vec![],
            blocked_mod_ids: vec![],
            forced_mod_ids: vec!["P0alt".to_string()],
        }],
    };
    let err = craft
        .apply(&item_with_fractured_pt0(), &db, &mut test_rng())
        .unwrap_err();
    assert!(
        err.to_string().contains("shares a mod group"),
        "expected group-conflict error, got: {err}"
    );
}

#[test]
fn fossil_forced_mod_blocked_by_slot_overflow() {
    let db = varied_db_with(vec![(
        "NewPrefix",
        make_mod(GenerationType::Prefix, "PTnew", "sword", 100),
    )]);
    let mut item = rare_sword();
    for i in 0..3 {
        item.fractured
            .push(marker(&format!("P{i}"), GenerationType::Prefix));
    }
    let craft = FossilCraft {
        display_name: "Overflowing Resonator".to_string(),
        cost_chaos: 10.0,
        fossils: vec![FossilModifier {
            boosted_tags: vec![],
            reduced_tags: vec![],
            blocked_mod_ids: vec![],
            forced_mod_ids: vec!["NewPrefix".to_string()],
        }],
    };
    let err = craft.apply(&item, &db, &mut test_rng()).unwrap_err();
    assert!(
        err.to_string().contains("no open prefix slot"),
        "expected slot-overflow error, got: {err}"
    );
}

#[test]
fn fossil_forced_mods_conflict_with_each_other() {
    // Two forced mods sharing group "PTX" must be rejected even without fractured mods.
    let db = varied_db_with(vec![
        ("X1", make_mod(GenerationType::Prefix, "PTX", "sword", 100)),
        ("X2", make_mod(GenerationType::Suffix, "PTX", "sword", 100)),
    ]);
    let craft = FossilCraft {
        display_name: "Self-Conflicting Resonator".to_string(),
        cost_chaos: 10.0,
        fossils: vec![FossilModifier {
            boosted_tags: vec![],
            reduced_tags: vec![],
            blocked_mod_ids: vec![],
            forced_mod_ids: vec!["X1".to_string(), "X2".to_string()],
        }],
    };
    let err = craft
        .apply(&rare_sword(), &db, &mut test_rng())
        .unwrap_err();
    assert!(
        err.to_string().contains("shares a mod group"),
        "expected forced-vs-forced conflict error, got: {err}"
    );
}

// ─── 5: basic-orb progression and bench crafts ───────────────────────────────

#[test]
fn transmutation_makes_magic_with_one_or_two_mods() {
    let db = varied_db();
    let outcomes = OrbOfTransmutation
        .apply(&sword(Rarity::Normal), &db, &mut test_rng())
        .unwrap();
    assert_weights_sum_to_one(&outcomes, "Orb of Transmutation");
    for (state, _) in &outcomes {
        assert_eq!(state.rarity, Rarity::Magic);
        let n = state.prefixes.len() + state.suffixes.len();
        assert!(
            (1..=2).contains(&n),
            "magic item must have 1-2 mods, got {n}"
        );
        assert!(state.prefixes.len() <= 1 && state.suffixes.len() <= 1);
    }
}

#[test]
fn alteration_rerolls_magic_and_removes_crafted_mod() {
    let db = varied_db();
    let mut item = sword(Rarity::Magic);
    item.prefixes.push(marker("P0", GenerationType::Prefix));

    let outcomes = OrbOfAlteration.apply(&item, &db, &mut test_rng()).unwrap();
    assert_weights_sum_to_one(&outcomes, "Orb of Alteration");
    for (state, _) in &outcomes {
        assert_eq!(state.rarity, Rarity::Magic);
        let n = state.prefixes.len() + state.suffixes.len();
        assert!((1..=2).contains(&n));
    }

    item.crafted_mod = Some(marker("C0", GenerationType::Suffix));
    let outcomes = OrbOfAlteration
        .apply(&item, &db, &mut test_rng())
        .expect("Alteration should reroll the crafted modifier away");
    assert!(outcomes
        .iter()
        .all(|(state, _)| state.crafted_mod.is_none()));
}

#[test]
fn augmentation_adds_exactly_one_mod_with_exact_probabilities() {
    let db = varied_db();
    let mut item = sword(Rarity::Magic);
    item.prefixes.push(marker("P0", GenerationType::Prefix));

    let outcomes = OrbOfAugmentation
        .apply(&item, &db, &mut test_rng())
        .unwrap();
    assert_weights_sum_to_one(&outcomes, "Orb of Augmentation");
    for (state, _) in &outcomes {
        assert_eq!(state.rarity, Rarity::Magic);
        // Prefix slot full on a magic item → only suffixes can be added.
        assert_eq!(state.prefixes.len(), 1);
        assert_eq!(state.suffixes.len(), 1);
    }
    // Full magic item (1 prefix + 1 suffix) cannot be augmented.
    let mut full = sword(Rarity::Magic);
    full.prefixes.push(marker("P0", GenerationType::Prefix));
    full.suffixes.push(marker("S0", GenerationType::Suffix));
    assert!(!OrbOfAugmentation.can_apply(&full, &db));
}

#[test]
fn regal_upgrades_to_rare_and_adds_one_mod() {
    let db = varied_db();
    let mut item = sword(Rarity::Magic);
    item.prefixes.push(marker("P0", GenerationType::Prefix));
    item.suffixes.push(marker("S0", GenerationType::Suffix));

    let outcomes = RegalOrb.apply(&item, &db, &mut test_rng()).unwrap();
    assert_weights_sum_to_one(&outcomes, "Regal Orb");
    for (state, _) in &outcomes {
        assert_eq!(state.rarity, Rarity::Rare);
        assert_eq!(
            state.prefixes.len() + state.suffixes.len(),
            3,
            "regal must add exactly one mod to the 2-mod magic item"
        );
    }
}

#[test]
fn divine_rerolls_values_but_keeps_mods() {
    let db = varied_db();
    let mut item = rare_sword();
    // Place P0 with an out-of-range sentinel value; divine must reroll it into [1, 10].
    item.prefixes.push(Modifier {
        mod_id: "P0".to_string(),
        generation_type: GenerationType::Prefix,
        rolls: vec![StatRoll {
            stat_id: "stat_PT0".to_string(),
            value: 999,
        }],
    });

    let outcomes = DivineOrb.apply(&item, &db, &mut test_rng()).unwrap();
    assert_weights_sum_to_one(&outcomes, "Divine Orb");
    for (state, _) in &outcomes {
        assert_eq!(
            state.prefixes.len(),
            1,
            "divine must not add or remove mods"
        );
        assert_eq!(
            state.prefixes[0].mod_id, "P0",
            "divine must not change mod identity"
        );
        let v = state.prefixes[0].rolls[0].value;
        assert!(
            (1..=10).contains(&v),
            "rerolled value {v} outside the mod's [1, 10] range"
        );
    }
}

#[test]
fn bench_craft_sets_crafted_mod_deterministically() {
    let mut crafted = make_mod(GenerationType::Suffix, "CraftRes", "sword", 0);
    crafted.domain = Domain::Crafted;
    let db = varied_db_with(vec![("BenchRes", crafted)]);
    let bench = BenchCraft {
        display_name: "Craft Resistance".to_string(),
        mod_id: "BenchRes".to_string(),
        cost_chaos: 2.0,
    };
    let item = rare_sword();
    let outcomes = bench.apply(&item, &db, &mut test_rng()).unwrap();
    assert_eq!(outcomes.len(), 1, "bench crafts are deterministic");
    assert!((outcomes[0].1 - 1.0).abs() < 1e-12);
    let crafted_mod = outcomes[0]
        .0
        .crafted_mod
        .as_ref()
        .expect("crafted mod must be set");
    assert_eq!(crafted_mod.mod_id, "BenchRes");

    // A second bench craft on the result must be rejected (one crafted mod max).
    assert!(!bench.can_apply(&outcomes[0].0, &db));
}

#[test]
fn bench_craft_blocked_by_group_conflict() {
    // Bench version of the PT0 group, which the item already has via "P0".
    let mut crafted = make_mod(GenerationType::Suffix, "PT0", "sword", 0);
    crafted.domain = Domain::Crafted;
    let db = varied_db_with(vec![("BenchPT0", crafted)]);
    let bench = BenchCraft {
        display_name: "Craft PT0".to_string(),
        mod_id: "BenchPT0".to_string(),
        cost_chaos: 2.0,
    };
    let mut item = rare_sword();
    item.prefixes.push(marker("P0", GenerationType::Prefix));
    let err = bench.apply(&item, &db, &mut test_rng()).unwrap_err();
    assert!(
        err.to_string().contains("shares a mod group"),
        "expected group-conflict error, got: {err}"
    );
}

#[test]
fn crafted_domain_mods_never_appear_in_random_pools() {
    let mut m = make_mod(GenerationType::Prefix, "CraftOnly", "sword", 1000);
    m.domain = Domain::Crafted;
    let db = db_from(vec![("CraftOnly", m)]);
    let outcomes = ExaltedOrb.apply(&rare_sword(), &db, &mut test_rng());
    assert!(
        outcomes.is_err(),
        "a pool containing only crafted-domain mods must be empty for exalts"
    );
}

// ─── 6: expected-cost model and reproducibility ──────────────────────────────

/// Synthetic coin flip: 25% "good" (score 10), 75% "bad" (score 0), cost 8c.
struct CoinFlip {
    repeatable: bool,
}

impl CraftingMethod for CoinFlip {
    fn name(&self) -> &str {
        "Coin Flip"
    }
    fn cost_chaos(&self) -> f64 {
        8.0
    }
    fn repeatable_on_failure(&self) -> bool {
        self.repeatable
    }
    fn can_apply(&self, item: &ItemState, _db: &GameData) -> bool {
        item.prefixes.is_empty()
    }
    fn apply(
        &self,
        item: &ItemState,
        _db: &GameData,
        _rng: &mut dyn RngCore,
    ) -> Result<Vec<(ItemState, f64)>> {
        let mut good = item.clone();
        good.prefixes.push(marker("good", GenerationType::Prefix));
        let mut bad = item.clone();
        bad.prefixes.push(marker("bad", GenerationType::Prefix));
        Ok(vec![(good, 0.25), (bad, 0.75)])
    }
}

fn coin_score(state: &ItemState) -> f64 {
    match state.prefixes.first().map(|m| m.mod_id.as_str()) {
        Some("good") => 10.0,
        _ => 0.0,
    }
}

fn run_coin_flip(repeatable: bool) -> poe1_htc::search::beam::SearchResult {
    let search = BeamSearch::new(
        BeamConfig {
            beam_width: 4,
            max_steps: 1,
            cost_weight: 0.0,
            seed: None,
        },
        empty_db(),
        vec![Arc::new(CoinFlip { repeatable }) as Arc<dyn CraftingMethod>],
    );
    search.run(rare_sword(), coin_score).unwrap()
}

#[test]
fn repeatable_step_prices_in_expected_retries() {
    let result = run_coin_flip(true);
    assert_eq!(result.steps.len(), 1);
    let step = &result.steps[0];
    assert!(
        (step.p_at_least - 0.25).abs() < 1e-12,
        "P(>= good) must be 0.25"
    );
    assert_eq!(result.total_cost, 8.0, "one application costs 8c");
    assert!(
        (result.expected_cost - 32.0).abs() < 1e-9,
        "expected cost must be cost/p = 8/0.25 = 32, got {}",
        result.expected_cost
    );
    assert!(
        (result.success_prob - 1.0).abs() < 1e-12,
        "repeatable steps always succeed eventually"
    );
}

#[test]
fn oneshot_step_reports_hit_probability_instead() {
    let result = run_coin_flip(false);
    assert_eq!(result.total_cost, 8.0);
    assert_eq!(
        result.expected_cost, 8.0,
        "one-shot steps cost exactly once"
    );
    assert!(
        (result.success_prob - 0.25).abs() < 1e-12,
        "one-shot hit chance must be 0.25, got {}",
        result.success_prob
    );
}

fn step(cost: f64, p_at_least: f64, repeatable: bool) -> PathStep {
    PathStep {
        method: "step".to_string(),
        cost,
        p_at_least,
        repeatable,
        mc_estimate: false,
    }
}

#[test]
fn run_k_returns_distinct_pathways_best_first() {
    let results = stamp_search(0.0).run_k(rare_sword(), stamp_score, 5);
    // Only two distinct method sequences exist: ["pricey"] and ["cheap"].
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].steps[0].method, "pricey", "best pathway first");
    assert_eq!(results[1].steps[0].method, "cheap");
    assert_eq!(results[0].score, 6.0);
    assert_eq!(results[1].score, 5.0);
}

#[test]
fn restart_cost_formula_matches_hand_computation() {
    // Deterministic 2c stage, then a 50% one-shot 8c slam.
    // One run costs 2 + 8 = 10 and completes half the time -> 20 expected.
    let steps = vec![step(2.0, 1.0, false), step(8.0, 0.5, false)];
    assert!((expected_cost_with_restarts(&steps) - 20.0).abs() < 1e-9);

    // Reroll stage (1c at 25% -> 4c expected per run), then the same slam.
    // One run costs 4 + 8 = 12, completes half the time -> 24 expected.
    let steps = vec![step(1.0, 0.25, true), step(8.0, 0.5, false)];
    assert!((expected_cost_with_restarts(&steps) - 24.0).abs() < 1e-9);

    // A failed FIRST slam means the second stage is never paid on that run:
    // R = 8 + 0.5 * 8 = 12, P = 0.25 -> 48 expected.
    let steps = vec![step(8.0, 0.5, false), step(8.0, 0.5, false)];
    assert!((expected_cost_with_restarts(&steps) - 48.0).abs() < 1e-9);

    // All-repeatable paths never restart: identical to plain expected cost.
    let steps = vec![step(1.0, 0.1, true)];
    assert!((expected_cost_with_restarts(&steps) - 10.0).abs() < 1e-9);
}

#[test]
fn repriced_overrides_cost_and_delegates_everything_else() {
    let repriced = Repriced {
        inner: Arc::new(ChaosOrb),
        cost: 3.5,
    };
    assert_eq!(repriced.cost_chaos(), 3.5);
    assert_eq!(repriced.name(), "Chaos Orb");
    assert!(!repriced.weights_are_probabilities());
    assert!(repriced.repeatable_on_failure());

    let db = varied_db();
    let outcomes = repriced.apply(&rare_sword(), &db, &mut test_rng()).unwrap();
    assert_eq!(
        outcomes.len(),
        MONTE_CARLO_SAMPLES,
        "apply must delegate to the inner method"
    );
}

#[test]
fn seeded_searches_are_reproducible() {
    let db = varied_db();
    let run = || {
        let search = BeamSearch::new(
            BeamConfig {
                beam_width: 5,
                max_steps: 3,
                cost_weight: 0.01,
                seed: Some(7),
            },
            &db,
            vec![
                Arc::new(ChaosOrb) as Arc<dyn CraftingMethod>,
                Arc::new(ExaltedOrb) as Arc<dyn CraftingMethod>,
            ],
        );
        search
            .run(rare_sword(), |s| {
                (s.prefixes.len() + s.suffixes.len()) as f64
            })
            .unwrap()
    };
    let a = run();
    let b = run();
    assert_eq!(
        a.score, b.score,
        "seeded runs must produce identical scores"
    );
    assert_eq!(step_names(&a), step_names(&b));
    assert_eq!(
        a.state.prefixes, b.state.prefixes,
        "seeded runs must produce identical prefixes (ids and roll values)"
    );
    assert_eq!(a.state.suffixes, b.state.suffixes);
}
