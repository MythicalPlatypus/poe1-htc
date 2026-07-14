//! Integration tests (milestone M6).
//!
//! Covers, with synthetic `GameData` (no data files needed):
//!   1. Beam-search ranking — `cost_weight` changes which path wins.
//!   2. Multi-step path construction and exact `path_weight` bookkeeping.
//!   3. Probability invariants — every `CraftingMethod::apply` returns weights
//!      that sum to 1.0 (true probabilities for exact methods, 1/N sample
//!      weights for Monte Carlo methods).
//!   4. Forced-mod legality — Essence/Fossil applications are rejected when the
//!      guaranteed/forced mod conflicts with a fractured mod's group or when
//!      fractured mods leave no open affix slot.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use poe1_htc::currency::{
    essences::Essence,
    fossils::{FossilCraft, FossilModifier},
    harvest::{HarvestCraft, HarvestOp, HarvestTarget},
    orbs::{ChaosOrb, ExaltedOrb, OrbOfAlchemy, OrbOfAnnulment},
    CraftingMethod, MONTE_CARLO_SAMPLES,
};
use poe1_htc::data::{
    mods::{Domain, GenerationType, Mod, ModStat, SpawnWeight},
    GameData,
};
use poe1_htc::item::{
    modifier::{Modifier, StatRoll},
    state::{ItemState, Rarity},
};
use poe1_htc::search::beam::{BeamConfig, BeamSearch};

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
    GameData {
        mods: mods
            .into_iter()
            .map(|(id, m)| (id.to_string(), m))
            .collect(),
        base_items: HashMap::new(),
    }
}

/// A varied pool: 4 prefixes and 4 suffixes with distinct groups and weights.
fn varied_db() -> GameData {
    let mut mods = Vec::new();
    for i in 0..4u32 {
        mods.push((
            format!("P{i}"),
            make_mod(
                GenerationType::Prefix,
                &format!("PT{i}"),
                "sword",
                100 * (i + 1),
            ),
        ));
        mods.push((
            format!("S{i}"),
            make_mod(
                GenerationType::Suffix,
                &format!("ST{i}"),
                "sword",
                50 * (i + 1),
            ),
        ));
    }
    GameData {
        mods: mods.into_iter().collect(),
        base_items: HashMap::new(),
    }
}

fn rare_sword() -> ItemState {
    let mut item = ItemState::new_base("sword", vec!["sword".to_string()], 84);
    item.rarity = Rarity::Rare;
    item
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
    fn apply(&self, item: &ItemState, _db: &GameData) -> Result<Vec<(ItemState, f64)>> {
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
    static EMPTY_DB: std::sync::OnceLock<GameData> = std::sync::OnceLock::new();
    let db = EMPTY_DB.get_or_init(|| GameData {
        mods: HashMap::new(),
        base_items: HashMap::new(),
    });
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
        },
        db,
        methods,
    )
}

#[test]
fn cost_weight_zero_picks_highest_raw_score() {
    let result = stamp_search(0.0).run(rare_sword(), stamp_score).unwrap();
    assert_eq!(
        result.path,
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
        result.path,
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
    fn apply(&self, item: &ItemState, _db: &GameData) -> Result<Vec<(ItemState, f64)>> {
        let mut next = item.clone();
        let id = format!("m{}", next.prefixes.len());
        next.prefixes.push(marker(&id, GenerationType::Prefix));
        Ok(vec![(next, 1.0)])
    }
}

#[test]
fn beam_search_builds_multistep_path() {
    static EMPTY_DB: std::sync::OnceLock<GameData> = std::sync::OnceLock::new();
    let db = EMPTY_DB.get_or_init(|| GameData {
        mods: HashMap::new(),
        base_items: HashMap::new(),
    });
    let search = BeamSearch::new(
        BeamConfig {
            beam_width: 4,
            max_steps: 5,
            cost_weight: 0.0,
        },
        db,
        vec![Arc::new(AddOne) as Arc<dyn CraftingMethod>],
    );
    let result = search
        .run(rare_sword(), |s| s.prefixes.len() as f64)
        .unwrap();

    assert_eq!(
        result.path,
        vec!["Add One"; 3],
        "must chain exactly 3 applications"
    );
    assert_eq!(result.total_cost, 3.0);
    assert_eq!(result.score, 3.0);
    // All steps are deterministic (prob 1.0) → exact path probability of 1.0.
    assert!((result.path_weight - 1.0).abs() < 1e-12);
}

// ─── 3: probability invariants ───────────────────────────────────────────────

#[test]
fn exalted_orb_probabilities_sum_to_one() {
    let db = varied_db();
    let outcomes = ExaltedOrb.apply(&rare_sword(), &db).unwrap();
    // One outcome per eligible mod (8 in the pool, empty item → all eligible).
    assert_eq!(outcomes.len(), 8);
    assert_weights_sum_to_one(&outcomes, "Exalted Orb");
    assert!(
        ExaltedOrb.weights_are_probabilities(),
        "Exalted Orb enumerates exact outcomes"
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

    let outcomes = OrbOfAnnulment.apply(&item, &db).unwrap();
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
    let outcomes = ChaosOrb.apply(&rare_sword(), &db).unwrap();
    assert_eq!(outcomes.len(), MONTE_CARLO_SAMPLES);
    assert_weights_sum_to_one(&outcomes, "Chaos Orb");
    assert!(
        !ChaosOrb.weights_are_probabilities(),
        "Chaos Orb must report its weights as Monte Carlo samples"
    );
}

#[test]
fn alchemy_produces_valid_rares() {
    let db = varied_db();
    let normal = ItemState::new_base("sword", vec!["sword".to_string()], 84);
    let outcomes = OrbOfAlchemy.apply(&normal, &db).unwrap();
    assert_weights_sum_to_one(&outcomes, "Orb of Alchemy");
    for (state, _) in &outcomes {
        assert_eq!(state.rarity, Rarity::Rare);
        let n = state.prefixes.len() + state.suffixes.len();
        // 4–6 requested; the 8-mod pool with 3+3 slot caps can floor a sample at 4.
        assert!(
            (4..=6).contains(&n),
            "alchemy rare must have 4-6 mods, got {n}"
        );
        assert!(state.prefixes.len() <= 3 && state.suffixes.len() <= 3);
    }
}

#[test]
fn harvest_add_probabilities_sum_to_one() {
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
        op: HarvestOp::Add,
    };
    let outcomes = craft.apply(&rare_sword(), &db).unwrap();
    assert_eq!(outcomes.len(), 2);
    assert_weights_sum_to_one(&outcomes, "Harvest Add");
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
    let mut db = varied_db();
    db.mods.insert(
        "P0alt".to_string(),
        make_mod(GenerationType::Prefix, "PT0", "sword", 100),
    );
    let essence = Essence {
        display_name: "Essence of Conflict".to_string(),
        guaranteed_mod_id: "P0alt".to_string(),
        cost_chaos: 5.0,
    };
    let err = essence.apply(&item_with_fractured_pt0(), &db).unwrap_err();
    assert!(
        err.to_string().contains("shares a mod group"),
        "expected group-conflict error, got: {err}"
    );
}

#[test]
fn essence_blocked_when_fractured_mods_fill_slots() {
    let mut db = varied_db();
    db.mods.insert(
        "NewPrefix".to_string(),
        make_mod(GenerationType::Prefix, "PTnew", "sword", 100),
    );
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
    let err = essence.apply(&item, &db).unwrap_err();
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
    let outcomes = essence.apply(&rare_sword(), &db).unwrap();
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
    let mut db = varied_db();
    db.mods.insert(
        "P0alt".to_string(),
        make_mod(GenerationType::Prefix, "PT0", "sword", 100),
    );
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
    let err = craft.apply(&item_with_fractured_pt0(), &db).unwrap_err();
    assert!(
        err.to_string().contains("shares a mod group"),
        "expected group-conflict error, got: {err}"
    );
}

#[test]
fn fossil_forced_mod_blocked_by_slot_overflow() {
    let mut db = varied_db();
    db.mods.insert(
        "NewPrefix".to_string(),
        make_mod(GenerationType::Prefix, "PTnew", "sword", 100),
    );
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
    let err = craft.apply(&item, &db).unwrap_err();
    assert!(
        err.to_string().contains("no open prefix slot"),
        "expected slot-overflow error, got: {err}"
    );
}

#[test]
fn fossil_forced_mods_conflict_with_each_other() {
    // Two forced mods sharing group "PTX" must be rejected even without fractured mods.
    let mut db = varied_db();
    db.mods.insert(
        "X1".to_string(),
        make_mod(GenerationType::Prefix, "PTX", "sword", 100),
    );
    db.mods.insert(
        "X2".to_string(),
        make_mod(GenerationType::Suffix, "PTX", "sword", 100),
    );
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
    let err = craft.apply(&rare_sword(), &db).unwrap_err();
    assert!(
        err.to_string().contains("shares a mod group"),
        "expected forced-vs-forced conflict error, got: {err}"
    );
}
