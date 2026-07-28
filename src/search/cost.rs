//! Explicit, serialization-safe cost values for search economics.
//!
//! Search may encounter a finite expected cost, a provably diverging
//! expectation, or a metric that was not computed. Keeping those states
//! distinct prevents `NaN` and infinity from leaking through JSON as `null`
//! and prevents unavailable metrics from silently passing a budget.

use std::fmt;
use std::iter::Sum;
use std::ops::Add;

use serde::de::Error as _;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A validated non-negative finite chaos-equivalent amount.
///
/// The field is private so [`CostValue::Finite`] cannot contain `NaN`,
/// infinity, or a negative amount. Negative zero is normalized to positive
/// zero at construction.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct FiniteCost(f64);

impl FiniteCost {
    /// Validate a non-negative finite chaos-equivalent amount.
    pub fn new(amount_chaos: f64) -> Result<Self, InvalidChaosAmount> {
        if !amount_chaos.is_finite() || amount_chaos < 0.0 {
            return Err(InvalidChaosAmount { amount_chaos });
        }

        Ok(Self(if amount_chaos == 0.0 {
            0.0
        } else {
            amount_chaos
        }))
    }

    /// Return the validated chaos-equivalent amount.
    pub fn amount_chaos(self) -> f64 {
        self.0
    }
}

// FiniteCost's private constructor excludes NaN, making equality reflexive.
impl Eq for FiniteCost {}

/// A cost metric with explicit non-finite and unavailable states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostValue {
    /// A computed, non-negative, finite chaos-equivalent amount.
    Finite(FiniteCost),
    /// A provably diverging expectation, such as retrying a zero-probability
    /// outcome forever.
    Unbounded,
    /// A metric that was not computed and therefore cannot be compared.
    Unavailable,
}

impl CostValue {
    /// Construct a validated finite cost.
    pub fn finite(amount_chaos: f64) -> Result<Self, InvalidChaosAmount> {
        FiniteCost::new(amount_chaos).map(Self::Finite)
    }

    /// Convert a completed non-negative calculation into an explicit value.
    ///
    /// Positive infinity, which can result from finite overflow or a
    /// diverging expectation, maps to [`Self::Unbounded`]. Negative and NaN
    /// values are rejected as invalid calculations.
    pub fn from_computed(amount_chaos: f64) -> Result<Self, InvalidChaosAmount> {
        if amount_chaos == f64::INFINITY {
            Ok(Self::Unbounded)
        } else {
            Self::finite(amount_chaos)
        }
    }

    /// The additive identity for computed costs.
    pub fn zero() -> Self {
        Self::Finite(FiniteCost(0.0))
    }

    /// Return the finite amount, or `None` for non-finite states.
    pub fn amount_chaos(self) -> Option<f64> {
        match self {
            Self::Finite(amount) => Some(amount.amount_chaos()),
            Self::Unbounded | Self::Unavailable => None,
        }
    }

    /// Whether this value contains a computed finite amount.
    pub fn is_finite(self) -> bool {
        matches!(self, Self::Finite(_))
    }

    /// Whether this value is a provably diverging expectation.
    pub fn is_unbounded(self) -> bool {
        matches!(self, Self::Unbounded)
    }

    /// Whether this metric was not computed.
    pub fn is_unavailable(self) -> bool {
        matches!(self, Self::Unavailable)
    }

    /// Add two non-negative cost values.
    ///
    /// Finite overflow becomes [`CostValue::Unbounded`]. An unbounded term
    /// proves the non-negative sum unbounded even when the other term is
    /// unavailable; otherwise an unavailable term keeps the sum unavailable.
    pub fn plus(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unbounded, _) | (_, Self::Unbounded) => Self::Unbounded,
            (Self::Unavailable, _) | (_, Self::Unavailable) => Self::Unavailable,
            (Self::Finite(left), Self::Finite(right)) => {
                Self::from_arithmetic(left.amount_chaos() + right.amount_chaos())
            }
        }
    }

    /// Multiply by a validated non-negative finite factor.
    ///
    /// Finite overflow becomes [`CostValue::Unbounded`]. Multiplying an
    /// unbounded expectation by zero is indeterminate and therefore returns
    /// [`CostValue::Unavailable`].
    pub fn scaled_by(self, factor: f64) -> Result<Self, InvalidChaosAmount> {
        let factor = FiniteCost::new(factor)?.amount_chaos();
        Ok(match self {
            Self::Finite(amount) => Self::from_arithmetic(amount.amount_chaos() * factor),
            Self::Unbounded if factor == 0.0 => Self::Unavailable,
            Self::Unbounded => Self::Unbounded,
            Self::Unavailable => Self::Unavailable,
        })
    }

    /// Divide by a success probability for retry-until-hit economics.
    ///
    /// A zero success probability produces [`CostValue::Unbounded`].
    /// Probabilities outside `[0, 1]` or non-finite values are rejected.
    /// Finite division overflow also becomes unbounded.
    pub fn divided_by_probability(self, probability: f64) -> Result<Self, InvalidProbability> {
        if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
            return Err(InvalidProbability { probability });
        }
        if probability == 0.0 {
            return Ok(Self::Unbounded);
        }

        Ok(match self {
            Self::Finite(amount) => Self::from_arithmetic(amount.amount_chaos() / probability),
            Self::Unbounded => Self::Unbounded,
            Self::Unavailable => Self::Unavailable,
        })
    }

    /// Compare this metric with an optional active hard cap.
    ///
    /// Without an active cap there is no comparison. With a cap, unbounded is
    /// always over budget while unavailable is never comparable. A finite
    /// value equal to the cap counts as under budget.
    pub fn compare_to_budget(
        self,
        active_cap_chaos: Option<f64>,
    ) -> Result<BudgetComparison, InvalidChaosAmount> {
        let Some(active_cap_chaos) = active_cap_chaos else {
            return Ok(BudgetComparison::NotComparable);
        };
        let active_cap = FiniteCost::new(active_cap_chaos)?.amount_chaos();

        Ok(match self {
            Self::Finite(amount) if amount.amount_chaos() <= active_cap => BudgetComparison::Under,
            Self::Finite(_) | Self::Unbounded => BudgetComparison::Over,
            Self::Unavailable => BudgetComparison::NotComparable,
        })
    }

    /// Return how far an over-budget value exceeds the active cap.
    ///
    /// A finite excess is finite; an unbounded cost has an unbounded excess.
    /// Under-budget, uncapped, and unavailable values return `None`.
    pub fn budget_excess(
        self,
        active_cap_chaos: Option<f64>,
    ) -> Result<Option<Self>, InvalidChaosAmount> {
        let Some(active_cap_chaos) = active_cap_chaos else {
            return Ok(None);
        };
        let active_cap = FiniteCost::new(active_cap_chaos)?.amount_chaos();

        Ok(match self {
            Self::Finite(amount) if amount.amount_chaos() > active_cap => {
                Some(Self::from_arithmetic(amount.amount_chaos() - active_cap))
            }
            Self::Unbounded => Some(Self::Unbounded),
            Self::Finite(_) | Self::Unavailable => None,
        })
    }

    fn from_arithmetic(amount_chaos: f64) -> Self {
        if amount_chaos.is_finite() {
            debug_assert!(amount_chaos >= 0.0);
            Self::Finite(FiniteCost(if amount_chaos == 0.0 {
                0.0
            } else {
                amount_chaos
            }))
        } else {
            Self::Unbounded
        }
    }
}

impl Default for CostValue {
    fn default() -> Self {
        Self::zero()
    }
}

impl Add for CostValue {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        self.plus(other)
    }
}

impl Sum for CostValue {
    fn sum<I: Iterator<Item = Self>>(values: I) -> Self {
        values.fold(Self::zero(), Self::plus)
    }
}

impl Serialize for CostValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Finite(amount) => {
                let mut state = serializer.serialize_struct("CostValue", 2)?;
                state.serialize_field("kind", "finite")?;
                state.serialize_field("amount_chaos", &amount.amount_chaos())?;
                state.end()
            }
            Self::Unbounded => {
                let mut state = serializer.serialize_struct("CostValue", 1)?;
                state.serialize_field("kind", "unbounded")?;
                state.end()
            }
            Self::Unavailable => {
                let mut state = serializer.serialize_struct("CostValue", 1)?;
                state.serialize_field("kind", "unavailable")?;
                state.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for CostValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum WireCostKind {
            Finite,
            Unbounded,
            Unavailable,
        }

        #[derive(Default)]
        enum WireAmount {
            #[default]
            Missing,
            Present(f64),
        }

        impl<'de> Deserialize<'de> for WireAmount {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                f64::deserialize(deserializer).map(Self::Present)
            }
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireCostValue {
            kind: WireCostKind,
            #[serde(default)]
            amount_chaos: WireAmount,
        }

        let wire = WireCostValue::deserialize(deserializer)?;
        match (wire.kind, wire.amount_chaos) {
            (WireCostKind::Finite, WireAmount::Present(amount_chaos)) => {
                Self::finite(amount_chaos).map_err(D::Error::custom)
            }
            (WireCostKind::Finite, WireAmount::Missing) => {
                Err(D::Error::missing_field("amount_chaos"))
            }
            (WireCostKind::Unbounded, WireAmount::Missing) => Ok(Self::Unbounded),
            (WireCostKind::Unavailable, WireAmount::Missing) => Ok(Self::Unavailable),
            (WireCostKind::Unbounded | WireCostKind::Unavailable, WireAmount::Present(_)) => Err(
                D::Error::custom("amount_chaos is only valid when kind is finite"),
            ),
        }
    }
}

/// Result of comparing a cost metric with an optional active budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetComparison {
    /// The computed cost is less than or equal to the active cap.
    Under,
    /// The computed cost exceeds the cap, or is unbounded.
    Over,
    /// No cap is active, or the metric is unavailable.
    NotComparable,
}

/// Cost metric selected for hard-cap pruning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BudgetMetric {
    FirstTry,
    RetryExpected,
    #[default]
    RestartAdjustedExpected,
}

impl BudgetMetric {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FirstTry => "first_try",
            Self::RetryExpected => "retry_expected",
            Self::RestartAdjustedExpected => "restart_adjusted_expected",
        }
    }
}

/// Optional chaos-equivalent hard cap and its binding cost metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetPolicy {
    hard_cap: Option<FiniteCost>,
    metric: BudgetMetric,
}

impl BudgetPolicy {
    /// No hard cap. The metric is retained for comparable reporting.
    pub const fn unbounded(metric: BudgetMetric) -> Self {
        Self {
            hard_cap: None,
            metric,
        }
    }

    /// Validate and construct a hard-cap policy.
    pub fn hard_cap(amount_chaos: f64, metric: BudgetMetric) -> Result<Self, InvalidChaosAmount> {
        Ok(Self {
            hard_cap: Some(FiniteCost::new(amount_chaos)?),
            metric,
        })
    }

    pub const fn metric(self) -> BudgetMetric {
        self.metric
    }

    pub fn hard_cap_chaos(self) -> Option<f64> {
        self.hard_cap.map(FiniteCost::amount_chaos)
    }
}

impl Default for BudgetPolicy {
    fn default() -> Self {
        Self::unbounded(BudgetMetric::RestartAdjustedExpected)
    }
}

/// A rejected chaos-equivalent amount.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InvalidChaosAmount {
    amount_chaos: f64,
}

impl InvalidChaosAmount {
    /// Return the rejected amount.
    pub fn amount_chaos(self) -> f64 {
        self.amount_chaos
    }
}

impl fmt::Display for InvalidChaosAmount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "chaos-equivalent amount must be a non-negative finite number, got {}",
            self.amount_chaos
        )
    }
}

impl std::error::Error for InvalidChaosAmount {}

/// A rejected retry success probability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InvalidProbability {
    probability: f64,
}

impl InvalidProbability {
    /// Return the rejected probability.
    pub fn probability(self) -> f64 {
        self.probability
    }
}

impl fmt::Display for InvalidProbability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "success probability must be finite and between 0 and 1, got {}",
            self.probability
        )
    }
}

impl std::error::Error for InvalidProbability {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn finite(amount_chaos: f64) -> CostValue {
        CostValue::finite(amount_chaos).expect("test cost should be valid")
    }

    #[test]
    fn finite_construction_validates_and_normalizes_amounts() {
        assert_eq!(finite(0.0).amount_chaos(), Some(0.0));
        assert_eq!(finite(-0.0).amount_chaos(), Some(0.0));
        assert!(!finite(-0.0).amount_chaos().unwrap().is_sign_negative());
        assert_eq!(finite(f64::MAX).amount_chaos(), Some(f64::MAX));

        for invalid in [-1.0, f64::NEG_INFINITY, f64::INFINITY] {
            let error = CostValue::finite(invalid).expect_err("amount must be rejected");
            assert_eq!(error.amount_chaos(), invalid);
        }

        let error = CostValue::finite(f64::NAN).expect_err("NaN must be rejected");
        assert!(error.amount_chaos().is_nan());

        assert_eq!(
            CostValue::from_computed(f64::INFINITY).unwrap(),
            CostValue::Unbounded
        );
        assert!(CostValue::from_computed(f64::NEG_INFINITY).is_err());
        assert!(CostValue::from_computed(f64::NAN).is_err());
    }

    #[test]
    fn budget_policy_validates_cap_and_defaults_to_restart_adjusted() {
        let default = BudgetPolicy::default();
        assert_eq!(default.hard_cap_chaos(), None);
        assert_eq!(default.metric(), BudgetMetric::RestartAdjustedExpected);

        let capped = BudgetPolicy::hard_cap(25.0, BudgetMetric::RetryExpected).unwrap();
        assert_eq!(capped.hard_cap_chaos(), Some(25.0));
        assert_eq!(capped.metric(), BudgetMetric::RetryExpected);
        assert_eq!(BudgetMetric::FirstTry.as_str(), "first_try");
        assert_eq!(BudgetMetric::RetryExpected.as_str(), "retry_expected");
        assert_eq!(
            BudgetMetric::RestartAdjustedExpected.as_str(),
            "restart_adjusted_expected"
        );

        for invalid in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(BudgetPolicy::hard_cap(invalid, BudgetMetric::FirstTry).is_err());
        }
    }

    #[test]
    fn state_queries_distinguish_all_three_cost_kinds() {
        let finite = finite(2.5);
        assert!(finite.is_finite());
        assert!(!finite.is_unbounded());
        assert!(!finite.is_unavailable());

        assert!(CostValue::Unbounded.is_unbounded());
        assert_eq!(CostValue::Unbounded.amount_chaos(), None);
        assert!(CostValue::Unavailable.is_unavailable());
        assert_eq!(CostValue::Unavailable.amount_chaos(), None);
    }

    #[test]
    fn serde_uses_the_explicit_tagged_union_and_round_trips() {
        let cases = [
            (
                finite(12.5),
                json!({"kind": "finite", "amount_chaos": 12.5}),
            ),
            (CostValue::Unbounded, json!({"kind": "unbounded"})),
            (CostValue::Unavailable, json!({"kind": "unavailable"})),
        ];

        for (value, expected_json) in cases {
            let serialized =
                serde_json::to_value(value).expect("CostValue should always serialize");
            assert_eq!(serialized, expected_json);
            let round_trip: CostValue =
                serde_json::from_value(serialized).expect("valid JSON should deserialize");
            assert_eq!(round_trip, value);
        }
    }

    #[test]
    fn serde_rejects_invalid_or_ambiguous_wire_values() {
        for invalid in [
            json!({"kind": "finite", "amount_chaos": -1.0}),
            json!({"kind": "finite", "amount_chaos": null}),
            json!({"kind": "finite"}),
            json!({"kind": "finite", "amount_chaos": 1.0, "extra": true}),
            json!({"kind": "unbounded", "amount_chaos": 1.0}),
            json!({"kind": "unbounded", "amount_chaos": null}),
            json!({"kind": "unavailable", "extra": true}),
            json!({"kind": "unknown"}),
            json!({}),
            json!(null),
        ] {
            assert!(
                serde_json::from_value::<CostValue>(invalid.clone()).is_err(),
                "wire value should be rejected: {invalid}"
            );
        }

        for invalid_text in [
            r#"{"kind":"finite","amount_chaos":NaN}"#,
            r#"{"kind":"finite","amount_chaos":Infinity}"#,
            r#"{"kind":"finite","amount_chaos":1e400}"#,
        ] {
            assert!(
                serde_json::from_str::<CostValue>(invalid_text).is_err(),
                "non-finite JSON must be rejected: {invalid_text}"
            );
        }
    }

    #[test]
    fn arithmetic_overflow_becomes_serializable_unbounded_cost() {
        let overflow = finite(f64::MAX) + finite(f64::MAX);
        assert_eq!(overflow, CostValue::Unbounded);
        assert_eq!(
            serde_json::to_value(overflow).unwrap(),
            json!({"kind": "unbounded"})
        );
    }

    #[test]
    fn addition_and_sum_propagate_cost_states_deliberately() {
        assert_eq!(finite(2.0) + finite(3.5), finite(5.5));
        assert_eq!(finite(2.0) + CostValue::Unavailable, CostValue::Unavailable);
        assert_eq!(
            CostValue::Unavailable + CostValue::Unbounded,
            CostValue::Unbounded
        );
        assert_eq!(
            [finite(1.0), finite(2.0), finite(3.0)]
                .into_iter()
                .sum::<CostValue>(),
            finite(6.0)
        );
        assert_eq!(
            std::iter::empty::<CostValue>().sum::<CostValue>(),
            CostValue::zero()
        );
    }

    #[test]
    fn scaling_validates_the_factor_and_handles_extended_values() {
        assert_eq!(finite(4.0).scaled_by(2.5).unwrap(), finite(10.0));
        assert_eq!(
            finite(f64::MAX).scaled_by(2.0).unwrap(),
            CostValue::Unbounded
        );
        assert_eq!(
            CostValue::Unbounded.scaled_by(2.0).unwrap(),
            CostValue::Unbounded
        );
        assert_eq!(
            CostValue::Unbounded.scaled_by(0.0).unwrap(),
            CostValue::Unavailable
        );
        assert_eq!(
            CostValue::Unavailable.scaled_by(2.0).unwrap(),
            CostValue::Unavailable
        );

        for invalid in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(finite(1.0).scaled_by(invalid).is_err());
        }
    }

    #[test]
    fn retry_division_maps_zero_success_to_unbounded() {
        assert_eq!(
            finite(5.0).divided_by_probability(0.5).unwrap(),
            finite(10.0)
        );
        assert_eq!(
            finite(5.0).divided_by_probability(0.0).unwrap(),
            CostValue::Unbounded
        );
        assert_eq!(
            CostValue::Unavailable.divided_by_probability(0.0).unwrap(),
            CostValue::Unbounded
        );
        assert_eq!(
            CostValue::Unavailable.divided_by_probability(0.5).unwrap(),
            CostValue::Unavailable
        );
        assert_eq!(
            finite(f64::MAX).divided_by_probability(0.5).unwrap(),
            CostValue::Unbounded
        );

        for invalid in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
            let error = finite(1.0)
                .divided_by_probability(invalid)
                .expect_err("probability must be rejected");
            if invalid.is_nan() {
                assert!(error.probability().is_nan());
            } else {
                assert_eq!(error.probability(), invalid);
            }
        }
    }

    #[test]
    fn budget_comparison_handles_caps_and_non_finite_states() {
        assert_eq!(
            finite(5.0).compare_to_budget(None).unwrap(),
            BudgetComparison::NotComparable
        );
        assert_eq!(
            CostValue::Unbounded.compare_to_budget(None).unwrap(),
            BudgetComparison::NotComparable
        );
        assert_eq!(
            finite(5.0).compare_to_budget(Some(5.0)).unwrap(),
            BudgetComparison::Under
        );
        assert_eq!(
            finite(4.0).compare_to_budget(Some(5.0)).unwrap(),
            BudgetComparison::Under
        );
        assert_eq!(
            finite(5.01).compare_to_budget(Some(5.0)).unwrap(),
            BudgetComparison::Over
        );
        assert_eq!(
            CostValue::Unbounded.compare_to_budget(Some(5.0)).unwrap(),
            BudgetComparison::Over
        );
        assert_eq!(
            CostValue::Unavailable.compare_to_budget(Some(5.0)).unwrap(),
            BudgetComparison::NotComparable
        );
    }

    #[test]
    fn budget_comparison_rejects_invalid_active_caps() {
        for invalid in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(finite(1.0).compare_to_budget(Some(invalid)).is_err());
            assert!(CostValue::Unavailable
                .compare_to_budget(Some(invalid))
                .is_err());
        }
    }

    #[test]
    fn budget_excess_reports_finite_and_unbounded_overage() {
        assert_eq!(finite(4.0).budget_excess(Some(5.0)).unwrap(), None);
        assert_eq!(finite(5.0).budget_excess(Some(5.0)).unwrap(), None);
        assert_eq!(
            finite(7.5).budget_excess(Some(5.0)).unwrap(),
            Some(finite(2.5))
        );
        assert_eq!(
            CostValue::Unbounded.budget_excess(Some(5.0)).unwrap(),
            Some(CostValue::Unbounded)
        );
        assert_eq!(
            CostValue::Unavailable.budget_excess(Some(5.0)).unwrap(),
            None
        );
        assert_eq!(finite(7.5).budget_excess(None).unwrap(), None);
    }
}
