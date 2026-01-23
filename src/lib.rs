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
//! - **Arbitrary precision**: Supports numbers with many significant digits
//! - **SQL NUMERIC compatibility**: Supports precision and scale constraints
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
//! ```

mod encoding;

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use encoding::DecimalError;
use encoding::{decode_to_string, encode_decimal, encode_decimal_with_constraints};

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
    /// - `scale`: Maximum digits after the decimal point (None = unlimited)
    ///
    /// # Examples
    ///
    /// ```
    /// use decimal_bytes::Decimal;
    ///
    /// // NUMERIC(5, 2) - up to 5 digits total, 2 after decimal
    /// let d = Decimal::with_precision_scale("123.456", Some(5), Some(2)).unwrap();
    /// assert_eq!(d.to_string(), "123.46"); // Rounded to 2 decimal places
    /// ```
    pub fn with_precision_scale(
        s: &str,
        precision: Option<u32>,
        scale: Option<u32>,
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

    /// Returns the number of bytes used to store this decimal.
    #[inline]
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
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
    }

    #[test]
    fn test_negative() {
        let d = Decimal::from_str("-123.456").unwrap();
        assert!(d.is_negative());
        assert!(!d.is_zero());
        assert!(!d.is_positive());
    }

    #[test]
    fn test_positive() {
        let d = Decimal::from_str("123.456").unwrap();
        assert!(d.is_positive());
        assert!(!d.is_zero());
        assert!(!d.is_negative());
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
        assert!(d.byte_len() <= 10, "Expected <= 10 bytes, got {}", d.byte_len());

        let d = Decimal::from_str("0.000001").unwrap();
        // Should be compact for small numbers too
        assert!(d.byte_len() <= 6, "Expected <= 6 bytes, got {}", d.byte_len());
    }
}
