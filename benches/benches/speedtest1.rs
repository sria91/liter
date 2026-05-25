//! Port of SQLite's `speedtest1.c` benchmark.
//!
//! This benchmark mirrors the official speedtest1 workload and is used to
//! track performance parity with C SQLite.

use criterion::{criterion_group, criterion_main, Criterion};

fn bench_speedtest1(c: &mut Criterion) {
    c.bench_function("speedtest1_placeholder", |b| {
        b.iter(|| {
            // TODO: wire up once Connection::execute is implemented.
            let _ = liter::Connection::open_in_memory();
        })
    });
}

criterion_group!(benches, bench_speedtest1);
criterion_main!(benches);
