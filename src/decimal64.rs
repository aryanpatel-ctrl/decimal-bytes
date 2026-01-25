//! Fixed-precision 64-bit decimal type with embedded scale.
//!
//! `Decimal64` provides an efficient fixed-size representation for decimal numbers
//! that fit within 64 bits. Both the value and scale are packed into a single i64.
//!
//! ## When to Use
//!
//! - **Decimal64**: For decimals with precision ≤ 16 and scale 0-18 (most financial data)
//! - **Decimal**: For arbitrary precision or when precision/scale exceed Decimal64 limits
//!
//! ## Storage Layout
//!
//! ```text
//! 64-bit packed representation:
//! ┌──────────────────┬─────────────────────────────────────────────────────┐
//! │ Scale (8 bits)   │ Value (56 bits, signed)                             │
//! │ Byte 0           │ Bytes 1-7                                           │
//! └──────────────────┴─────────────────────────────────────────────────────┘
//! ```
//!
//! - **Scale byte**: 0-18 for normal values, 253/254/255 for special values
//! - **Value**: 56-bit signed integer (-2^55 to 2^55-1)
//!
//! ## PostgreSQL Compatibility
//!
//! `Decimal64` supports PostgreSQL NUMERIC semantics including:
//! - Precision up to 16 significant digits
//! - Scale from 0 to 18
//! - Special values: `Infinity`, `-Infinity`, and `NaN`
//! - Sort order: `-Infinity < numbers < +Infinity < NaN`
//! - `NaN == NaN` (PostgreSQL semantics, not IEEE 754)
//!
//! ## Example
//!
//! ```
//! use decimal_bytes::Decimal64;
//!
//! // Create with precision and scale
//! let price = Decimal64::new("123.45", 2).unwrap();
//! assert_eq!(price.to_string(), "123.45");
//! assert_eq!(price.scale(), 2);
//!
//! // Efficient 8-byte representation
//! assert_eq!(std::mem::size_of_val(&price), 8);
//!
//! // Special values
//! let inf = Decimal64::infinity();
//! let nan = Decimal64::nan();
//! assert!(price < inf);
//! assert!(inf < nan);
//! ```

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::encoding::DecimalError;
use crate::Decimal;

/// Maximum precision that fits in 56-bit value (16 digits).
pub const MAX_DECIMAL64_PRECISION: u32 = 16;

/// Maximum scale supported (0-18).
pub const MAX_DECIMAL64_SCALE: u8 = 18;

// Scale byte values for special values
const SCALE_NEG_INFINITY: u8 = 253;
const SCALE_POS_INFINITY: u8 = 254;
const SCALE_NAN: u8 = 255;

// 56-bit value limits
const VALUE_BITS: u32 = 56;
const MAX_VALUE: i64 = (1i64 << (VALUE_BITS - 1)) - 1; // 2^55 - 1
const MIN_VALUE: i64 = -(1i64 << (VALUE_BITS - 1)); // -2^55

/// A fixed-precision decimal stored as a 64-bit integer with embedded scale.
///
/// ## Storage Design
///
/// Unlike designs where scale is external, `Decimal64` packs both value and scale
/// into a single 64-bit integer:
///
/// ```text
/// Byte:    [0]      [1]      [2]      [3]      [4]      [5]      [6]      [7]
///          Scale    |<---------------- 56-bit signed value ---------------->|
/// ```
///
/// - **Scale (byte 0)**: 0-18 for normal values, special markers for Infinity/NaN
/// - **Value (bytes 1-7)**: 56-bit signed integer representing `actual_value * 10^scale`
///
/// ## Trade-offs
///
/// | Aspect | Decimal64 (embedded scale) | External scale design |
/// |--------|---------------------------|----------------------|
/// | Precision | 16 digits | 18 digits |
/// | Self-contained | Yes (scale included) | No (need metadata) |
/// | API simplicity | `to_string()` works | Need `to_string_with_scale(s)` |
/// | Columnar storage | Scale repeated per value | Scale in column metadata |
///
/// ## Special Values
///
/// Special values are encoded using reserved scale bytes:
/// - Scale = 253: -Infinity
/// - Scale = 254: +Infinity
/// - Scale = 255: NaN
///
/// ## Ordering
///
/// Within the same scale, values are ordered correctly. Across different scales,
/// ordering requires conversion to a common scale or use of `cmp()`.
#[derive(Clone, Copy)]
pub struct Decimal64 {
    /// Packed representation: scale in high byte, 56-bit value in low 7 bytes
    packed: i64,
}

impl Decimal64 {
    // ==================== Constructors ====================

    /// Creates a Decimal64 from a string with automatic scale detection.
    ///
    /// The scale is determined from the number of digits after the decimal point.
    ///
    /// # Examples
    ///
    /// ```
    /// use decimal_bytes::Decimal64;
    ///
    /// let d = Decimal64::new("123.45", 2).unwrap();
    /// assert_eq!(d.to_string(), "123.45");
    /// assert_eq!(d.scale(), 2);
    ///
    /// let d = Decimal64::new("100", 0).unwrap();
    /// assert_eq!(d.to_string(), "100");
    /// assert_eq!(d.scale(), 0);
    /// ```
    pub fn new(s: &str, scale: u8) -> Result<Self, DecimalError> {
        Self::with_precision_scale(s, None, Some(scale as i32))
    }

    /// Creates a Decimal64 with precision and scale constraints (PostgreSQL NUMERIC semantics).
    ///
    /// - `precision`: Maximum total significant digits (1-16, or None for no limit)
    /// - `scale`: Digits after decimal point (0-18, or negative for rounding left of decimal)
    ///
    /// # Examples
    ///
    /// ```
    /// use decimal_bytes::Decimal64;
    ///
    /// // NUMERIC(5, 2) - up to 5 digits total, 2 after decimal
    /// let d = Decimal64::with_precision_scale("123.456", Some(5), Some(2)).unwrap();
    /// assert_eq!(d.to_string(), "123.46");
    ///
    /// // NUMERIC(2, -3) - rounds to nearest 1000
    /// let d = Decimal64::with_precision_scale("12345", Some(2), Some(-3)).unwrap();
    /// assert_eq!(d.to_string(), "12000");
    /// ```
    pub fn with_precision_scale(
        s: &str,
        precision: Option<u32>,
        scale: Option<i32>,
    ) -> Result<Self, DecimalError> {
        // Validate precision
        if let Some(p) = precision {
            if p > MAX_DECIMAL64_PRECISION {
                return Err(DecimalError::InvalidFormat(format!(
                    "Precision {} exceeds maximum {} for Decimal64",
                    p, MAX_DECIMAL64_PRECISION
                )));
            }
            if p == 0 {
                return Err(DecimalError::InvalidFormat(
                    "Precision must be at least 1".to_string(),
                ));
            }
        }

        let scale_val = scale.unwrap_or(0);

        // For positive scale, validate it doesn't exceed max
        if scale_val > 0 && scale_val as u8 > MAX_DECIMAL64_SCALE {
            return Err(DecimalError::InvalidFormat(format!(
                "Scale {} exceeds maximum {} for Decimal64",
                scale_val, MAX_DECIMAL64_SCALE
            )));
        }

        let s = s.trim();

        // Handle special values (case-insensitive)
        let lower = s.to_lowercase();
        match lower.as_str() {
            "nan" | "-nan" | "+nan" => return Ok(Self::nan()),
            "infinity" | "inf" | "+infinity" | "+inf" => return Ok(Self::infinity()),
            "-infinity" | "-inf" => return Ok(Self::neg_infinity()),
            _ => {}
        }

        // Parse sign
        let (is_negative, s) = if let Some(rest) = s.strip_prefix('-') {
            (true, rest)
        } else if let Some(rest) = s.strip_prefix('+') {
            (false, rest)
        } else {
            (false, s)
        };

        // Split into integer and fractional parts
        let (int_part, frac_part) = if let Some(dot_pos) = s.find('.') {
            (&s[..dot_pos], &s[dot_pos + 1..])
        } else {
            (s, "")
        };

        // Trim leading zeros from integer part
        let int_part = int_part.trim_start_matches('0');
        let int_part = if int_part.is_empty() { "0" } else { int_part };

        // Handle negative scale (round to left of decimal point)
        if scale_val < 0 {
            return Self::parse_negative_scale(int_part, is_negative, precision, scale_val);
        }

        let scale_u8 = scale_val as u8;

        // Apply scale to fractional part (truncate/round)
        let (int_part, frac_digits) = Self::apply_scale(int_part, frac_part, scale_u8 as usize);

        // Apply precision constraint
        let (final_int, final_frac) =
            Self::apply_precision(&int_part, &frac_digits, precision, scale_u8 as usize);

        // Convert to packed representation
        Self::parts_to_packed(&final_int, &final_frac, is_negative, scale_u8)
    }

    /// Creates a Decimal64 from raw value and scale components.
    ///
    /// # Arguments
    /// * `value` - The scaled integer value (must fit in 56 bits)
    /// * `scale` - The scale (0-18)
    ///
    /// # Errors
    /// Returns an error if value doesn't fit in 56 bits or scale > 18.
    pub fn from_parts(value: i64, scale: u8) -> Result<Self, DecimalError> {
        if scale > MAX_DECIMAL64_SCALE {
            return Err(DecimalError::InvalidFormat(format!(
                "Scale {} exceeds maximum {}",
                scale, MAX_DECIMAL64_SCALE
            )));
        }
        if !(MIN_VALUE..=MAX_VALUE).contains(&value) {
            return Err(DecimalError::InvalidFormat(format!(
                "Value {} doesn't fit in 56 bits (range {} to {})",
                value, MIN_VALUE, MAX_VALUE
            )));
        }

        Ok(Self::pack(value, scale))
    }

    /// Creates a Decimal64 from a raw packed i64 (for deserialization).
    #[inline]
    pub const fn from_raw(packed: i64) -> Self {
        Self { packed }
    }

    // ==================== Special Value Constructors ====================

    /// Creates positive infinity.
    #[inline]
    pub const fn infinity() -> Self {
        Self::pack_special(SCALE_POS_INFINITY)
    }

    /// Creates negative infinity.
    #[inline]
    pub const fn neg_infinity() -> Self {
        Self::pack_special(SCALE_NEG_INFINITY)
    }

    /// Creates NaN (Not a Number).
    ///
    /// Follows PostgreSQL semantics: `NaN == NaN` is `true`.
    #[inline]
    pub const fn nan() -> Self {
        Self::pack_special(SCALE_NAN)
    }

    // ==================== Accessors ====================

    /// Returns the raw packed i64 value.
    #[inline]
    pub const fn raw(&self) -> i64 {
        self.packed
    }

    /// Returns the scale (digits after decimal point).
    ///
    /// Returns 0 for special values (NaN, Infinity).
    #[inline]
    pub fn scale(&self) -> u8 {
        let scale_byte = self.scale_byte();
        if scale_byte > MAX_DECIMAL64_SCALE {
            0 // Special values
        } else {
            scale_byte
        }
    }

    /// Returns the 56-bit value component.
    ///
    /// For special values, returns 0.
    #[inline]
    pub fn value(&self) -> i64 {
        if self.is_special() {
            0
        } else {
            self.unpack_value()
        }
    }

    /// Returns true if this value is zero.
    #[inline]
    pub fn is_zero(&self) -> bool {
        !self.is_special() && self.unpack_value() == 0
    }

    /// Returns true if this value is negative (excluding -Infinity).
    #[inline]
    pub fn is_negative(&self) -> bool {
        !self.is_special() && self.unpack_value() < 0
    }

    /// Returns true if this value is positive (excluding +Infinity and NaN).
    #[inline]
    pub fn is_positive(&self) -> bool {
        !self.is_special() && self.unpack_value() > 0
    }

    /// Returns true if this value is positive infinity.
    #[inline]
    pub fn is_pos_infinity(&self) -> bool {
        self.scale_byte() == SCALE_POS_INFINITY
    }

    /// Returns true if this value is negative infinity.
    #[inline]
    pub fn is_neg_infinity(&self) -> bool {
        self.scale_byte() == SCALE_NEG_INFINITY
    }

    /// Returns true if this value is positive or negative infinity.
    #[inline]
    pub fn is_infinity(&self) -> bool {
        self.is_pos_infinity() || self.is_neg_infinity()
    }

    /// Returns true if this value is NaN (Not a Number).
    #[inline]
    pub fn is_nan(&self) -> bool {
        self.scale_byte() == SCALE_NAN
    }

    /// Returns true if this is a special value (Infinity or NaN).
    #[inline]
    pub fn is_special(&self) -> bool {
        self.scale_byte() > MAX_DECIMAL64_SCALE
    }

    /// Returns true if this is a finite number (not Infinity or NaN).
    #[inline]
    pub fn is_finite(&self) -> bool {
        !self.is_special()
    }

    // ==================== Conversions ====================

    /// Formats the decimal as a string (internal helper for Display).
    fn format_decimal(&self) -> String {
        // Handle special values
        if self.is_neg_infinity() {
            return "-Infinity".to_string();
        }
        if self.is_pos_infinity() {
            return "Infinity".to_string();
        }
        if self.is_nan() {
            return "NaN".to_string();
        }

        let value = self.unpack_value();
        let scale = self.scale_byte() as u32;

        if value == 0 {
            return "0".to_string();
        }

        let is_negative = value < 0;
        let abs_value = value.unsigned_abs();

        if scale == 0 {
            return if is_negative {
                format!("-{}", abs_value)
            } else {
                abs_value.to_string()
            };
        }

        let scale_factor = 10u64.pow(scale);
        let int_part = abs_value / scale_factor;
        let frac_part = abs_value % scale_factor;

        let result = if frac_part == 0 {
            int_part.to_string()
        } else {
            let frac_str = format!("{:0>width$}", frac_part, width = scale as usize);
            let frac_str = frac_str.trim_end_matches('0');
            format!("{}.{}", int_part, frac_str)
        };

        if is_negative {
            format!("-{}", result)
        } else {
            result
        }
    }

    /// Returns the 8-byte big-endian representation.
    #[inline]
    pub fn to_be_bytes(&self) -> [u8; 8] {
        self.packed.to_be_bytes()
    }

    /// Creates a Decimal64 from big-endian bytes.
    #[inline]
    pub fn from_be_bytes(bytes: [u8; 8]) -> Self {
        Self {
            packed: i64::from_be_bytes(bytes),
        }
    }

    /// Converts to the variable-length `Decimal` type.
    pub fn to_decimal(&self) -> Decimal {
        if self.is_neg_infinity() {
            return Decimal::neg_infinity();
        }
        if self.is_pos_infinity() {
            return Decimal::infinity();
        }
        if self.is_nan() {
            return Decimal::nan();
        }

        Decimal::from_str(&self.to_string()).expect("Decimal64 string is always valid")
    }

    /// Creates a Decimal64 from a Decimal with the specified scale.
    pub fn from_decimal(decimal: &Decimal, scale: u8) -> Result<Self, DecimalError> {
        if decimal.is_nan() {
            return Ok(Self::nan());
        }
        if decimal.is_pos_infinity() {
            return Ok(Self::infinity());
        }
        if decimal.is_neg_infinity() {
            return Ok(Self::neg_infinity());
        }

        Self::new(&decimal.to_string(), scale)
    }

    /// Returns the minimum finite value that can be stored (at scale 0).
    #[inline]
    pub const fn min_value() -> Self {
        Self::pack(MIN_VALUE, 0)
    }

    /// Returns the maximum finite value that can be stored (at scale 0).
    #[inline]
    pub const fn max_value() -> Self {
        Self::pack(MAX_VALUE, 0)
    }

    // ==================== Internal Helpers ====================

    #[inline]
    const fn pack(value: i64, scale: u8) -> Self {
        // Pack: scale in high byte, value in low 56 bits
        let scale_part = (scale as i64) << VALUE_BITS;
        let value_part = value & ((1i64 << VALUE_BITS) - 1);
        Self {
            packed: scale_part | value_part,
        }
    }

    #[inline]
    const fn pack_special(scale: u8) -> Self {
        // Special values have scale marker and zero value
        Self {
            packed: (scale as i64) << VALUE_BITS,
        }
    }

    #[inline]
    fn scale_byte(&self) -> u8 {
        ((self.packed >> VALUE_BITS) & 0xFF) as u8
    }

    #[inline]
    fn unpack_value(&self) -> i64 {
        // Sign-extend the 56-bit value
        let raw = self.packed & ((1i64 << VALUE_BITS) - 1);
        // Check if sign bit (bit 55) is set
        if raw & (1i64 << (VALUE_BITS - 1)) != 0 {
            // Negative: extend sign bits
            raw | (!0i64 << VALUE_BITS)
        } else {
            raw
        }
    }

    /// Helper: Parse with negative scale (rounds to powers of 10).
    fn parse_negative_scale(
        int_part: &str,
        is_negative: bool,
        precision: Option<u32>,
        scale: i32,
    ) -> Result<Self, DecimalError> {
        let round_digits = (-scale) as usize;

        if int_part == "0" {
            return Ok(Self::pack(0, 0));
        }

        let int_len = int_part.len();

        if int_len <= round_digits {
            // Number is smaller than rounding unit
            let num_val: u64 = int_part.parse().unwrap_or(0);
            let rounding_unit = 10u64.pow(round_digits as u32);
            let half_unit = rounding_unit / 2;

            let result = if num_val >= half_unit {
                rounding_unit as i64
            } else {
                0
            };

            let value = if is_negative && result != 0 {
                -result
            } else {
                result
            };

            if !(MIN_VALUE..=MAX_VALUE).contains(&value) {
                return Err(DecimalError::InvalidFormat(
                    "Value too large for Decimal64".to_string(),
                ));
            }

            return Ok(Self::pack(value, 0));
        }

        // Round the integer part
        let keep_len = int_len - round_digits;
        let keep_part = &int_part[..keep_len];
        let round_part = &int_part[keep_len..];

        let first_rounded_digit = round_part.chars().next().unwrap_or('0');
        let mut result_int: String = keep_part.to_string();

        if first_rounded_digit >= '5' {
            result_int = add_one_to_string(&result_int);
        }

        // Apply precision constraint
        if let Some(p) = precision {
            let sig_len = result_int.trim_start_matches('0').len();
            if sig_len > p as usize && p > 0 {
                let start = result_int.len().saturating_sub(p as usize);
                result_int = result_int[start..].to_string();
            }
        }

        // Parse the significant part and scale back
        let significant: i64 = result_int.parse().unwrap_or(0);
        let value = significant
            .checked_mul(10i64.pow(round_digits as u32))
            .ok_or_else(|| {
                DecimalError::InvalidFormat("Value too large for Decimal64".to_string())
            })?;

        let value = if is_negative { -value } else { value };

        if !(MIN_VALUE..=MAX_VALUE).contains(&value) {
            return Err(DecimalError::InvalidFormat(
                "Value too large for Decimal64".to_string(),
            ));
        }

        Ok(Self::pack(value, 0))
    }

    /// Helper: Apply scale constraint to fractional part.
    fn apply_scale(int_part: &str, frac_part: &str, scale: usize) -> (String, String) {
        if scale == 0 {
            let first_frac = frac_part.chars().next().unwrap_or('0');
            if first_frac >= '5' {
                return (add_one_to_string(int_part), String::new());
            }
            return (int_part.to_string(), String::new());
        }

        if frac_part.len() <= scale {
            let padded = format!("{:0<width$}", frac_part, width = scale);
            return (int_part.to_string(), padded);
        }

        let truncated = &frac_part[..scale];
        let next_digit = frac_part.chars().nth(scale).unwrap_or('0');

        if next_digit >= '5' {
            let rounded = add_one_to_string(truncated);
            if rounded.len() > scale {
                return (add_one_to_string(int_part), "0".repeat(scale));
            }
            let padded = format!("{:0>width$}", rounded, width = scale);
            return (int_part.to_string(), padded);
        }

        (int_part.to_string(), truncated.to_string())
    }

    /// Helper: Apply precision constraint.
    fn apply_precision(
        int_part: &str,
        frac_part: &str,
        precision: Option<u32>,
        scale: usize,
    ) -> (String, String) {
        let Some(p) = precision else {
            return (int_part.to_string(), frac_part.to_string());
        };

        let p = p as usize;
        let max_int_digits = p.saturating_sub(scale);

        let int_part = int_part.trim_start_matches('0');
        let int_part = if int_part.is_empty() { "0" } else { int_part };

        if int_part.len() > max_int_digits && max_int_digits > 0 {
            let start = int_part.len() - max_int_digits;
            return (int_part[start..].to_string(), frac_part.to_string());
        } else if max_int_digits == 0 && int_part != "0" {
            return ("0".to_string(), frac_part.to_string());
        }

        (int_part.to_string(), frac_part.to_string())
    }

    /// Helper: Convert parsed parts to packed representation.
    fn parts_to_packed(
        int_part: &str,
        frac_part: &str,
        is_negative: bool,
        scale: u8,
    ) -> Result<Self, DecimalError> {
        let int_value: i64 = int_part.parse().unwrap_or(0);

        let scale_factor = 10i64.pow(scale as u32);

        let scaled_int = int_value.checked_mul(scale_factor).ok_or_else(|| {
            DecimalError::InvalidFormat("Value too large for Decimal64".to_string())
        })?;

        let frac_value: i64 = if frac_part.is_empty() {
            0
        } else {
            frac_part.parse().unwrap_or(0)
        };

        let value = scaled_int + frac_value;
        let value = if is_negative && value != 0 {
            -value
        } else {
            value
        };

        if !(MIN_VALUE..=MAX_VALUE).contains(&value) {
            return Err(DecimalError::InvalidFormat(
                "Value too large for Decimal64".to_string(),
            ));
        }

        Ok(Self::pack(value, scale))
    }
}

// ==================== Helper Functions ====================

fn add_one_to_string(s: &str) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    let mut carry = true;

    for c in chars.iter_mut().rev() {
        if carry {
            if *c == '9' {
                *c = '0';
            } else {
                *c = char::from_digit(c.to_digit(10).unwrap() + 1, 10).unwrap();
                carry = false;
            }
        }
    }

    if carry {
        format!("1{}", chars.iter().collect::<String>())
    } else {
        chars.iter().collect()
    }
}

// ==================== Trait Implementations ====================

impl PartialEq for Decimal64 {
    fn eq(&self, other: &Self) -> bool {
        // Same scale and same value
        self.packed == other.packed
    }
}

impl Eq for Decimal64 {}

impl PartialOrd for Decimal64 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Decimal64 {
    fn cmp(&self, other: &Self) -> Ordering {
        // Handle special values
        match (self.is_special(), other.is_special()) {
            (true, true) => {
                // Both special: compare scale bytes
                // Order: -Infinity (253) < +Infinity (254) < NaN (255)
                self.scale_byte().cmp(&other.scale_byte())
            }
            (true, false) => {
                // self is special
                if self.is_neg_infinity() {
                    Ordering::Less
                } else {
                    Ordering::Greater // +Infinity or NaN
                }
            }
            (false, true) => {
                // other is special
                if other.is_neg_infinity() {
                    Ordering::Greater
                } else {
                    Ordering::Less // +Infinity or NaN
                }
            }
            (false, false) => {
                // Both normal: compare actual decimal values
                let self_scale = self.scale_byte();
                let other_scale = other.scale_byte();
                let self_value = self.unpack_value();
                let other_value = other.unpack_value();

                if self_scale == other_scale {
                    // Same scale: direct comparison
                    self_value.cmp(&other_value)
                } else {
                    // Different scales: normalize to common scale
                    let max_scale = self_scale.max(other_scale);

                    let self_normalized = if self_scale < max_scale {
                        self_value.saturating_mul(10i64.pow((max_scale - self_scale) as u32))
                    } else {
                        self_value
                    };

                    let other_normalized = if other_scale < max_scale {
                        other_value.saturating_mul(10i64.pow((max_scale - other_scale) as u32))
                    } else {
                        other_value
                    };

                    self_normalized.cmp(&other_normalized)
                }
            }
        }
    }
}

impl Hash for Decimal64 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.packed.hash(state);
    }
}

impl fmt::Debug for Decimal64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_nan() {
            write!(f, "Decimal64(NaN)")
        } else if self.is_pos_infinity() {
            write!(f, "Decimal64(Infinity)")
        } else if self.is_neg_infinity() {
            write!(f, "Decimal64(-Infinity)")
        } else {
            f.debug_struct("Decimal64")
                .field("value", &self.to_string())
                .field("scale", &self.scale())
                .finish()
        }
    }
}

impl fmt::Display for Decimal64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_decimal())
    }
}

impl Default for Decimal64 {
    fn default() -> Self {
        Self::pack(0, 0)
    }
}

impl From<i64> for Decimal64 {
    fn from(value: i64) -> Self {
        // Clamp to 56-bit range
        let clamped = value.clamp(MIN_VALUE, MAX_VALUE);
        Self::pack(clamped, 0)
    }
}

impl From<i32> for Decimal64 {
    fn from(value: i32) -> Self {
        Self::pack(value as i64, 0)
    }
}

// Serde support
impl Serialize for Decimal64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(self.packed)
    }
}

impl<'de> Deserialize<'de> for Decimal64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let packed = i64::deserialize(deserializer)?;
        Ok(Self::from_raw(packed))
    }
}

impl FromStr for Decimal64 {
    type Err = DecimalError;

    /// Parses with automatic scale detection from the string.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        // Handle special values
        let lower = s.to_lowercase();
        match lower.as_str() {
            "nan" | "-nan" | "+nan" => return Ok(Self::nan()),
            "infinity" | "inf" | "+infinity" | "+inf" => return Ok(Self::infinity()),
            "-infinity" | "-inf" => return Ok(Self::neg_infinity()),
            _ => {}
        }

        // Detect scale from decimal point position
        let scale = if let Some(dot_pos) = s.find('.') {
            let after_dot = &s[dot_pos + 1..];
            // Count digits after decimal (excluding trailing non-digits)
            after_dot.chars().take_while(|c| c.is_ascii_digit()).count() as u8
        } else {
            0
        };

        Self::new(s, scale.min(MAX_DECIMAL64_SCALE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Basic Tests ====================

    #[test]
    fn test_new_basic() {
        let d = Decimal64::new("123.45", 2).unwrap();
        assert_eq!(d.to_string(), "123.45");
        assert_eq!(d.scale(), 2);
        assert_eq!(d.value(), 12345);

        let d = Decimal64::new("100", 0).unwrap();
        assert_eq!(d.to_string(), "100");
        assert_eq!(d.scale(), 0);

        let d = Decimal64::new("-50.5", 1).unwrap();
        assert_eq!(d.to_string(), "-50.5");
        assert_eq!(d.scale(), 1);
        assert_eq!(d.value(), -505);
    }

    #[test]
    fn test_zero() {
        let d = Decimal64::new("0", 0).unwrap();
        assert!(d.is_zero());
        assert!(!d.is_negative());
        assert!(!d.is_positive());
        assert!(d.is_finite());

        let d = Decimal64::new("0.00", 2).unwrap();
        assert!(d.is_zero());
        assert_eq!(d.scale(), 2);
    }

    #[test]
    fn test_from_str() {
        let d: Decimal64 = "123.456".parse().unwrap();
        assert_eq!(d.to_string(), "123.456");
        assert_eq!(d.scale(), 3);

        let d: Decimal64 = "100".parse().unwrap();
        assert_eq!(d.to_string(), "100");
        assert_eq!(d.scale(), 0);
    }

    // ==================== Special Values ====================

    #[test]
    fn test_infinity() {
        let inf = Decimal64::infinity();
        assert!(inf.is_pos_infinity());
        assert!(inf.is_infinity());
        assert!(inf.is_special());
        assert!(!inf.is_finite());
        assert_eq!(inf.to_string(), "Infinity");

        let neg_inf = Decimal64::neg_infinity();
        assert!(neg_inf.is_neg_infinity());
        assert!(neg_inf.is_infinity());
        assert_eq!(neg_inf.to_string(), "-Infinity");
    }

    #[test]
    fn test_nan() {
        let nan = Decimal64::nan();
        assert!(nan.is_nan());
        assert!(nan.is_special());
        assert!(!nan.is_finite());
        assert_eq!(nan.to_string(), "NaN");

        // PostgreSQL semantics: NaN == NaN
        assert_eq!(nan, Decimal64::nan());
    }

    #[test]
    fn test_special_from_str() {
        assert!(Decimal64::from_str("Infinity").unwrap().is_pos_infinity());
        assert!(Decimal64::from_str("-Infinity").unwrap().is_neg_infinity());
        assert!(Decimal64::from_str("NaN").unwrap().is_nan());
        assert!(Decimal64::from_str("inf").unwrap().is_pos_infinity());
        assert!(Decimal64::from_str("-inf").unwrap().is_neg_infinity());
    }

    // ==================== Ordering ====================

    #[test]
    fn test_ordering_same_scale() {
        let a = Decimal64::new("100", 0).unwrap();
        let b = Decimal64::new("200", 0).unwrap();
        let c = Decimal64::new("-50", 0).unwrap();

        assert!(c < a);
        assert!(a < b);
    }

    #[test]
    fn test_ordering_different_scale() {
        let a = Decimal64::new("1.5", 1).unwrap(); // 15 at scale 1
        let b = Decimal64::new("1.50", 2).unwrap(); // 150 at scale 2

        // 1.5 == 1.50 when normalized
        assert_eq!(a.cmp(&b), Ordering::Equal);

        let c = Decimal64::new("1.51", 2).unwrap();
        assert!(a < c);
    }

    #[test]
    fn test_ordering_with_special() {
        let neg_inf = Decimal64::neg_infinity();
        let neg = Decimal64::new("-1000", 0).unwrap();
        let zero = Decimal64::new("0", 0).unwrap();
        let pos = Decimal64::new("1000", 0).unwrap();
        let inf = Decimal64::infinity();
        let nan = Decimal64::nan();

        assert!(neg_inf < neg);
        assert!(neg < zero);
        assert!(zero < pos);
        assert!(pos < inf);
        assert!(inf < nan);
    }

    // ==================== Precision/Scale ====================

    #[test]
    fn test_precision_scale() {
        let d = Decimal64::with_precision_scale("123.456", Some(5), Some(2)).unwrap();
        assert_eq!(d.to_string(), "123.46");

        let d = Decimal64::with_precision_scale("12345.67", Some(5), Some(2)).unwrap();
        assert_eq!(d.to_string(), "345.67");
    }

    #[test]
    fn test_negative_scale() {
        let d = Decimal64::with_precision_scale("12345", None, Some(-2)).unwrap();
        assert_eq!(d.to_string(), "12300");

        let d = Decimal64::with_precision_scale("12350", None, Some(-2)).unwrap();
        assert_eq!(d.to_string(), "12400");
    }

    // ==================== Roundtrip ====================

    #[test]
    fn test_roundtrip() {
        let values = ["0", "123.45", "-99.99", "1000000", "-1"];

        for s in values {
            let d: Decimal64 = s.parse().unwrap();
            let packed = d.raw();
            let restored = Decimal64::from_raw(packed);
            assert_eq!(
                d.to_string(),
                restored.to_string(),
                "Roundtrip failed for {}",
                s
            );
        }
    }

    #[test]
    fn test_byte_roundtrip() {
        let d = Decimal64::new("123.45", 2).unwrap();
        let bytes = d.to_be_bytes();
        let restored = Decimal64::from_be_bytes(bytes);
        assert_eq!(d, restored);
    }

    // ==================== Conversion ====================

    #[test]
    fn test_decimal_conversion() {
        let d64 = Decimal64::new("123.456", 3).unwrap();
        let decimal = d64.to_decimal();
        assert_eq!(decimal.to_string(), "123.456");

        let d64_back = Decimal64::from_decimal(&decimal, 3).unwrap();
        assert_eq!(d64.to_string(), d64_back.to_string());
    }

    #[test]
    fn test_from_parts() {
        let d = Decimal64::from_parts(12345, 2).unwrap();
        assert_eq!(d.to_string(), "123.45");
        assert_eq!(d.value(), 12345);
        assert_eq!(d.scale(), 2);
    }

    // ==================== Error Cases ====================

    #[test]
    fn test_precision_too_large() {
        assert!(Decimal64::with_precision_scale("1", Some(17), Some(0)).is_err());
    }

    #[test]
    fn test_scale_too_large() {
        assert!(Decimal64::new("1", 19).is_err());
    }

    #[test]
    fn test_value_too_large() {
        // Value that exceeds 56 bits
        assert!(Decimal64::from_parts(i64::MAX, 0).is_err());
        assert!(Decimal64::from_parts(MIN_VALUE - 1, 0).is_err());
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_min_max_values() {
        let min = Decimal64::min_value();
        let max = Decimal64::max_value();

        assert!(min.is_finite());
        assert!(max.is_finite());
        assert!(min < max);
        assert!(Decimal64::neg_infinity() < min);
        assert!(max < Decimal64::infinity());
    }

    #[test]
    fn test_hash() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(Decimal64::new("100", 0).unwrap());
        set.insert(Decimal64::new("100", 0).unwrap()); // Duplicate
        set.insert(Decimal64::new("200", 0).unwrap());
        set.insert(Decimal64::nan());

        assert_eq!(set.len(), 3);
    }
}
