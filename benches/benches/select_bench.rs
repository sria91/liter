//! SELECT throughput benchmark.

use criterion::{criterion_group, criterion_main, Criterion};

fn bench_select(c: &mut Criterion) {
    c.bench_function("select_placeholder", |b| {
        b.iter(|| {
            let _ = sqlite3::Connection::open_in_memory();
            // TODO: full-table scan benchmark once implemented.
        })
    });
}

criterion_group!(benches, bench_select);
criterion_main!(benches);
