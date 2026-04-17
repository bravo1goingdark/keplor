//! [`Cost`] — an int64 count of nanodollars.
//!
//! 1 USD = 1 000 000 000 nanodollars; 1 US-cent = 10 000 000 nanodollars.
//! `i64` gives ±9.2 × 10¹⁸ ≈ ±$9.2 billion of range, which covers every
//! realistic bill plus negative adjustments (refunds, credits).
//!
//! Arithmetic is saturating — we never silently overflow a cost field.

use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// 1 USD expressed in nanodollars.
pub const NANOS_PER_DOLLAR: i64 = 1_000_000_000;

/// Monetary amount in nanodollars.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Cost(pub i64);

impl Cost {
    /// The zero cost (for `None`-ish initialisation without `Option`).
    pub const ZERO: Self = Self(0);

    /// Build a `Cost` from a raw nanodollar count.
    #[must_use]
    pub const fn from_nanodollars(n: i64) -> Self {
        Self(n)
    }

    /// Build a `Cost` from whole dollars (saturating on overflow).
    #[must_use]
    pub const fn from_dollars(dollars: i64) -> Self {
        match dollars.checked_mul(NANOS_PER_DOLLAR) {
            Some(n) => Self(n),
            None if dollars < 0 => Self(i64::MIN),
            None => Self(i64::MAX),
        }
    }

    /// Raw nanodollar count.
    #[must_use]
    pub const fn nanodollars(self) -> i64 {
        self.0
    }

    /// Convert to a floating-point dollar amount.  Display / UI use only;
    /// never round-trip through this for accounting — round-trip through
    /// [`Cost::nanodollars`] instead.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn to_dollars_f64(self) -> f64 {
        self.0 as f64 / NANOS_PER_DOLLAR as f64
    }
}

impl fmt::Display for Cost {
    /// Formats with 8 fractional digits: `"$0.00000000"`.
    ///
    /// 8 digits represents 10⁻⁸ USD — one order of magnitude coarser than
    /// the stored nanodollar precision — matching the spec-requested
    /// `$0.00000000` shape.  The last (ninth) nanodollar digit is
    /// truncated toward zero.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        // unsigned_abs avoids the i64::MIN edge case where `-i64::MIN`
        // would overflow.
        let abs = self.0.unsigned_abs();
        let dollars = abs / NANOS_PER_DOLLAR as u64;
        // Truncate the lowest decimal digit to end up with 8 fractional
        // digits instead of 9.
        #[allow(clippy::cast_sign_loss)]
        let hundred_millionths = (abs % NANOS_PER_DOLLAR as u64) / 10;
        write!(f, "{sign}${dollars}.{hundred_millionths:08}")
    }
}

impl Add for Cost {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl Sub for Cost {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl AddAssign for Cost {
    fn add_assign(&mut self, rhs: Self) {
        self.0 = self.0.saturating_add(rhs.0);
    }
}

impl SubAssign for Cost {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 = self.0.saturating_sub(rhs.0);
    }
}

impl Neg for Cost {
    type Output = Self;
    fn neg(self) -> Self {
        Self(self.0.saturating_neg())
    }
}

impl std::iter::Sum for Cost {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, |acc, c| acc + c)
    }
}

impl<'a> std::iter::Sum<&'a Cost> for Cost {
    fn sum<I: Iterator<Item = &'a Cost>>(iter: I) -> Self {
        iter.copied().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_zero() {
        assert_eq!(Cost::ZERO.to_string(), "$0.00000000");
    }

    #[test]
    fn display_one_dollar() {
        assert_eq!(Cost::from_dollars(1).to_string(), "$1.00000000");
    }

    #[test]
    fn display_truncates_to_eight_decimals() {
        // 1_234_567_890 nanodollars = $1.23456789 → $1.23456789 display
        // truncates to $1.23456789 (8 digits — we drop the 10⁻⁹ digit).
        let c = Cost::from_nanodollars(1_234_567_890);
        assert_eq!(c.to_string(), "$1.23456789");

        // Below the 10⁻⁸ floor, display shows zeros after the last digit.
        let c = Cost::from_nanodollars(5); // $0.000000005 → truncates
        assert_eq!(c.to_string(), "$0.00000000");

        let c = Cost::from_nanodollars(15); // $0.000000015 → 0.00000001
        assert_eq!(c.to_string(), "$0.00000001");
    }

    #[test]
    fn display_negative() {
        let c = Cost::from_nanodollars(-1_230_000_000);
        assert_eq!(c.to_string(), "-$1.23000000");
    }

    #[test]
    fn display_min_does_not_panic() {
        // i64::MIN has magnitude 1 larger than i64::MAX, so naïve `-n`
        // would overflow.  `unsigned_abs` avoids that.
        let c = Cost::from_nanodollars(i64::MIN);
        let s = c.to_string();
        assert!(s.starts_with("-$"));
    }

    #[test]
    fn add_and_sub_are_saturating() {
        let a = Cost::from_nanodollars(i64::MAX - 5);
        let b = Cost::from_nanodollars(100);
        assert_eq!((a + b).nanodollars(), i64::MAX);

        let a = Cost::from_nanodollars(i64::MIN + 5);
        let b = Cost::from_nanodollars(100);
        assert_eq!((a - b).nanodollars(), i64::MIN);
    }

    #[test]
    fn add_assign() {
        let mut a = Cost::from_nanodollars(10);
        a += Cost::from_nanodollars(20);
        assert_eq!(a.nanodollars(), 30);
    }

    #[test]
    fn neg_and_sum() {
        let items =
            [Cost::from_nanodollars(10), Cost::from_nanodollars(20), Cost::from_nanodollars(-5)];
        let total: Cost = items.iter().copied().sum();
        assert_eq!(total.nanodollars(), 25);
        assert_eq!((-Cost::from_nanodollars(7)).nanodollars(), -7);
    }

    #[test]
    fn to_f64_is_close_enough_for_ui() {
        let c = Cost::from_nanodollars(1_234_567_890);
        let approx = c.to_dollars_f64();
        assert!((approx - 1.234_567_89).abs() < 1e-9, "approx = {approx}");
    }

    #[test]
    fn serde_is_transparent_to_the_integer() {
        let c = Cost::from_nanodollars(42);
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(j, "42");
        let back: Cost = serde_json::from_str(&j).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn from_dollars_saturates_on_overflow() {
        let big = Cost::from_dollars(i64::MAX);
        assert_eq!(big.nanodollars(), i64::MAX);
        let small = Cost::from_dollars(i64::MIN);
        assert_eq!(small.nanodollars(), i64::MIN);
    }
}
