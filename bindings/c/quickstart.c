/* OryqDB C99 Quickstart Example (C ABI v1.1)
 *
 * Build (Linux/macOS):
 *   cargo build -p oryq-ffi
 *   cc -std=c99 -Wall -Wextra -Werror -Iinclude bindings/c/quickstart.c \
 *      -Ltarget/debug -loryq -Wl,-rpath,target/debug -lm -o quickstart_c
 *   ./quickstart_c /tmp/example_c.oryq /tmp/example_c_backup.oryq
 */
#include "oryq.h"

#include <stdio.h>
#include <stdlib.h>

static void die_if_err(oryq_status st, const char *step) {
    if (st != ORYQ_OK) {
        fprintf(stderr, "OryqDB error at %s (status=%d): %s\n",
                step, (int)st, oryq_last_error_message());
        exit(1);
    }
}

int main(int argc, char **argv) {
    const char *db_path = (argc > 1) ? argv[1] : "quickstart_c.oryq";
    const char *backup_path = (argc > 2) ? argv[2] : "quickstart_c_backup.oryq";

    printf("OryqDB %s (ABI %u.%u)\n",
           oryq_version(),
           oryq_abi_version() >> 16,
           oryq_abi_version() & 0xFFFFu);

    OryqDb *db = NULL;
    die_if_err(oryq_db_open(db_path, &db), "oryq_db_open");

    /* Write transaction: create collection, unique index, and insert two docs */
    OryqWriteTxn *wtx = NULL;
    die_if_err(oryq_write_txn_begin(db, &wtx), "oryq_write_txn_begin");
    die_if_err(oryq_create_collection(wtx, "sensors"), "oryq_create_collection");
    die_if_err(oryq_create_index(wtx, "sensors", "code", 1, true, false, "code_uniq"),
               "oryq_create_index");
    die_if_err(oryq_insert_json(wtx, "sensors",
               "{\"_id\":1,\"code\":\"S-01\",\"temp\":21.5,\"online\":true}"),
               "insert #1");
    die_if_err(oryq_insert_json(wtx, "sensors",
               "{\"_id\":2,\"code\":\"S-02\",\"temp\":19.25,\"online\":true}"),
               "insert #2");
    die_if_err(oryq_write_txn_commit(wtx), "oryq_write_txn_commit");

    /* Read transaction: zero-copy cursor getters */
    OryqReadTxn *rtx = NULL;
    OryqCursor *cur = NULL;
    die_if_err(oryq_read_txn_begin(db, &rtx), "oryq_read_txn_begin");
    die_if_err(oryq_find_json(rtx, "sensors", "{\"online\":true}", &cur), "oryq_find_json");

    size_t count = 0;
    for (;;) {
        oryq_status st = oryq_cursor_next(cur);
        if (st == ORYQ_CURSOR_DONE) {
            break;
        }
        die_if_err(st, "oryq_cursor_next");

        int64_t id = 0;
        double temp = 0.0;
        bool online = false;
        const char *code_ptr = NULL;
        size_t code_len = 0;
        die_if_err(oryq_cursor_get_i64(cur, "_id", &id), "get_i64");
        die_if_err(oryq_cursor_get_f64(cur, "temp", &temp), "get_f64");
        die_if_err(oryq_cursor_get_bool(cur, "online", &online), "get_bool");
        die_if_err(oryq_cursor_get_utf8(cur, "code", &code_ptr, &code_len), "get_utf8");

        printf("  sensor id=%lld code=%.*s temp=%.2f online=%d\n",
               (long long)id, (int)code_len, code_ptr, temp, online ? 1 : 0);
        count++;
    }
    die_if_err(oryq_cursor_close(cur), "oryq_cursor_close");
    die_if_err(oryq_read_txn_close(rtx), "oryq_read_txn_close");

    if (count != 2) {
        fprintf(stderr, "expected 2 sensors, got %zu\n", count);
        return 1;
    }

    /* Integrity check + sealed online backup */
    bool ok = false;
    die_if_err(oryq_db_integrity_check(db, &ok), "oryq_db_integrity_check");
    if (!ok) {
        fprintf(stderr, "integrity check reported issues\n");
        return 1;
    }
    die_if_err(oryq_db_backup_to(db, backup_path), "oryq_db_backup_to");
    die_if_err(oryq_db_close(db), "oryq_db_close");
    return 0;
}
