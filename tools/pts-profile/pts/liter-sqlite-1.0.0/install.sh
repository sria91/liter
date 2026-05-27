#!/bin/sh
# pts/liter-sqlite-1.0.0 — install.sh
# Builds liter-shell from the workspace and generates the sqlite-benchmark script.

set -e

WORKSPACE=/workspaces/liter

# ---- 1. Extract test data ---------------------------------------------------
tar -zxf pts-sqlite-tests-1.tar.gz

# ---- 2. Build liter-shell ---------------------------------------------------
cargo build --release \
    --manifest-path "$WORKSPACE/Cargo.toml" \
    -p liter-shell

cp "$WORKSPACE/target/release/liter-shell" ./liter-shell
chmod +x ./liter-shell

echo "liter-shell version: $(./liter-shell --version 2>/dev/null || echo 'N/A')"

# ---- 3. Generate the benchmark driver script --------------------------------
cat > sqlite-benchmark << 'SCRIPT'
#!/bin/bash
thread_num=$1
if ! [[ "$thread_num" =~ ^[0-9]+$ ]]; then
    thread_num=1
fi

LITER_SHELL=./liter-shell

# Pre-process the original pts INSERT file to use standard SQL identifiers
# (liter does not support single-quoted identifiers or CURRENT_TIMESTAMP yet).
sed \
    -e "s/INSERT INTO 'pts1' ('I', 'DT', 'F1', 'F2')/INSERT INTO pts1 (I, DT, F1, F2)/" \
    -e "s/CURRENT_TIMESTAMP/'2024-01-01 00:00:00'/" \
    sqlite-2500-insertions.txt > sqlite-insertions.txt

do_test() {
    local idx=$1
    local DB="benchmark-${idx}.db"
    # Simplified schema compatible with liter's parser.
    echo 'CREATE TABLE pts1 (I TEXT NOT NULL, DT TEXT NOT NULL, F1 TEXT NOT NULL, F2 TEXT NOT NULL);' \
        | "$LITER_SHELL" "$DB"
    cat sqlite-insertions.txt | "$LITER_SHELL" "$DB"
    cat sqlite-insertions.txt | "$LITER_SHELL" "$DB"
}

pids=""
for i in $(seq 1 "$thread_num"); do
    do_test "$i" &
    pids="$pids $!"
done
wait $pids
SCRIPT

chmod +x sqlite-benchmark

echo "pts/liter-sqlite-1.0.0 install complete."
