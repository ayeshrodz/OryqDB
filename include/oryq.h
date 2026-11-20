/* OryqDB C Application Binary Interface (v1.0)
 * License: MIT
 */
#ifndef ORYQ_H
#define ORYQ_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define ORYQ_ABI_VERSION_MAJOR 1
#define ORYQ_ABI_VERSION_MINOR 0

typedef struct OryqDb       OryqDb;
typedef struct OryqReadTxn  OryqReadTxn;
typedef struct OryqWriteTxn OryqWriteTxn;
typedef struct OryqCursor   OryqCursor;

typedef enum {
    ORYQ_OK                         = 0,
    ORYQ_CURSOR_DONE                = 1,
    ORYQ_ERR_INVALID_ARGUMENT       = 2,
    ORYQ_ERR_IO                     = 3,
    ORYQ_ERR_CORRUPTION             = 4,
    ORYQ_ERR_NOT_FOUND              = 5,
    ORYQ_ERR_DUPLICATE_KEY          = 6,
    ORYQ_ERR_LOCKED_BY_PROCESS      = 7,
    ORYQ_ERR_SNAPSHOT_TOO_OLD       = 8,
    ORYQ_ERR_QUERY_MEMORY_LIMIT     = 9,
    ORYQ_ERR_PARALLEL_ARRAY_INDEX   = 10,
    ORYQ_ERR_UNSUPPORTED_FEATURE    = 11,
    ORYQ_ERR_TYPE_MISMATCH          = 12,
    ORYQ_ERR_BUSY                   = 13,
    ORYQ_ERR_COMMIT_OUTCOME_UNKNOWN = 14,
    ORYQ_ERR_WAL_LIMIT              = 15,
    ORYQ_ERR_TOO_MANY_READERS       = 16,
    ORYQ_ERR_TRANSACTION_MEMORY     = 17,
    ORYQ_ERR_NUMERIC_OVERFLOW       = 18,
    ORYQ_ERR_NUMERIC_PRECISION      = 19,
    ORYQ_ERR_INVALID_QUERY          = 20,
    ORYQ_ERR_DOCUMENT_TOO_LARGE     = 21,
    ORYQ_ERR_INDEX_KEY_TOO_LARGE    = 22,
    ORYQ_ERR_INVALID_ID_TYPE        = 23,
    ORYQ_ERR_CONFLICTING_PATH       = 24,
    ORYQ_ERR_INDEX_NOT_FOUND        = 25,
    ORYQ_ERR_INTERNAL_PANIC         = 255
} oryq_status;

/* Diagnostics & Version */
const char *oryq_version(void);
uint32_t    oryq_abi_version(void);
const char *oryq_last_error_message(void);
void        oryq_free_string(char *ptr);

/* Database Lifecycle */
oryq_status oryq_db_open(const char *path_utf8, OryqDb **out_db);
oryq_status oryq_db_close(OryqDb *db);
oryq_status oryq_db_checkpoint(const OryqDb *db);

/* Transactions */
oryq_status oryq_read_txn_begin(const OryqDb *db, OryqReadTxn **out_txn);
oryq_status oryq_read_txn_close(OryqReadTxn *txn);
oryq_status oryq_write_txn_begin(const OryqDb *db, OryqWriteTxn **out_txn);
oryq_status oryq_write_txn_commit(OryqWriteTxn *txn);
oryq_status oryq_write_txn_rollback(OryqWriteTxn *txn);

/* Collections & CRUD */
oryq_status oryq_create_collection(OryqWriteTxn *txn, const char *name);
oryq_status oryq_drop_collection(OryqWriteTxn *txn, const char *name);
oryq_status oryq_insert_json(OryqWriteTxn *txn, const char *collection, const char *doc_json);
oryq_status oryq_update_by_id_json(OryqWriteTxn *txn, const char *collection, const char *id_json, const char *update_json);
oryq_status oryq_delete_by_id_json(OryqWriteTxn *txn, const char *collection, const char *id_json);

/* Query & Cursor */
oryq_status oryq_find_json(const OryqReadTxn *txn, const char *collection, const char *filter_json, OryqCursor **out_cursor);
oryq_status oryq_cursor_next_json(OryqCursor *cursor, char **out_json);
oryq_status oryq_cursor_get_i64(const OryqCursor *cursor, const char *field, int64_t *out_val);
oryq_status oryq_cursor_close(OryqCursor *cursor);

#ifdef __cplusplus
}
#endif

#endif /* ORYQ_H */
