# decimal-bytes

Arbitrary precision decimals with lexicographically sortable byte encoding.

## Overview

This crate provides a `Decimal` type that stores decimal numbers as bytes in a format that preserves numerical ordering when compared lexicographically. This makes it ideal for use in databases and search engines where efficient range queries on decimal values are needed.

## Features

- **Bytes-first storage**: The primary representation is a compact byte array - no constant conversions
- **Lexicographic ordering**: Byte comparison matches numerical comparison
- **Arbitrary precision**: Supports numbers with many significant digits
- **SQL NUMERIC compatibility**: Supports precision and scale constraints

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

// Efficient byte access (primary representation)
let bytes: &[u8] = d.as_bytes();

// Reconstruct from bytes
let restored = Decimal::from_bytes(bytes).unwrap();
assert_eq!(d, restored);
```

## Storage Efficiency

The encoding is compact:
- 1 byte for sign
- 2 bytes for exponent  
- ~N/2 bytes for N-digit mantissa (BCD encoding: 2 digits per byte)

Example: A 9-digit number like `123456789` requires only ~8 bytes total.

## License

MIT
