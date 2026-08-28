//! Coverage for exponents that do not fit the inline 2-byte exponent field.
//!
//! PostgreSQL accepts NUMERIC values up to 1e131071, far beyond what the inline
//! field can hold, so those values take an escaped form with a wider exponent.
//! The escaped form has to keep two guarantees: it must sort correctly against
//! every other encoding, and it must not disturb the bytes of values that
//! already fit inline, because those are already sitting in on-disk indexes.

use decimal_bytes::Decimal;
use proptest::prelude::*;
use std::str::FromStr;

/// The last exponent that still fits the inline field, expressed as the number
/// of zeros in `1e<n>`. `1e49147` is inline, `1e49148` is escaped.
const LAST_INLINE_ZEROS: usize = 49_147;

/// `1e131071` is the largest finite value PostgreSQL's NUMERIC can represent.
const MAX_PG_ZEROS: usize = 131_071;

/// Builds `1e<zeros>` in the plain digit form that PostgreSQL's `numeric_out`
/// produces, optionally negated.
fn pow10(zeros: usize, negative: bool) -> String {
    let mut s = String::with_capacity(zeros + 2);
    if negative {
        s.push('-');
    }
    s.push('1');
    for _ in 0..zeros {
        s.push('0');
    }
    s
}

fn encode(value: &str) -> Vec<u8> {
    Decimal::from_str(value)
        .unwrap_or_else(|e| panic!("failed to encode a {}-byte value: {e:?}", value.len()))
        .into_bytes()
}

#[test]
fn issue_6107_value_is_accepted() {
    // The exact value from the bug report, which previously hit PrecisionOverflow.
    let s = pow10(20_000, false);
    let d = Decimal::from_str(&s).expect("1e20000 should encode");
    assert_eq!(d.to_string(), s);
}

#[test]
fn covers_the_whole_postgres_numeric_range() {
    for zeros in [
        0,
        1,
        100,
        16_380,
        16_381,
        LAST_INLINE_ZEROS,
        49_148,
        100_000,
        MAX_PG_ZEROS,
    ] {
        for negative in [false, true] {
            let s = pow10(zeros, negative);
            let d = Decimal::from_str(&s)
                .unwrap_or_else(|e| panic!("1e{zeros} (negative={negative}) failed: {e:?}"));
            assert_eq!(
                d.to_string(),
                s,
                "1e{zeros} (negative={negative}) round-trip"
            );
        }
    }
}

#[test]
fn rejects_values_postgres_cannot_represent() {
    assert!(Decimal::from_str(&pow10(MAX_PG_ZEROS + 1, false)).is_err());
    assert!(Decimal::from_str(&pow10(MAX_PG_ZEROS + 1, true)).is_err());
}

#[test]
fn escaped_form_is_only_used_once_the_inline_field_is_exhausted() {
    let inline = encode(&pow10(LAST_INLINE_ZEROS, false));
    let escaped = encode(&pow10(LAST_INLINE_ZEROS + 1, false));

    // Sign byte + 2-byte exponent + mantissa, versus the same plus 4 more
    // exponent bytes.
    assert_eq!(escaped.len(), inline.len() + 4);
    assert!(inline < escaped);

    let inline_neg = encode(&pow10(LAST_INLINE_ZEROS, true));
    let escaped_neg = encode(&pow10(LAST_INLINE_ZEROS + 1, true));
    assert_eq!(escaped_neg.len(), inline_neg.len() + 4);
    assert!(escaped_neg < inline_neg);
}

#[test]
fn inline_encodings_are_byte_for_byte_unchanged() {
    // Captured from the crate before the escape marker existed. Any drift here
    // means existing on-disk indexes would stop decoding or stop sorting.
    let golden: &[(&str, &str)] = &[
        ("0", "80"),
        ("1", "ff400110"),
        ("-1", "00bffe89ff"),
        ("0.0001", "ff3ffd10"),
        ("-0.0001", "00c00289ff"),
        ("123.456", "ff4003123456"),
        ("-123.456", "00bffc876543ff"),
        ("9223372036854775807", "ff401392233720368547758070"),
        ("-9223372036854775808", "00bfec07766279631452241919ff"),
        ("1e100", "ff406510"),
        ("-1e100", "00bf9a89ff"),
        ("1e-16000", "ff018110"),
        ("-1e-16000", "00fe7e89ff"),
        ("1e16000", "ff7e8110"),
        ("-1e16000", "00817e89ff"),
        ("1e16380", "ff7ffd10"),
        ("-1e16380", "00800289ff"),
        ("Infinity", "fffffe"),
        ("-Infinity", "000000"),
        ("NaN", "ffffff"),
    ];

    for (value, expected) in golden {
        let hex: String = encode(value).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(&hex, expected, "encoding of {value} changed");
    }
}

#[test]
fn escaped_values_sort_between_inline_values_and_infinity() {
    // Ascending by value. Sorting the encodings must reproduce this exact order.
    let ordered = [
        "-Infinity".to_string(),
        pow10(MAX_PG_ZEROS, true),
        pow10(100_000, true),
        pow10(LAST_INLINE_ZEROS + 1, true),
        pow10(LAST_INLINE_ZEROS, true),
        pow10(16_380, true),
        pow10(100, true),
        "-1".to_string(),
        "-0.0001".to_string(),
        "0".to_string(),
        "0.0001".to_string(),
        "1".to_string(),
        pow10(100, false),
        pow10(16_380, false),
        pow10(LAST_INLINE_ZEROS, false),
        pow10(LAST_INLINE_ZEROS + 1, false),
        pow10(100_000, false),
        pow10(MAX_PG_ZEROS, false),
        "Infinity".to_string(),
        "NaN".to_string(),
    ];

    let encoded: Vec<Vec<u8>> = ordered.iter().map(|v| encode(v)).collect();

    for (i, pair) in encoded.windows(2).enumerate() {
        assert!(
            pair[0] < pair[1],
            "encoding at position {i} does not sort below position {}",
            i + 1
        );
    }

    let mut shuffled: Vec<usize> = (0..encoded.len()).collect();
    shuffled.sort_by(|&a, &b| encoded[a].cmp(&encoded[b]));
    assert_eq!(
        shuffled,
        (0..encoded.len()).collect::<Vec<_>>(),
        "sorting by encoded bytes did not reproduce numeric order"
    );
}

#[test]
fn malformed_escaped_exponents_are_rejected() {
    // Positive escape marker with a truncated extended exponent.
    assert!(Decimal::from_bytes(&[0xff, 0xff, 0xfd]).is_err());

    // The escape form must not be used for an inline exponent or for a value
    // outside PostgreSQL's supported range.
    assert!(Decimal::from_bytes(&[0xff, 0xff, 0xfd, 0, 0, 0, 1, 0x10]).is_err());
    assert!(Decimal::from_bytes(&[0xff, 0xff, 0xfd, 0, 2, 0, 1, 0x10]).is_err());

    // Negative escaped exponents are complemented on the wire as well.
    assert!(Decimal::from_bytes(&[0x00, 0x00, 0x02, 0xff, 0xff, 0xff, 0xff, 0xef]).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// For any two powers of ten across the inline/escaped boundary, byte order
    /// must agree with numeric order for both signs.
    #[test]
    fn byte_order_matches_numeric_order(
        a in 0usize..=MAX_PG_ZEROS,
        b in 0usize..=MAX_PG_ZEROS,
        negative in any::<bool>(),
    ) {
        let ea = encode(&pow10(a, negative));
        let eb = encode(&pow10(b, negative));

        // Larger exponent means a larger value when positive, and a smaller
        // value when negative.
        let expected = if negative { b.cmp(&a) } else { a.cmp(&b) };
        prop_assert_eq!(ea.cmp(&eb), expected);
    }

    /// Every value in range must survive a round trip unchanged.
    #[test]
    fn round_trips(zeros in 0usize..=MAX_PG_ZEROS, negative in any::<bool>()) {
        let s = pow10(zeros, negative);
        let d = Decimal::from_str(&s).map_err(|e| TestCaseError::fail(format!("{e:?}")))?;
        prop_assert_eq!(d.to_string(), s);
    }

    /// Mantissas longer than one digit must also survive, since the escaped
    /// exponent sits directly in front of the mantissa bytes.
    #[test]
    fn round_trips_with_multi_digit_mantissa(
        zeros in 49_140usize..=49_160,
        mantissa in 1u64..=999_999,
        negative in any::<bool>(),
    ) {
        let mut s = String::new();
        if negative {
            s.push('-');
        }
        let digits = mantissa.to_string();
        let digits = digits.trim_end_matches('0');
        let digits = if digits.is_empty() { "0" } else { digits };
        s.push_str(digits);
        for _ in 0..zeros {
            s.push('0');
        }

        let d = Decimal::from_str(&s).map_err(|e| TestCaseError::fail(format!("{e:?}")))?;
        prop_assert_eq!(d.to_string(), s);
    }
}
