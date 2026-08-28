//! Differential and stress coverage for the escaped-exponent encoding.
//!
//! The tests in `large_exponent.rs` pin down specific values. These ones try to
//! break the encoding instead: they generate values across PostgreSQL's whole
//! NUMERIC range and check the encoding against two independent oracles.
//!
//! The primary oracle never looks at the encoding. Values are built from the
//! parts that define them (sign, significant digits, power of ten), and the
//! expected order is derived from those parts directly: for two normalized
//! positive values, the one with the larger normalized exponent is larger, and
//! on a tie the digit strings compare lexicographically, because both are read
//! as the fraction 0.<digits>. Negatives invert that. `BigDecimal` acts as a
//! second opinion over a narrower range, where aligning scales stays cheap.

use bigdecimal::BigDecimal;
use decimal_bytes::Decimal;
use proptest::prelude::*;
use std::cmp::Ordering;
use std::str::FromStr;

/// Widest normalized exponent the encoding accepts, matching PostgreSQL's limit
/// of 131072 digits to the left of the decimal point.
const MAX_NORMALIZED_EXPONENT: i64 = 131_072;

/// Smallest normalized exponent, matching PostgreSQL's 16383 digits to the
/// right of the decimal point.
const MIN_NORMALIZED_EXPONENT: i64 = -16_383;

/// Last normalized exponent that still fits the inline 2-byte exponent field.
/// Values above this take the escaped form.
const LAST_INLINE_EXPONENT: i64 = 49_148;

/// A value expressed the way the encoding thinks of it: `sign * 0.<digits> *
/// 10^normalized_exponent`, with `digits` carrying no leading or trailing zero.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Value {
    negative: bool,
    digits: String,
    normalized_exponent: i64,
}

impl Value {
    /// Renders as scientific notation, which every parser here accepts and
    /// which avoids materializing 131072-character strings.
    fn to_scientific(&self) -> String {
        format!(
            "{}0.{}e{}",
            if self.negative { "-" } else { "" },
            self.digits,
            self.normalized_exponent
        )
    }

    /// The mathematical ordering, derived only from the parts, never from bytes.
    fn cmp_mathematically(&self, other: &Self) -> Ordering {
        let magnitude = self
            .normalized_exponent
            .cmp(&other.normalized_exponent)
            .then_with(|| self.digits.cmp(&other.digits));

        match (self.negative, other.negative) {
            (false, false) => magnitude,
            (true, true) => magnitude.reverse(),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        }
    }

    fn encode(&self) -> Vec<u8> {
        Decimal::from_str(&self.to_scientific())
            .unwrap_or_else(|e| panic!("{} failed to encode: {e:?}", self.to_scientific()))
            .into_bytes()
    }
}

/// Digit strings with no leading or trailing zero, so every generated value is
/// already in the normalized form the encoder produces.
fn digits_strategy(max_len: usize) -> impl Strategy<Value = String> {
    (1..=max_len).prop_flat_map(|len| {
        if len == 1 {
            prop::collection::vec(1u8..=9, 1).boxed()
        } else {
            (1u8..=9, prop::collection::vec(0u8..=9, len - 2), 1u8..=9)
                .prop_map(|(first, middle, last)| {
                    let mut v = vec![first];
                    v.extend(middle);
                    v.push(last);
                    v
                })
                .boxed()
        }
        .prop_map(|ds| ds.into_iter().map(|d| (b'0' + d) as char).collect())
    })
}

fn value_strategy(
    exponent_range: std::ops::RangeInclusive<i64>,
    max_digits: usize,
) -> impl Strategy<Value = Value> {
    (any::<bool>(), digits_strategy(max_digits), exponent_range).prop_map(
        |(negative, digits, normalized_exponent)| Value {
            negative,
            digits,
            normalized_exponent,
        },
    )
}

/// Spans the full range, so most draws land in the escaped region while a
/// meaningful share stay inline.
fn any_value() -> impl Strategy<Value = Value> {
    value_strategy(MIN_NORMALIZED_EXPONENT..=MAX_NORMALIZED_EXPONENT, 40)
}

/// Concentrates draws within a few exponents of the inline/escaped transition,
/// which is where an ordering bug would hide.
fn boundary_value() -> impl Strategy<Value = Value> {
    value_strategy(LAST_INLINE_EXPONENT - 3..=LAST_INLINE_EXPONENT + 3, 40)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    /// Byte order must equal mathematical order for any two values anywhere in
    /// PostgreSQL's range.
    #[test]
    fn byte_order_matches_mathematical_order(a in any_value(), b in any_value()) {
        prop_assert_eq!(a.encode().cmp(&b.encode()), a.cmp_mathematically(&b));
    }

    /// Same property, but with both values crowded around the escape boundary.
    #[test]
    fn byte_order_holds_across_the_escape_boundary(a in boundary_value(), b in boundary_value()) {
        prop_assert_eq!(a.encode().cmp(&b.encode()), a.cmp_mathematically(&b));
    }

    /// One value inline and one escaped, in every sign combination.
    #[test]
    fn inline_versus_escaped_order(
        inline in value_strategy(MIN_NORMALIZED_EXPONENT..=LAST_INLINE_EXPONENT, 40),
        escaped in value_strategy(LAST_INLINE_EXPONENT + 1..=MAX_NORMALIZED_EXPONENT, 40),
    ) {
        prop_assert_eq!(
            inline.encode().cmp(&escaped.encode()),
            inline.cmp_mathematically(&escaped)
        );
    }

    /// Equal values must produce identical bytes, so equality lookups work.
    #[test]
    fn equal_values_encode_identically(v in any_value()) {
        prop_assert_eq!(v.encode(), v.encode());
        // The same number written with a different exponent/digit split must
        // still collapse to one encoding.
        let shifted = Value {
            negative: v.negative,
            digits: format!("{}0", v.digits),
            normalized_exponent: v.normalized_exponent,
        };
        prop_assert_eq!(v.encode(), shifted.encode());
    }

    /// Encodings must never collide for values that differ.
    #[test]
    fn distinct_values_never_collide(a in any_value(), b in any_value()) {
        prop_assume!(a.cmp_mathematically(&b) != Ordering::Equal);
        prop_assert_ne!(a.encode(), b.encode());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(192))]

    /// Round trip through the decoder, checked against the value's own parts.
    #[test]
    fn round_trips_anywhere_in_range(v in any_value()) {
        let decoded = Decimal::from_str(&v.to_scientific())
            .map_err(|e| TestCaseError::fail(format!("{e:?}")))?
            .to_string();
        let reparsed = Decimal::from_str(&decoded)
            .map_err(|e| TestCaseError::fail(format!("{e:?}")))?;
        prop_assert_eq!(reparsed.into_bytes(), v.encode());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Second opinion from BigDecimal, over a window where scale alignment does
    /// not blow up into six-figure-digit integers.
    #[test]
    fn agrees_with_bigdecimal(
        a in value_strategy(49_100..=49_200, 24),
        b in value_strategy(49_100..=49_200, 24),
    ) {
        let (sa, sb) = (a.to_scientific(), b.to_scientific());
        let ba = BigDecimal::from_str(&sa).map_err(|e| TestCaseError::fail(format!("{e:?}")))?;
        let bb = BigDecimal::from_str(&sb).map_err(|e| TestCaseError::fail(format!("{e:?}")))?;
        prop_assert_eq!(a.encode().cmp(&b.encode()), ba.cmp(&bb));
    }
}

/// Walks every exponent in a window straddling the escape boundary and checks
/// the whole sequence is strictly increasing, for both signs.
#[test]
fn exhaustive_sweep_across_the_escape_boundary() {
    for negative in [false, true] {
        let values: Vec<Value> = (LAST_INLINE_EXPONENT - 40..=LAST_INLINE_EXPONENT + 40)
            .map(|normalized_exponent| Value {
                negative,
                digits: "1234567890123456789".to_string(),
                normalized_exponent,
            })
            .collect();

        let mut sorted = values.clone();
        sorted.sort_by(|a, b| a.cmp_mathematically(b));

        let mut by_bytes = values.clone();
        by_bytes.sort_by_key(|v| v.encode());

        assert_eq!(by_bytes, sorted, "sweep mismatch (negative={negative})");
    }
}

/// The extremes of PostgreSQL's range, including full-width mantissas.
#[test]
fn extremes_round_trip_and_order() {
    let widest_integer: String = (0..131_072)
        .map(|i| (b'1' + (i % 9) as u8) as char)
        .collect();
    let widest_full = {
        let mut s = widest_integer.clone();
        s.push('.');
        s.extend((0..16_383).map(|i| (b'1' + (i % 9) as u8) as char));
        s
    };

    for input in [&widest_integer, &widest_full] {
        let d = Decimal::from_str(input).expect("widest PostgreSQL value should encode");
        assert_eq!(&d.to_string(), input, "widest value did not round-trip");
    }

    // 147455 significant digits is the most PostgreSQL can hold, and it must
    // still sort above every shorter value with the same leading digits.
    let smaller = Decimal::from_str(&widest_integer).unwrap().into_bytes();
    let larger = Decimal::from_str(&widest_full).unwrap().into_bytes();
    assert!(smaller < larger);
}

/// Anything past PostgreSQL's own limits must be refused rather than silently
/// wrapping into a wrong encoding.
#[test]
fn out_of_range_is_rejected() {
    let cases = [
        format!("0.1e{}", MAX_NORMALIZED_EXPONENT + 1),
        format!("-0.1e{}", MAX_NORMALIZED_EXPONENT + 1),
        format!("0.1e{}", MIN_NORMALIZED_EXPONENT - 1),
        format!("-0.1e{}", MIN_NORMALIZED_EXPONENT - 1),
        "1e999999999".to_string(),
        "-1e999999999".to_string(),
    ];
    for case in cases {
        assert!(
            Decimal::from_str(&case).is_err(),
            "{case} should have been rejected"
        );
    }
}

/// Special values must keep their positions now that escaped encodings sit
/// between the ordinary numbers and the infinities.
#[test]
fn specials_still_bound_the_escaped_range() {
    let neg_inf = Decimal::from_str("-Infinity").unwrap().into_bytes();
    let pos_inf = Decimal::from_str("Infinity").unwrap().into_bytes();
    let nan = Decimal::from_str("NaN").unwrap().into_bytes();

    let biggest = Decimal::from_str(&format!("0.9e{MAX_NORMALIZED_EXPONENT}"))
        .unwrap()
        .into_bytes();
    let smallest = Decimal::from_str(&format!("-0.9e{MAX_NORMALIZED_EXPONENT}"))
        .unwrap()
        .into_bytes();

    assert!(neg_inf < smallest, "-Infinity must sort below every value");
    assert!(biggest < pos_inf, "+Infinity must sort above every value");
    assert!(pos_inf < nan, "NaN must sort last, as in PostgreSQL");
}
