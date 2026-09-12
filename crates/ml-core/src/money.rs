//! Exact decimal money with a currency tag.
//!
//! Amounts are [`rust_decimal::Decimal`] — never floats. Arithmetic and
//! comparison across different currencies is a hard error, not a coercion.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

/// ISO-4217 code or asset symbol (`INR`, `USD`, `USDC`).
/// Upper-case ASCII letters/digits, 2–10 characters.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Currency(String);

impl Currency {
    /// Validate and construct a currency code.
    pub fn new(code: impl AsRef<str>) -> Result<Self, MoneyError> {
        let c = code.as_ref();
        let valid = (2..=10).contains(&c.len())
            && c.chars()
                .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit());
        if valid {
            Ok(Self(c.to_owned()))
        } else {
            Err(MoneyError::InvalidCurrency(c.to_owned()))
        }
    }

    /// The code as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Currency {
    type Error = MoneyError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl From<Currency> for String {
    fn from(c: Currency) -> Self {
        c.0
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An exact amount in a single currency.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    amount: Decimal,
    currency: Currency,
}

impl Money {
    /// Construct from a decimal and currency.
    #[must_use]
    pub const fn new(amount: Decimal, currency: Currency) -> Self {
        Self { amount, currency }
    }

    /// Parse from strings, e.g. `Money::parse("128.00", "INR")`.
    pub fn parse(amount: &str, currency: &str) -> Result<Self, MoneyError> {
        let amount = Decimal::from_str_exact(amount)
            .map_err(|_| MoneyError::InvalidAmount(amount.to_owned()))?;
        Ok(Self::new(amount, Currency::new(currency)?))
    }

    /// Zero in the given currency.
    #[must_use]
    pub const fn zero(currency: Currency) -> Self {
        Self::new(Decimal::ZERO, currency)
    }

    /// The numeric amount.
    #[must_use]
    pub const fn amount(&self) -> Decimal {
        self.amount
    }

    /// The currency.
    #[must_use]
    pub const fn currency(&self) -> &Currency {
        &self.currency
    }

    /// `true` if the amount is strictly negative.
    #[must_use]
    pub fn is_negative(&self) -> bool {
        self.amount.is_sign_negative() && !self.amount.is_zero()
    }

    /// `true` if the amount is zero.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.amount.is_zero()
    }

    /// `self + other`, failing on currency mismatch or overflow.
    pub fn checked_add(&self, other: &Money) -> Result<Money, MoneyError> {
        self.ensure_same_currency(other)?;
        let amount = self
            .amount
            .checked_add(other.amount)
            .ok_or(MoneyError::Overflow)?;
        Ok(Money::new(amount, self.currency.clone()))
    }

    /// `self - other`, failing on currency mismatch or overflow.
    pub fn checked_sub(&self, other: &Money) -> Result<Money, MoneyError> {
        self.ensure_same_currency(other)?;
        let amount = self
            .amount
            .checked_sub(other.amount)
            .ok_or(MoneyError::Overflow)?;
        Ok(Money::new(amount, self.currency.clone()))
    }

    /// Compare two amounts of the same currency.
    pub fn cmp_same_currency(&self, other: &Money) -> Result<Ordering, MoneyError> {
        self.ensure_same_currency(other)?;
        Ok(self.amount.cmp(&other.amount))
    }

    fn ensure_same_currency(&self, other: &Money) -> Result<(), MoneyError> {
        if self.currency == other.currency {
            Ok(())
        } else {
            Err(MoneyError::CurrencyMismatch(
                self.currency.clone(),
                other.currency.clone(),
            ))
        }
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.amount, self.currency)
    }
}

impl fmt::Debug for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Errors from money construction and arithmetic.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    /// Currency code failed validation.
    #[error("invalid currency `{0}`")]
    InvalidCurrency(String),
    /// Amount string is not an exact decimal.
    #[error("invalid amount `{0}`")]
    InvalidAmount(String),
    /// Operation mixed two currencies.
    #[error("currency mismatch: {0} vs {1}")]
    CurrencyMismatch(Currency, Currency),
    /// Decimal arithmetic overflowed.
    #[error("arithmetic overflow")]
    Overflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_compare() {
        let a = Money::parse("128.00", "INR").unwrap();
        let b = Money::parse("128", "INR").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.cmp_same_currency(&b).unwrap(), Ordering::Equal);
    }

    #[test]
    fn cross_currency_is_error() {
        let a = Money::parse("1", "INR").unwrap();
        let b = Money::parse("1", "USD").unwrap();
        assert!(matches!(
            a.checked_add(&b),
            Err(MoneyError::CurrencyMismatch(_, _))
        ));
    }

    #[test]
    fn rejects_bad_currency() {
        assert!(Currency::new("inr").is_err());
        assert!(Currency::new("I").is_err());
        assert!(Currency::new("USDC").is_ok());
    }

    #[test]
    fn serializes_amount_as_string() {
        let m = Money::parse("0.10", "USDC").unwrap();
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json, r#"{"amount":"0.10","currency":"USDC"}"#);
    }
}
