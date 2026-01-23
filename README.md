# decimal-bytes

Arbitrary precision decimals with lexicographically sortable byte encoding.

## Overview

This crate provides a `Decimal` type that stores decimal numbers as bytes in a format that preserves numerical ordering when compared lexicographically. This makes it ideal for use in databases and search engines where efficient range queries on decimal values are needed.

## Features

- **Bytes-first storage**: The primary representation is a compact byte array - no constant conversions
- **Lexicographic ordering**: Byte comparison matches numerical comparison
- **Arbitrary precision**: Supports up to 131,072 digits before and 16,383 digits after the decimal point
- **PostgreSQL NUMERIC compatibility**: Full support for precision, scale (including negative), and special values
- **Special values**: Infinity, -Infinity, and NaN with correct PostgreSQL sort order

## Usage

```rust
use decimal_bytes::Decimal;

// Create decimals from strings
let a = Decimal::from_str("123.456").unwrap();
let b = Decimal::from_str("123.457").unwrap();

// Byte comparison matches numerical comparison
assert!(a.as_bytes() < b.as_bytes());
assert!(a < b);

// With precision and scale constraints (SQL NUMERIC semantics)
let d = Decimal::with_precision_scale("123.456", Some(10), Some(2)).unwrap();
assert_eq!(d.to_string(), "123.46"); // Rounded to 2 decimal places

// Negative scale (rounds to left of decimal point)
let d = Decimal::with_precision_scale("12345", Some(10), Some(-3)).unwrap();
assert_eq!(d.to_string(), "12000"); // Rounded to nearest 1000

// Efficient byte access (primary representation)
let bytes: &[u8] = d.as_bytes();

// Reconstruct from bytes
let restored = Decimal::from_bytes(bytes).unwrap();
assert_eq!(d, restored);
```

## Special Values

PostgreSQL-compatible special values with correct sort ordering:

```rust
use decimal_bytes::Decimal;

// Create special values
let pos_inf = Decimal::infinity();
let neg_inf = Decimal::neg_infinity();
let nan = Decimal::nan();

// Or parse from strings (case-insensitive)
let inf = Decimal::from_str("Infinity").unwrap();
let inf = Decimal::from_str("inf").unwrap();
let nan = Decimal::from_str("NaN").unwrap();

// Check for special values
assert!(pos_inf.is_infinity());
assert!(pos_inf.is_pos_infinity());
assert!(neg_inf.is_neg_infinity());
assert!(nan.is_nan());
assert!(!pos_inf.is_finite());

// Sort order: -Infinity < negatives < zero < positives < Infinity < NaN
assert!(neg_inf < Decimal::from_str("-1000000").unwrap());
assert!(Decimal::from_str("1000000").unwrap() < pos_inf);
assert!(pos_inf < nan);
```

## PostgreSQL Compatibility

This crate implements the PostgreSQL NUMERIC specification:

| Feature | Support |
|---------|---------|
| Max digits before decimal | 131,072 |
| Max digits after decimal | 16,383 |
| Precision constraint | ✓ |
| Scale constraint (positive) | ✓ |
| Scale constraint (negative) | ✓ |
| Infinity | ✓ |
| -Infinity | ✓ |
| NaN | ✓ |
| Rounding (ties away from zero) | ✓ |

## Storage Efficiency

The encoding matches PostgreSQL's storage efficiency (2 bytes per 4 decimal digits):

- 1 byte for sign
- 2 bytes for exponent  
- ~N/2 bytes for N-digit mantissa (BCD encoding: 2 digits per byte)
- Special values: 3 bytes each

Example: A 9-digit number like `123456789` requires only ~8 bytes total.

## Sort Order

The lexicographic byte order matches the PostgreSQL NUMERIC sort order:

```
-Infinity < negative numbers < zero < positive numbers < +Infinity < NaN
```

This enables efficient range queries in sorted key-value stores without decoding.

## License

MIT
