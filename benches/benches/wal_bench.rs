//! WAL write throughput benchmark.

use criterion::{criterion_group, criterion_main, Criterion};

fn bench_wal(c: &mut Criterion) {
    c.bench_function("wal_write_placeholder", |b| {
        b.iter(|| {
            let _ = sqlite3::Connection::open_in_memory();
            // TODO: WAL-mode benchmark once implemented.
        })
    });
}

criterion_group!(benches, bench_wal);
criterion_main!(benches);
