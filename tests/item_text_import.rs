//! Focused integration coverage for Path of Exile clipboard-item importing.
//!
//! These tests intentionally use a small synthetic `GameData`: matching must
//! depend on the public RePoE schema and not on a developer's local data files.

use std::collections::HashMap;

use poe1_htc::data::{
    base_items::BaseItem,
    mods::{Domain, GenerationType, Mod, ModStat, SpawnWeight},
    GameData,
};
use poe1_htc::import::{import_item_text, ImportOptions, ImportedItem};
use poe1_htc::item::state::Rarity;

const TEST_BASE_ID: &str = "Metadata/Items/Armours/BodyArmours/TestVest";
const TWILIGHT_REGALIA_ID: &str = "Metadata/Items/Armours/BodyArmours/BodyInt20";
const HEIST_ENCHANT_ID: &str = "ArmourEnchantmentHeistDefenceEffectResistanceEffectPenalty1";

fn base_item(name: &str) -> BaseItem {
    BaseItem {
        name: name.to_string(),
        item_class: "Body Armour".to_string(),
        tags: vec![
            "int_armour".to_string(),
            "body_armour".to_string(),
            "armour".to_string(),
            "default".to_string(),
        ],
        implicits: Vec::new(),
        drop_level: 1,
        inventory_height: 3,
        inventory_width: 2,
    }
}

#[allow(clippy::too_many_arguments)]
fn test_mod(
    generation_type: GenerationType,
    domain: Domain,
    required_level: u32,
    text: &str,
    stats: &[(&str, i32, i32)],
    spawn_tag: &str,
    group: &str,
    tags: &[&str],
) -> Mod {
    Mod {
        name: format!("Synthetic {group}"),
        generation_type,
        required_level,
        stats: stats
            .iter()
            .map(|(id, min, max)| ModStat {
                id: (*id).to_string(),
                min: *min,
                max: *max,
            })
            .collect(),
        spawn_weights: vec![
            SpawnWeight {
                tag: spawn_tag.to_string(),
                weight: 1_000,
            },
            SpawnWeight {
                tag: "default".to_string(),
                weight: 0,
            },
        ],
        generation_weights: Vec::new(),
        adds_tags: Vec::new(),
        tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
        domain,
        mod_type: group.to_string(),
        groups: vec![group.to_string()],
        is_essence_only: false,
        text: Some(text.to_string()),
    }
}

fn game_data(base_id: &str, base_name: &str, mods: Vec<(&str, Mod)>) -> GameData {
    game_data_with_base(base_id, base_item(base_name), mods)
}

fn game_data_with_base(base_id: &str, base: BaseItem, mods: Vec<(&str, Mod)>) -> GameData {
    let base_items = HashMap::from([(base_id.to_string(), base)]);
    let mods = mods
        .into_iter()
        .map(|(id, modifier)| (id.to_string(), modifier))
        .collect();
    GameData::new(mods, base_items)
}

fn strict_options(fallback_item_level: Option<u32>) -> ImportOptions {
    ImportOptions {
        fallback_item_level,
        strict: true,
    }
}

fn imported_mod<'a>(
    imported: &'a ImportedItem,
    mod_id: &str,
) -> &'a poe1_htc::import::ImportedModifier {
    imported
        .explicit_mods
        .iter()
        .find(|modifier| modifier.mod_id == mod_id)
        .unwrap_or_else(|| panic!("expected imported explicit modifier '{mod_id}'"))
}

fn imported_with_explicit(modifier: poe1_htc::import::ImportedModifier) -> ImportedItem {
    ImportedItem {
        item_name: Some("Contract Ward".to_string()),
        base_name: "Test Vest".to_string(),
        rarity: Rarity::Rare,
        item_level: Some(86),
        quality: None,
        sockets: None,
        displayed_energy_shield: None,
        explicit_mods: vec![modifier],
        implicit_mods: Vec::new(),
        enchantments: Vec::new(),
        corrupted: false,
        mirrored: false,
        warnings: Vec::new(),
    }
}

#[test]
fn current_repoe_unveiled_domain_deserializes_as_veiled() {
    let domain: Domain =
        serde_json::from_str("\"unveiled\"").expect("RePoE domain should deserialize");
    assert_eq!(domain, Domain::Veiled);
}

#[test]
fn active_repoe_item_domains_do_not_collapse_to_unknown() {
    for raw in [
        "abyss_jewel",
        "affliction_charm",
        "affliction_jewel",
        "flask",
        "heist_npc",
        "heist_trinket",
        "misc",
        "sanctum_relic",
        "tincture",
    ] {
        let domain: Domain = serde_json::from_str(&format!("\"{raw}\""))
            .unwrap_or_else(|error| panic!("RePoE domain '{raw}' should deserialize: {error}"));
        assert_ne!(
            domain,
            Domain::Unknown,
            "active RePoE item domain '{raw}' lost its identity"
        );
    }
}

#[test]
fn specialized_domains_cannot_outrank_the_correct_abyss_jewel_affix() {
    let abyss_resistance = test_mod(
        GenerationType::Suffix,
        Domain::AbyssJewel,
        1,
        "+(8-10)% to all Elemental Resistances",
        &[("base_resist_all_elements_%", 8, 10)],
        "default",
        "AbyssAllResistances",
        &["elemental", "resistance"],
    );
    let sanctum_resistance = test_mod(
        GenerationType::Suffix,
        Domain::SanctumRelic,
        75,
        "+(10-15)% to all Elemental Resistances",
        &[("base_resist_all_elements_%", 10, 15)],
        "default",
        "SanctumAllResistances",
        &["elemental", "resistance"],
    );
    let mut abyss_jewel = base_item("Murderous Eye Jewel");
    abyss_jewel.item_class = "AbyssJewel".to_string();
    abyss_jewel.tags = vec![
        "abyss_jewel_melee".to_string(),
        "abyss_jewel".to_string(),
        "default".to_string(),
    ];
    let db = game_data_with_base(
        "Metadata/Items/Jewels/JewelAbyssMelee",
        abyss_jewel,
        vec![
            ("AbyssAllResistancesJewel1", abyss_resistance),
            ("SanctumSpecialAllResistances", sanctum_resistance),
        ],
    );

    let imported = import_item_text(
        "Rarity: Rare\nVivid Gaze\nMurderous Eye Jewel\nItem Level: 86\n\
         +10% to all Elemental Resistances",
        &db,
        strict_options(None),
    )
    .expect("a foreign Sanctum Relic domain must be excluded from an Abyss Jewel");

    assert_eq!(imported.explicit_mods.len(), 1);
    assert_eq!(
        imported.explicit_mods[0].mod_id,
        "AbyssAllResistancesJewel1"
    );
}

#[test]
fn unknown_explicit_domains_fail_closed() {
    let unknown = test_mod(
        GenerationType::Prefix,
        Domain::Unknown,
        1,
        "+(10-15) to maximum Life",
        &[("base_maximum_life", 10, 15)],
        "default",
        "UnsupportedDomainLife",
        &["life"],
    );
    let db = game_data(
        TEST_BASE_ID,
        "Test Vest",
        vec![("UnsupportedDomainLife1", unknown)],
    );
    let error = import_item_text(
        "Rarity: Rare\nUnmapped Ward\nTest Vest\nItem Level: 86\n\
         +12 to maximum Life",
        &db,
        strict_options(None),
    )
    .expect_err("an unmodeled RePoE domain must never be guessed as an item affix");

    assert!(
        error
            .to_string()
            .contains("could not resolve explicit modifier"),
        "got: {error:#}"
    );
}

#[test]
fn malformed_clipboard_text_reports_context_instead_of_guessing() {
    let db = game_data(TEST_BASE_ID, "Test Vest", Vec::new());
    let error = import_item_text(
        "Rare\nNameless Thing\nTest Vest",
        &db,
        strict_options(Some(86)),
    )
    .expect_err("a clipboard item without a Rarity header must be rejected");

    let message = error.to_string().to_ascii_lowercase();
    assert!(
        message.contains("rarity") || message.contains("clipboard"),
        "malformed-item error should identify the input problem: {error:#}"
    );
}

#[test]
fn resolves_base_by_metadata_id_and_case_insensitive_display_name() {
    let db = game_data(TEST_BASE_ID, "Test Vest", Vec::new());

    for base_line in [TEST_BASE_ID, "tEsT vEsT"] {
        let text = format!("Rarity: Normal\n{base_line}\nItem Level: 20");
        let imported = import_item_text(&text, &db, strict_options(None))
            .unwrap_or_else(|error| panic!("base line '{base_line}' should resolve: {error:#}"));

        assert_eq!(imported.base_name, "Test Vest");
        assert_eq!(imported.rarity, Rarity::Normal);
        assert_eq!(imported.item_level, Some(20));
    }
}

#[test]
fn base_name_matching_is_unicode_case_insensitive() {
    let db = game_data(
        "Metadata/Items/Weapons/TwoHandWeapons/Staves/MaelstromStaff",
        "Maelström Staff",
        Vec::new(),
    );
    let imported = import_item_text(
        "Rarity: Normal\nMAELSTRÖM STAFF\nItem Level: 80",
        &db,
        strict_options(None),
    )
    .expect("non-ASCII base letters should participate in case folding");

    assert_eq!(imported.base_name, "Maelström Staff");
}

#[test]
fn unsupported_split_status_emits_an_explicit_warning() {
    let db = game_data(TEST_BASE_ID, "Test Vest", Vec::new());
    let imported = import_item_text(
        "Rarity: Rare\nSplit Ward\nTest Vest\nItem Level: 86\nSplit",
        &db,
        strict_options(None),
    )
    .expect("split status does not prevent ordinary crafting modeled by this optimizer");

    assert!(imported.warnings.iter().any(|warning| {
        warning.code == "unsupported_metadata" && warning.message.contains("Split")
    }));
}

#[test]
fn duplicate_base_names_preserve_the_resolved_metadata_identity() {
    let base_items = HashMap::from([
        ("Metadata/Items/A".to_string(), base_item("Duplicated Vest")),
        ("Metadata/Items/Z".to_string(), base_item("Duplicated Vest")),
    ]);
    let db = GameData::new(HashMap::new(), base_items);

    let exact = import_item_text(
        "Rarity: Normal\nMetadata/Items/Z\nItem Level: 20",
        &db,
        strict_options(None),
    )
    .expect("an exact metadata ID should remain authoritative");
    assert_eq!(exact.base_name, "Metadata/Items/Z");
    assert!(exact
        .warnings
        .iter()
        .any(|warning| warning.code == "duplicate_base_name"));

    let display = import_item_text(
        "Rarity: Normal\nDuplicated Vest\nItem Level: 20",
        &db,
        strict_options(None),
    )
    .expect("an ambiguous display name should resolve deterministically");
    assert_eq!(display.base_name, "Metadata/Items/A");
    assert!(display
        .warnings
        .iter()
        .any(|warning| warning.code == "ambiguous_base_name"));
}

#[test]
fn duplicate_base_names_use_the_displayed_base_implicit() {
    let cold_lightning = test_mod(
        GenerationType::Corrupted,
        Domain::Item,
        1,
        "+(8-12)% to Cold and Lightning Resistances",
        &[
            ("base_cold_damage_resistance_%", 8, 12),
            ("base_lightning_damage_resistance_%", 8, 12),
        ],
        "boots",
        "ColdLightningBootsImplicit",
        &["resistance"],
    );
    let fire_cold = test_mod(
        GenerationType::Corrupted,
        Domain::Item,
        1,
        "+(8-12)% to Fire and Cold Resistances",
        &[
            ("base_fire_damage_resistance_%", 8, 12),
            ("base_cold_damage_resistance_%", 8, 12),
        ],
        "boots",
        "FireColdBootsImplicit",
        &["resistance"],
    );
    let atlas_lookalike = test_mod(
        GenerationType::Corrupted,
        Domain::Item,
        80,
        "+(8-12)% to Fire and Cold Resistances",
        &[
            ("base_fire_damage_resistance_%", 8, 12),
            ("base_cold_damage_resistance_%", 8, 12),
        ],
        "boots",
        "AtlasBootsImplicit",
        &["resistance"],
    );
    let mut cold_lightning_boots = base_item("Two-Toned Boots");
    cold_lightning_boots.item_class = "Boots".to_string();
    cold_lightning_boots.tags = vec!["boots".to_string(), "default".to_string()];
    cold_lightning_boots.implicits = vec!["ColdAndLightningResistImplicitBoots1".to_string()];
    let mut fire_cold_boots = cold_lightning_boots.clone();
    fire_cold_boots.implicits = vec!["FireAndColdResistImplicitBoots1_".to_string()];
    let db = GameData::new(
        HashMap::from([
            ("BootsAtlas3".to_string(), atlas_lookalike),
            (
                "ColdAndLightningResistImplicitBoots1".to_string(),
                cold_lightning,
            ),
            ("FireAndColdResistImplicitBoots1_".to_string(), fire_cold),
        ]),
        HashMap::from([
            (
                "Metadata/Items/Armours/Boots/TwoTonedBootsColdLightning".to_string(),
                cold_lightning_boots,
            ),
            (
                "Metadata/Items/Armours/Boots/TwoTonedBootsFireCold".to_string(),
                fire_cold_boots,
            ),
        ]),
    );
    let imported = import_item_text(
        "Rarity: Rare\nPrismatic Pace\nTwo-Toned Boots\nItem Level: 86\n\
         +10% to Fire and Cold Resistances (implicit)",
        &db,
        strict_options(None),
    )
    .expect("the displayed implicit should identify the duplicate base metadata ID");

    assert_eq!(
        imported.base_name,
        "Metadata/Items/Armours/Boots/TwoTonedBootsFireCold"
    );
    assert_eq!(imported.implicit_mods.len(), 1);
    assert_eq!(
        imported.implicit_mods[0].mod_id,
        "FireAndColdResistImplicitBoots1_"
    );
    assert!(imported
        .warnings
        .iter()
        .any(|warning| warning.code == "base_implicit_disambiguation"));
}

#[test]
fn duplicate_base_names_use_the_displayed_implicit_roll_range() {
    let legacy_life_on_hit = test_mod(
        GenerationType::Unique,
        Domain::Item,
        1,
        "Gain (3-4) Life per Enemy Hit with Attacks",
        &[("base_life_gain_per_target", 3, 4)],
        "quiver",
        "LegacyQuiverImplicit",
        &["life"],
    );
    let current_life_on_hit = test_mod(
        GenerationType::Unique,
        Domain::Item,
        1,
        "Gain (6-8) Life per Enemy Hit with Attacks",
        &[("base_life_gain_per_target", 6, 8)],
        "quiver",
        "CurrentQuiverImplicit",
        &["life"],
    );
    let mut legacy = base_item("Sharktooth Arrow Quiver");
    legacy.item_class = "Quiver".to_string();
    legacy.tags = vec!["quiver".to_string(), "default".to_string()];
    legacy.implicits = vec!["LocalLifeGainPerTargetImplicit1".to_string()];
    let mut current = legacy.clone();
    current.implicits = vec!["LocalLifeGainPerTargetImplicit2".to_string()];
    let db = GameData::new(
        HashMap::from([
            (
                "LocalLifeGainPerTargetImplicit1".to_string(),
                legacy_life_on_hit,
            ),
            (
                "LocalLifeGainPerTargetImplicit2".to_string(),
                current_life_on_hit,
            ),
        ]),
        HashMap::from([
            ("Metadata/Items/Weapons/Quivers/Quiver8".to_string(), legacy),
            (
                "Metadata/Items/Weapons/Quivers/QuiverNew3".to_string(),
                current,
            ),
        ]),
    );

    let imported = import_item_text(
        "Rarity: Rare\nViper Skewer\nSharktooth Arrow Quiver\nItem Level: 86\n\
         Gain 7 Life per Enemy Hit with Attacks (implicit)",
        &db,
        strict_options(None),
    )
    .expect("the displayed implicit roll must select the base whose range contains it");

    assert_eq!(
        imported.base_name,
        "Metadata/Items/Weapons/Quivers/QuiverNew3"
    );
    assert_eq!(
        imported.implicit_mods[0].mod_id,
        "LocalLifeGainPerTargetImplicit2"
    );
    assert!(imported
        .warnings
        .iter()
        .any(|warning| warning.code == "base_implicit_disambiguation"));

    let magic = import_item_text(
        "Rarity: Magic\nGleaming Sharktooth Arrow Quiver of Testing\nItem Level: 86\n\
         Gain 7 Life per Enemy Hit with Attacks (implicit)",
        &db,
        strict_options(None),
    )
    .expect("magic combined names must use the same implicit-range disambiguation");

    assert_eq!(magic.base_name, "Metadata/Items/Weapons/Quivers/QuiverNew3");
    assert_eq!(
        magic.implicit_mods[0].mod_id,
        "LocalLifeGainPerTargetImplicit2"
    );
    assert!(magic
        .warnings
        .iter()
        .any(|warning| warning.code == "base_implicit_disambiguation"));
}

#[test]
fn parses_standard_clipboard_sections_and_augmented_properties() {
    let life = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        60,
        "+(30-40) to maximum Life",
        &[("base_maximum_life", 30, 40)],
        "body_armour",
        "MaximumLife",
        &["life"],
    );
    let db = game_data(TEST_BASE_ID, "Test Vest", vec![("RobustLife", life)]);
    let text = "\
Item Class: Body Armours
Rarity: Rare
Stalwart Shelter
Test Vest
--------
Quality: +20% (augmented)
Energy Shield: 321 (augmented)
--------
Requirements:
Level: 60
Int: 100
--------
Sockets: B-B-B-B-B-B
--------
Item Level: 86
--------
{ Prefix Modifier \"Robust\" (Tier: 1) — Life }
+37(30-40) to maximum Life (fractured)
--------
Corrupted";

    let imported = import_item_text(text, &db, strict_options(None))
        .expect("standard section separators and augmented properties should parse");
    assert_eq!(imported.item_name.as_deref(), Some("Stalwart Shelter"));
    assert_eq!(imported.item_level, Some(86));
    assert_eq!(imported.quality, Some(20));
    assert_eq!(imported.sockets.as_deref(), Some("B-B-B-B-B-B"));
    assert_eq!(imported.displayed_energy_shield, Some(321));
    assert!(imported.corrupted);
    assert_eq!(imported.explicit_mods.len(), 1);
    assert_eq!(imported.explicit_mods[0].mod_id, "RobustLife");
    assert_eq!(imported.explicit_mods[0].values, [37]);
    assert!(imported.explicit_mods[0].fractured);
    // Advanced mod-description headers are segmentation anchors now, not
    // ignored metadata — they must import without an unsupported warning.
    assert!(imported
        .warnings
        .iter()
        .all(|warning| warning.code != "unsupported_metadata"));
}

#[test]
fn standard_weapon_properties_do_not_split_a_hybrid_modifier() {
    let hybrid = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        46,
        "(45-54)% increased Physical Damage\n+(98-123) to Accuracy Rating",
        &[
            ("local_physical_damage_+%", 45, 54),
            ("local_accuracy_rating", 98, 123),
        ],
        "weapon",
        "LocalPhysicalDamageAndAccuracy",
        &["physical", "attack"],
    );
    let mut foil = base_item("Jewelled Foil");
    foil.item_class = "One Hand Sword".to_string();
    foil.tags = vec![
        "weapon".to_string(),
        "sword".to_string(),
        "one_hand_weapon".to_string(),
        "default".to_string(),
    ];
    let db = game_data_with_base(
        "Metadata/Items/Weapons/OneHandWeapons/OneHandSwords/OneHandSword20",
        foil,
        vec![(
            "LocalIncreasedPhysicalDamagePercentAndAccuracyRating5",
            hybrid,
        )],
    );
    let imported = import_item_text(
        "Rarity: Rare\nTempered Edge\nJewelled Foil\n--------\n\
         One Handed Sword\nQuality: +20%\nPhysical Damage: 35-65 (augmented)\n\
         Chaos Damage: 1-2\nCritical Strike Chance: 5.50%\n\
         Attacks per Second: 1.60\nWeapon Range: 1.1 metres\n--------\n\
         Requirements:\nLevel: 68\nDex: 212\n--------\nItem Level: 86\n--------\n\
         45% increased Physical Damage\n+98 to Accuracy Rating",
        &db,
        strict_options(None),
    )
    .expect("weapon class/properties are metadata and the two stat lines form one hybrid");

    assert_eq!(imported.explicit_mods.len(), 1);
    assert_eq!(
        imported.explicit_mods[0].mod_id,
        "LocalIncreasedPhysicalDamagePercentAndAccuracyRating5"
    );
    assert_eq!(imported.explicit_mods[0].values, [45, 98]);
}

#[test]
fn superior_synthesised_base_keeps_the_rare_item_name() {
    let db = game_data(TEST_BASE_ID, "Test Vest", Vec::new());
    let imported = import_item_text(
        "Rarity: Rare\nDoom Bulwark\nSuperior Synthesised Test Vest\n\
         Quality: +20%\nItem Level: 86\nSynthesised Item",
        &db,
        strict_options(None),
    )
    .expect("quality and synthesis decorations should be removed from the base typeline");

    assert_eq!(imported.item_name.as_deref(), Some("Doom Bulwark"));
    assert_eq!(imported.base_name, "Test Vest");
    assert_eq!(imported.quality, Some(20));
    assert!(imported
        .warnings
        .iter()
        .any(|warning| warning.message.contains("synthesis")));
}

#[test]
fn infers_magic_base_from_the_combined_affixed_name() {
    let db = game_data(
        "Metadata/Items/Flasks/FlaskUtility1",
        "Granite Flask",
        Vec::new(),
    );
    let imported = import_item_text(
        "Rarity: Magic\nChemist's Granite Flask of the Deer\nItem Level: 80",
        &db,
        strict_options(None),
    )
    .expect("standard magic names should reveal their embedded base");

    assert_eq!(imported.base_name, "Granite Flask");
    assert_eq!(
        imported.item_name.as_deref(),
        Some("Chemist's Granite Flask of the Deer")
    );
    assert!(imported
        .warnings
        .iter()
        .any(|warning| warning.code == "inferred_base_name"));
}

#[test]
fn standard_flask_properties_and_instructions_are_not_affixes() {
    let mut flask = base_item("Granite Flask");
    flask.item_class = "UtilityFlask".to_string();
    flask.tags = vec!["flask".to_string(), "default".to_string()];
    let db = game_data_with_base("Metadata/Items/Flasks/FlaskUtility1", flask, Vec::new());
    let imported = import_item_text(
        "Rarity: Magic\nChemist's Granite Flask of the Deer\n--------\n\
         Lasts 6.00 Seconds\nConsumes 30 of 60 Charges on use\n\
         Currently has 0 Charges\n--------\nItem Level: 80\n--------\n\
         Right click to drink. Can only hold charges while in belt. Refills as you kill monsters.",
        &db,
        strict_options(None),
    )
    .expect("standard flask properties and footer text should be metadata");

    assert_eq!(imported.base_name, "Granite Flask");
    assert!(imported.explicit_mods.is_empty());
}

#[test]
fn catalyst_quality_is_rejected_until_its_display_scaling_is_modeled() {
    let db = game_data(TEST_BASE_ID, "Test Vest", Vec::new());
    let error = import_item_text(
        "Rarity: Rare\nCatalysed Ward\nTest Vest\n\
         Quality (Attribute Modifiers): +20%\nItem Level: 86",
        &db,
        strict_options(None),
    )
    .expect_err("catalyst-scaled modifier values cannot be imported as ordinary quality");

    assert!(
        error.to_string().contains("catalyst quality"),
        "got: {error:#}"
    );
}

#[test]
fn unidentified_items_are_rejected_instead_of_fabricating_empty_affixes() {
    let db = game_data(TEST_BASE_ID, "Test Vest", Vec::new());
    let error = import_item_text(
        "Rarity: Rare\nHidden Ward\nTest Vest\nItem Level: 86\nUnidentified",
        &db,
        strict_options(None),
    )
    .expect_err("hidden modifiers cannot be reconstructed from clipboard text");

    assert!(error.to_string().contains("unidentified"), "got: {error:#}");
}

#[test]
fn malformed_unicode_modifier_returns_an_error_without_panicking() {
    let db = game_data(TEST_BASE_ID, "Test Vest", Vec::new());
    let error = import_item_text(
        "Rarity: Rare\nUnicode Ward\nTest Vest\nItem Level: 86\né",
        &db,
        strict_options(None),
    )
    .expect_err("unknown Unicode text should be diagnosed, not sliced unsafely");
    assert!(error.to_string().contains('é'));
}

#[test]
fn non_strict_import_keeps_resolved_mods_around_an_unknown_line() {
    let life = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        1,
        "+(30-40) to maximum Life",
        &[("base_maximum_life", 30, 40)],
        "body_armour",
        "MaximumLife",
        &["life"],
    );
    let db = game_data(TEST_BASE_ID, "Test Vest", vec![("RobustLife", life)]);
    let imported = import_item_text(
        "Rarity: Rare\nLenient Ward\nTest Vest\nItem Level: 86\n\
         +37 to maximum Life\nUnknown future modifier",
        &db,
        ImportOptions {
            fallback_item_level: None,
            strict: false,
        },
    )
    .expect("non-strict import should salvage independently resolved modifiers");

    assert_eq!(imported.explicit_mods.len(), 1);
    assert_eq!(imported.explicit_mods[0].mod_id, "RobustLife");
    assert!(imported
        .warnings
        .iter()
        .any(|warning| warning.code == "unresolved_modifier"));
}

#[test]
fn genuinely_ambiguous_explicit_modifier_lists_sorted_candidates() {
    let ambiguous_a = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        1,
        "+(10-20) to maximum Life",
        &[("base_maximum_life", 10, 20)],
        "body_armour",
        "AmbiguousA",
        &["life"],
    );
    let ambiguous_z = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        50,
        "+(10-20) to maximum Life",
        &[("base_maximum_life", 10, 20)],
        "body_armour",
        "AmbiguousZ",
        &["life"],
    );
    let db = game_data(
        TEST_BASE_ID,
        "Test Vest",
        vec![
            ("ZuluCandidate", ambiguous_z),
            ("AlphaCandidate", ambiguous_a),
        ],
    );
    let error = import_item_text(
        "Rarity: Rare\nUncertain Ward\nTest Vest\nItem Level: 86\n+15 to maximum Life",
        &db,
        strict_options(None),
    )
    .expect_err("required level alone must not break a clipboard-visible ambiguity");

    let message = error.to_string();
    let alpha = message
        .find("AlphaCandidate")
        .unwrap_or_else(|| panic!("diagnostic omitted AlphaCandidate: {error:#}"));
    let zulu = message
        .find("ZuluCandidate")
        .unwrap_or_else(|| panic!("diagnostic omitted ZuluCandidate: {error:#}"));
    assert!(
        alpha < zulu,
        "candidate diagnostics should be deterministic and sorted: {error:#}"
    );
}

#[test]
fn a_normal_base_prefers_its_item_domain_over_foreign_default_weights() {
    let item_mana = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        35,
        "+(40-44) to maximum Mana",
        &[("base_maximum_mana", 40, 44)],
        "body_armour",
        "IncreasedMana",
        &["mana"],
    );
    let foreign_mana = test_mod(
        GenerationType::Prefix,
        Domain::Unknown,
        83,
        "+(36-40) to maximum Mana",
        &[("base_maximum_mana", 36, 40)],
        "default",
        "AbyssJewelMana",
        &["mana"],
    );
    let db = game_data(
        TEST_BASE_ID,
        "Test Vest",
        vec![
            ("AbyssJewelAddedMana4", foreign_mana),
            ("IncreasedMana6", item_mana),
        ],
    );
    let imported = import_item_text(
        "Rarity: Rare\nAqua Ward\nTest Vest\nItem Level: 86\n+40 to maximum Mana",
        &db,
        strict_options(None),
    )
    .expect("a domain-local default weight must not beat the base's item-domain affix");

    assert_eq!(imported.explicit_mods.len(), 1);
    assert_eq!(imported.explicit_mods[0].mod_id, "IncreasedMana6");
    assert_eq!(imported.explicit_mods[0].values, [40]);
}

#[test]
fn a_jewel_default_weight_beats_zero_weight_item_lookalikes() {
    let jewel_leech = test_mod(
        GenerationType::Suffix,
        Domain::Misc,
        1,
        "(0.2-0.4)% of Physical Attack Damage Leeched as Life",
        &[("life_leech_from_physical_attack_damage_permyriad", 20, 40)],
        "default",
        "JewelLifeLeech",
        &["life"],
    );
    let obsolete_item_leech = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        50,
        "(0.2-0.4)% of Physical Attack Damage Leeched as Life",
        &[("life_leech_from_physical_attack_damage_permyriad", 20, 40)],
        "ring",
        "ObsoleteItemLifeLeech",
        &["life"],
    );
    let mut jewel = base_item("Cobalt Jewel");
    jewel.item_class = "Jewel".to_string();
    jewel.tags = vec!["jewel".to_string(), "default".to_string()];
    let db = game_data_with_base(
        "Metadata/Items/Jewels/JewelInt",
        jewel,
        vec![
            ("LifeLeechPermyriad1", obsolete_item_leech),
            ("LifeLeechPermyriadSuffixJewel", jewel_leech),
        ],
    );
    let imported = import_item_text(
        "Rarity: Rare\nSiphoning Eye\nCobalt Jewel\nItem Level: 86\n\
         0.3% of Physical Attack Damage Leeched as Life",
        &db,
        strict_options(None),
    )
    .expect("domain-local default weight should beat a zero-weight obsolete item affix");

    assert_eq!(imported.explicit_mods.len(), 1);
    assert_eq!(
        imported.explicit_mods[0].mod_id,
        "LifeLeechPermyriadSuffixJewel"
    );
    assert_eq!(imported.explicit_mods[0].values, [30]);
}

#[test]
fn multi_line_template_is_one_modifier_with_ordered_values() {
    let hybrid = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        50,
        "{0}% increased Armour\n+{1} to maximum Life",
        &[("local_armour_+%", 20, 30), ("base_maximum_life", 40, 50)],
        "body_armour",
        "ArmourLifeHybrid",
        &["defences", "life"],
    );
    let db = game_data(
        TEST_BASE_ID,
        "Test Vest",
        vec![("ArmourLifeHybrid1", hybrid)],
    );
    let imported = import_item_text(
        "Rarity: Rare\nJoined Guard\nTest Vest\nItem Level: 86\n\
         25% increased Armour\n+45 to maximum Life",
        &db,
        strict_options(None),
    )
    .expect("the two displayed lines should segment as one RePoE modifier");

    assert_eq!(imported.explicit_mods.len(), 1);
    let modifier = &imported.explicit_mods[0];
    assert_eq!(modifier.mod_id, "ArmourLifeHybrid1");
    assert_eq!(modifier.values, [25, 45]);
    assert_eq!(
        modifier.displayed_lines,
        ["25% increased Armour", "+45 to maximum Life"]
    );
}

#[test]
fn hybrid_versus_separate_modifier_segmentation_is_rejected() {
    let hybrid = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        50,
        "{0}% increased Armour\n+{1} to maximum Life",
        &[("local_armour_+%", 20, 30), ("base_maximum_life", 40, 50)],
        "body_armour",
        "ArmourLifeHybrid",
        &["defences", "life"],
    );
    let armour = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        1,
        "{0}% increased Armour",
        &[("local_armour_+%", 20, 30)],
        "body_armour",
        "LocalArmour",
        &["defences"],
    );
    let life = test_mod(
        GenerationType::Suffix,
        Domain::Item,
        1,
        "+{0} to maximum Life",
        &[("base_maximum_life", 40, 50)],
        "body_armour",
        "MaximumLife",
        &["life"],
    );
    let db = game_data(
        TEST_BASE_ID,
        "Test Vest",
        vec![
            ("ArmourLifeHybrid1", hybrid),
            ("LocalArmour1", armour),
            ("MaximumLife1", life),
        ],
    );
    let error = import_item_text(
        "Rarity: Rare\nUncertain Guard\nTest Vest\nItem Level: 86\n\
         25% increased Armour\n+45 to maximum Life",
        &db,
        strict_options(None),
    )
    .expect_err("clipboard text cannot prove whether these are one hybrid or two affixes");

    let message = error.to_string();
    assert!(message.contains("ambiguous explicit modifier segmentation"));
    assert!(message.contains("ArmourLifeHybrid1"));
    assert!(message.contains("LocalArmour1"));
    assert!(message.contains("MaximumLife1"));
}

#[test]
fn colon_bearing_multiline_modifier_is_not_treated_as_metadata() {
    let ring_slot_essence = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        1,
        "Right ring slot: Shockwave has +1 to Cooldown Uses\n\
         Left ring slot: Skills supported by Unleash have +1 to maximum number of Seals",
        &[
            ("shockwave_cooldown_uses_+", 1, 1),
            ("unleash_maximum_seal_count_+", 1, 1),
        ],
        "body_armour",
        "ShockwaveUnleashCount",
        &[],
    );
    let db = game_data(
        TEST_BASE_ID,
        "Test Vest",
        vec![("ShockwaveUnleashCountEssence1", ring_slot_essence)],
    );
    let imported = import_item_text(
        "Rarity: Rare\nResonant Shell\nTest Vest\nItem Level: 86\n\
         Right ring slot: Shockwave has +1 to Cooldown Uses\n\
         Left ring slot: Skills supported by Unleash have +1 to maximum number of Seals",
        &db,
        strict_options(None),
    )
    .expect("a colon inside RePoE display text is part of the modifier");

    assert_eq!(imported.explicit_mods.len(), 1);
    assert_eq!(
        imported.explicit_mods[0].mod_id,
        "ShockwaveUnleashCountEssence1"
    );
    assert_eq!(imported.explicit_mods[0].values, [1, 1]);
    assert!(imported
        .warnings
        .iter()
        .all(|warning| warning.code != "unsupported_metadata"));
}

#[test]
fn missing_item_level_uses_fallback_and_emits_stable_warning() {
    let db = game_data(TEST_BASE_ID, "Test Vest", Vec::new());
    let imported = import_item_text(
        "Rarity: Normal\nTest Vest\nQuality: +20%",
        &db,
        strict_options(Some(84)),
    )
    .expect("an abbreviated item should use the caller's item-level fallback");

    assert_eq!(imported.item_level, Some(84));
    assert_eq!(imported.quality, Some(20));
    assert!(
        imported
            .warnings
            .iter()
            .any(|warning| warning.code == "missing_item_level"),
        "using the fallback must never be silent: {:?}",
        imported.warnings
    );
}

#[test]
fn annotation_suffixes_classify_modifiers_and_item_flags() {
    let fractured = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        1,
        "+(25-35) to maximum Life",
        &[("base_maximum_life", 25, 35)],
        "body_armour",
        "MaximumLife",
        &["life"],
    );
    let crafted = test_mod(
        GenerationType::Suffix,
        Domain::Crafted,
        1,
        "+(16-20)% to Fire Resistance",
        &[("base_fire_damage_resistance_%", 16, 20)],
        "body_armour",
        "CraftedFireResistance",
        &["fire", "resistance"],
    );
    let implicit = test_mod(
        GenerationType::Unique,
        Domain::Item,
        1,
        "(4-6)% increased Movement Speed",
        &[("base_movement_velocity_+%", 4, 6)],
        "body_armour",
        "SyntheticImplicit",
        &["speed"],
    );
    let enchant = test_mod(
        GenerationType::Enchantment,
        Domain::Item,
        1,
        "+2 to Level of Socketed Gems",
        &[("local_socketed_gem_level_+", 2, 2)],
        "body_armour",
        "SyntheticEnchant",
        &[],
    );
    let db = game_data(
        TEST_BASE_ID,
        "Test Vest",
        vec![
            ("FracturedLife", fractured),
            ("CraftedFireResistance", crafted),
            ("BuiltInMovement", implicit),
            ("LabyrinthGemLevel", enchant),
        ],
    );
    let imported = import_item_text(
        "Rarity: Rare\nMarked Shell\nTest Vest\nItem Level: 86\n\
         +30 to maximum Life (FrAcTuReD)\n\
         +20% to Fire Resistance (CrAfTeD)\n\
         5% increased Movement Speed (ImPlIcIt)\n\
         +2 to Level of Socketed Gems (EnChAnT)\n\
         Corrupted\nMirrored",
        &db,
        strict_options(None),
    )
    .expect("known suffix annotations should be parsed case-insensitively");

    assert_eq!(imported.explicit_mods.len(), 2);
    let fractured = imported_mod(&imported, "FracturedLife");
    assert!(fractured.fractured);
    assert!(!fractured.crafted);
    let crafted = imported_mod(&imported, "CraftedFireResistance");
    assert!(crafted.crafted);
    assert!(!crafted.fractured);
    assert_eq!(imported.implicit_mods.len(), 1);
    assert_eq!(imported.implicit_mods[0].mod_id, "BuiltInMovement");
    assert_eq!(imported.enchantments.len(), 1);
    assert_eq!(imported.enchantments[0].mod_id, "LabyrinthGemLevel");
    assert!(imported.corrupted);
    assert!(imported.mirrored);
}

#[test]
fn hidden_correlated_stats_preserve_the_displayed_raw_roll() {
    let suppression = test_mod(
        GenerationType::Suffix,
        Domain::Delve,
        1,
        "+(4.5-6)% chance to Suppress Spell Damage",
        &[
            ("base_spell_suppression_chance_150%_of_value", 3, 4),
            ("dummy_stat_display_nothing", 3, 4),
        ],
        "body_armour",
        "DelveSuppression",
        &["defences"],
    );
    let db = game_data(
        TEST_BASE_ID,
        "Test Vest",
        vec![("DelveArmourDodgeAndSpellDodge_", suppression)],
    );
    let imported = import_item_text(
        "Rarity: Rare\nSuppressed Ward\nTest Vest\nItem Level: 86\n\
         +4.5% chance to Suppress Spell Damage",
        &db,
        strict_options(None),
    )
    .expect("a hidden display-nothing stat with the same range shares the captured roll");

    assert_eq!(imported.explicit_mods.len(), 1);
    assert_eq!(
        imported.explicit_mods[0].mod_id,
        "DelveArmourDodgeAndSpellDodge_"
    );
    assert_eq!(imported.explicit_mods[0].values, [3, 3]);
}

#[test]
fn the_base_declared_implicit_beats_a_wrong_tag_lookalike() {
    let correct = test_mod(
        GenerationType::Corrupted,
        Domain::Item,
        1,
        "+(8-12)% to Fire and Lightning Resistances",
        &[
            ("base_fire_damage_resistance_%", 8, 12),
            ("base_lightning_damage_resistance_%", 8, 12),
        ],
        "ring",
        "TwoTonedBootsImplicit",
        &["resistance"],
    );
    let wrong = test_mod(
        GenerationType::Corrupted,
        Domain::Item,
        75,
        "+(8-12)% to Fire and Lightning Resistances",
        &[
            ("base_fire_damage_resistance_%", 8, 12),
            ("base_lightning_damage_resistance_%", 8, 12),
        ],
        "body_armour",
        "AtlasBootsImplicit",
        &["resistance"],
    );
    let mut boots = base_item("Two-Toned Boots");
    boots.item_class = "Boots".to_string();
    boots.tags = vec![
        "boots".to_string(),
        "dex_armour".to_string(),
        "default".to_string(),
    ];
    boots.implicits = vec!["TwoTonedBootsFireLightningImplicit".to_string()];
    let db = game_data_with_base(
        "Metadata/Items/Armours/Boots/TwoTonedBootsFireLightning",
        boots,
        vec![
            ("BootsAtlas1", wrong),
            ("TwoTonedBootsFireLightningImplicit", correct),
        ],
    );
    let imported = import_item_text(
        "Rarity: Rare\nDual Stride\nTwo-Toned Boots\nItem Level: 86\n\
         +10% to Fire and Lightning Resistances (implicit)",
        &db,
        strict_options(None),
    )
    .expect("the base catalog's implicit identity is authoritative");

    assert_eq!(imported.implicit_mods.len(), 1);
    assert_eq!(
        imported.implicit_mods[0].mod_id,
        "TwoTonedBootsFireLightningImplicit"
    );
    assert_eq!(imported.implicit_mods[0].values, [10, 10]);
}

#[test]
fn variable_hidden_rolls_are_rejected_instead_of_selecting_a_legacy_lookalike() {
    let current = test_mod(
        GenerationType::Corrupted,
        Domain::Item,
        30,
        "Curse Enemies with Despair on Hit",
        &[("curse_on_hit_level_despair", 10, 12)],
        "body_armour",
        "CurrentDespairOnHit",
        &["curse"],
    );
    let legacy = test_mod(
        GenerationType::Unique,
        Domain::Item,
        68,
        "Curse Enemies with Despair on Hit",
        &[("curse_on_hit_%_despair", 100, 100)],
        "body_armour",
        "LegacyDespairOnHit",
        &["curse"],
    );
    let db = game_data(
        TEST_BASE_ID,
        "Test Vest",
        vec![
            ("HellscapeUpsideCurseOnHitDespair1_", legacy),
            ("V2CurseOnHitDespair", current),
        ],
    );
    let error = import_item_text(
        "Rarity: Rare\nCursed Ward\nTest Vest\nItem Level: 86\n\
         Curse Enemies with Despair on Hit (implicit)",
        &db,
        strict_options(None),
    )
    .expect_err("clipboard text does not expose the current curse level roll");

    assert!(
        error.to_string().contains("hidden stat values"),
        "got: {error:#}"
    );
}

#[test]
fn imported_state_rejects_empty_values_for_a_stat_bearing_modifier() {
    let life = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        1,
        "+(25-35) to maximum Life",
        &[("base_maximum_life", 25, 35)],
        "body_armour",
        "MaximumLife",
        &["life"],
    );
    let db = game_data(TEST_BASE_ID, "Test Vest", vec![("LifePrefix", life)]);
    let imported = imported_with_explicit(poe1_htc::import::ImportedModifier {
        mod_id: "LifePrefix".to_string(),
        values: Vec::new(),
        fractured: false,
        crafted: false,
        displayed_lines: vec!["+30 to maximum Life".to_string()],
    });
    let error = poe1_htc::goal::build_imported_state(
        &imported,
        TEST_BASE_ID.to_string(),
        base_item("Test Vest").tags,
        &db,
    )
    .expect_err("clipboard imports must never replace a missing capture with midpoint rolls");
    assert!(error.to_string().contains("values"), "got: {error:#}");
}

#[test]
fn imported_state_rejects_an_unmarked_crafted_domain_modifier() {
    let crafted = test_mod(
        GenerationType::Suffix,
        Domain::Crafted,
        1,
        "+(16-20)% to Fire Resistance",
        &[("base_fire_damage_resistance_%", 16, 20)],
        "body_armour",
        "CraftedFireResistance",
        &["fire", "resistance"],
    );
    let db = game_data(
        TEST_BASE_ID,
        "Test Vest",
        vec![("CraftedFireResistance", crafted)],
    );
    let imported = imported_with_explicit(poe1_htc::import::ImportedModifier {
        mod_id: "CraftedFireResistance".to_string(),
        values: vec![20],
        fractured: false,
        crafted: false,
        displayed_lines: vec!["+20% to Fire Resistance".to_string()],
    });
    let error = poe1_htc::goal::build_imported_state(
        &imported,
        TEST_BASE_ID.to_string(),
        base_item("Test Vest").tags,
        &db,
    )
    .expect_err("crafted-domain mods must occupy the one crafted-mod slot");
    assert!(error.to_string().contains("crafted"), "got: {error:#}");
}

#[test]
fn enchant_defence_magnitude_is_removed_before_range_matching() {
    let enchant = test_mod(
        GenerationType::Unique,
        Domain::Item,
        69,
        "12% increased Explicit Defence Modifier magnitudes\n\
         50% reduced Explicit Resistance Modifier magnitudes",
        &[
            ("heist_enchantment_defence_mod_effect_+%", 12, 12),
            ("heist_enchantment_resistance_mod_effect_+%", -50, -50),
        ],
        "body_armour",
        "ArmourEnchantmentHeistDefenceModifierEffectResistanceModifierEffectPenalty",
        &[],
    );
    let flat_es = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        80,
        "+(90-100) to maximum Energy Shield",
        &[("base_maximum_energy_shield", 90, 100)],
        "body_armour",
        "FlatEnergyShield",
        &["defences"],
    );
    let percent_es = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        86,
        "(101-110)% increased Energy Shield",
        &[("local_energy_shield_+%", 101, 110)],
        "body_armour",
        "PercentEnergyShield",
        &["defences"],
    );
    let regeneration = test_mod(
        GenerationType::Suffix,
        Domain::Veiled,
        60,
        "Regenerate 200 Energy Shield per second while a Rare or Unique Enemy is Nearby",
        &[(
            "energy_shield_regeneration_rate_per_minute_if_rare_or_unique_enemy_nearby",
            12_000,
            12_000,
        )],
        "body_armour",
        "NearbyEnergyShieldRegeneration",
        &["defences"],
    );
    let db = game_data(
        TEST_BASE_ID,
        "Test Vest",
        vec![
            (HEIST_ENCHANT_ID, enchant),
            ("FlatEnergyShieldTop", flat_es),
            ("PercentEnergyShieldTop", percent_es),
            ("NearbyEnergyShieldRegeneration", regeneration),
        ],
    );
    let imported = import_item_text(
        "Rarity: Rare\nMagnified Mantle\nTest Vest\nItem Level: 86\n\
         12% increased Explicit Defence Modifier magnitudes (enchant)\n\
         50% reduced Explicit Resistance Modifier magnitudes (enchant)\n\
         +112 to maximum Energy Shield\n\
         123% increased Energy Shield\n\
         Regenerate 224 Energy Shield per second while a Rare or Unique Enemy is Nearby",
        &db,
        strict_options(None),
    )
    .expect("displayed defence rolls should be tested against de-magnified ranges");

    assert_eq!(imported.enchantments.len(), 1);
    assert_eq!(imported.enchantments[0].mod_id, HEIST_ENCHANT_ID);
    assert_eq!(imported.enchantments[0].values, [12, -50]);
    assert_eq!(imported_mod(&imported, "FlatEnergyShieldTop").values, [100]);
    assert_eq!(
        imported_mod(&imported, "PercentEnergyShieldTop").values,
        [110]
    );
    // Clipboard text displays this stat per second, but RePoE stores it per minute.
    assert_eq!(
        imported_mod(&imported, "NearbyEnergyShieldRegeneration").values,
        [12_000]
    );
}

#[test]
fn candidate_filtering_is_deterministic_across_map_insertion_order() {
    fn candidates(reverse: bool) -> Vec<(&'static str, Mod)> {
        let eligible = test_mod(
            GenerationType::Prefix,
            Domain::Item,
            80,
            "+(30-40) to maximum Life",
            &[("base_maximum_life", 30, 40)],
            "body_armour",
            "EligibleLife",
            &["life"],
        );
        let wrong_base = test_mod(
            GenerationType::Prefix,
            Domain::Item,
            80,
            "+(30-40) to maximum Life",
            &[("base_maximum_life", 30, 40)],
            "ring",
            "RingLife",
            &["life"],
        );
        let too_high_level = test_mod(
            GenerationType::Prefix,
            Domain::Item,
            90,
            "+(30-40) to maximum Life",
            &[("base_maximum_life", 30, 40)],
            "body_armour",
            "HighLevelLife",
            &["life"],
        );
        let mut result = vec![
            ("RingOnlyLife", wrong_base),
            ("ItemLevel90Life", too_high_level),
            ("BodyArmourLife", eligible),
        ];
        if reverse {
            result.reverse();
        }
        result
    }

    let text = "Rarity: Rare\nRepeatable Ward\nTest Vest\nItem Level: 86\n+37 to maximum Life";
    let first = import_item_text(
        text,
        &game_data(TEST_BASE_ID, "Test Vest", candidates(false)),
        strict_options(None),
    )
    .expect("eligible candidate should resolve");
    let second = import_item_text(
        text,
        &game_data(TEST_BASE_ID, "Test Vest", candidates(true)),
        strict_options(None),
    )
    .expect("candidate insertion order must not affect resolution");

    assert_eq!(first.explicit_mods.len(), 1);
    assert_eq!(second.explicit_mods.len(), 1);
    assert_eq!(first.explicit_mods[0].mod_id, "BodyArmourLife");
    assert_eq!(second.explicit_mods[0].mod_id, "BodyArmourLife");
    assert_eq!(first.explicit_mods[0].values, [37]);
    assert_eq!(second.explicit_mods[0].values, [37]);
}

#[test]
fn abbreviated_acceptance_fixture_imports_complete_twilight_regalia() {
    let enchant = test_mod(
        GenerationType::Unique,
        Domain::Item,
        69,
        "12% increased Explicit Defence Modifier magnitudes\n\
         50% reduced Explicit Resistance Modifier magnitudes",
        &[
            ("heist_enchantment_defence_mod_effect_+%", 12, 12),
            ("heist_enchantment_resistance_mod_effect_+%", -50, -50),
        ],
        "body_armour",
        "ArmourEnchantmentHeistDefenceModifierEffectResistanceModifierEffectPenalty",
        &[],
    );
    let physical_as_chaos = test_mod(
        GenerationType::Unique,
        Domain::Item,
        1,
        "(10-15)% of Physical Damage from Hits taken as Chaos Damage",
        &[("physical_damage_taken_%_as_chaos", 10, 15)],
        "body_armour",
        "PhysicalTakenAsChaosImplicit",
        &[],
    );
    let flask_effect = test_mod(
        GenerationType::Unique,
        Domain::Item,
        1,
        "Flasks applied to you have (10-20)% increased Effect",
        &[("flask_effect_+%", 10, 20)],
        "body_armour",
        "FlaskEffectImplicit",
        &[],
    );
    let mut spectres = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        60,
        "+1 to maximum number of Spectres",
        &[
            ("base_number_of_zombies_allowed", 0, 0),
            ("base_number_of_skeletons_allowed", 0, 0),
            ("base_number_of_spectres_allowed", 1, 1),
        ],
        "body_armour",
        "MaximumSpectreCount",
        &["minion"],
    );
    spectres.spawn_weights = vec![SpawnWeight {
        tag: "default".to_string(),
        weight: 0,
    }];
    let strength_gems = test_mod(
        GenerationType::Suffix,
        Domain::Delve,
        1,
        "+1 to Level of Socketed Strength Gems",
        &[("local_socketed_strength_gem_level_+", 1, 1)],
        "body_armour",
        "DelveStrengthGemLevel",
        &[],
    );
    let intelligence_gems = test_mod(
        GenerationType::Suffix,
        Domain::Delve,
        1,
        "+1 to Level of Socketed Intelligence Gems",
        &[("local_socketed_intelligence_gem_level_+", 1, 1)],
        "body_armour",
        "DelveIntelligenceGemLevel",
        &[],
    );
    let flat_es = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        80,
        "+(90-100) to maximum Energy Shield",
        &[("base_maximum_energy_shield", 90, 100)],
        "body_armour",
        "FlatEnergyShield",
        &["defences"],
    );
    let percent_es = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        86,
        "(101-110)% increased Energy Shield",
        &[("local_energy_shield_+%", 101, 110)],
        "body_armour",
        "PercentEnergyShield",
        &["defences"],
    );
    let regeneration = test_mod(
        GenerationType::Suffix,
        Domain::Veiled,
        60,
        "Regenerate 200 Energy Shield per second while a Rare or Unique Enemy is Nearby",
        &[(
            "energy_shield_regeneration_rate_per_minute_if_rare_or_unique_enemy_nearby",
            12_000,
            12_000,
        )],
        "body_armour",
        "NearbyEnergyShieldRegeneration",
        &["defences"],
    );
    let db = game_data(
        TWILIGHT_REGALIA_ID,
        "Twilight Regalia",
        vec![
            (HEIST_ENCHANT_ID, enchant),
            ("PhysicalTakenAsChaosImplicit", physical_as_chaos),
            ("FlaskEffectImplicit", flask_effect),
            ("MaximumMinionCountSpectreDelve", spectres),
            ("DelveStrengthGemLevel1", strength_gems),
            ("DelveIntelligenceGemLevel1", intelligence_gems),
            ("FlatEnergyShieldTop", flat_es),
            ("PercentEnergyShieldTop", percent_es),
            ("NearbyEnergyShieldRegeneration", regeneration),
        ],
    );
    let text = "\
Rarity: Rare
Damnation Wrap
Twilight Regalia
Quality: +30%
Sockets: W-W-W-W-W-W
Energy Shield: 1200
12% increased Explicit Defence Modifier magnitudes (enchant)
50% reduced Explicit Resistance Modifier magnitudes (enchant)
12% of Physical Damage from Hits taken as Chaos Damage (implicit)
Flasks applied to you have 15% increased Effect (implicit)
+1 to maximum number of Spectres (fractured)
+1 to Level of Socketed Strength Gems
+1 to Level of Socketed Intelligence Gems
+112 to maximum Energy Shield
123% increased Energy Shield
Regenerate 224 Energy Shield per second while a Rare or Unique Enemy is Nearby";

    let imported = import_item_text(text, &db, strict_options(Some(86)))
        .expect("the required abbreviated acceptance fixture should import");

    assert_eq!(imported.item_name.as_deref(), Some("Damnation Wrap"));
    assert_eq!(imported.base_name, "Twilight Regalia");
    assert_eq!(imported.rarity, Rarity::Rare);
    assert_eq!(imported.item_level, Some(86));
    assert_eq!(imported.quality, Some(30));
    assert_eq!(imported.sockets.as_deref(), Some("W-W-W-W-W-W"));
    assert_eq!(imported.displayed_energy_shield, Some(1200));
    assert_eq!(imported.explicit_mods.len(), 6);
    assert_eq!(imported.implicit_mods.len(), 2);
    assert_eq!(imported.enchantments.len(), 1);
    assert!(imported
        .warnings
        .iter()
        .any(|warning| warning.code == "missing_item_level"));

    let spectres = imported_mod(&imported, "MaximumMinionCountSpectreDelve");
    assert!(spectres.fractured);
    assert_eq!(spectres.values, [0, 0, 1]);
    assert_eq!(imported_mod(&imported, "FlatEnergyShieldTop").values, [100]);
    assert_eq!(
        imported_mod(&imported, "PercentEnergyShieldTop").values,
        [110]
    );
    assert_eq!(
        imported_mod(&imported, "NearbyEnergyShieldRegeneration").values,
        [12_000]
    );

    let prefix_count = imported
        .explicit_mods
        .iter()
        .filter(|modifier| {
            db.mods
                .get(&modifier.mod_id)
                .is_some_and(|definition| definition.generation_type == GenerationType::Prefix)
        })
        .count();
    let suffix_count = imported.explicit_mods.len() - prefix_count;
    assert_eq!((prefix_count, suffix_count), (3, 3));

    let mut implicit_ids: Vec<&str> = imported
        .implicit_mods
        .iter()
        .map(|modifier| modifier.mod_id.as_str())
        .collect();
    implicit_ids.sort_unstable();
    assert_eq!(
        implicit_ids,
        ["FlaskEffectImplicit", "PhysicalTakenAsChaosImplicit"]
    );
    assert_eq!(
        imported.enchantments[0].values,
        [12, -50],
        "the two-line enchant must keep one value per RePoE stat"
    );

    let base_tags = db
        .base_items
        .get(TWILIGHT_REGALIA_ID)
        .expect("synthetic base must exist")
        .tags
        .clone();
    let state = poe1_htc::goal::build_imported_state(
        &imported,
        TWILIGHT_REGALIA_ID.to_string(),
        base_tags,
        &db,
    )
    .expect("the parsed acceptance fixture should build a validated starting state");

    assert_eq!(state.prefix_count(), 3);
    assert_eq!(state.suffix_count(), 3);
    assert_eq!(state.mod_count(), 6);
    assert!(state.is_full());
    assert_eq!(state.prefixes.len(), 2);
    assert_eq!(state.suffixes.len(), 3);
    assert_eq!(state.fractured.len(), 1);
    assert_eq!(state.fractured[0].mod_id, "MaximumMinionCountSpectreDelve");
    let fractured_values: Vec<i32> = state.fractured[0]
        .rolls
        .iter()
        .map(|roll| roll.value)
        .collect();
    assert_eq!(fractured_values, [0, 0, 1]);
    assert_eq!(state.implicits.len(), 2);
    assert_eq!(state.enchants.len(), 1);
    let mut state_implicit_values: Vec<(&str, Vec<i32>)> = state
        .implicits
        .iter()
        .map(|modifier| {
            (
                modifier.mod_id.as_str(),
                modifier.rolls.iter().map(|roll| roll.value).collect(),
            )
        })
        .collect();
    state_implicit_values.sort_unstable_by_key(|(mod_id, _)| *mod_id);
    assert_eq!(
        state_implicit_values,
        [
            ("FlaskEffectImplicit", vec![15]),
            ("PhysicalTakenAsChaosImplicit", vec![12]),
        ]
    );
    let enchant_values: Vec<i32> = state.enchants[0]
        .rolls
        .iter()
        .map(|roll| roll.value)
        .collect();
    assert_eq!(enchant_values, [12, -50]);
    assert_eq!(
        state.enchants[0].generation_type,
        GenerationType::Unique,
        "RePoE classifies this Heist enchant as Unique despite its clipboard annotation"
    );
    assert_eq!(state.quality, 30);
    assert_eq!(state.sockets.as_deref(), Some("W-W-W-W-W-W"));
    assert_eq!(state.displayed_energy_shield, Some(1200));
}

#[test]
fn advanced_mod_headers_anchor_segmentation_and_crafted_status() {
    let hybrid = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        30,
        "+(21-42) to Evasion Rating\n+(24-28) to maximum Life",
        &[
            ("local_base_evasion_rating", 21, 42),
            ("base_maximum_life", 24, 28),
        ],
        "body_armour",
        "EvasionLifeHybrid",
        &["life", "evasion"],
    );
    let flat_evasion = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        30,
        "+(21-42) to Evasion Rating",
        &[("local_base_evasion_rating", 21, 42)],
        "body_armour",
        "FlatEvasion",
        &["evasion"],
    );
    let flat_life = test_mod(
        GenerationType::Prefix,
        Domain::Item,
        30,
        "+(24-28) to maximum Life",
        &[("base_maximum_life", 24, 28)],
        "body_armour",
        "FlatLife",
        &["life"],
    );
    let crafted_cold = test_mod(
        GenerationType::Suffix,
        Domain::Crafted,
        30,
        "+(11-15)% to Cold Resistance",
        &[("base_cold_damage_resistance_%", 11, 15)],
        "body_armour",
        "CraftedColdRes",
        &["resistance"],
    );
    let natural_cold = test_mod(
        GenerationType::Suffix,
        Domain::Item,
        30,
        "+(11-15)% to Cold Resistance",
        &[("base_cold_damage_resistance_%", 11, 15)],
        "body_armour",
        "NaturalColdRes",
        &["resistance"],
    );
    let db = game_data(
        TEST_BASE_ID,
        "Test Vest",
        vec![
            ("EvasionLifeHybrid", hybrid),
            ("FlatEvasion", flat_evasion),
            ("FlatLife", flat_life),
            ("CraftedColdRes", crafted_cold),
            ("NaturalColdRes", natural_cold),
        ],
    );

    // Without headers the hybrid's two lines also parse as two flat mods:
    // strict import must refuse to guess.
    let bare = "\
Rarity: Rare
Storm Shelter
Test Vest
--------
Item Level: 77
--------
+36 to Evasion Rating
+25 to maximum Life";
    let error = import_item_text(bare, &db, strict_options(None))
        .expect_err("ambiguous segmentation without headers must fail closed");
    assert!(error
        .to_string()
        .contains("genuinely ambiguous explicit modifier segmentation"));

    // Advanced headers pin each modifier's line span, generation type, and
    // crafted status, so the same stat lines import unambiguously.
    let advanced = "\
Rarity: Rare
Storm Shelter
Test Vest
--------
Item Level: 77
--------
{ Prefix Modifier \"Fawn's\" (Tier: 3) — Life, Defences, Evasion }
+36 to Evasion Rating
+25 to maximum Life
{ Master Crafted Suffix Modifier \"of Craft\" (Tier: 1) — Cold, Resistance }
+14% to Cold Resistance";
    let imported = import_item_text(advanced, &db, strict_options(None))
        .expect("headers should disambiguate the hybrid and the crafted suffix");
    assert_eq!(imported.explicit_mods.len(), 2);
    assert_eq!(imported.explicit_mods[0].mod_id, "EvasionLifeHybrid");
    assert_eq!(imported.explicit_mods[0].values, [36, 25]);
    assert_eq!(imported.explicit_mods[1].mod_id, "CraftedColdRes");
    assert_eq!(imported.explicit_mods[1].values, [14]);
    assert!(
        imported.explicit_mods[1].crafted,
        "the Master Crafted header must mark the mod crafted without a per-line annotation"
    );
    assert!(imported
        .warnings
        .iter()
        .all(|warning| warning.code != "ambiguous_modifier_segmentation"));
}
