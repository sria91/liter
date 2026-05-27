#!/bin/sh
# pts/sqlite-2.3.0 — install.sh
# Builds SQLite 3.53.0 from source and generates the sqlite-benchmark script.
# The benchmark creates a 4-column table and performs 2500 INSERTs × 2 passes
# per concurrent thread, then measures total elapsed wall-clock seconds.

set -e

# ---- 1. Extract test data ---------------------------------------------------
tar -zxf pts-sqlite-tests-1.tar.gz
# Produces: sqlite-2500-insertions.txt  (2500 INSERT INTO pts1 statements)

# ---- 2. Build SQLite 3.53.0 from source ------------------------------------
tar -zxf sqlite-autoconf-3530000.tar.gz
mkdir -p sqlite_

cd sqlite-autoconf-3530000
./configure --prefix="$(pwd)/../sqlite_"
make -j"$(nproc 2>/dev/null || echo 1)"
echo $? > ~/install-exit-status
make install
cd ..
rm -rf sqlite-autoconf-3530000

# ---- 3. Generate the benchmark driver script --------------------------------
cat > sqlite-benchmark << 'SCRIPT'
#!/bin/bash
thread_num=$1
if ! [[ "$thread_num" =~ ^[0-9]+$ ]]; then
    thread_num=1
fi

SQLITE=./sqlite_/bin/sqlite3

cat sqlite-2500-insertions.txt > sqlite-insertions.txt

do_test() {
    local idx=$1
    local DB="benchmark-${idx}.db"
    "$SQLITE" "$DB" \
        "CREATE TABLE pts1 ('I' SMALLINT NOT NULL, 'DT' TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, 'F1' VARCHAR(4) NOT NULL, 'F2' VARCHAR(16) NOT NULL);"
    cat sqlite-insertions.txt | "$SQLITE" "$DB"
    cat sqlite-insertions.txt | "$SQLITE" "$DB"
}

pids=""
for i in $(seq 1 "$thread_num"); do
    do_test "$i" &
    pids="$pids $!"
done
wait $pids
SCRIPT

chmod +x sqlite-benchmark

echo "pts/sqlite-2.3.0 install complete."
