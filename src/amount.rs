//! Money types.
//!
//! [`Amount`] is the validated amount of a single transaction.
//! [`Balance`] is a signed account balance.
//!
//! Both are exact decimals with at most four decimal places. Floats are never
//! used. That includes parsing.

use std::fmt;
use std::str::FromStr;

use rust_decimal::Decimal;
use thiserror::Error;

/// Decimal places that amounts and balances may carry.
const DECIMAL_PLACES: u32 = 4;

/// The largest magnitude an amount or a balance may have.
///
/// `Decimal` has a 96 bit mantissa. Its largest mantissa is 2^96 - 1. At four
/// decimal places that is 7922816251426433759354395.0335. Above this value
/// the crate drops decimal places to make a result fit. It still reports
/// success. Every value at or below it is exact.
const CAP: Decimal = Decimal::from_parts(u32::MAX, u32::MAX, u32::MAX, false, DECIMAL_PLACES);

/// Why a piece of text is not a valid [`Amount`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AmountError {
    #[error("amount is not a decimal number")]
    Malformed,
    #[error("amount has more than four decimal places")]
    TooPrecise,
    #[error("amount must be greater than zero")]
    NotPositive,
    #[error("amount exceeds the largest supported value")]
    TooLarge,
}

/// The amount of a deposit or a withdrawal.
///
/// An `Amount` is always greater than zero, has at most four decimal places
/// and does not exceed the cap. Parsing text is the only way to create one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Amount(Decimal);

impl FromStr for Amount {
    type Err = AmountError;

    /// Parses an amount without ever changing its value.
    ///
    /// Trailing zeros past the fourth decimal place are accepted. Any other
    /// digit there is an error. The text must not have surrounding whitespace.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let value = parse_exact(text).ok_or(AmountError::Malformed)?.normalize();

        if value.scale() > DECIMAL_PLACES {
            return Err(AmountError::TooPrecise);
        }
        if value <= Decimal::ZERO {
            return Err(AmountError::NotPositive);
        }
        if value > CAP {
            return Err(AmountError::TooLarge);
        }
        Ok(Self(value))
    }
}

/// Parses a decimal and refuses any input it would have to round.
fn parse_exact(text: &str) -> Option<Decimal> {
    // `from_str` would round a value that does not fit. The exact parser
    // returns an error instead.
    if let Ok(value) = Decimal::from_str_exact(text) {
        return Some(value);
    }

    // The exact parser also refuses more than 28 decimal places. Trailing
    // zeros in the fraction do not change the value.
    if !text.contains('.') {
        return None;
    }
    Decimal::from_str_exact(text.trim_end_matches('0')).ok()
}

/// A balance operation whose result would fall outside the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("balance would exceed the largest supported value")]
pub struct ArithmeticError;

/// A signed account balance.
///
/// A balance may be negative. A dispute can hold more than the client has
/// available. The magnitude never exceeds the cap. That keeps every balance
/// exact.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Balance(Decimal);

impl Balance {
    pub const ZERO: Self = Self(Decimal::ZERO);

    /// Returns the sum. Fails if it would exceed the cap.
    pub fn checked_add(self, other: Self) -> Result<Self, ArithmeticError> {
        Self::within_cap(self.0.checked_add(other.0))
    }

    /// Returns the difference. Fails if it would exceed the cap.
    pub fn checked_sub(self, other: Self) -> Result<Self, ArithmeticError> {
        Self::within_cap(self.0.checked_sub(other.0))
    }

    // `Decimal` arithmetic can succeed with a rounded result. Such a result
    // always lies outside the cap. This check is what guarantees exactness.
    fn within_cap(result: Option<Decimal>) -> Result<Self, ArithmeticError> {
        match result {
            Some(value) if value.abs() <= CAP => Ok(Self(value)),
            _ => Err(ArithmeticError),
        }
    }
}

impl From<Amount> for Balance {
    fn from(amount: Amount) -> Self {
        Self(amount.0)
    }
}

/// Prints the balance without trailing zeros. Examples are `1.5`, `2` and
/// `-0.25`.
impl fmt::Display for Balance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.normalize().fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{CAP as CAP_TEXT, amount};

    fn balance(text: &str) -> Balance {
        amount(text).into()
    }

    fn error(text: &str) -> AmountError {
        text.parse::<Amount>().unwrap_err()
    }

    #[test]
    fn cap_is_the_largest_value_exact_at_four_decimal_places() {
        assert_eq!(CAP, Decimal::from_str_exact(CAP_TEXT).unwrap());
        assert_eq!(CAP.mantissa(), 2_i128.pow(96) - 1);
        assert_eq!(CAP.scale(), 4);
    }

    #[test]
    fn parses_plain_decimals() {
        for text in ["1", "1.0", "1.5", "0.0001", "12345.6789"] {
            assert!(text.parse::<Amount>().is_ok(), "{text}");
        }
    }

    #[test]
    fn parses_every_form_the_exact_parser_accepts() {
        assert_eq!(amount("+1.5"), amount("1.5"));
        assert_eq!(amount(".5"), amount("0.5"));
        assert_eq!(amount("5."), amount("5"));
        assert_eq!(amount("1_000.5"), amount("1000.5"));
        assert_eq!(amount("00012.34"), amount("12.34"));
    }

    #[test]
    fn keeps_every_digit_of_a_long_value() {
        let text = "1234567890123456.7891";
        assert_eq!(Balance::from(amount(text)).to_string(), text);
    }

    #[test]
    fn accepts_trailing_zeros_past_four_decimal_places() {
        assert_eq!(amount("1.50000"), amount("1.5"));
        assert_eq!(amount("0.000100000000"), amount("0.0001"));
    }

    #[test]
    fn accepts_any_number_of_trailing_zeros() {
        let zeros = "0".repeat(40);

        assert_eq!(amount(&format!("1.{zeros}")), amount("1"));
        assert_eq!(amount(&format!("1.5{zeros}")), amount("1.5"));
        assert_eq!(amount(&format!("{CAP_TEXT}{zeros}")), amount(CAP_TEXT));
        assert_eq!(error(&format!("0.{zeros}")), AmountError::NotPositive);
        assert_eq!(error(&format!("1.00001{zeros}")), AmountError::TooPrecise);
    }

    #[test]
    fn trailing_zeros_of_a_whole_number_are_significant() {
        // No fraction here. Dropping these zeros would change the value.
        let digits = format!("1{}", "0".repeat(40));

        assert_eq!(error(&digits), AmountError::Malformed);
        assert_eq!(amount("100"), amount("100.0"));
    }

    #[test]
    fn rejects_a_nonzero_digit_past_four_decimal_places() {
        assert_eq!(error("1.23456"), AmountError::TooPrecise);
        assert_eq!(error("0.00001"), AmountError::TooPrecise);
        assert_eq!(error("1.00001"), AmountError::TooPrecise);
    }

    #[test]
    fn rejects_input_it_would_have_to_round() {
        // 35 decimal places do not fit. Rounding would turn this into 1.
        assert!(
            "1.00000000000000000000000000000000001"
                .parse::<Amount>()
                .is_err()
        );
        assert!("7922816251426433759354395.0336".parse::<Amount>().is_err());
    }

    #[test]
    fn rejects_zero_and_negative_values() {
        for text in ["0", "0.0", "0.0000", "0.00000", "-0", "-1.5", "-0.0001"] {
            assert_eq!(error(text), AmountError::NotPositive, "{text}");
        }
    }

    #[test]
    fn rejects_text_that_is_not_a_decimal_number() {
        for text in [
            "", " ", "abc", "1,000.5", "1.2.3", "NaN", "inf", "0x10", " 1.5", "1.5 ", "1 000",
        ] {
            assert_eq!(error(text), AmountError::Malformed, "{text:?}");
        }
    }

    #[test]
    fn rejects_scientific_notation() {
        for text in ["1e3", "1E3", "1e+3", "1.5e-3", "0e5", "-1e3"] {
            assert_eq!(error(text), AmountError::Malformed, "{text}");
        }
    }

    #[test]
    fn accepts_the_cap_and_rejects_anything_above_it() {
        assert!(CAP_TEXT.parse::<Amount>().is_ok());
        assert_eq!(error("7922816251426433759354396"), AmountError::TooLarge);
        assert_eq!(
            error("79228162514264337593543950335"),
            AmountError::TooLarge
        );
    }

    #[test]
    fn adds_and_subtracts_exactly() {
        let sum = balance("0.1").checked_add(balance("0.2")).unwrap();
        assert_eq!(sum, balance("0.3"));

        let difference = balance("2.0001").checked_sub(balance("0.0002")).unwrap();
        assert_eq!(difference, balance("1.9999"));
    }

    #[test]
    fn a_balance_can_go_negative() {
        let negative = Balance::ZERO.checked_sub(balance("8")).unwrap();
        assert!(negative < Balance::ZERO);
        assert_eq!(negative.to_string(), "-8");
        assert_eq!(negative.checked_add(balance("8")).unwrap(), Balance::ZERO);
    }

    #[test]
    fn compares_by_value() {
        assert!(balance("2") >= balance("2.0000"));
        assert!(balance("2") > balance("1.9999"));
        assert!(balance("0.0001") > Balance::ZERO);
    }

    #[test]
    fn refuses_a_result_above_the_cap() {
        let cap = balance(CAP_TEXT);
        let step = balance("0.0001");

        assert_eq!(cap.checked_add(step), Err(ArithmeticError));
        assert_eq!(cap.checked_add(cap), Err(ArithmeticError));
    }

    #[test]
    fn refuses_a_result_below_the_negative_cap() {
        let cap = balance(CAP_TEXT);
        let step = balance("0.0001");
        let negative_cap = Balance::ZERO.checked_sub(cap).unwrap();

        assert_eq!(negative_cap.checked_sub(step), Err(ArithmeticError));
        assert_eq!(negative_cap.checked_sub(cap), Err(ArithmeticError));
    }

    #[test]
    fn stays_exact_right_at_the_cap() {
        let cap = balance(CAP_TEXT);
        let step = balance("0.0001");

        let below = cap.checked_sub(step).unwrap();
        assert_eq!(below.to_string(), "7922816251426433759354395.0334");
        assert_eq!(below.checked_add(step).unwrap(), cap);
        assert_eq!(cap.checked_sub(cap).unwrap(), Balance::ZERO);
    }

    #[test]
    fn prints_without_trailing_zeros() {
        assert_eq!(Balance::ZERO.to_string(), "0");
        assert_eq!(balance("2.0000").to_string(), "2");
        assert_eq!(balance("1.5000").to_string(), "1.5");
        assert_eq!(balance("0.1234").to_string(), "0.1234");
        assert_eq!(
            balance("1.5")
                .checked_sub(balance("1.5"))
                .unwrap()
                .to_string(),
            "0"
        );
        assert_eq!(
            Balance::ZERO
                .checked_sub(balance("0.25"))
                .unwrap()
                .to_string(),
            "-0.25"
        );
    }
}
