//! Criterion benchmarks for keplor-pricing catalog lookup.

#![allow(clippy::unwrap_used)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use keplor_pricing::{Catalog, ModelKey};

fn bench_catalog_lookup(c: &mut Criterion) {
    let catalog = Catalog::load_bundled().unwrap();

    let mut group = c.benchmark_group("catalog_lookup");

    // Exact match (most common case).
    let exact = ModelKey::new("gpt-4o");
    group.bench_function("exact_match", |b| {
        b.iter(|| black_box(catalog.lookup(&exact)));
    });

    // Date-suffix fallback.
    let dated = ModelKey::new("gpt-4o-2024-08-06");
    group.bench_function("date_fallback", |b| {
        b.iter(|| black_box(catalog.lookup(&dated)));
    });

    // Provider-prefix fallback (must try multiple prefixes).
    let prefixed = ModelKey::new("claude-sonnet-4-20250514");
    group.bench_function("prefix_fallback", |b| {
        b.iter(|| black_box(catalog.lookup(&prefixed)));
    });

    // Complete miss (worst case: all 12 prefixes, no hit).
    let miss = ModelKey::new("nonexistent-model-xyz-9999");
    group.bench_function("miss_worst_case", |b| {
        b.iter(|| black_box(catalog.lookup(&miss)));
    });

    group.finish();
}

fn bench_catalog_load(c: &mut Criterion) {
    c.bench_function("catalog_load_bundled", |b| {
        b.iter(|| black_box(Catalog::load_bundled().unwrap()));
    });
}

fn bench_model_key_new(c: &mut Criterion) {
    c.bench_function("model_key_new", |b| {
        b.iter(|| black_box(ModelKey::new("gpt-4o-2024-08-06")));
    });
}

criterion_group!(benches, bench_catalog_lookup, bench_catalog_load, bench_model_key_new);
criterion_main!(benches);
