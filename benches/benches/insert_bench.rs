//! INSERT throughput benchmark.

use criterion::{criterion_group, criterion_main, Criterion};

fn bench_insert(c: &mut Criterion) {
    c.bench_function("insert_1k_placeholder", |b| {
        b.iter(|| {
            let _ = sqlite3::Connection::open_in_memory();
            // TODO: execute 1000 INSERTs once implemented.
        })
    });
}

criterion_group!(benches, bench_insert);
criterion_main!(benches);
