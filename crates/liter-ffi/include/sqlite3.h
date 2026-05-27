/*
 * sqlite3.h — minimal header for liter-ffi.so
 *
 * Declares only the subset of the sqlite3 C API implemented by libliter_ffi.so.
 * Sufficient for compiling liter-cli-shim.c and other simple C clients.
 *
 * For the full sqlite3.h, see https://sqlite.org/download.html
 */
#pragma once

#include <stddef.h>

/* Result codes */
#define SQLITE_OK        0
#define SQLITE_ERROR     1
#define SQLITE_INTERNAL  2
#define SQLITE_PERM      3
#define SQLITE_ABORT     4
#define SQLITE_BUSY      5
#define SQLITE_LOCKED    6
#define SQLITE_NOMEM     7
#define SQLITE_READONLY  8
#define SQLITE_IOERR    10
#define SQLITE_CORRUPT  11
#define SQLITE_NOTFOUND 12
#define SQLITE_MISUSE   21
#define SQLITE_ROW     100
#define SQLITE_DONE    101

/* Column type codes */
#define SQLITE_INTEGER  1
#define SQLITE_FLOAT    2
#define SQLITE_TEXT     3
#define SQLITE_BLOB     4
#define SQLITE_NULL     5

#ifdef __cplusplus
extern "C" {
#endif

typedef struct sqlite3 sqlite3;
typedef struct sqlite3_stmt sqlite3_stmt;

/* Database connection */
int sqlite3_open(const char *filename, sqlite3 **ppDb);
int sqlite3_close(sqlite3 *db);
const char *sqlite3_errmsg(sqlite3 *db);
void sqlite3_free(void *ptr);

/* One-shot SQL execution */
typedef int (*sqlite3_exec_callback)(void *pArg, int nCol, char **azVals, char **azCols);
int sqlite3_exec(sqlite3 *db, const char *sql,
                 sqlite3_exec_callback callback, void *pArg,
                 char **pzErrMsg);

/* Prepared statements */
int sqlite3_prepare_v2(sqlite3 *db, const char *zSql, int nByte,
                       sqlite3_stmt **ppStmt, const char **pzTail);
int sqlite3_step(sqlite3_stmt *pStmt);
int sqlite3_reset(sqlite3_stmt *pStmt);
int sqlite3_finalize(sqlite3_stmt *pStmt);

/* Column access */
int         sqlite3_column_count(sqlite3_stmt *pStmt);
int         sqlite3_column_type(sqlite3_stmt *pStmt, int iCol);
long long   sqlite3_column_int64(sqlite3_stmt *pStmt, int iCol);
double      sqlite3_column_double(sqlite3_stmt *pStmt, int iCol);
const unsigned char *sqlite3_column_text(sqlite3_stmt *pStmt, int iCol);
int         sqlite3_column_bytes(sqlite3_stmt *pStmt, int iCol);
const char *sqlite3_column_name(sqlite3_stmt *pStmt, int iCol);

/* Metadata */
long long   sqlite3_changes(sqlite3 *db);
long long   sqlite3_last_insert_rowid(sqlite3 *db);
const char *sqlite3_libversion(void);
int         sqlite3_libversion_number(void);

#ifdef __cplusplus
}
#endif
