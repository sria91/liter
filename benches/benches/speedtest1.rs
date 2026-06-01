//! Port of SQLite's `speedtest1.c` benchmark.
//!
//! Each bench function corresponds to a numbered test in the original C
//! program.  The scale constant controls rows-per-iteration; criterion's
//! `Throughput` API then reports the normalised rows/second figure so the
//! numbers can be compared directly against C SQLite even at different scales.
//!
//! Test mapping
//! ─────────────────────────────────────────────────────────────────────────
//!  t100  – bulk INSERT, no index, single transaction
//!  t110  – bulk INSERT, with index, single transaction
//!  t120  – INSERT one row per auto-committed statement (no explicit txn)
//!  t130  – SELECT COUNT(*) full-table scan
//!  t140  – SELECT … WHERE (equality on unindexed column)
//!  t150  – DELETE upper-half of rows (no index)
//!  t160  – UPDATE every row (no index)

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use liter::Connection;

/// Number of rows inserted in each timed iteration.
/// The original speedtest1 uses 25 000; we default to 1 000 so that criterion
/// can collect enough samples in a reasonable wall-clock time.  Set the
/// `SPEEDTEST_N` environment variable at bench-time to override.
fn scale() -> usize {
    std::env::var("SPEEDTEST_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000)
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Create an empty in-memory DB with table `t1 (a INTEGER, b INTEGER, c TEXT)`.
fn empty_t1() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE t1 (a INTEGER, b INTEGER, c TEXT)",
        [] as [(); 0],
    )
    .unwrap();
    conn
}

/// Create an in-memory DB with `t1` pre-populated with `n` rows.
fn populated_t1(n: usize) -> Connection {
    let conn = empty_t1();
    conn.execute("BEGIN", [] as [(); 0]).unwrap();
    for i in 1..=(n as i64) {
        conn.execute(
            &format!("INSERT INTO t1 VALUES ({}, {}, 'abcdefghij')", i, i * 2),
            [] as [(); 0],
        )
        .unwrap();
    }
    conn.execute("COMMIT", [] as [(); 0]).unwrap();
    conn
}

// ── Test 100 – bulk INSERT, no index, single transaction ────────────────────

fn bench_t100_insert_no_index(c: &mut Criterion) {
    let n = scale();
    let mut group = c.benchmark_group("t100_insert_no_index");
    group.throughput(Throughput::Elements(n as u64));
    group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
        b.iter_with_setup(empty_t1, |conn| {
            conn.execute("BEGIN", [] as [(); 0]).unwrap();
            for i in 1..=(n as i64) {
                conn.execute(
                    &format!("INSERT INTO t1 VALUES ({}, {}, 'abcdefghij')", i, i * 2),
                    [] as [(); 0],
                )
                .unwrap();
            }
            conn.execute("COMMIT", [] as [(); 0]).unwrap();
            criterion::black_box(conn)
        });
    });
    group.finish();
}

// ── Test 110 – bulk INSERT with index, single transaction ───────────────────

fn bench_t110_insert_with_index(c: &mut Criterion) {
    let n = scale();

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE t2 (a INTEGER, b INTEGER, c TEXT)",
            [] as [(); 0],
        )
        .unwrap();
        conn
    }

    let mut group = c.benchmark_group("t110_insert_with_index");
    group.throughput(Throughput::Elements(n as u64));
    group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
        b.iter_with_setup(setup, |conn| {
            conn.execute("BEGIN", [] as [(); 0]).unwrap();
            for i in 1..=(n as i64) {
                conn.execute(
                    &format!("INSERT INTO t2 VALUES ({}, {}, 'abcdefghij')", i, i * 2),
                    [] as [(); 0],
                )
                .unwrap();
            }
            conn.execute("COMMIT", [] as [(); 0]).unwrap();
            criterion::black_box(conn)
        });
    });
    group.finish();
}

// ── Test 120 – INSERT one row per auto-committed statement ──────────────────

fn bench_t120_insert_autocommit(c: &mut Criterion) {
    let n = scale();
    let mut group = c.benchmark_group("t120_insert_autocommit");
    group.throughput(Throughput::Elements(n as u64));
    group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
        b.iter_with_setup(empty_t1, |conn| {
            for i in 1..=(n as i64) {
                conn.execute(
                    &format!("INSERT INTO t1 VALUES ({}, {}, 'abcdefghij')", i, i * 2),
                    [] as [(); 0],
                )
                .unwrap();
            }
            criterion::black_box(conn)
        });
    });
    group.finish();
}

// ── Test 130 – SELECT COUNT(*) full-table scan ───────────────────────────────

fn bench_t130_select_count(c: &mut Criterion) {
    let n = scale();
    let conn = populated_t1(n);
    let mut group = c.benchmark_group("t130_select_count");
    group.throughput(Throughput::Elements(n as u64));
    group.bench_function(BenchmarkId::from_parameter(n), |b| {
        b.iter(|| {
            let rows = conn
                .query("SELECT COUNT(*) FROM t1", [] as [(); 0])
                .unwrap();
            criterion::black_box(rows)
        });
    });
    group.finish();
}

// ── Test 140 – SELECT … WHERE (unindexed equality) ───────────────────────────

fn bench_t140_select_where(c: &mut Criterion) {
    let n = scale();
    let conn = populated_t1(n);
    // Search for the middle row so we exercise a realistic scan depth.
    let target = (n / 2) as i64 * 2; // b = a * 2 in populated_t1
    let sql = format!("SELECT a, b, c FROM t1 WHERE b = {}", target);
    let mut group = c.benchmark_group("t140_select_where");
    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::from_parameter(n), |b| {
        b.iter(|| {
            let rows = conn.query(&sql, [] as [(); 0]).unwrap();
            criterion::black_box(rows)
        });
    });
    group.finish();
}

// ── Test 150 – DELETE upper-half of rows (no index) ─────────────────────────

fn bench_t150_delete(c: &mut Criterion) {
    let n = scale();
    let half = (n / 2) as i64;
    let mut group = c.benchmark_group("t150_delete");
    group.throughput(Throughput::Elements(half as u64));
    group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
        b.iter_with_setup(
            || populated_t1(n),
            |conn| {
                conn.execute("BEGIN", [] as [(); 0]).unwrap();
                conn.execute(&format!("DELETE FROM t1 WHERE a > {}", half), [] as [(); 0])
                    .unwrap();
                conn.execute("COMMIT", [] as [(); 0]).unwrap();
                criterion::black_box(conn)
            },
        );
    });
    group.finish();
}

// ── Test 160 – UPDATE every row (no index) ──────────────────────────────────

fn bench_t160_update(c: &mut Criterion) {
    let n = scale();
    let mut group = c.benchmark_group("t160_update");
    group.throughput(Throughput::Elements(n as u64));
    group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
        b.iter_with_setup(
            || populated_t1(n),
            |conn| {
                conn.execute("BEGIN", [] as [(); 0]).unwrap();
                conn.execute("UPDATE t1 SET c = 'xxxxxxxxxx'", [] as [(); 0])
                    .unwrap();
                conn.execute("COMMIT", [] as [(); 0]).unwrap();
                criterion::black_box(conn)
            },
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_t100_insert_no_index,
    bench_t110_insert_with_index,
    bench_t120_insert_autocommit,
    bench_t130_select_count,
    bench_t140_select_where,
    bench_t150_delete,
    bench_t160_update,
);
criterion_main!(benches);
