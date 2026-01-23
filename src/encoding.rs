//! Byte encoding for decimal values.
//!
//! This module implements a lexicographically sortable encoding for decimal numbers.
//! The encoding ensures that byte-wise comparison yields the same result as numerical comparison.
//!
//! ## Encoding Format
//!
//! ```text
//! [sign byte] [exponent bytes] [mantissa bytes]
//! ```
//!
//! - **Sign byte**: 0x00 for negative, 0x80 for zero, 0xFF for positive
//! - **Exponent**: Variable-length, biased encoding (inverted for negative numbers)
//! - **Mantissa**: BCD-encoded digits, 2 per byte (inverted for negative numbers)

use thiserror::Error;

/// Sign byte values
pub(crate) const SIGN_NEGATIVE: u8 = 0x00;
pub(crate) const SIGN_ZERO: u8 = 0x80;
pub(crate) const SIGN_POSITIVE: u8 = 0xFF;

/// Exponent bias to make all exponents positive for encoding
const EXPONENT_BIAS: i32 = 16384;
const MAX_EXPONENT: i32 = 32767 - EXPONENT_BIAS; // ~16383
const MIN_EXPONENT: i32 = -EXPONENT_BIAS; // -16384

/// Errors that can occur during decimal encoding/decoding.
#[derive(Error, Debug, Clone, PartialEq)]
pub enum DecimalError {
    /// The input string format is invalid.
    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    /// The number exceeds the supported precision range.
    #[error("Precision overflow: exponent out of range")]
    PrecisionOverflow,

    /// The encoded bytes are invalid.
    #[error("Invalid encoding")]
    InvalidEncoding,
}

/// Encodes a decimal string to sortable bytes.
pub fn encode_decimal(value: &str) -> Result<Vec<u8>, DecimalError> {
    let (is_negative, digits, exponent) = parse_decimal(value)?;

    // Handle zero
    if digits.is_empty() {
        return Ok(vec![SIGN_ZERO]);
    }

    let mut result = Vec::with_capacity(1 + 2 + (digits.len() + 1) / 2);

    // Sign byte
    result.push(if is_negative {
        SIGN_NEGATIVE
    } else {
        SIGN_POSITIVE
    });

    // Encode exponent
    encode_exponent(&mut result, exponent, is_negative);

    // Encode mantissa (BCD, 2 digits per byte)
    encode_mantissa(&mut result, &digits, is_negative);

    Ok(result)
}

/// Encodes a decimal string with precision and scale constraints.
pub fn encode_decimal_with_constraints(
    value: &str,
    precision: Option<u32>,
    scale: Option<u32>,
) -> Result<Vec<u8>, DecimalError> {
    let truncated = truncate_decimal(value, precision, scale)?;
    encode_decimal(&truncated)
}

/// Decodes bytes back to a decimal string.
pub fn decode_to_string(bytes: &[u8]) -> Result<String, DecimalError> {
    if bytes.is_empty() {
        return Err(DecimalError::InvalidEncoding);
    }

    let sign_byte = bytes[0];

    // Handle zero
    if sign_byte == SIGN_ZERO {
        return Ok("0".to_string());
    }

    let is_negative = sign_byte == SIGN_NEGATIVE;

    if sign_byte != SIGN_NEGATIVE && sign_byte != SIGN_POSITIVE {
        return Err(DecimalError::InvalidEncoding);
    }

    // Decode exponent
    let (exponent, mantissa_start) = decode_exponent(&bytes[1..], is_negative)?;

    // Decode mantissa
    let mantissa_bytes = &bytes[1 + mantissa_start..];
    let digits = decode_mantissa(mantissa_bytes, is_negative)?;

    // Format as string
    format_decimal(is_negative, &digits, exponent)
}

/// Parses a decimal string into sign, digits, and exponent.
fn parse_decimal(value: &str) -> Result<(bool, Vec<u8>, i32), DecimalError> {
    let value = value.trim();
    let mut chars = value.chars().peekable();

    // Handle sign
    let is_negative = if chars.peek() == Some(&'-') {
        chars.next();
        true
    } else if chars.peek() == Some(&'+') {
        chars.next();
        false
    } else {
        false
    };

    // Collect the numeric part (before 'e' or 'E')
    let mut integer_part = String::new();
    let mut fractional_part = String::new();
    let mut seen_decimal = false;

    while let Some(&c) = chars.peek() {
        if c == '.' {
            if seen_decimal {
                return Err(DecimalError::InvalidFormat(
                    "Multiple decimal points".to_string(),
                ));
            }
            seen_decimal = true;
            chars.next();
        } else if c.is_ascii_digit() {
            if seen_decimal {
                fractional_part.push(c);
            } else {
                integer_part.push(c);
            }
            chars.next();
        } else if c == 'e' || c == 'E' {
            chars.next();
            break;
        } else {
            return Err(DecimalError::InvalidFormat(format!(
                "Invalid character: {}",
                c
            )));
        }
    }

    // Parse optional exponent
    let mut exp_offset: i32 = 0;
    if chars.peek().is_some() {
        let exp_str: String = chars.collect();
        exp_offset = exp_str
            .parse()
            .map_err(|_| DecimalError::InvalidFormat(format!("Invalid exponent: {}", exp_str)))?;
    }

    // Handle empty input
    if integer_part.is_empty() && fractional_part.is_empty() {
        return Ok((false, vec![], 0));
    }

    // If only fractional part, integer part is "0"
    if integer_part.is_empty() {
        integer_part = "0".to_string();
    }

    // Combine all digits
    let combined = format!("{}{}", integer_part, fractional_part);

    // Find the first and last non-zero digit positions
    let first_nonzero = combined.chars().position(|c| c != '0');
    let last_nonzero = combined.chars().rev().position(|c| c != '0');

    // If all zeros, return zero
    if first_nonzero.is_none() {
        return Ok((false, vec![], 0));
    }

    let first_nonzero = first_nonzero.unwrap();
    let last_nonzero = combined.len() - 1 - last_nonzero.unwrap();

    // Extract the significant digits
    let significant = &combined[first_nonzero..=last_nonzero];

    // Calculate the exponent
    let decimal_position = integer_part.len();
    let exponent = (decimal_position as i32) - (first_nonzero as i32) + exp_offset;

    // Convert significant digits to bytes
    let digits: Vec<u8> = significant
        .chars()
        .map(|c| c.to_digit(10).unwrap() as u8)
        .collect();

    // Validate exponent range
    if exponent > MAX_EXPONENT || exponent < MIN_EXPONENT {
        return Err(DecimalError::PrecisionOverflow);
    }

    Ok((is_negative, digits, exponent))
}

/// Encodes the exponent as variable-length bytes.
fn encode_exponent(result: &mut Vec<u8>, exponent: i32, is_negative: bool) {
    // Bias the exponent to make it always positive
    let biased = (exponent + EXPONENT_BIAS) as u16;

    // For negative numbers, invert the exponent so larger negative numbers sort first
    let encoded = if is_negative { !biased } else { biased };

    // Use 2 bytes for the exponent (big-endian)
    result.push((encoded >> 8) as u8);
    result.push((encoded & 0xFF) as u8);
}

/// Decodes the exponent from bytes.
fn decode_exponent(bytes: &[u8], is_negative: bool) -> Result<(i32, usize), DecimalError> {
    if bytes.len() < 2 {
        return Err(DecimalError::InvalidEncoding);
    }

    let encoded = ((bytes[0] as u16) << 8) | (bytes[1] as u16);
    let biased = if is_negative { !encoded } else { encoded };
    let exponent = (biased as i32) - EXPONENT_BIAS;

    Ok((exponent, 2))
}

/// Encodes the mantissa as BCD (2 digits per byte).
fn encode_mantissa(result: &mut Vec<u8>, digits: &[u8], is_negative: bool) {
    // Pack 2 digits per byte
    let mut i = 0;
    while i < digits.len() {
        let high = digits[i];
        let low = if i + 1 < digits.len() {
            digits[i + 1]
        } else {
            0 // Pad with 0 if odd number of digits
        };

        let byte = (high << 4) | low;

        // For negative numbers, invert to reverse the sort order
        result.push(if is_negative { !byte } else { byte });

        i += 2;
    }
}

/// Decodes the mantissa from BCD bytes.
fn decode_mantissa(bytes: &[u8], is_negative: bool) -> Result<Vec<u8>, DecimalError> {
    let mut digits = Vec::with_capacity(bytes.len() * 2);

    for &byte in bytes {
        let byte = if is_negative { !byte } else { byte };
        let high = (byte >> 4) & 0x0F;
        let low = byte & 0x0F;

        if high > 9 || low > 9 {
            return Err(DecimalError::InvalidEncoding);
        }

        digits.push(high);
        digits.push(low);
    }

    // Remove trailing zeros (padding)
    while digits.last() == Some(&0) && digits.len() > 1 {
        digits.pop();
    }

    Ok(digits)
}

/// Formats digits and exponent back to a decimal string.
fn format_decimal(is_negative: bool, digits: &[u8], exponent: i32) -> Result<String, DecimalError> {
    if digits.is_empty() {
        return Ok("0".to_string());
    }

    let mut result = String::new();

    if is_negative {
        result.push('-');
    }

    let num_digits = digits.len() as i32;

    if exponent >= num_digits {
        // All digits are before the decimal point (integer part)
        for d in digits {
            result.push(char::from_digit(*d as u32, 10).unwrap());
        }
        // Add trailing zeros if needed
        for _ in 0..(exponent - num_digits) {
            result.push('0');
        }
    } else if exponent <= 0 {
        // All digits are after the decimal point
        result.push('0');
        result.push('.');
        for _ in 0..(-exponent) {
            result.push('0');
        }
        for d in digits {
            result.push(char::from_digit(*d as u32, 10).unwrap());
        }
    } else {
        // Some digits before decimal, some after
        let decimal_pos = exponent as usize;
        for (i, d) in digits.iter().enumerate() {
            if i == decimal_pos {
                result.push('.');
            }
            result.push(char::from_digit(*d as u32, 10).unwrap());
        }
    }

    Ok(result)
}

/// Truncates a decimal string to fit precision and scale constraints.
fn truncate_decimal(
    value: &str,
    precision: Option<u32>,
    scale: Option<u32>,
) -> Result<String, DecimalError> {
    // Parse to get sign and parts
    let value = value.trim();
    let is_negative = value.starts_with('-');
    let value = value.trim_start_matches(['-', '+']);

    // Split into integer and fractional parts
    let (integer_part, fractional_part) = if let Some(dot_pos) = value.find('.') {
        (&value[..dot_pos], &value[dot_pos + 1..])
    } else {
        (value, "")
    };

    // Trim leading zeros from integer part (but keep at least one digit)
    let integer_part = integer_part.trim_start_matches('0');
    let integer_part = if integer_part.is_empty() {
        "0"
    } else {
        integer_part
    };

    // Apply scale constraint (truncate/round fractional part)
    let (mut integer_part, fractional_part) = if let Some(s) = scale {
        if (fractional_part.len() as u32) > s {
            // Round the last digit
            let truncated = &fractional_part[..s as usize];
            let next_digit = fractional_part.chars().nth(s as usize).unwrap_or('0');

            if next_digit >= '5' {
                // Round up - this may carry into integer part
                let rounded = round_up(truncated);
                if rounded.len() > s as usize {
                    // Carry into integer part
                    let new_int = add_one_to_integer(integer_part);
                    (new_int, "0".repeat(s as usize))
                } else {
                    (integer_part.to_string(), rounded)
                }
            } else {
                (integer_part.to_string(), truncated.to_string())
            }
        } else {
            (integer_part.to_string(), fractional_part.to_string())
        }
    } else {
        (integer_part.to_string(), fractional_part.to_string())
    };

    // Apply precision constraint
    if let Some(p) = precision {
        let scale_val = scale.unwrap_or(0);
        let max_integer_digits = if p > scale_val {
            (p - scale_val) as usize
        } else {
            0
        };

        if integer_part.len() > max_integer_digits && max_integer_digits > 0 {
            // Truncate from the left (keep least significant digits)
            integer_part = integer_part[integer_part.len() - max_integer_digits..].to_string();
        } else if max_integer_digits == 0 {
            integer_part = "0".to_string();
        }
    }

    // Reconstruct
    let result = if fractional_part.is_empty() || fractional_part.chars().all(|c| c == '0') {
        integer_part
    } else {
        format!("{}.{}", integer_part, fractional_part.trim_end_matches('0'))
    };

    if is_negative && result != "0" {
        Ok(format!("-{}", result))
    } else {
        Ok(result)
    }
}

/// Adds 1 to an integer string.
fn add_one_to_integer(s: &str) -> String {
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

/// Rounds up a digit string by adding 1 to the last digit.
fn round_up(s: &str) -> String {
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
        // All 9s became 0s, prepend 1
        format!("1{}", chars.iter().collect::<String>())
    } else {
        chars.iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_roundtrip() {
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
            "1000",
            "-0.001",
            "999999999999999999",
        ];

        for s in values {
            let encoded = encode_decimal(s).unwrap();
            let decoded = decode_to_string(&encoded).unwrap();
            // Re-encode to normalize
            let re_encoded = encode_decimal(&decoded).unwrap();
            assert_eq!(encoded, re_encoded, "Roundtrip failed for {}", s);
        }
    }

    #[test]
    fn test_lexicographic_ordering() {
        let values = vec![
            "-1000", "-100", "-10", "-1", "-0.1", "-0.01", "0", "0.01", "0.1", "1", "10", "100",
            "1000",
        ];

        let encoded: Vec<Vec<u8>> = values
            .iter()
            .map(|s| encode_decimal(s).unwrap())
            .collect();

        // Verify encoding preserves order
        for i in 0..encoded.len() - 1 {
            assert!(
                encoded[i] < encoded[i + 1],
                "Ordering failed: {} should be < {}",
                values[i],
                values[i + 1]
            );
        }
    }

    #[test]
    fn test_zero_encoding() {
        let encoded = encode_decimal("0").unwrap();
        assert_eq!(encoded, vec![SIGN_ZERO]);

        let encoded = encode_decimal("0.0").unwrap();
        assert_eq!(encoded, vec![SIGN_ZERO]);

        let encoded = encode_decimal("-0").unwrap();
        assert_eq!(encoded, vec![SIGN_ZERO]);
    }

    #[test]
    fn test_truncate_scale() {
        assert_eq!(truncate_decimal("123.456", None, Some(2)).unwrap(), "123.46");
        assert_eq!(truncate_decimal("123.454", None, Some(2)).unwrap(), "123.45");
        assert_eq!(truncate_decimal("123.995", None, Some(2)).unwrap(), "124");
        assert_eq!(truncate_decimal("9.999", None, Some(2)).unwrap(), "10");
    }

    #[test]
    fn test_storage_efficiency() {
        // 9 digit number: should be ~1 sign + 2 exp + 5 mantissa = 8 bytes
        let encoded = encode_decimal("123456789").unwrap();
        assert!(
            encoded.len() <= 8,
            "Expected <= 8 bytes, got {}",
            encoded.len()
        );

        // Small decimal
        let encoded = encode_decimal("0.1").unwrap();
        assert!(
            encoded.len() <= 4,
            "Expected <= 4 bytes, got {}",
            encoded.len()
        );
    }
}
