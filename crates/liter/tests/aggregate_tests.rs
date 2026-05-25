use liter::{Connection, Value};

#[test]
fn test_simple_aggregate() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE t1 (a INTEGER, b REAL)", [])
        .unwrap();
    conn.execute("INSERT INTO t1 VALUES (1, 10.5)", []).unwrap();
    conn.execute("INSERT INTO t1 VALUES (2, 20.0)", []).unwrap();
    conn.execute("INSERT INTO t1 VALUES (3, NULL)", []).unwrap();

    let res = conn
        .query(
            "SELECT COUNT(*), SUM(a), AVG(b), MIN(a), MAX(b) FROM t1",
            [],
        )
        .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(
        res[0],
        vec![
            Value::Int(3),
            Value::Int(6),
            Value::Real(15.25), // (10.5 + 20) / 2
            Value::Int(1),
            Value::Real(20.0),
        ]
    );
}

#[test]
fn test_group_by() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE t1 (grp TEXT, val INTEGER)", [])
        .unwrap();
    conn.execute("INSERT INTO t1 VALUES ('A', 10)", []).unwrap();
    conn.execute("INSERT INTO t1 VALUES ('A', 20)", []).unwrap();
    conn.execute("INSERT INTO t1 VALUES ('B', 5)", []).unwrap();
    conn.execute("INSERT INTO t1 VALUES ('B', 15)", []).unwrap();
    conn.execute("INSERT INTO t1 VALUES ('C', 100)", [])
        .unwrap();

    // Grouping by grp.
    let res = conn
        .query("SELECT grp, SUM(val), COUNT(*) FROM t1 GROUP BY grp", [])
        .unwrap();

    // We expect 3 rows (A, B, C), sorted since Sorter groups them.
    assert_eq!(res.len(), 3);
    assert_eq!(
        res[0],
        vec![Value::Text(b"A".to_vec()), Value::Int(30), Value::Int(2)]
    );
    assert_eq!(
        res[1],
        vec![Value::Text(b"B".to_vec()), Value::Int(20), Value::Int(2)]
    );
    assert_eq!(
        res[2],
        vec![Value::Text(b"C".to_vec()), Value::Int(100), Value::Int(1)]
    );
}

#[test]
fn test_having() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE t1 (grp TEXT, val INTEGER)", [])
        .unwrap();
    conn.execute("INSERT INTO t1 VALUES ('A', 10)", []).unwrap();
    conn.execute("INSERT INTO t1 VALUES ('A', 20)", []).unwrap();
    conn.execute("INSERT INTO t1 VALUES ('B', 5)", []).unwrap();
    conn.execute("INSERT INTO t1 VALUES ('B', 15)", []).unwrap();
    conn.execute("INSERT INTO t1 VALUES ('C', 100)", [])
        .unwrap();

    // Having sum(val) > 25
    let res = conn
        .query(
            "SELECT grp, SUM(val) FROM t1 GROUP BY grp HAVING SUM(val) > 25",
            [],
        )
        .unwrap();

    // Expect only A and C
    assert_eq!(res.len(), 2);
    assert_eq!(res[0], vec![Value::Text(b"A".to_vec()), Value::Int(30)]);
    assert_eq!(res[1], vec![Value::Text(b"C".to_vec()), Value::Int(100)]);
}
