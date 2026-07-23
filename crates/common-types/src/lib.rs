#![forbid(unsafe_code)]

//! Validated fixed-point primitives for financial calculations.
//!
//! One whole dollar, token, or probability unit is represented by one million
//! micros. Required collateral rounds upward; conservative proceeds round down.

use std::error::Error;
use std::fmt::{Display, Formatter};

/// Number of micros in one whole unit.
pub const MICROS_PER_UNIT: i128 = 1_000_000;

/// Errors returned instead of wrapping, clamping, or silently rounding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinancialError {
    NegativeQuantity(i64),
    NegativeQuotePrice(i64),
    PriceOutOfRange(i64),
    ArithmeticOverflow,
}

/// Errors from strict external decimal conversion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecimalError {
    InvalidSyntax,
    PrecisionLoss,
    ArithmeticOverflow,
    Financial(FinancialError),
}

impl Display for DecimalError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSyntax => formatter.write_str("invalid unsigned decimal syntax"),
            Self::PrecisionLoss => {
                formatter.write_str("decimal exceeds the supported fixed-point precision")
            }
            Self::ArithmeticOverflow => formatter.write_str("decimal conversion overflow"),
            Self::Financial(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for DecimalError {}

impl Display for FinancialError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NegativeQuantity(value) => {
                write!(formatter, "quantity cannot be negative: {value}")
            }
            Self::NegativeQuotePrice(value) => {
                write!(formatter, "quote price cannot be negative: {value}")
            }
            Self::PriceOutOfRange(value) => {
                write!(formatter, "price micros must be in 0..=1_000_000: {value}")
            }
            Self::ArithmeticOverflow => formatter.write_str("fixed-point arithmetic overflow"),
        }
    }
}

/// Underlying-asset quote price in millionths of one quote-currency unit.
///
/// Unlike [`PriceMicros`], this is not a binary probability and is therefore
/// not capped at one unit. The explicit type prevents BTC/USDT or ETH/USDT
/// prices from being confused with prediction-market token prices.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct QuotePriceMicros(i64);

impl QuotePriceMicros {
    pub const ZERO: Self = Self(0);

    /// Rejects negative quote prices at the external boundary.
    ///
    /// # Errors
    ///
    /// Returns [`FinancialError::NegativeQuotePrice`] for a negative value.
    pub fn new(value: i64) -> Result<Self, FinancialError> {
        if value >= 0 {
            Ok(Self(value))
        } else {
            Err(FinancialError::NegativeQuotePrice(value))
        }
    }

    #[must_use]
    pub const fn as_micros(self) -> i64 {
        self.0
    }
}

impl Error for FinancialError {}

/// Outcome-token price in millionths of one collateral dollar.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PriceMicros(i64);

impl PriceMicros {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(1_000_000);

    /// Validates that a binary-outcome price is between zero and one dollar.
    ///
    /// # Errors
    ///
    /// Returns [`FinancialError::PriceOutOfRange`] outside the valid interval.
    pub fn new(value: i64) -> Result<Self, FinancialError> {
        if (0..=1_000_000).contains(&value) {
            Ok(Self(value))
        } else {
            Err(FinancialError::PriceOutOfRange(value))
        }
    }

    #[must_use]
    pub const fn as_micros(self) -> i64 {
        self.0
    }
}

/// Token quantity in millionths of one token.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct QuantityMicros(i64);

impl QuantityMicros {
    pub const ZERO: Self = Self(0);

    /// Rejects negative quantities at the boundary.
    ///
    /// # Errors
    ///
    /// Returns [`FinancialError::NegativeQuantity`] for a negative value.
    pub fn new(value: i64) -> Result<Self, FinancialError> {
        if value >= 0 {
            Ok(Self(value))
        } else {
            Err(FinancialError::NegativeQuantity(value))
        }
    }

    #[must_use]
    pub const fn as_micros(self) -> i64 {
        self.0
    }
}

/// External reference-feed quantity in hundred-millionths of one unit.
///
/// Binance Spot publishes base and quote quantities with up to eight decimal
/// places. This distinct type prevents those observations from being rounded
/// into the six-decimal trading/accounting quantity domain.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ReferenceQuantityE8(i64);

impl ReferenceQuantityE8 {
    pub const ZERO: Self = Self(0);

    /// Rejects negative reference quantities at the external boundary.
    ///
    /// # Errors
    ///
    /// Returns [`FinancialError::NegativeQuantity`] for a negative value.
    pub fn new(value: i64) -> Result<Self, FinancialError> {
        if value >= 0 {
            Ok(Self(value))
        } else {
            Err(FinancialError::NegativeQuantity(value))
        }
    }

    #[must_use]
    pub const fn as_e8(self) -> i64 {
        self.0
    }
}

/// Parses an unsigned base-ten price without binary floating point.
///
/// More than six decimal places are accepted only when the excess digits are
/// zero, so conversion never rounds.
///
/// # Errors
///
/// Returns [`DecimalError`] for invalid syntax, precision loss, overflow, or a
/// value outside the binary-outcome price range.
pub fn parse_price_micros(value: &str) -> Result<PriceMicros, DecimalError> {
    let micros = parse_unsigned_micros(value)?;
    PriceMicros::new(micros).map_err(DecimalError::Financial)
}

/// Parses an unsigned base-ten token quantity without binary floating point.
///
/// More than six decimal places are accepted only when the excess digits are
/// zero, so conversion never rounds.
///
/// # Errors
///
/// Returns [`DecimalError`] for invalid syntax, precision loss, or overflow.
pub fn parse_quantity_micros(value: &str) -> Result<QuantityMicros, DecimalError> {
    QuantityMicros::new(parse_unsigned_micros(value)?).map_err(DecimalError::Financial)
}

/// Parses an unsigned reference-feed quantity at exactly 1e-8 scale.
///
/// More than eight decimal places are accepted only when all excess digits are
/// zero. Conversion never rounds.
///
/// # Errors
///
/// Returns [`DecimalError`] for invalid syntax, precision loss, or overflow.
pub fn parse_reference_quantity_e8(value: &str) -> Result<ReferenceQuantityE8, DecimalError> {
    ReferenceQuantityE8::new(parse_unsigned_fixed(value, 8)?).map_err(DecimalError::Financial)
}

/// Parses an unsigned underlying quote price without binary floating point.
///
/// # Errors
///
/// Returns [`DecimalError`] for invalid syntax, precision loss, or overflow.
pub fn parse_quote_price_micros(value: &str) -> Result<QuotePriceMicros, DecimalError> {
    QuotePriceMicros::new(parse_unsigned_micros(value)?).map_err(DecimalError::Financial)
}

fn parse_unsigned_micros(value: &str) -> Result<i64, DecimalError> {
    parse_unsigned_fixed(value, 6)
}

fn parse_unsigned_fixed(value: &str, fractional_digits: usize) -> Result<i64, DecimalError> {
    if value.is_empty() || value.starts_with(['+', '-']) {
        return Err(DecimalError::InvalidSyntax);
    }
    let mut parts = value.split('.');
    let whole = parts.next().ok_or(DecimalError::InvalidSyntax)?;
    let fraction = parts.next();
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(DecimalError::InvalidSyntax);
    }
    let fraction = fraction.unwrap_or("");
    if value.contains('.') && fraction.is_empty() {
        return Err(DecimalError::InvalidSyntax);
    }
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DecimalError::InvalidSyntax);
    }
    if fraction.len() > fractional_digits
        && fraction.as_bytes()[fractional_digits..]
            .iter()
            .any(|digit| *digit != b'0')
    {
        return Err(DecimalError::PrecisionLoss);
    }

    let whole = parse_digits(whole)?;
    let kept_fraction = &fraction[..fraction.len().min(fractional_digits)];
    let mut fraction_micros = parse_digits(kept_fraction)?;
    for _ in kept_fraction.len()..fractional_digits {
        fraction_micros = fraction_micros
            .checked_mul(10)
            .ok_or(DecimalError::ArithmeticOverflow)?;
    }
    let scale = 10_i64
        .checked_pow(
            u32::try_from(fractional_digits).map_err(|_| DecimalError::ArithmeticOverflow)?,
        )
        .ok_or(DecimalError::ArithmeticOverflow)?;
    whole
        .checked_mul(scale)
        .and_then(|scaled| scaled.checked_add(fraction_micros))
        .ok_or(DecimalError::ArithmeticOverflow)
}

fn parse_digits(value: &str) -> Result<i64, DecimalError> {
    let mut result = 0_i64;
    for digit in value.bytes() {
        result = result
            .checked_mul(10)
            .and_then(|current| current.checked_add(i64::from(digit - b'0')))
            .ok_or(DecimalError::ArithmeticOverflow)?;
    }
    Ok(result)
}

/// Collateral money in millionths of one dollar.
///
/// The signed representation supports P&L and ledger deltas. Callers that need
/// non-negative balances must enforce that invariant in their domain type.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MoneyMicros(i128);

impl MoneyMicros {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn new(value: i128) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_micros(self) -> i128 {
        self.0
    }

    /// Adds two values without wrapping.
    ///
    /// # Errors
    ///
    /// Returns [`FinancialError::ArithmeticOverflow`] when the sum exceeds
    /// `i128`.
    pub fn checked_add(self, other: Self) -> Result<Self, FinancialError> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .ok_or(FinancialError::ArithmeticOverflow)
    }

    /// Subtracts two values without wrapping.
    ///
    /// # Errors
    ///
    /// Returns [`FinancialError::ArithmeticOverflow`] when the difference
    /// exceeds `i128`.
    pub fn checked_sub(self, other: Self) -> Result<Self, FinancialError> {
        self.0
            .checked_sub(other.0)
            .map(Self)
            .ok_or(FinancialError::ArithmeticOverflow)
    }
}

/// Calculates required buy collateral, rounding any fractional micro upward.
///
/// # Errors
///
/// Returns [`FinancialError::ArithmeticOverflow`] if an intermediate result
/// cannot be represented.
pub fn required_collateral(
    price: PriceMicros,
    quantity: QuantityMicros,
) -> Result<MoneyMicros, FinancialError> {
    let product = i128::from(price.0)
        .checked_mul(i128::from(quantity.0))
        .ok_or(FinancialError::ArithmeticOverflow)?;

    let rounded = product
        .checked_add(MICROS_PER_UNIT - 1)
        .ok_or(FinancialError::ArithmeticOverflow)?
        / MICROS_PER_UNIT;
    Ok(MoneyMicros(rounded))
}

/// Calculates conservative sale proceeds, discarding any fractional micro.
///
/// # Errors
///
/// Returns [`FinancialError::ArithmeticOverflow`] if an intermediate result
/// cannot be represented.
pub fn conservative_proceeds(
    price: PriceMicros,
    quantity: QuantityMicros,
) -> Result<MoneyMicros, FinancialError> {
    let product = i128::from(price.0)
        .checked_mul(i128::from(quantity.0))
        .ok_or(FinancialError::ArithmeticOverflow)?;
    Ok(MoneyMicros(product / MICROS_PER_UNIT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn validates_financial_boundaries() {
        assert_eq!(PriceMicros::new(0), Ok(PriceMicros::ZERO));
        assert_eq!(PriceMicros::new(1_000_000), Ok(PriceMicros::ONE));
        assert!(PriceMicros::new(-1).is_err());
        assert!(PriceMicros::new(1_000_001).is_err());
        assert!(QuantityMicros::new(-1).is_err());
        assert!(QuotePriceMicros::new(-1).is_err());
        assert_eq!(
            parse_quote_price_micros("123456.123456"),
            Ok(QuotePriceMicros::new(123_456_123_456).expect("quote price"))
        );
    }

    #[test]
    fn collateral_rounds_against_the_account() {
        let price = PriceMicros::new(333_333).expect("valid price");
        let quantity = QuantityMicros::new(1).expect("valid quantity");

        assert_eq!(
            required_collateral(price, quantity).expect("cost"),
            MoneyMicros::new(1)
        );
        assert_eq!(
            conservative_proceeds(price, quantity).expect("proceeds"),
            MoneyMicros::ZERO
        );
    }

    #[test]
    fn parses_external_decimals_exactly() {
        assert_eq!(
            parse_price_micros("0.5"),
            Ok(PriceMicros::new(500_000).expect("price"))
        );
        assert_eq!(parse_price_micros("1.0000000"), Ok(PriceMicros::ONE));
        assert_eq!(
            parse_quantity_micros("123.000001"),
            Ok(QuantityMicros::new(123_000_001).expect("quantity"))
        );
        assert_eq!(
            parse_reference_quantity_e8("0.12345678"),
            Ok(ReferenceQuantityE8::new(12_345_678).expect("reference quantity"))
        );
        assert_eq!(
            parse_reference_quantity_e8("0.123456789"),
            Err(DecimalError::PrecisionLoss)
        );
        assert_eq!(
            parse_price_micros("0.0000001"),
            Err(DecimalError::PrecisionLoss)
        );
        for invalid in ["", ".5", "1.", "-1", "+1", "1e-3", "1.2.3"] {
            assert_eq!(
                parse_quantity_micros(invalid),
                Err(DecimalError::InvalidSyntax)
            );
        }
        assert!(matches!(
            parse_quantity_micros("999999999999999999999999"),
            Err(DecimalError::ArithmeticOverflow)
        ));
    }

    proptest! {
        #[test]
        fn decimal_price_round_trips_every_generated_micro(price in 0_i64..=1_000_000) {
            let encoded = format!("{}.{:06}", price / 1_000_000, price % 1_000_000);
            prop_assert_eq!(
                parse_price_micros(&encoded),
                Ok(PriceMicros::new(price).expect("generated valid price"))
            );
        }

        #[test]
        fn required_collateral_never_understates_exact_cost(
            price in 0_i64..=1_000_000,
            quantity in 0_i64..=i64::MAX,
        ) {
            let price = PriceMicros::new(price).expect("generated valid price");
            let quantity = QuantityMicros::new(quantity).expect("generated valid quantity");
            let product = i128::from(price.as_micros()) * i128::from(quantity.as_micros());
            let reserved = required_collateral(price, quantity).expect("i64 product fits i128");

            prop_assert!(reserved.as_micros() * MICROS_PER_UNIT >= product);
            if product > 0 {
                prop_assert!((reserved.as_micros() - 1) * MICROS_PER_UNIT < product);
            }
        }

        #[test]
        fn proceeds_never_overstates_exact_value(
            price in 0_i64..=1_000_000,
            quantity in 0_i64..=i64::MAX,
        ) {
            let price = PriceMicros::new(price).expect("generated valid price");
            let quantity = QuantityMicros::new(quantity).expect("generated valid quantity");
            let product = i128::from(price.as_micros()) * i128::from(quantity.as_micros());
            let proceeds = conservative_proceeds(price, quantity).expect("i64 product fits i128");

            prop_assert!(proceeds.as_micros() * MICROS_PER_UNIT <= product);
            prop_assert!((proceeds.as_micros() + 1) * MICROS_PER_UNIT > product);
        }
    }
}
