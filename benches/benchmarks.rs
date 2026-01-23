use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use decimal_bytes::Decimal;
use std::str::FromStr;

/// Sample decimal strings of varying complexity
const SMALL_INT: &str = "42";
const MEDIUM_INT: &str = "123456789";
const LARGE_INT: &str = "123456789012345678901234567890";
const SMALL_DECIMAL: &str = "3.14";
const MEDIUM_DECIMAL: &str = "123456.789012";
const LARGE_DECIMAL: &str = "123456789.012345678901234567890123456789";
const SCIENTIFIC: &str = "1.23456789e15";
const NEGATIVE: &str = "-987654321.123456789";

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");

    let cases = [
        ("small_int", SMALL_INT),
        ("medium_int", MEDIUM_INT),
        ("large_int", LARGE_INT),
        ("small_decimal", SMALL_DECIMAL),
        ("medium_decimal", MEDIUM_DECIMAL),
        ("large_decimal", LARGE_DECIMAL),
        ("scientific", SCIENTIFIC),
        ("negative", NEGATIVE),
    ];

    for (name, input) in cases {
        group.throughput(Throughput::Bytes(input.len() as u64));
        group.bench_with_input(BenchmarkId::new("from_str", name), input, |b, s| {
            b.iter(|| Decimal::from_str(black_box(s)).unwrap())
        });
    }

    group.finish();
}

fn bench_to_string(c: &mut Criterion) {
    let mut group = c.benchmark_group("to_string");

    let cases = [
        ("small_int", SMALL_INT),
        ("medium_int", MEDIUM_INT),
        ("large_int", LARGE_INT),
        ("small_decimal", SMALL_DECIMAL),
        ("medium_decimal", MEDIUM_DECIMAL),
        ("large_decimal", LARGE_DECIMAL),
        ("negative", NEGATIVE),
    ];

    for (name, input) in cases {
        let decimal = Decimal::from_str(input).unwrap();
        group.bench_with_input(BenchmarkId::new("to_string", name), &decimal, |b, d| {
            b.iter(|| black_box(d).to_string())
        });
    }

    group.finish();
}

fn bench_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("comparison");

    let a = Decimal::from_str("123456.789").unwrap();
    let b = Decimal::from_str("123456.790").unwrap();
    let c_val = Decimal::from_str("123456.789").unwrap();

    group.bench_function("cmp_less", |bench| {
        bench.iter(|| black_box(&a) < black_box(&b))
    });

    group.bench_function("cmp_equal", |bench| {
        bench.iter(|| black_box(&a) == black_box(&c_val))
    });

    // Compare bytes directly (the key use case)
    group.bench_function("cmp_bytes", |bench| {
        bench.iter(|| black_box(a.as_bytes()) < black_box(b.as_bytes()))
    });

    group.finish();
}

fn bench_special_values(c: &mut Criterion) {
    let mut group = c.benchmark_group("special_values");

    group.bench_function("create_infinity", |b| b.iter(|| Decimal::infinity()));

    group.bench_function("create_nan", |b| b.iter(|| Decimal::nan()));

    group.bench_function("parse_infinity", |b| {
        b.iter(|| Decimal::from_str(black_box("Infinity")).unwrap())
    });

    group.bench_function("parse_nan", |b| {
        b.iter(|| Decimal::from_str(black_box("NaN")).unwrap())
    });

    let inf = Decimal::infinity();
    let nan = Decimal::nan();

    group.bench_function("is_infinity", |b| b.iter(|| black_box(&inf).is_infinity()));

    group.bench_function("is_nan", |b| b.iter(|| black_box(&nan).is_nan()));

    group.finish();
}

fn bench_precision_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("precision_scale");

    group.bench_function("with_precision_scale", |b| {
        b.iter(|| {
            Decimal::with_precision_scale(black_box("123.456789"), Some(10), Some(2)).unwrap()
        })
    });

    group.bench_function("negative_scale", |b| {
        b.iter(|| Decimal::with_precision_scale(black_box("123456"), Some(10), Some(-3)).unwrap())
    });

    group.finish();
}

fn bench_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization");

    let decimal = Decimal::from_str("123456.789012345").unwrap();
    let json = serde_json::to_string(&decimal).unwrap();

    group.bench_function("serialize_json", |b| {
        b.iter(|| serde_json::to_string(black_box(&decimal)).unwrap())
    });

    group.bench_function("deserialize_json", |b| {
        b.iter(|| serde_json::from_str::<Decimal>(black_box(&json)).unwrap())
    });

    group.finish();
}

fn bench_from_bytes(c: &mut Criterion) {
    let mut group = c.benchmark_group("from_bytes");

    let cases = [
        ("small", SMALL_INT),
        ("medium", MEDIUM_DECIMAL),
        ("large", LARGE_DECIMAL),
    ];

    for (name, input) in cases {
        let decimal = Decimal::from_str(input).unwrap();
        let bytes = decimal.as_bytes().to_vec();

        group.bench_with_input(BenchmarkId::new("from_bytes", name), &bytes, |b, bytes| {
            b.iter(|| Decimal::from_bytes(black_box(bytes)).unwrap())
        });

        group.bench_with_input(
            BenchmarkId::new("from_bytes_unchecked", name),
            &bytes,
            |b, bytes| b.iter(|| Decimal::from_bytes_unchecked(black_box(bytes.clone()))),
        );
    }

    group.finish();
}

fn bench_batch_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch");

    // Simulate sorting a batch of decimals (common database operation)
    let inputs: Vec<&str> = vec![
        "100.5", "-50.25", "0", "999.999", "-0.001", "1e10", "42", "-1e5", "3.14159", "2.71828",
    ];

    let decimals: Vec<Decimal> = inputs
        .iter()
        .map(|s| Decimal::from_str(s).unwrap())
        .collect();

    group.bench_function("sort_10_decimals", |b| {
        b.iter(|| {
            let mut d = decimals.clone();
            d.sort();
            black_box(d)
        })
    });

    // Batch parsing
    group.throughput(Throughput::Elements(inputs.len() as u64));
    group.bench_function("parse_10_decimals", |b| {
        b.iter(|| {
            inputs
                .iter()
                .map(|s| Decimal::from_str(black_box(s)).unwrap())
                .collect::<Vec<_>>()
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_parse,
    bench_to_string,
    bench_comparison,
    bench_special_values,
    bench_precision_scale,
    bench_serialization,
    bench_from_bytes,
    bench_batch_operations,
);

criterion_main!(benches);
