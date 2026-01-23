//! # decimal-bytes
//!
//! Arbitrary precision decimals with lexicographically sortable byte encoding.
//!
//! This crate provides a `Decimal` type that stores decimal numbers as bytes
//! in a format that preserves numerical ordering when compared lexicographically.
//! This makes it ideal for use in databases and search engines where efficient
//! range queries on decimal values are needed.
//!
//! ## Features
//!
//! - **Bytes-first storage**: The primary representation is a compact byte array
//! - **Lexicographic ordering**: Byte comparison matches numerical comparison
//! - **Arbitrary precision**: Supports up to 131,072 digits before and 16,383 after decimal
//! - **PostgreSQL NUMERIC compatibility**: Full support for precision, scale, and special values
//! - **Special values**: Infinity, -Infinity, and NaN with correct PostgreSQL sort order
//!
//! ## Example
//!
//! ```
//! use decimal_bytes::Decimal;
//!
//! // Create decimals
//! let a = Decimal::from_str("123.456").unwrap();
//! let b = Decimal::from_str("123.457").unwrap();
//!
//! // Byte comparison matches numerical comparison
//! assert!(a.as_bytes() < b.as_bytes());
//! assert!(a < b);
//!
//! // Display the value
//! assert_eq!(a.to_string(), "123.456");
//!
//! // Special values (PostgreSQL compatible)
//! let inf = Decimal::infinity();
//! let nan = Decimal::nan();
//! assert!(a < inf);
//! assert!(inf < nan);
//! ```
//!
//! ## Sort Order
//!
//! The lexicographic byte order matches PostgreSQL NUMERIC:
//!
//! ```text
//! -Infinity < negative numbers < zero < positive numbers < +Infinity < NaN
//! ```

mod encoding;

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use encoding::DecimalError;
pub use encoding::SpecialValue;
use encoding::{
    decode_special_value, decode_to_string, encode_decimal, encode_decimal_with_constraints,
    encode_special_value, ENCODING_NAN, ENCODING_NEG_INFINITY, ENCODING_POS_INFINITY,
};

/// An arbitrary precision decimal number stored as sortable bytes.
///
/// The internal byte representation is designed to be lexicographically sortable,
/// meaning that comparing the bytes directly yields the same result as comparing
/// the numerical values. This enables efficient range queries in databases.
///
/// # Storage Efficiency
///
/// The encoding uses:
/// - 1 byte for the sign
/// - Variable bytes for the exponent (typically 1-3 bytes)
/// - 4 bits per decimal digit (BCD encoding, 2 digits per byte)
#[derive(Clone)]
pub struct Decimal {
    bytes: Vec<u8>,
}

impl Decimal {
    /// Creates a new Decimal from a string representation.
    ///
    /// # Examples
    ///
    /// ```
    /// use decimal_bytes::Decimal;
    ///
    /// let d = Decimal::from_str("123.456").unwrap();
    /// let d = Decimal::from_str("-0.001").unwrap();
    /// let d = Decimal::from_str("1e10").unwrap();
    /// ```
    pub fn from_str(s: &str) -> Result<Self, DecimalError> {
        let bytes = encode_decimal(s)?;
        Ok(Self { bytes })
    }

    /// Creates a new Decimal with precision and scale constraints.
    ///
    /// Values that exceed the constraints are truncated/rounded to fit.
    /// This is compatible with SQL NUMERIC(precision, scale) semantics.
    ///
    /// - `precision`: Maximum total number of significant digits (None = unlimited)
    /// - `scale`: Digits after decimal point; negative values round to left of decimal
    ///
    /// # PostgreSQL Compatibility
    ///
    /// Supports negative scale (rounds to powers of 10):
    /// - `scale = -3` rounds to nearest 1000
    /// - `NUMERIC(2, -3)` allows values like -99000 to 99000
    ///
    /// # Examples
    ///
    /// ```
    /// use decimal_bytes::Decimal;
    ///
    /// // NUMERIC(5, 2) - up to 5 digits total, 2 after decimal
    /// let d = Decimal::with_precision_scale("123.456", Some(5), Some(2)).unwrap();
    /// assert_eq!(d.to_string(), "123.46"); // Rounded to 2 decimal places
    ///
    /// // NUMERIC(2, -3) - rounds to nearest 1000, max 2 significant digits
    /// let d = Decimal::with_precision_scale("12345", Some(2), Some(-3)).unwrap();
    /// assert_eq!(d.to_string(), "12000"); // Rounded to nearest 1000
    /// ```
    pub fn with_precision_scale(
        s: &str,
        precision: Option<u32>,
        scale: Option<i32>,
    ) -> Result<Self, DecimalError> {
        let bytes = encode_decimal_with_constraints(s, precision, scale)?;
        Ok(Self { bytes })
    }

    /// Creates a Decimal from raw bytes.
    ///
    /// The bytes must be a valid encoding produced by `as_bytes()`.
    /// Returns an error if the bytes are invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use decimal_bytes::Decimal;
    ///
    /// let original = Decimal::from_str("123.456").unwrap();
    /// let bytes = original.as_bytes();
    /// let restored = Decimal::from_bytes(bytes).unwrap();
    /// assert_eq!(original, restored);
    /// ```
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecimalError> {
        // Validate by attempting to decode
        let _ = decode_to_string(bytes)?;
        Ok(Self {
            bytes: bytes.to_vec(),
        })
    }

    /// Creates a Decimal from raw bytes without validation.
    ///
    /// # Safety
    ///
    /// The caller must ensure the bytes are a valid encoding.
    /// Using invalid bytes may cause panics or incorrect results.
    #[inline]
    pub fn from_bytes_unchecked(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// Returns the raw byte representation.
    ///
    /// These bytes are lexicographically sortable - comparing them directly
    /// yields the same result as comparing the numerical values.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consumes the Decimal and returns the underlying bytes.
    #[inline]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Returns the string representation of this decimal.
    ///
    /// Note: This is computed on demand from the byte representation.
    pub fn to_string(&self) -> String {
        decode_to_string(&self.bytes).expect("Decimal contains valid bytes")
    }

    /// Returns true if this decimal represents zero.
    pub fn is_zero(&self) -> bool {
        self.bytes.len() == 1 && self.bytes[0] == encoding::SIGN_ZERO
    }

    /// Returns true if this decimal is negative.
    pub fn is_negative(&self) -> bool {
        !self.bytes.is_empty() && self.bytes[0] == encoding::SIGN_NEGATIVE
    }

    /// Returns true if this decimal is positive (and not zero).
    pub fn is_positive(&self) -> bool {
        !self.bytes.is_empty() && self.bytes[0] == encoding::SIGN_POSITIVE
    }

    /// Returns true if this decimal represents positive infinity.
    pub fn is_pos_infinity(&self) -> bool {
        self.bytes.as_slice() == ENCODING_POS_INFINITY
    }

    /// Returns true if this decimal represents negative infinity.
    pub fn is_neg_infinity(&self) -> bool {
        self.bytes.as_slice() == ENCODING_NEG_INFINITY
    }

    /// Returns true if this decimal represents positive or negative infinity.
    pub fn is_infinity(&self) -> bool {
        self.is_pos_infinity() || self.is_neg_infinity()
    }

    /// Returns true if this decimal represents NaN (Not a Number).
    pub fn is_nan(&self) -> bool {
        self.bytes.as_slice() == ENCODING_NAN
    }

    /// Returns true if this decimal is a special value (Infinity or NaN).
    pub fn is_special(&self) -> bool {
        decode_special_value(&self.bytes).is_some()
    }

    /// Returns true if this decimal is a finite number (not Infinity or NaN).
    pub fn is_finite(&self) -> bool {
        !self.is_special()
    }

    /// Returns the number of bytes used to store this decimal.
    #[inline]
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    /// Creates positive infinity.
    pub fn infinity() -> Self {
        Self {
            bytes: encode_special_value(SpecialValue::Infinity),
        }
    }

    /// Creates negative infinity.
    pub fn neg_infinity() -> Self {
        Self {
            bytes: encode_special_value(SpecialValue::NegInfinity),
        }
    }

    /// Creates NaN (Not a Number).
    pub fn nan() -> Self {
        Self {
            bytes: encode_special_value(SpecialValue::NaN),
        }
    }
}

impl FromStr for Decimal {
    type Err = DecimalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Decimal::from_str(s)
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

impl fmt::Debug for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decimal")
            .field("value", &self.to_string())
            .field("bytes", &self.bytes)
            .finish()
    }
}

impl PartialEq for Decimal {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for Decimal {}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        // Byte comparison is equivalent to numerical comparison
        self.bytes.cmp(&other.bytes)
    }
}

impl Hash for Decimal {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bytes.hash(state);
    }
}

impl Serialize for Decimal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize as string for human readability in JSON
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Decimal::from_str(&s).map_err(serde::de::Error::custom)
    }
}

// Conversion from integer types
macro_rules! impl_from_int {
    ($($t:ty),*) => {
        $(
            impl From<$t> for Decimal {
                fn from(val: $t) -> Self {
                    Decimal::from_str(&val.to_string()).expect("Integer is always valid")
                }
            }
        )*
    };
}

impl_from_int!(i8, i16, i32, i64, i128, u8, u16, u32, u64, u128);

impl Default for Decimal {
    fn default() -> Self {
        Decimal {
            bytes: vec![encoding::SIGN_ZERO],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_str() {
        let d = Decimal::from_str("123.456").unwrap();
        assert_eq!(d.to_string(), "123.456");
    }

    #[test]
    fn test_zero() {
        let d = Decimal::from_str("0").unwrap();
        assert!(d.is_zero());
        assert!(!d.is_negative());
        assert!(!d.is_positive());
        assert!(d.is_finite());
        assert!(!d.is_special());
    }

    #[test]
    fn test_negative() {
        let d = Decimal::from_str("-123.456").unwrap();
        assert!(d.is_negative());
        assert!(!d.is_zero());
        assert!(!d.is_positive());
        assert!(d.is_finite());
    }

    #[test]
    fn test_positive() {
        let d = Decimal::from_str("123.456").unwrap();
        assert!(d.is_positive());
        assert!(!d.is_zero());
        assert!(!d.is_negative());
        assert!(d.is_finite());
    }

    #[test]
    fn test_ordering() {
        let values = vec!["-100", "-10", "-1", "-0.1", "0", "0.1", "1", "10", "100"];
        let decimals: Vec<Decimal> = values
            .iter()
            .map(|s| Decimal::from_str(s).unwrap())
            .collect();

        // Check that ordering is correct
        for i in 0..decimals.len() - 1 {
            assert!(
                decimals[i] < decimals[i + 1],
                "{} should be < {}",
                values[i],
                values[i + 1]
            );
        }

        // Check that byte ordering matches
        for i in 0..decimals.len() - 1 {
            assert!(
                decimals[i].as_bytes() < decimals[i + 1].as_bytes(),
                "bytes of {} should be < bytes of {}",
                values[i],
                values[i + 1]
            );
        }
    }

    #[test]
    fn test_roundtrip() {
        let values = vec![
            "0",
            "1",
            "-1",
            "123.456",
            "-123.456",
            "0.001",
            "0.1",
            "10",
            "100",
            "1000000",
            "-1000000",
        ];

        for s in values {
            let d = Decimal::from_str(s).unwrap();
            let bytes = d.as_bytes();
            let restored = Decimal::from_bytes(bytes).unwrap();
            assert_eq!(d, restored, "Roundtrip failed for {}", s);
        }
    }

    #[test]
    fn test_precision_scale() {
        // Round to 2 decimal places
        let d = Decimal::with_precision_scale("123.456", Some(10), Some(2)).unwrap();
        assert_eq!(d.to_string(), "123.46");

        // When precision is exceeded, least significant integer digits are kept
        let d = Decimal::with_precision_scale("12345.67", Some(5), Some(2)).unwrap();
        assert_eq!(d.to_string(), "345.67"); // 5 digits total, 2 after decimal = 3 integer digits max

        // Rounding within precision limits
        let d = Decimal::with_precision_scale("99.999", Some(5), Some(2)).unwrap();
        assert_eq!(d.to_string(), "100"); // Rounds up, fits in precision
    }

    #[test]
    fn test_from_integer() {
        let d = Decimal::from(42i64);
        assert_eq!(d.to_string(), "42");

        let d = Decimal::from(-100i32);
        assert_eq!(d.to_string(), "-100");
    }

    #[test]
    fn test_serialization() {
        let d = Decimal::from_str("123.456").unwrap();
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "\"123.456\"");

        let restored: Decimal = serde_json::from_str(&json).unwrap();
        assert_eq!(d, restored);
    }

    #[test]
    fn test_byte_efficiency() {
        // Check that storage is reasonably efficient
        let d = Decimal::from_str("123456789").unwrap();
        // 1 byte sign + ~2 bytes exponent + ~5 bytes mantissa (9 digits / 2)
        assert!(
            d.byte_len() <= 10,
            "Expected <= 10 bytes, got {}",
            d.byte_len()
        );

        let d = Decimal::from_str("0.000001").unwrap();
        // Should be compact for small numbers too
        assert!(
            d.byte_len() <= 6,
            "Expected <= 6 bytes, got {}",
            d.byte_len()
        );
    }

    // ==================== Special Values Tests ====================

    #[test]
    fn test_infinity_creation() {
        let pos_inf = Decimal::infinity();
        assert!(pos_inf.is_pos_infinity());
        assert!(pos_inf.is_infinity());
        assert!(!pos_inf.is_neg_infinity());
        assert!(!pos_inf.is_nan());
        assert!(pos_inf.is_special());
        assert!(!pos_inf.is_finite());
        assert_eq!(pos_inf.to_string(), "Infinity");

        let neg_inf = Decimal::neg_infinity();
        assert!(neg_inf.is_neg_infinity());
        assert!(neg_inf.is_infinity());
        assert!(!neg_inf.is_pos_infinity());
        assert!(!neg_inf.is_nan());
        assert!(neg_inf.is_special());
        assert!(!neg_inf.is_finite());
        assert_eq!(neg_inf.to_string(), "-Infinity");
    }

    #[test]
    fn test_nan_creation() {
        let nan = Decimal::nan();
        assert!(nan.is_nan());
        assert!(nan.is_special());
        assert!(!nan.is_finite());
        assert!(!nan.is_infinity());
        assert!(!nan.is_zero());
        assert_eq!(nan.to_string(), "NaN");
    }

    #[test]
    fn test_special_value_from_str() {
        let pos_inf = Decimal::from_str("Infinity").unwrap();
        assert!(pos_inf.is_pos_infinity());

        let neg_inf = Decimal::from_str("-Infinity").unwrap();
        assert!(neg_inf.is_neg_infinity());

        let nan = Decimal::from_str("NaN").unwrap();
        assert!(nan.is_nan());

        // Case-insensitive
        let inf = Decimal::from_str("infinity").unwrap();
        assert!(inf.is_pos_infinity());

        let inf = Decimal::from_str("INF").unwrap();
        assert!(inf.is_pos_infinity());
    }

    #[test]
    fn test_special_value_ordering() {
        // PostgreSQL order: -Infinity < negatives < zero < positives < Infinity < NaN
        let neg_inf = Decimal::neg_infinity();
        let neg_num = Decimal::from_str("-1000").unwrap();
        let zero = Decimal::from_str("0").unwrap();
        let pos_num = Decimal::from_str("1000").unwrap();
        let pos_inf = Decimal::infinity();
        let nan = Decimal::nan();

        assert!(neg_inf < neg_num);
        assert!(neg_num < zero);
        assert!(zero < pos_num);
        assert!(pos_num < pos_inf);
        assert!(pos_inf < nan);

        // Verify byte ordering matches
        assert!(neg_inf.as_bytes() < neg_num.as_bytes());
        assert!(neg_num.as_bytes() < zero.as_bytes());
        assert!(zero.as_bytes() < pos_num.as_bytes());
        assert!(pos_num.as_bytes() < pos_inf.as_bytes());
        assert!(pos_inf.as_bytes() < nan.as_bytes());
    }

    #[test]
    fn test_special_value_equality() {
        // All NaNs are equal (PostgreSQL semantics)
        let nan1 = Decimal::from_str("NaN").unwrap();
        let nan2 = Decimal::from_str("nan").unwrap();
        let nan3 = Decimal::nan();
        assert_eq!(nan1, nan2);
        assert_eq!(nan2, nan3);

        // Infinities are equal to themselves
        let inf1 = Decimal::infinity();
        let inf2 = Decimal::from_str("Infinity").unwrap();
        assert_eq!(inf1, inf2);

        let neg_inf1 = Decimal::neg_infinity();
        let neg_inf2 = Decimal::from_str("-Infinity").unwrap();
        assert_eq!(neg_inf1, neg_inf2);
    }

    #[test]
    fn test_special_value_serialization() {
        let inf = Decimal::infinity();
        let json = serde_json::to_string(&inf).unwrap();
        assert_eq!(json, "\"Infinity\"");
        let restored: Decimal = serde_json::from_str(&json).unwrap();
        assert_eq!(inf, restored);

        let nan = Decimal::nan();
        let json = serde_json::to_string(&nan).unwrap();
        assert_eq!(json, "\"NaN\"");
        let restored: Decimal = serde_json::from_str(&json).unwrap();
        assert_eq!(nan, restored);
    }

    #[test]
    fn test_special_value_byte_efficiency() {
        // Special values should be compact (3 bytes each)
        assert_eq!(Decimal::infinity().byte_len(), 3);
        assert_eq!(Decimal::neg_infinity().byte_len(), 3);
        assert_eq!(Decimal::nan().byte_len(), 3);
    }

    // ==================== Negative Scale Tests ====================

    #[test]
    fn test_negative_scale() {
        // Round to nearest 1000
        let d = Decimal::with_precision_scale("12345", Some(10), Some(-3)).unwrap();
        assert_eq!(d.to_string(), "12000");

        // Round up
        let d = Decimal::with_precision_scale("12500", Some(10), Some(-3)).unwrap();
        assert_eq!(d.to_string(), "13000");

        // Round to nearest 100
        let d = Decimal::with_precision_scale("1234", Some(10), Some(-2)).unwrap();
        assert_eq!(d.to_string(), "1200");
    }

    #[test]
    fn test_negative_scale_with_precision() {
        // NUMERIC(2, -3): 2 significant digits, round to nearest 1000
        let d = Decimal::with_precision_scale("12345", Some(2), Some(-3)).unwrap();
        assert_eq!(d.to_string(), "12000");
    }
}
