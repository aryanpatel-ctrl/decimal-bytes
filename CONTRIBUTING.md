# Contributing to decimal-bytes

Thanks for your interest in contributing to decimal-bytes!

## Prerequisites

- **Rust 1.85+** (the minimum supported Rust version)

## Development

```bash
cargo build                          # Default build
cargo build --features rust_decimal  # With rust_decimal interop
cargo build --features bigdecimal    # With bigdecimal interop
cargo test --all-features            # Run all tests
cargo bench                          # Run benchmarks
```

Before submitting a PR, make sure CI checks pass locally:

```bash
cargo fmt --all -- --check
cargo clippy --all-features -- -D warnings
cargo test --all-features
```

## Submitting Changes

1. Fork the repository and create a branch from `main`.
2. Add tests for any new functionality or bug fixes.
3. Run the checks above.
4. Open a pull request against `main`.

## Reporting Issues

Open an issue on [GitHub](https://github.com/paradedb/decimal-bytes/issues) with a clear description of the problem or feature request.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
