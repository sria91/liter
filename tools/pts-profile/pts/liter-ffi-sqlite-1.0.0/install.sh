#!/bin/sh
# pts/liter-ffi-sqlite-1.0.0 — install.sh
# Builds libliter_ffi.so and liter-ffi-cli (C shim), then generates the
# sqlite-benchmark driver script.

set -e

WORKSPACE=/workspaces/liter
SHIM_SRC="$WORKSPACE/tools/pts-profile/liter-cli-shim.c"

# ---- 1. Extract test data ---------------------------------------------------
tar -zxf pts-sqlite-tests-1.tar.gz

# ---- 2. Build libliter_ffi.so -----------------------------------------------
cargo build --release \
    --manifest-path "$WORKSPACE/Cargo.toml" \
    -p liter-ffi

LIB_DIR="$WORKSPACE/target/release"
FFI_HEADER="$WORKSPACE/crates/liter-ffi/include/sqlite3.h"

# Fallback: if the header shim doesn't exist yet, look for the bundled one
if [ ! -f "$FFI_HEADER" ]; then
    FFI_HEADER="$WORKSPACE/crates/liter-ffi/src/sqlite3.h"
fi
if [ ! -f "$FFI_HEADER" ]; then
    # Last resort: generate a minimal header in-place
    FFI_HEADER="./sqlite3.h"
    cat > "$FFI_HEADER" << 'HDR'
#pragma once
#include <stddef.h>
#define SQLITE_OK 0
#define SQLITE_ERROR 1
typedef struct sqlite3 sqlite3;
int sqlite3_open(const char *filename, sqlite3 **ppDb);
int sqlite3_close(sqlite3 *db);
int sqlite3_exec(sqlite3*, const char *sql, int (*)(void*,int,char**,char**), void*, char **errmsg);
const char *sqlite3_errmsg(sqlite3*);
void sqlite3_free(void*);
HDR
fi

# ---- 3. Compile liter-ffi-cli -----------------------------------------------
cc -O2 "$SHIM_SRC" \
    -I"$(dirname "$FFI_HEADER")" \
    -L"$LIB_DIR" \
    -lliter_ffi \
    -Wl,-rpath,"$LIB_DIR" \
    -o liter-ffi-cli

chmod +x ./liter-ffi-cli

# Symlink (or copy) the shared library next to the binary for rpath fallback.
cp "$LIB_DIR/libliter_ffi.so" ./libliter_ffi.so

echo "liter-ffi-cli built successfully."

# ---- 4. Generate the benchmark driver script --------------------------------
cat > sqlite-benchmark << 'SCRIPT'
#!/bin/bash
thread_num=$1
if ! [[ "$thread_num" =~ ^[0-9]+$ ]]; then
    thread_num=1
fi

CLI=./liter-ffi-cli

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
    "$CLI" "$DB" \
        'CREATE TABLE pts1 (I TEXT NOT NULL, DT TEXT NOT NULL, F1 TEXT NOT NULL, F2 TEXT NOT NULL);'
    cat sqlite-insertions.txt | "$CLI" "$DB"
    cat sqlite-insertions.txt | "$CLI" "$DB"
}

pids=""
for i in $(seq 1 "$thread_num"); do
    do_test "$i" &
    pids="$pids $!"
done
wait $pids
SCRIPT

chmod +x sqlite-benchmark

echo "pts/liter-ffi-sqlite-1.0.0 install complete."
