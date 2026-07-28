//! Version 1 saved optimization-request contract.
//!
//! These types deliberately mirror the application request through an explicit
//! adapter instead of making the domain model itself a persistence format.

use std::collections::BTreeSet;
use std::fmt;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::app::pricing::PriceBook;
use crate::app::request::{
    GoalSetRequest, MethodAccessPolicy, MethodSelection, MethodSetRequest, OptimizeRequest,
    SearchRequest, StartingItemRequest,
};
use crate::currency::{MethodFamily, MethodId};
use crate::goal::{
    FossilPartSpec, GoalScoringMode, ItemSpec, MethodSpec, StartingModSpec, WantSpec,
};
use crate::search::{BudgetMetric, BudgetPolicy};

/// Numeric schema version emitted by [`SavedOptimizeRequestV1`].
pub const SAVED_OPTIMIZE_REQUEST_SCHEMA_VERSION: u32 = 1;

/// Zero-sized proof that a request carries the literal numeric v1 schema tag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SchemaVersionV1;

impl Serialize for SchemaVersionV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(SAVED_OPTIMIZE_REQUEST_SCHEMA_VERSION)
    }
}

impl<'de> Deserialize<'de> for SchemaVersionV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version = u32::deserialize(deserializer)?;
        if version == SAVED_OPTIMIZE_REQUEST_SCHEMA_VERSION {
            Ok(Self)
        } else {
            Err(de::Error::custom(format!(
                "unsupported schema_version {version}; expected {SAVED_OPTIMIZE_REQUEST_SCHEMA_VERSION}"
            )))
        }
    }
}

/// Strictly positive finite JSON number.
///
/// The inner value is private so invalid floating-point values cannot enter a
/// serializable request through the public API.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct PositiveFiniteV1(f64);

impl PositiveFiniteV1 {
    pub fn new(value: f64) -> Option<Self> {
        (value.is_finite() && value > 0.0).then_some(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Serialize for PositiveFiniteV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if Self::new(self.0).is_none() {
            return Err(serde::ser::Error::custom(
                "expected a strictly positive finite number",
            ));
        }
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for PositiveFiniteV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::new(value)
            .ok_or_else(|| de::Error::custom("expected a strictly positive finite number"))
    }
}

/// Non-negative finite JSON number with canonical positive zero.
///
/// Negative zero is rejected so equivalent requests have one stable JSON
/// representation.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct NonNegativeFiniteV1(f64);

impl NonNegativeFiniteV1 {
    pub fn new(value: f64) -> Option<Self> {
        (value.is_finite() && value >= 0.0 && !value.is_sign_negative()).then_some(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Serialize for NonNegativeFiniteV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if Self::new(self.0).is_none() {
            return Err(serde::ser::Error::custom(
                "expected a non-negative finite number",
            ));
        }
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for NonNegativeFiniteV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| de::Error::custom("expected a non-negative finite number"))
    }
}

/// Canonical decimal-string representation of a `u64`.
///
/// JSON numbers cannot represent every `u64` exactly in common JavaScript
/// runtimes, so seeds cross the saved-request boundary as strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecimalU64V1(u64);

impl DecimalU64V1 {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Serialize for DecimalU64V1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for DecimalU64V1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let canonical = raw == "0"
            || (raw
                .as_bytes()
                .first()
                .is_some_and(|first| first.is_ascii_digit() && *first != b'0')
                && raw.as_bytes()[1..].iter().all(u8::is_ascii_digit));
        if !canonical {
            return Err(de::Error::custom(
                "expected a canonical unsigned decimal string",
            ));
        }
        raw.parse::<u64>()
            .map(Self)
            .map_err(|_| de::Error::custom("decimal string is outside the u64 range"))
    }
}

/// Stable v1 saved optimization request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedOptimizeRequestV1 {
    pub schema_version: SchemaVersionV1,
    pub starting_item: StartingItemV1,
    pub goals: Vec<GoalV1>,
    pub methods: MethodSetV1,
    pub budget: BudgetV1,
    pub search: SearchV1,
}

/// Starting-item source saved with a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StartingItemV1 {
    Described {
        item: DescribedItemV1,
    },
    ImportedText {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback_item_level: Option<u32>,
    },
}

/// Programmatically described starting item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescribedItemV1 {
    pub base: String,
    pub item_level: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rarity: Option<ItemRarityV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mods: Vec<StartingModifierV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub influences: Vec<ItemInfluenceV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exarch_implicit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eater_implicit: Option<String>,
}

/// One existing modifier on a described starting item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartingModifierV1 {
    pub mod_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<i32>>,
    #[serde(default)]
    pub fractured: bool,
    #[serde(default)]
    pub crafted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemRarityV1 {
    Normal,
    Magic,
    Rare,
}

impl ItemRarityV1 {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Magic => "magic",
            Self::Rare => "rare",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemInfluenceV1 {
    Shaper,
    Elder,
    Crusader,
    Hunter,
    Redeemer,
    Warlord,
}

impl ItemInfluenceV1 {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Shaper => "shaper",
            Self::Elder => "elder",
            Self::Crusader => "crusader",
            Self::Hunter => "hunter",
            Self::Redeemer => "redeemer",
            Self::Warlord => "warlord",
        }
    }
}

/// One desired modifier and its scoring semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mod_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stat: Option<String>,
    #[serde(default)]
    pub mode: ScoringModeV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_value: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_value: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<i64>,
    #[serde(default = "default_goal_weight")]
    pub weight: PositiveFiniteV1,
    #[serde(default = "default_required")]
    pub required: bool,
}

fn default_goal_weight() -> PositiveFiniteV1 {
    PositiveFiniteV1(1.0)
}

const fn default_required() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoringModeV1 {
    #[default]
    Presence,
    Threshold,
    PerUnit,
}

/// Configured methods, access selection, and semantic price overrides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodSetV1 {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub configured: Vec<ConfiguredMethodV1>,
    pub access: MethodAccessV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub price_overrides: Vec<MethodPriceV1>,
}

/// One current configured crafting-method variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfiguredMethodV1 {
    Essence {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        essence: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mod_id: Option<String>,
        cost: PositiveFiniteV1,
    },
    Bench {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        mod_id: String,
        cost: PositiveFiniteV1,
    },
    Fossil {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fossil: Option<String>,
        cost: PositiveFiniteV1,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        boosted_tags: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        reduced_tags: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        blocked_mod_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        forced_mod_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fossils: Vec<FossilPartV1>,
    },
    Harvest {
        op: HarvestOperationV1,
        target: HarvestTargetV1,
        cost: PositiveFiniteV1,
    },
    EldritchChaos {
        god: EldritchSideV1,
    },
    EldritchExalt {
        god: EldritchSideV1,
    },
    EldritchAnnul {
        god: EldritchSideV1,
    },
    ConquerorExalt {
        influence: ConquerorInfluenceV1,
    },
    BestiarySwap {
        add: AffixSideV1,
        beast_level: u32,
        cost: PositiveFiniteV1,
    },
}

/// One fossil part in a multi-fossil resonator.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FossilPartV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fossil: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boosted_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reduced_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_mod_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forced_mod_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarvestOperationV1 {
    Reforge,
    Augment,
}

impl HarvestOperationV1 {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Reforge => "reforge",
            Self::Augment => "augment",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarvestTargetV1 {
    Attack,
    Caster,
    Speed,
    Life,
    Defence,
    Resistance,
    Chaos,
    Fire,
    Cold,
    Lightning,
    Physical,
    Critical,
    Minion,
    Mana,
}

impl HarvestTargetV1 {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Attack => "attack",
            Self::Caster => "caster",
            Self::Speed => "speed",
            Self::Life => "life",
            Self::Defence => "defence",
            Self::Resistance => "resistance",
            Self::Chaos => "chaos",
            Self::Fire => "fire",
            Self::Cold => "cold",
            Self::Lightning => "lightning",
            Self::Physical => "physical",
            Self::Critical => "critical",
            Self::Minion => "minion",
            Self::Mana => "mana",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EldritchSideV1 {
    Exarch,
    Eater,
}

impl EldritchSideV1 {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Exarch => "exarch",
            Self::Eater => "eater",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConquerorInfluenceV1 {
    Crusader,
    Hunter,
    Redeemer,
    Warlord,
}

impl ConquerorInfluenceV1 {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Crusader => "crusader",
            Self::Hunter => "hunter",
            Self::Redeemer => "redeemer",
            Self::Warlord => "warlord",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AffixSideV1 {
    Prefix,
    Suffix,
}

impl AffixSideV1 {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Prefix => "prefix",
            Self::Suffix => "suffix",
        }
    }
}

/// Saved method-selection policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MethodAccessV1 {
    LegacyDefaultsAndConfigured,
    Allowlist { method_ids: Vec<MethodId> },
    Explicit { selection: MethodSelectionV1 },
}

/// Family and method inclusions/exclusions for explicit method access.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodSelectionV1 {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled_families: Vec<MethodFamilyV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled_methods: Vec<MethodId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_methods: Vec<MethodId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodFamilyV1 {
    Currency,
    Bench,
    Essence,
    Fossil,
    Harvest,
    Eldritch,
    Influence,
    Bestiary,
}

/// One semantic method price override.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodPriceV1 {
    pub method_id: MethodId,
    pub amount_chaos: PositiveFiniteV1,
}

/// Saved hard-budget policy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BudgetV1 {
    Unbounded {
        #[serde(default)]
        metric: BudgetMetricV1,
    },
    HardCap {
        amount_chaos: NonNegativeFiniteV1,
        #[serde(default)]
        metric: BudgetMetricV1,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetMetricV1 {
    FirstTry,
    RetryExpected,
    #[default]
    RestartAdjustedExpected,
}

/// Fully resolved search settings persisted in v1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchV1 {
    pub beam_width: u64,
    pub max_steps: u64,
    pub cost_weight: NonNegativeFiniteV1,
    pub restart_cost: NonNegativeFiniteV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<DecimalU64V1>,
    pub top: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Request/domain conversion failure with a stable JSON-pointer field path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DtoConversionError {
    field_path: String,
    message: String,
}

impl DtoConversionError {
    pub fn new(field_path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field_path: field_path.into(),
            message: message.into(),
        }
    }

    pub fn field_path(&self) -> &str {
        &self.field_path
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for DtoConversionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.field_path, self.message)
    }
}

impl std::error::Error for DtoConversionError {}

impl From<ScoringModeV1> for GoalScoringMode {
    fn from(value: ScoringModeV1) -> Self {
        match value {
            ScoringModeV1::Presence => Self::Presence,
            ScoringModeV1::Threshold => Self::Threshold,
            ScoringModeV1::PerUnit => Self::PerUnit,
        }
    }
}

impl From<GoalScoringMode> for ScoringModeV1 {
    fn from(value: GoalScoringMode) -> Self {
        match value {
            GoalScoringMode::Presence => Self::Presence,
            GoalScoringMode::Threshold => Self::Threshold,
            GoalScoringMode::PerUnit => Self::PerUnit,
        }
    }
}

impl From<MethodFamilyV1> for MethodFamily {
    fn from(value: MethodFamilyV1) -> Self {
        match value {
            MethodFamilyV1::Currency => Self::Currency,
            MethodFamilyV1::Bench => Self::Bench,
            MethodFamilyV1::Essence => Self::Essence,
            MethodFamilyV1::Fossil => Self::Fossil,
            MethodFamilyV1::Harvest => Self::Harvest,
            MethodFamilyV1::Eldritch => Self::Eldritch,
            MethodFamilyV1::Influence => Self::Influence,
            MethodFamilyV1::Bestiary => Self::Bestiary,
        }
    }
}

impl From<MethodFamily> for MethodFamilyV1 {
    fn from(value: MethodFamily) -> Self {
        match value {
            MethodFamily::Currency => Self::Currency,
            MethodFamily::Bench => Self::Bench,
            MethodFamily::Essence => Self::Essence,
            MethodFamily::Fossil => Self::Fossil,
            MethodFamily::Harvest => Self::Harvest,
            MethodFamily::Eldritch => Self::Eldritch,
            MethodFamily::Influence => Self::Influence,
            MethodFamily::Bestiary => Self::Bestiary,
        }
    }
}

impl From<BudgetMetricV1> for BudgetMetric {
    fn from(value: BudgetMetricV1) -> Self {
        match value {
            BudgetMetricV1::FirstTry => Self::FirstTry,
            BudgetMetricV1::RetryExpected => Self::RetryExpected,
            BudgetMetricV1::RestartAdjustedExpected => Self::RestartAdjustedExpected,
        }
    }
}

impl From<BudgetMetric> for BudgetMetricV1 {
    fn from(value: BudgetMetric) -> Self {
        match value {
            BudgetMetric::FirstTry => Self::FirstTry,
            BudgetMetric::RetryExpected => Self::RetryExpected,
            BudgetMetric::RestartAdjustedExpected => Self::RestartAdjustedExpected,
        }
    }
}

impl TryFrom<SavedOptimizeRequestV1> for OptimizeRequest {
    type Error = DtoConversionError;

    fn try_from(value: SavedOptimizeRequestV1) -> Result<Self, Self::Error> {
        let SavedOptimizeRequestV1 {
            schema_version: _,
            starting_item,
            goals,
            methods,
            budget,
            search,
        } = value;

        if goals.is_empty() {
            return Err(DtoConversionError::new(
                "/goals",
                "at least one goal is required",
            ));
        }

        let wants = goals
            .into_iter()
            .enumerate()
            .map(|(index, goal)| goal_into_domain(goal, index))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            starting_item: starting_item_into_domain(starting_item)?,
            goals: GoalSetRequest { wants },
            methods: method_set_into_domain(methods)?,
            budget: budget_into_domain(budget)?,
            search: search_into_domain(search)?,
        })
    }
}

impl TryFrom<&OptimizeRequest> for SavedOptimizeRequestV1 {
    type Error = DtoConversionError;

    fn try_from(value: &OptimizeRequest) -> Result<Self, Self::Error> {
        if value.goals.wants.is_empty() {
            return Err(DtoConversionError::new(
                "/goals",
                "at least one goal is required",
            ));
        }

        Ok(Self {
            schema_version: SchemaVersionV1,
            starting_item: starting_item_from_domain(&value.starting_item)?,
            goals: value
                .goals
                .wants
                .iter()
                .enumerate()
                .map(|(index, goal)| goal_from_domain(goal, index))
                .collect::<Result<Vec<_>, _>>()?,
            methods: method_set_from_domain(&value.methods)?,
            budget: budget_from_domain(value.budget)?,
            search: search_from_domain(value.search)?,
        })
    }
}

fn starting_item_into_domain(
    value: StartingItemV1,
) -> Result<StartingItemRequest, DtoConversionError> {
    match value {
        StartingItemV1::Described { item } => {
            validate_item_level(item.item_level, "/starting_item/item/item_level")?;
            if item.base.trim().is_empty() {
                return Err(DtoConversionError::new(
                    "/starting_item/item/base",
                    "base must not be empty",
                ));
            }
            Ok(StartingItemRequest::Described(ItemSpec {
                base: item.base,
                item_level: item.item_level,
                rarity: item.rarity.map(|rarity| rarity.as_str().to_string()),
                mods: item
                    .mods
                    .into_iter()
                    .map(|modifier| StartingModSpec {
                        mod_id: modifier.mod_id,
                        values: modifier.values,
                        fractured: modifier.fractured,
                        crafted: modifier.crafted,
                    })
                    .collect(),
                influences: item
                    .influences
                    .into_iter()
                    .map(|influence| influence.as_str().to_string())
                    .collect(),
                exarch_implicit: item.exarch_implicit,
                eater_implicit: item.eater_implicit,
            }))
        }
        StartingItemV1::ImportedText {
            text,
            fallback_item_level,
        } => {
            if let Some(item_level) = fallback_item_level {
                validate_item_level(item_level, "/starting_item/fallback_item_level")?;
            }
            Ok(StartingItemRequest::ImportedText {
                text,
                fallback_item_level,
            })
        }
    }
}

fn starting_item_from_domain(
    value: &StartingItemRequest,
) -> Result<StartingItemV1, DtoConversionError> {
    match value {
        StartingItemRequest::Described(item) => {
            validate_item_level(item.item_level, "/starting_item/item/item_level")?;
            if item.base.trim().is_empty() {
                return Err(DtoConversionError::new(
                    "/starting_item/item/base",
                    "base must not be empty",
                ));
            }
            let rarity = item
                .rarity
                .as_deref()
                .map(item_rarity_from_domain)
                .transpose()?;
            let influences = item
                .influences
                .iter()
                .enumerate()
                .map(|(index, influence)| item_influence_from_domain(influence, index))
                .collect::<Result<Vec<_>, _>>()?;

            Ok(StartingItemV1::Described {
                item: DescribedItemV1 {
                    base: item.base.clone(),
                    item_level: item.item_level,
                    rarity,
                    mods: item
                        .mods
                        .iter()
                        .map(|modifier| StartingModifierV1 {
                            mod_id: modifier.mod_id.clone(),
                            values: modifier.values.clone(),
                            fractured: modifier.fractured,
                            crafted: modifier.crafted,
                        })
                        .collect(),
                    influences,
                    exarch_implicit: item.exarch_implicit.clone(),
                    eater_implicit: item.eater_implicit.clone(),
                },
            })
        }
        StartingItemRequest::ImportedText {
            text,
            fallback_item_level,
        } => {
            if let Some(item_level) = fallback_item_level {
                validate_item_level(*item_level, "/starting_item/fallback_item_level")?;
            }
            Ok(StartingItemV1::ImportedText {
                text: text.clone(),
                fallback_item_level: *fallback_item_level,
            })
        }
    }
}

fn validate_item_level(value: u32, path: &str) -> Result<(), DtoConversionError> {
    if (1..=100).contains(&value) {
        Ok(())
    } else {
        Err(DtoConversionError::new(
            path,
            "item level must be between 1 and 100",
        ))
    }
}

fn item_rarity_from_domain(value: &str) -> Result<ItemRarityV1, DtoConversionError> {
    match value {
        "normal" => Ok(ItemRarityV1::Normal),
        "magic" => Ok(ItemRarityV1::Magic),
        "rare" => Ok(ItemRarityV1::Rare),
        other => Err(DtoConversionError::new(
            "/starting_item/item/rarity",
            format!("unsupported item rarity '{other}'"),
        )),
    }
}

fn item_influence_from_domain(
    value: &str,
    index: usize,
) -> Result<ItemInfluenceV1, DtoConversionError> {
    let path = format!("/starting_item/item/influences/{index}");
    match value {
        "shaper" => Ok(ItemInfluenceV1::Shaper),
        "elder" => Ok(ItemInfluenceV1::Elder),
        "crusader" => Ok(ItemInfluenceV1::Crusader),
        "hunter" => Ok(ItemInfluenceV1::Hunter),
        "redeemer" => Ok(ItemInfluenceV1::Redeemer),
        "warlord" => Ok(ItemInfluenceV1::Warlord),
        other => Err(DtoConversionError::new(
            path,
            format!("unsupported item influence '{other}'"),
        )),
    }
}

fn goal_into_domain(value: GoalV1, index: usize) -> Result<WantSpec, DtoConversionError> {
    validate_goal_shape(&value, index)?;
    Ok(WantSpec {
        mod_id: value.mod_id,
        group: value.group,
        stat: value.stat,
        min_value: checked_i32(value.min_value, &goal_path(index, "min_value"))?,
        max_value: checked_i32(value.max_value, &goal_path(index, "max_value"))?,
        mode: Some(value.mode.into()),
        cap: checked_i32(value.cap, &goal_path(index, "cap"))?,
        weight: value.weight.get(),
        required: value.required,
    })
}

fn goal_from_domain(value: &WantSpec, index: usize) -> Result<GoalV1, DtoConversionError> {
    let goal = GoalV1 {
        mod_id: value.mod_id.clone(),
        group: value.group.clone(),
        stat: value.stat.clone(),
        mode: value.scoring_mode().into(),
        min_value: value.min_value.map(i64::from),
        max_value: value.max_value.map(i64::from),
        cap: value.cap.map(i64::from),
        weight: positive_from_domain(value.weight, &goal_path(index, "weight"))?,
        required: value.required,
    };
    validate_goal_shape(&goal, index)?;
    Ok(goal)
}

fn validate_goal_shape(value: &GoalV1, index: usize) -> Result<(), DtoConversionError> {
    for (field, selector) in [
        ("mod_id", value.mod_id.as_deref()),
        ("group", value.group.as_deref()),
        ("stat", value.stat.as_deref()),
    ] {
        if selector.is_some_and(|selector| selector.trim().is_empty()) {
            return Err(DtoConversionError::new(
                goal_path(index, field),
                "selector must not be empty",
            ));
        }
    }

    if value.mod_id.is_none() && value.group.is_none() && value.stat.is_none() {
        return Err(DtoConversionError::new(
            goal_path(index, ""),
            "specify at least one of mod_id, group, or stat",
        ));
    }
    if value.min_value.is_some() && value.max_value.is_some() {
        return Err(DtoConversionError::new(
            goal_path(index, ""),
            "min_value and max_value are mutually exclusive",
        ));
    }

    match value.mode {
        ScoringModeV1::Presence => {
            if value.min_value.is_some() || value.max_value.is_some() {
                return Err(DtoConversionError::new(
                    goal_path(index, ""),
                    "presence mode does not accept min_value or max_value",
                ));
            }
            if value.cap.is_some() {
                return Err(DtoConversionError::new(
                    goal_path(index, "cap"),
                    "presence mode does not accept cap",
                ));
            }
        }
        ScoringModeV1::Threshold => {
            if value.min_value.is_none() && value.max_value.is_none() {
                return Err(DtoConversionError::new(
                    goal_path(index, ""),
                    "threshold mode requires exactly one of min_value or max_value",
                ));
            }
            if value.cap.is_some() {
                return Err(DtoConversionError::new(
                    goal_path(index, "cap"),
                    "threshold mode does not accept cap",
                ));
            }
        }
        ScoringModeV1::PerUnit => {
            if value.required && value.min_value.is_none() && value.max_value.is_none() {
                return Err(DtoConversionError::new(
                    goal_path(index, ""),
                    "a required per_unit goal needs min_value or max_value",
                ));
            }
            if value.max_value.is_some() && value.cap.is_none() {
                return Err(DtoConversionError::new(
                    goal_path(index, "cap"),
                    "lower-is-better per_unit scoring requires cap",
                ));
            }
        }
    }
    Ok(())
}

fn goal_path(index: usize, field: &str) -> String {
    if field.is_empty() {
        format!("/goals/{index}")
    } else {
        format!("/goals/{index}/{field}")
    }
}

fn checked_i32(value: Option<i64>, path: &str) -> Result<Option<i32>, DtoConversionError> {
    value
        .map(|value| {
            i32::try_from(value).map_err(|_| {
                DtoConversionError::new(path, "value is outside the supported i32 range")
            })
        })
        .transpose()
}

fn method_set_into_domain(value: MethodSetV1) -> Result<MethodSetRequest, DtoConversionError> {
    let configured = value
        .configured
        .into_iter()
        .enumerate()
        .map(|(index, method)| configured_method_into_domain(method, index))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(MethodSetRequest {
        configured,
        access: method_access_into_domain(value.access),
        price_overrides: price_book_from_entries(value.price_overrides)?,
    })
}

fn method_set_from_domain(value: &MethodSetRequest) -> Result<MethodSetV1, DtoConversionError> {
    Ok(MethodSetV1 {
        configured: value
            .configured
            .iter()
            .enumerate()
            .map(|(index, method)| configured_method_from_domain(method, index))
            .collect::<Result<Vec<_>, _>>()?,
        access: method_access_from_domain(&value.access),
        price_overrides: value
            .price_overrides
            .iter()
            .enumerate()
            .map(|(index, (method_id, amount))| {
                Ok(MethodPriceV1 {
                    method_id: method_id.clone(),
                    amount_chaos: positive_from_domain(
                        *amount,
                        &format!("/methods/price_overrides/{index}/amount_chaos"),
                    )?,
                })
            })
            .collect::<Result<Vec<_>, DtoConversionError>>()?,
    })
}

fn configured_method_into_domain(
    value: ConfiguredMethodV1,
    _index: usize,
) -> Result<MethodSpec, DtoConversionError> {
    Ok(match value {
        ConfiguredMethodV1::Essence {
            name,
            essence,
            mod_id,
            cost,
        } => MethodSpec::Essence {
            name,
            essence,
            mod_id,
            cost: cost.get(),
        },
        ConfiguredMethodV1::Bench { name, mod_id, cost } => MethodSpec::Bench {
            name,
            mod_id,
            cost: cost.get(),
        },
        ConfiguredMethodV1::Fossil {
            name,
            fossil,
            cost,
            boosted_tags,
            reduced_tags,
            blocked_mod_ids,
            forced_mod_ids,
            fossils,
        } => MethodSpec::Fossil {
            name,
            fossil,
            cost: cost.get(),
            boosted_tags,
            reduced_tags,
            blocked_mod_ids,
            forced_mod_ids,
            fossils: fossils.into_iter().map(fossil_part_into_domain).collect(),
        },
        ConfiguredMethodV1::Harvest { op, target, cost } => MethodSpec::Harvest {
            op: op.as_str().to_string(),
            target: target.as_str().to_string(),
            cost: cost.get(),
        },
        ConfiguredMethodV1::EldritchChaos { god } => MethodSpec::EldritchChaos {
            god: god.as_str().to_string(),
        },
        ConfiguredMethodV1::EldritchExalt { god } => MethodSpec::EldritchExalt {
            god: god.as_str().to_string(),
        },
        ConfiguredMethodV1::EldritchAnnul { god } => MethodSpec::EldritchAnnul {
            god: god.as_str().to_string(),
        },
        ConfiguredMethodV1::ConquerorExalt { influence } => MethodSpec::ConquerorExalt {
            influence: influence.as_str().to_string(),
        },
        ConfiguredMethodV1::BestiarySwap {
            add,
            beast_level,
            cost,
        } => MethodSpec::BestiarySwap {
            add: add.as_str().to_string(),
            beast_level,
            cost: cost.get(),
        },
    })
}

fn configured_method_from_domain(
    value: &MethodSpec,
    index: usize,
) -> Result<ConfiguredMethodV1, DtoConversionError> {
    let base = format!("/methods/configured/{index}");
    Ok(match value {
        MethodSpec::Essence {
            name,
            essence,
            mod_id,
            cost,
        } => ConfiguredMethodV1::Essence {
            name: name.clone(),
            essence: essence.clone(),
            mod_id: mod_id.clone(),
            cost: positive_from_domain(*cost, &format!("{base}/cost"))?,
        },
        MethodSpec::Bench { name, mod_id, cost } => ConfiguredMethodV1::Bench {
            name: name.clone(),
            mod_id: mod_id.clone(),
            cost: positive_from_domain(*cost, &format!("{base}/cost"))?,
        },
        MethodSpec::Fossil {
            name,
            fossil,
            cost,
            boosted_tags,
            reduced_tags,
            blocked_mod_ids,
            forced_mod_ids,
            fossils,
        } => ConfiguredMethodV1::Fossil {
            name: name.clone(),
            fossil: fossil.clone(),
            cost: positive_from_domain(*cost, &format!("{base}/cost"))?,
            boosted_tags: boosted_tags.clone(),
            reduced_tags: reduced_tags.clone(),
            blocked_mod_ids: blocked_mod_ids.clone(),
            forced_mod_ids: forced_mod_ids.clone(),
            fossils: fossils.iter().map(fossil_part_from_domain).collect(),
        },
        MethodSpec::Harvest { op, target, cost } => ConfiguredMethodV1::Harvest {
            op: harvest_operation_from_domain(op, &format!("{base}/op"))?,
            target: harvest_target_from_domain(target, &format!("{base}/target"))?,
            cost: positive_from_domain(*cost, &format!("{base}/cost"))?,
        },
        MethodSpec::EldritchChaos { god } => ConfiguredMethodV1::EldritchChaos {
            god: eldritch_side_from_domain(god, &format!("{base}/god"))?,
        },
        MethodSpec::EldritchExalt { god } => ConfiguredMethodV1::EldritchExalt {
            god: eldritch_side_from_domain(god, &format!("{base}/god"))?,
        },
        MethodSpec::EldritchAnnul { god } => ConfiguredMethodV1::EldritchAnnul {
            god: eldritch_side_from_domain(god, &format!("{base}/god"))?,
        },
        MethodSpec::ConquerorExalt { influence } => ConfiguredMethodV1::ConquerorExalt {
            influence: conqueror_influence_from_domain(influence, &format!("{base}/influence"))?,
        },
        MethodSpec::BestiarySwap {
            add,
            beast_level,
            cost,
        } => ConfiguredMethodV1::BestiarySwap {
            add: affix_side_from_domain(add, &format!("{base}/add"))?,
            beast_level: *beast_level,
            cost: positive_from_domain(*cost, &format!("{base}/cost"))?,
        },
    })
}

fn fossil_part_into_domain(value: FossilPartV1) -> FossilPartSpec {
    FossilPartSpec {
        fossil: value.fossil,
        boosted_tags: value.boosted_tags,
        reduced_tags: value.reduced_tags,
        blocked_mod_ids: value.blocked_mod_ids,
        forced_mod_ids: value.forced_mod_ids,
    }
}

fn fossil_part_from_domain(value: &FossilPartSpec) -> FossilPartV1 {
    FossilPartV1 {
        fossil: value.fossil.clone(),
        boosted_tags: value.boosted_tags.clone(),
        reduced_tags: value.reduced_tags.clone(),
        blocked_mod_ids: value.blocked_mod_ids.clone(),
        forced_mod_ids: value.forced_mod_ids.clone(),
    }
}

fn harvest_operation_from_domain(
    value: &str,
    path: &str,
) -> Result<HarvestOperationV1, DtoConversionError> {
    match value {
        "reforge" => Ok(HarvestOperationV1::Reforge),
        "augment" => Ok(HarvestOperationV1::Augment),
        other => Err(DtoConversionError::new(
            path,
            format!("unsupported Harvest operation '{other}'"),
        )),
    }
}

fn harvest_target_from_domain(
    value: &str,
    path: &str,
) -> Result<HarvestTargetV1, DtoConversionError> {
    match value {
        "attack" => Ok(HarvestTargetV1::Attack),
        "caster" => Ok(HarvestTargetV1::Caster),
        "speed" => Ok(HarvestTargetV1::Speed),
        "life" => Ok(HarvestTargetV1::Life),
        "defence" | "defences" => Ok(HarvestTargetV1::Defence),
        "resistance" | "elemental" => Ok(HarvestTargetV1::Resistance),
        "chaos" => Ok(HarvestTargetV1::Chaos),
        "fire" => Ok(HarvestTargetV1::Fire),
        "cold" => Ok(HarvestTargetV1::Cold),
        "lightning" => Ok(HarvestTargetV1::Lightning),
        "physical" => Ok(HarvestTargetV1::Physical),
        "critical" => Ok(HarvestTargetV1::Critical),
        "minion" => Ok(HarvestTargetV1::Minion),
        "mana" => Ok(HarvestTargetV1::Mana),
        other => Err(DtoConversionError::new(
            path,
            format!("unsupported Harvest target '{other}'"),
        )),
    }
}

fn eldritch_side_from_domain(
    value: &str,
    path: &str,
) -> Result<EldritchSideV1, DtoConversionError> {
    match value {
        "exarch" => Ok(EldritchSideV1::Exarch),
        "eater" => Ok(EldritchSideV1::Eater),
        other => Err(DtoConversionError::new(
            path,
            format!("unsupported Eldritch side '{other}'"),
        )),
    }
}

fn conqueror_influence_from_domain(
    value: &str,
    path: &str,
) -> Result<ConquerorInfluenceV1, DtoConversionError> {
    match value {
        "crusader" => Ok(ConquerorInfluenceV1::Crusader),
        "hunter" => Ok(ConquerorInfluenceV1::Hunter),
        "redeemer" => Ok(ConquerorInfluenceV1::Redeemer),
        "warlord" => Ok(ConquerorInfluenceV1::Warlord),
        other => Err(DtoConversionError::new(
            path,
            format!("unsupported Conqueror influence '{other}'"),
        )),
    }
}

fn affix_side_from_domain(value: &str, path: &str) -> Result<AffixSideV1, DtoConversionError> {
    match value {
        "prefix" => Ok(AffixSideV1::Prefix),
        "suffix" => Ok(AffixSideV1::Suffix),
        other => Err(DtoConversionError::new(
            path,
            format!("unsupported affix side '{other}'"),
        )),
    }
}

fn method_access_into_domain(value: MethodAccessV1) -> MethodAccessPolicy {
    match value {
        MethodAccessV1::LegacyDefaultsAndConfigured => {
            MethodAccessPolicy::LegacyDefaultsAndConfigured
        }
        MethodAccessV1::Allowlist { method_ids } => MethodAccessPolicy::Allowlist(method_ids),
        MethodAccessV1::Explicit { selection } => MethodAccessPolicy::Explicit(MethodSelection {
            enabled_families: selection
                .enabled_families
                .into_iter()
                .map(Into::into)
                .collect(),
            enabled_methods: selection.enabled_methods,
            disabled_methods: selection.disabled_methods,
        }),
    }
}

fn method_access_from_domain(value: &MethodAccessPolicy) -> MethodAccessV1 {
    match value {
        MethodAccessPolicy::LegacyDefaultsAndConfigured => {
            MethodAccessV1::LegacyDefaultsAndConfigured
        }
        MethodAccessPolicy::Allowlist(method_ids) => MethodAccessV1::Allowlist {
            method_ids: method_ids.clone(),
        },
        MethodAccessPolicy::Explicit(selection) => MethodAccessV1::Explicit {
            selection: MethodSelectionV1 {
                enabled_families: selection
                    .enabled_families
                    .iter()
                    .copied()
                    .map(Into::into)
                    .collect(),
                enabled_methods: selection.enabled_methods.clone(),
                disabled_methods: selection.disabled_methods.clone(),
            },
        },
    }
}

fn price_book_from_entries(entries: Vec<MethodPriceV1>) -> Result<PriceBook, DtoConversionError> {
    let mut seen = BTreeSet::new();
    let mut price_book = PriceBook::new();
    for (index, entry) in entries.into_iter().enumerate() {
        if !seen.insert(entry.method_id.clone()) {
            return Err(DtoConversionError::new(
                format!("/methods/price_overrides/{index}/method_id"),
                format!("duplicate method price for '{}'", entry.method_id),
            ));
        }
        price_book
            .set(entry.method_id, entry.amount_chaos.get())
            .map_err(|error| {
                DtoConversionError::new(
                    format!("/methods/price_overrides/{index}/amount_chaos"),
                    error.to_string(),
                )
            })?;
    }
    Ok(price_book)
}

fn budget_into_domain(value: BudgetV1) -> Result<BudgetPolicy, DtoConversionError> {
    match value {
        BudgetV1::Unbounded { metric } => Ok(BudgetPolicy::unbounded(metric.into())),
        BudgetV1::HardCap {
            amount_chaos,
            metric,
        } => BudgetPolicy::hard_cap(amount_chaos.get(), metric.into()).map_err(|error| {
            DtoConversionError::new(
                "/budget/amount_chaos",
                format!("invalid hard cap: {}", error.amount_chaos()),
            )
        }),
    }
}

fn budget_from_domain(value: BudgetPolicy) -> Result<BudgetV1, DtoConversionError> {
    let metric = value.metric().into();
    match value.hard_cap_chaos() {
        Some(amount) => Ok(BudgetV1::HardCap {
            amount_chaos: nonnegative_from_domain(amount, "/budget/amount_chaos")?,
            metric,
        }),
        None => Ok(BudgetV1::Unbounded { metric }),
    }
}

fn search_into_domain(value: SearchV1) -> Result<SearchRequest, DtoConversionError> {
    if value.beam_width == 0 {
        return Err(DtoConversionError::new(
            "/search/beam_width",
            "beam_width must be greater than zero",
        ));
    }
    if value.max_steps == 0 {
        return Err(DtoConversionError::new(
            "/search/max_steps",
            "max_steps must be greater than zero",
        ));
    }
    if value.top == 0 {
        return Err(DtoConversionError::new(
            "/search/top",
            "top must be greater than zero",
        ));
    }
    Ok(SearchRequest {
        beam_width: checked_usize(value.beam_width, "/search/beam_width")?,
        max_steps: checked_usize(value.max_steps, "/search/max_steps")?,
        cost_weight: value.cost_weight.get(),
        restart_cost: value.restart_cost.get(),
        seed: value.seed.map(DecimalU64V1::get),
        top: checked_usize(value.top, "/search/top")?,
        expansion_limit: value.expansion_limit,
        timeout_ms: value.timeout_ms,
    })
}

fn search_from_domain(value: SearchRequest) -> Result<SearchV1, DtoConversionError> {
    if value.beam_width == 0 {
        return Err(DtoConversionError::new(
            "/search/beam_width",
            "beam_width must be greater than zero",
        ));
    }
    if value.max_steps == 0 {
        return Err(DtoConversionError::new(
            "/search/max_steps",
            "max_steps must be greater than zero",
        ));
    }
    if value.top == 0 {
        return Err(DtoConversionError::new(
            "/search/top",
            "top must be greater than zero",
        ));
    }
    Ok(SearchV1 {
        beam_width: checked_u64(value.beam_width, "/search/beam_width")?,
        max_steps: checked_u64(value.max_steps, "/search/max_steps")?,
        cost_weight: nonnegative_from_domain(value.cost_weight, "/search/cost_weight")?,
        restart_cost: nonnegative_from_domain(value.restart_cost, "/search/restart_cost")?,
        seed: value.seed.map(DecimalU64V1::new),
        top: checked_u64(value.top, "/search/top")?,
        expansion_limit: value.expansion_limit,
        timeout_ms: value.timeout_ms,
    })
}

fn checked_usize(value: u64, path: &str) -> Result<usize, DtoConversionError> {
    usize::try_from(value).map_err(|_| {
        DtoConversionError::new(path, "value is outside the supported platform usize range")
    })
}

fn checked_u64(value: usize, path: &str) -> Result<u64, DtoConversionError> {
    u64::try_from(value)
        .map_err(|_| DtoConversionError::new(path, "value is outside the saved-request u64 range"))
}

fn positive_from_domain(value: f64, path: &str) -> Result<PositiveFiniteV1, DtoConversionError> {
    PositiveFiniteV1::new(value)
        .ok_or_else(|| DtoConversionError::new(path, "expected a strictly positive finite number"))
}

fn nonnegative_from_domain(
    value: f64,
    path: &str,
) -> Result<NonNegativeFiniteV1, DtoConversionError> {
    NonNegativeFiniteV1::new(value)
        .ok_or_else(|| DtoConversionError::new(path, "expected a non-negative finite number"))
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    fn positive(value: f64) -> PositiveFiniteV1 {
        PositiveFiniteV1::new(value).expect("test value should be positive and finite")
    }

    fn nonnegative(value: f64) -> NonNegativeFiniteV1 {
        NonNegativeFiniteV1::new(value).expect("test value should be non-negative and finite")
    }

    fn method_id(value: &str) -> MethodId {
        MethodId::parse(value).expect("test method ID should be canonical")
    }

    fn sample_request() -> SavedOptimizeRequestV1 {
        SavedOptimizeRequestV1 {
            schema_version: SchemaVersionV1,
            starting_item: StartingItemV1::Described {
                item: DescribedItemV1 {
                    base: "Astral Plate".to_string(),
                    item_level: 86,
                    rarity: Some(ItemRarityV1::Rare),
                    mods: vec![StartingModifierV1 {
                        mod_id: "IncreasedLife9".to_string(),
                        values: Some(vec![101]),
                        fractured: true,
                        crafted: false,
                    }],
                    influences: vec![ItemInfluenceV1::Hunter],
                    exarch_implicit: None,
                    eater_implicit: None,
                },
            },
            goals: vec![
                GoalV1 {
                    mod_id: None,
                    group: Some("IncreasedLife".to_string()),
                    stat: None,
                    mode: ScoringModeV1::Presence,
                    min_value: None,
                    max_value: None,
                    cap: None,
                    weight: positive(10.0),
                    required: true,
                },
                GoalV1 {
                    mod_id: None,
                    group: Some("IncreasedLife".to_string()),
                    stat: Some("base_maximum_life".to_string()),
                    mode: ScoringModeV1::Threshold,
                    min_value: Some(90),
                    max_value: None,
                    cap: None,
                    weight: positive(2.5),
                    required: true,
                },
                GoalV1 {
                    mod_id: None,
                    group: Some("LocalIncreasedEnergyShield".to_string()),
                    stat: None,
                    mode: ScoringModeV1::PerUnit,
                    min_value: None,
                    max_value: None,
                    cap: Some(300),
                    weight: positive(0.25),
                    required: false,
                },
                GoalV1 {
                    mod_id: Some("ReducedRequirement".to_string()),
                    group: None,
                    stat: Some("local_attribute_requirements_+%".to_string()),
                    mode: ScoringModeV1::PerUnit,
                    min_value: None,
                    max_value: Some(-10),
                    cap: Some(0),
                    weight: positive(1.0),
                    required: true,
                },
            ],
            methods: MethodSetV1 {
                configured: vec![
                    ConfiguredMethodV1::Essence {
                        name: Some("Greed".to_string()),
                        essence: Some("Metadata/Items/Currency/Essence/Greed7".to_string()),
                        mod_id: None,
                        cost: positive(5.0),
                    },
                    ConfiguredMethodV1::Bench {
                        name: None,
                        mod_id: "EinharMasterIncreasedLife5_".to_string(),
                        cost: positive(2.0),
                    },
                    ConfiguredMethodV1::Fossil {
                        name: Some("Life resonator".to_string()),
                        fossil: Some(
                            "Metadata/Items/Currency/CurrencyDelveCraftingLife".to_string(),
                        ),
                        cost: positive(12.0),
                        boosted_tags: vec!["life".to_string()],
                        reduced_tags: vec!["defences".to_string()],
                        blocked_mod_ids: vec!["BlockedMod".to_string()],
                        forced_mod_ids: vec!["ForcedMod".to_string()],
                        fossils: vec![FossilPartV1 {
                            fossil: Some(
                                "Metadata/Items/Currency/CurrencyDelveCraftingDefences".to_string(),
                            ),
                            boosted_tags: vec!["defences".to_string()],
                            reduced_tags: Vec::new(),
                            blocked_mod_ids: Vec::new(),
                            forced_mod_ids: Vec::new(),
                        }],
                    },
                    ConfiguredMethodV1::Harvest {
                        op: HarvestOperationV1::Reforge,
                        target: HarvestTargetV1::Life,
                        cost: positive(30.0),
                    },
                    ConfiguredMethodV1::EldritchChaos {
                        god: EldritchSideV1::Exarch,
                    },
                    ConfiguredMethodV1::EldritchExalt {
                        god: EldritchSideV1::Eater,
                    },
                    ConfiguredMethodV1::EldritchAnnul {
                        god: EldritchSideV1::Exarch,
                    },
                    ConfiguredMethodV1::ConquerorExalt {
                        influence: ConquerorInfluenceV1::Hunter,
                    },
                    ConfiguredMethodV1::BestiarySwap {
                        add: AffixSideV1::Prefix,
                        beast_level: 83,
                        cost: positive(12.0),
                    },
                ],
                access: MethodAccessV1::Explicit {
                    selection: MethodSelectionV1 {
                        enabled_families: vec![MethodFamilyV1::Currency, MethodFamilyV1::Essence],
                        enabled_methods: vec![method_id("bench/add-explicit/Example")],
                        disabled_methods: vec![method_id("currency/divine")],
                    },
                },
                price_overrides: vec![
                    MethodPriceV1 {
                        method_id: method_id("currency/chaos"),
                        amount_chaos: positive(1.0),
                    },
                    MethodPriceV1 {
                        method_id: method_id("currency/divine"),
                        amount_chaos: positive(220.0),
                    },
                ],
            },
            budget: BudgetV1::HardCap {
                amount_chaos: nonnegative(500.0),
                metric: BudgetMetricV1::RestartAdjustedExpected,
            },
            search: SearchV1 {
                beam_width: 64,
                max_steps: 12,
                cost_weight: nonnegative(0.02),
                restart_cost: nonnegative(3.0),
                seed: Some(DecimalU64V1::new(u64::MAX)),
                top: 5,
                expansion_limit: Some(100_000),
                timeout_ms: Some(15_000),
            },
        }
    }

    #[test]
    fn request_json_round_trip_covers_every_current_method_variant() {
        let request = sample_request();
        let value = serde_json::to_value(&request).expect("request should serialize");

        assert_eq!(value["schema_version"], json!(1));
        assert_eq!(value["search"]["seed"], json!(u64::MAX.to_string()));
        assert_eq!(value["methods"]["configured"].as_array().unwrap().len(), 9);
        assert_eq!(
            value["methods"]["configured"][2]["fossils"][0]["fossil"],
            json!("Metadata/Items/Currency/CurrencyDelveCraftingDefences")
        );

        let round_trip: SavedOptimizeRequestV1 =
            serde_json::from_value(value).expect("serialized request should deserialize");
        assert_eq!(round_trip, request);
    }

    #[test]
    fn both_starting_item_variants_round_trip() {
        let mut request = sample_request();
        request.starting_item = StartingItemV1::ImportedText {
            text: "Rarity: Rare\nExample".to_string(),
            fallback_item_level: Some(86),
        };

        let json = serde_json::to_string(&request).expect("request should serialize");
        let round_trip: SavedOptimizeRequestV1 =
            serde_json::from_str(&json).expect("request should deserialize");
        assert_eq!(round_trip, request);
    }

    #[test]
    fn schema_version_is_required_numeric_and_exactly_one() {
        let mut value = serde_json::to_value(sample_request()).unwrap();

        value.as_object_mut().unwrap().remove("schema_version");
        assert!(serde_json::from_value::<SavedOptimizeRequestV1>(value.clone()).is_err());

        value["schema_version"] = json!(2);
        let error = serde_json::from_value::<SavedOptimizeRequestV1>(value.clone())
            .expect_err("wrong version must be rejected");
        assert!(error.to_string().contains("unsupported schema_version 2"));

        value["schema_version"] = json!("1");
        assert!(serde_json::from_value::<SavedOptimizeRequestV1>(value).is_err());
    }

    #[test]
    fn unknown_fields_and_variants_are_rejected() {
        let base = serde_json::to_value(sample_request()).unwrap();

        let mut top_level = base.clone();
        top_level["future_field"] = json!(true);
        assert!(serde_json::from_value::<SavedOptimizeRequestV1>(top_level).is_err());

        let mut nested = base.clone();
        nested["search"]["future_field"] = json!(1);
        assert!(serde_json::from_value::<SavedOptimizeRequestV1>(nested).is_err());

        let mut method_variant = base.clone();
        method_variant["methods"]["configured"][0]["type"] = json!("future_method");
        assert!(serde_json::from_value::<SavedOptimizeRequestV1>(method_variant).is_err());

        let mut scoring_variant = base;
        scoring_variant["goals"][0]["mode"] = json!("future_mode");
        assert!(serde_json::from_value::<SavedOptimizeRequestV1>(scoring_variant).is_err());
    }

    #[test]
    fn finite_newtypes_reject_invalid_deserialization_and_serialization() {
        for invalid in ["0", "-0.0", "-1", "null", "\"1\"", "1e400"] {
            assert!(
                serde_json::from_str::<PositiveFiniteV1>(invalid).is_err(),
                "{invalid} should not be a positive finite value"
            );
        }
        for invalid in ["-0.0", "-1", "null", "\"1\"", "1e400"] {
            assert!(
                serde_json::from_str::<NonNegativeFiniteV1>(invalid).is_err(),
                "{invalid} should not be a canonical non-negative finite value"
            );
        }
        assert_eq!(
            serde_json::from_str::<NonNegativeFiniteV1>("0")
                .unwrap()
                .get(),
            0.0
        );

        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.0] {
            assert!(serde_json::to_value(PositiveFiniteV1(invalid)).is_err());
        }
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.0, -1.0] {
            assert!(serde_json::to_value(NonNegativeFiniteV1(invalid)).is_err());
        }
    }

    #[test]
    fn invalid_nested_numeric_values_are_rejected() {
        let base = serde_json::to_value(sample_request()).unwrap();

        for invalid in [json!(0), json!(-1), Value::Null] {
            let mut value = base.clone();
            value["goals"][0]["weight"] = invalid;
            assert!(serde_json::from_value::<SavedOptimizeRequestV1>(value).is_err());
        }

        let mut price = base.clone();
        price["methods"]["price_overrides"][0]["amount_chaos"] = json!(-1);
        assert!(serde_json::from_value::<SavedOptimizeRequestV1>(price).is_err());

        let mut budget = base.clone();
        budget["budget"]["amount_chaos"] = json!(-1);
        assert!(serde_json::from_value::<SavedOptimizeRequestV1>(budget).is_err());

        let mut search = base;
        search["search"]["cost_weight"] = Value::Null;
        assert!(serde_json::from_value::<SavedOptimizeRequestV1>(search).is_err());
    }

    #[test]
    fn seed_is_a_canonical_decimal_u64_string() {
        let maximum: DecimalU64V1 = serde_json::from_value(json!(u64::MAX.to_string())).unwrap();
        assert_eq!(maximum.get(), u64::MAX);
        assert_eq!(
            serde_json::to_value(maximum).unwrap(),
            json!(u64::MAX.to_string())
        );

        for invalid in [
            json!(42),
            json!(-1),
            json!(""),
            json!("+1"),
            json!("-1"),
            json!("01"),
            json!(" 1"),
            json!("18446744073709551616"),
            Value::Null,
        ] {
            assert!(
                serde_json::from_value::<DecimalU64V1>(invalid.clone()).is_err(),
                "{invalid} should not be a canonical u64 string"
            );
        }
    }

    #[test]
    fn duplicate_price_ids_fail_with_the_second_entry_path() {
        let mut request = sample_request();
        request.methods.price_overrides.push(MethodPriceV1 {
            method_id: method_id("currency/chaos"),
            amount_chaos: positive(2.0),
        });

        let error = OptimizeRequest::try_from(request)
            .expect_err("duplicate semantic prices must be rejected");
        assert_eq!(error.field_path(), "/methods/price_overrides/2/method_id");
        assert!(error.message().contains("currency/chaos"));
    }

    #[test]
    fn semantic_goal_errors_and_checked_integer_conversion_have_paths() {
        let mut missing_threshold = sample_request();
        missing_threshold.goals[1].min_value = None;
        let error = OptimizeRequest::try_from(missing_threshold)
            .expect_err("threshold without a bound must fail");
        assert_eq!(error.field_path(), "/goals/1");

        let mut overflowing = sample_request();
        overflowing.goals[1].min_value = Some(i64::MAX);
        let error = OptimizeRequest::try_from(overflowing).expect_err("out-of-range i32 must fail");
        assert_eq!(error.field_path(), "/goals/1/min_value");
        assert!(error.message().contains("i32"));
    }

    #[test]
    fn dto_domain_dto_conversion_is_lossless_and_canonical() {
        let dto = sample_request();
        let domain =
            OptimizeRequest::try_from(dto.clone()).expect("sample DTO should map to the domain");
        let round_trip = SavedOptimizeRequestV1::try_from(&domain)
            .expect("mapped domain request should map back to v1");

        assert_eq!(round_trip, dto);
        assert_eq!(domain.budget.hard_cap_chaos(), Some(500.0));
        assert_eq!(
            domain.budget.metric(),
            BudgetMetric::RestartAdjustedExpected
        );
        assert_eq!(domain.search.seed, Some(u64::MAX));
        assert_eq!(domain.search.expansion_limit, Some(100_000));
        assert_eq!(domain.search.timeout_ms, Some(15_000));
        assert_eq!(domain.methods.configured.len(), 9);
    }

    #[test]
    fn reverse_conversion_rejects_invalid_programmatic_numbers_with_paths() {
        let mut domain = OptimizeRequest::try_from(sample_request()).unwrap();
        domain.search.cost_weight = f64::NAN;

        let error = SavedOptimizeRequestV1::try_from(&domain)
            .expect_err("invalid domain number must not enter serialized DTO");
        assert_eq!(error.field_path(), "/search/cost_weight");
    }

    #[test]
    fn price_output_is_sorted_by_semantic_method_id() {
        let mut domain = OptimizeRequest::try_from(sample_request()).unwrap();
        let prices = PriceBook::try_from_iter([
            (method_id("currency/divine"), 220.0),
            (method_id("currency/chaos"), 1.0),
        ])
        .unwrap();
        domain.methods.price_overrides = prices;

        let dto = SavedOptimizeRequestV1::try_from(&domain).unwrap();
        let ids = dto
            .methods
            .price_overrides
            .iter()
            .map(|entry| entry.method_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["currency/chaos", "currency/divine"]);
    }
}
