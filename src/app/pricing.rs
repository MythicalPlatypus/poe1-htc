//! Deterministic, validated pricing keyed by semantic crafting-method identity.

use std::collections::BTreeMap;
use std::fmt;

use crate::currency::MethodId;

/// Chaos-equivalent costs for crafting methods.
///
/// Iteration is ordered by [`MethodId`], independent of insertion order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PriceBook {
    costs: BTreeMap<MethodId, f64>,
}

impl PriceBook {
    /// Create an empty price book.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the price book contains no overrides.
    pub fn is_empty(&self) -> bool {
        self.costs.is_empty()
    }

    /// Number of priced method IDs.
    pub fn len(&self) -> usize {
        self.costs.len()
    }

    /// Return the configured cost for `method_id`, if present.
    pub fn get(&self, method_id: &MethodId) -> Option<f64> {
        self.costs.get(method_id).copied()
    }

    /// Iterate over prices in ascending [`MethodId`] order.
    pub fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = (&MethodId, &f64)> + DoubleEndedIterator + '_ {
        self.costs.iter()
    }

    /// Set a strictly positive, finite cost.
    ///
    /// Returns the prior cost when the method already had an entry. Invalid
    /// replacement values leave the existing entry unchanged.
    pub fn set(&mut self, method_id: MethodId, cost: f64) -> Result<Option<f64>, PriceBookError> {
        if !cost.is_finite() || cost <= 0.0 {
            return Err(PriceBookError {
                method_id,
                invalid_value: cost,
            });
        }

        Ok(self.costs.insert(method_id, cost))
    }

    /// Build a price book from ID-cost pairs.
    ///
    /// Every entry is validated through [`PriceBook::set`]. When an ID occurs
    /// more than once, its last valid value wins.
    pub fn try_from_iter(
        prices: impl IntoIterator<Item = (MethodId, f64)>,
    ) -> Result<Self, PriceBookError> {
        let mut price_book = Self::new();
        for (method_id, cost) in prices {
            price_book.set(method_id, cost)?;
        }
        Ok(price_book)
    }
}

/// A rejected price-book insertion.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceBookError {
    method_id: MethodId,
    invalid_value: f64,
}

impl PriceBookError {
    /// Method whose price was rejected.
    pub fn method_id(&self) -> &MethodId {
        &self.method_id
    }

    /// Rejected numeric value.
    pub fn invalid_value(&self) -> f64 {
        self.invalid_value
    }
}

impl fmt::Display for PriceBookError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid price for method \"{}\": {} is not a strictly positive finite cost",
            self.method_id, self.invalid_value
        )
    }
}

impl std::error::Error for PriceBookError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn method_id(value: &str) -> MethodId {
        MethodId::parse(value).expect("test method ID should be valid")
    }

    #[test]
    fn new_and_default_are_empty() {
        let new = PriceBook::new();
        let default = PriceBook::default();

        assert!(new.is_empty());
        assert_eq!(new.len(), 0);
        assert_eq!(new, default);
        assert_eq!(new.iter().len(), 0);
    }

    #[test]
    fn set_get_and_replace_prices() {
        let chaos = method_id("currency/chaos");
        let mut prices = PriceBook::new();

        assert_eq!(prices.set(chaos.clone(), 2.5), Ok(None));
        assert_eq!(prices.get(&chaos), Some(2.5));
        assert_eq!(prices.set(chaos.clone(), 3.0), Ok(Some(2.5)));
        assert_eq!(prices.get(&chaos), Some(3.0));
        assert_eq!(prices.len(), 1);
    }

    #[test]
    fn iteration_and_equality_are_independent_of_insertion_order() {
        let chaos = method_id("currency/chaos");
        let divine = method_id("currency/divine");
        let exalted = method_id("currency/exalted");

        let mut forward = PriceBook::new();
        forward.set(exalted.clone(), 8.0).unwrap();
        forward.set(chaos.clone(), 1.0).unwrap();
        forward.set(divine.clone(), 12.0).unwrap();

        let reverse =
            PriceBook::try_from_iter([(divine, 12.0), (chaos, 1.0), (exalted, 8.0)]).unwrap();

        assert_eq!(forward, reverse);
        assert_eq!(
            forward
                .iter()
                .map(|(method_id, cost)| (method_id.as_str(), *cost))
                .collect::<Vec<_>>(),
            vec![
                ("currency/chaos", 1.0),
                ("currency/divine", 12.0),
                ("currency/exalted", 8.0),
            ]
        );
    }

    #[test]
    fn set_rejects_every_nonpositive_or_nonfinite_value_without_mutating() {
        let chaos = method_id("currency/chaos");
        let invalid_values = [0.0, -0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY];

        for invalid_value in invalid_values {
            let mut prices =
                PriceBook::try_from_iter([(chaos.clone(), 4.0)]).expect("valid initial price");
            let error = prices
                .set(chaos.clone(), invalid_value)
                .expect_err("invalid replacement must be rejected");

            assert_eq!(error.method_id(), &chaos);
            if invalid_value.is_nan() {
                assert!(error.invalid_value().is_nan());
            } else {
                assert_eq!(error.invalid_value(), invalid_value);
            }
            assert!(
                error.to_string().contains(chaos.as_str()),
                "display must mention method ID: {error}"
            );
            assert!(
                error.to_string().contains(&invalid_value.to_string()),
                "display must mention invalid value: {error}"
            );
            assert_eq!(prices.get(&chaos), Some(4.0));
            assert_eq!(prices.len(), 1);
        }
    }

    #[test]
    fn positive_finite_boundaries_are_accepted() {
        let minimum = method_id("test/minimum");
        let maximum = method_id("test/maximum");
        let prices = PriceBook::try_from_iter([
            (minimum.clone(), f64::MIN_POSITIVE),
            (maximum.clone(), f64::MAX),
        ])
        .unwrap();

        assert_eq!(prices.get(&minimum), Some(f64::MIN_POSITIVE));
        assert_eq!(prices.get(&maximum), Some(f64::MAX));
    }

    #[test]
    fn try_from_iter_validates_entries_and_uses_the_last_duplicate() {
        let chaos = method_id("currency/chaos");
        let divine = method_id("currency/divine");
        let prices =
            PriceBook::try_from_iter([(chaos.clone(), 1.0), (divine, 12.0), (chaos.clone(), 2.0)])
                .unwrap();

        assert_eq!(prices.get(&chaos), Some(2.0));
        assert_eq!(prices.len(), 2);

        let error = PriceBook::try_from_iter([(chaos.clone(), f64::INFINITY)])
            .expect_err("try_from_iter must apply insertion validation");
        assert_eq!(error.method_id(), &chaos);
        assert_eq!(
            error.to_string(),
            "invalid price for method \"currency/chaos\": inf is not a strictly positive finite cost"
        );
    }
}
