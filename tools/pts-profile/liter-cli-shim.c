/*
 * liter-cli-shim.c
 *
 * Minimal sqlite3-compatible CLI backed by liter-ffi.so.
 * Reads SQL statements from stdin (or a named SQL file via second arg),
 * executes them against the database file given as the first argument.
 *
 * Usage:
 *   liter-ffi-cli <db-file>            # read SQL from stdin
 *   liter-ffi-cli <db-file> "SQL;"     # execute a single statement
 *
 * Behaviour mirrors how pts/sqlite drives the C sqlite3 binary:
 *   ./sqlite3 benchmark.db "CREATE TABLE ..."
 *   cat insertions.txt | ./sqlite3 benchmark.db
 *
 * Build (after `cargo build --release -p liter-ffi`):
 *   cc -O2 liter-cli-shim.c \
 *       -I../../crates/liter-ffi/include \
 *       -L../../target/release -lliter_ffi \
 *       -Wl,-rpath,'$ORIGIN/../../target/release' \
 *       -o liter-ffi-cli
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "sqlite3.h"

#define BUF_INIT  4096
#define BUF_GROW  4096

/* Execute one complete SQL statement; print errors to stderr. */
static int exec_one(sqlite3 *db, const char *sql)
{
    char *errmsg = NULL;
    int rc = sqlite3_exec(db, sql, NULL, NULL, &errmsg);
    if (rc != SQLITE_OK) {
        fprintf(stderr, "Error: %s\n", errmsg ? errmsg : "(unknown)");
        sqlite3_free(errmsg);
    }
    return rc;
}

int main(int argc, char **argv)
{
    if (argc < 2) {
        fprintf(stderr, "Usage: %s <db-file> [\"SQL;\"]\n", argv[0]);
        return 1;
    }

    const char *db_path = argv[1];
    sqlite3 *db = NULL;

    int rc = sqlite3_open(db_path, &db);
    if (rc != SQLITE_OK) {
        fprintf(stderr, "Cannot open database '%s': %s\n",
                db_path, sqlite3_errmsg(db));
        sqlite3_close(db);
        return 1;
    }

    /* If a SQL string was supplied as the second argument, execute it and exit. */
    if (argc >= 3) {
        rc = exec_one(db, argv[2]);
        sqlite3_close(db);
        return (rc == SQLITE_OK) ? 0 : 1;
    }

    /* Otherwise read SQL from stdin, accumulating until ';' ends a statement. */
    size_t buf_cap  = BUF_INIT;
    size_t buf_used = 0;
    char  *buf      = malloc(buf_cap);
    if (!buf) { perror("malloc"); sqlite3_close(db); return 1; }

    char line[4096];
    int  any_error = 0;

    while (fgets(line, sizeof(line), stdin)) {
        size_t line_len = strlen(line);

        /* Grow buffer if needed. */
        if (buf_used + line_len + 1 > buf_cap) {
            buf_cap += BUF_GROW + line_len;
            char *tmp = realloc(buf, buf_cap);
            if (!tmp) { perror("realloc"); break; }
            buf = tmp;
        }

        memcpy(buf + buf_used, line, line_len);
        buf_used += line_len;
        buf[buf_used] = '\0';

        /* Check if the accumulated buffer ends with a ';' (ignoring trailing
         * whitespace / newline) which signals a complete statement. */
        const char *p = buf + buf_used;
        while (p > buf && (*(p-1) == '\n' || *(p-1) == '\r' || *(p-1) == ' ' || *(p-1) == '\t'))
            --p;

        if (p > buf && *(p-1) == ';') {
            rc = exec_one(db, buf);
            if (rc != SQLITE_OK) any_error = 1;
            buf_used = 0;
            buf[0]   = '\0';
        }
    }

    /* Execute any trailing SQL that lacks a terminating semicolon. */
    if (buf_used > 0) {
        /* Trim whitespace */
        char *p = buf + buf_used;
        while (p > buf && (*(p-1) == '\n' || *(p-1) == '\r' || *(p-1) == ' ' || *(p-1) == '\t'))
            --p;
        *p = '\0';
        if (p > buf) {
            rc = exec_one(db, buf);
            if (rc != SQLITE_OK) any_error = 1;
        }
    }

    free(buf);
    sqlite3_close(db);
    return any_error ? 1 : 0;
}
