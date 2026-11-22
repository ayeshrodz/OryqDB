/* Native C99 integration test exercising OryqDB ABI v1.1 (`include/oryq.h`). */
#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "oryq.h"

int main(int argc, char **argv) {
    const char *db_path = (argc > 1) ? argv[1] : "/tmp/oryqdb_c99_integration.oryq";
    const char *bk_path = (argc > 2) ? argv[2] : "/tmp/oryqdb_c99_integration_bk.oryq";

    assert(oryq_abi_version() == (((uint32_t)ORYQ_ABI_VERSION_MAJOR << 16) | (uint32_t)ORYQ_ABI_VERSION_MINOR));
    assert(oryq_version() != NULL);

    OryqDb *db = NULL;
    assert(oryq_db_open(db_path, &db) == ORYQ_OK);
    assert(db != NULL);

    /* Write transaction: collection, indexes (named Asc + auto-named Desc), inserts */
    OryqWriteTxn *wtx = NULL;
    assert(oryq_write_txn_begin(db, &wtx) == ORYQ_OK);
    assert(oryq_create_collection(wtx, "items") == ORYQ_OK);
    assert(oryq_create_index(wtx, "items", "sku", 0, true, false, NULL) == ORYQ_ERR_INVALID_ARGUMENT);
    assert(oryq_create_index(wtx, "items", "sku", 1, true, false, "sku_idx") == ORYQ_OK);
    assert(oryq_create_index(wtx, "items", "price", -1, false, false, NULL) == ORYQ_OK);
    assert(oryq_insert_json(wtx, "items", "{\"_id\": 1, \"sku\": \"X-1\", \"note\": \"\", \"price\": 12.5, \"active\": true}") == ORYQ_OK);
    assert(oryq_insert_json(wtx, "items", "{\"_id\": 2, \"sku\": \"X-2\", \"note\": \"ok\", \"price\": 27.5, \"active\": false}") == ORYQ_OK);
    assert(oryq_write_txn_commit(wtx) == ORYQ_OK);

    /* Read transaction: zero-copy cursor advance + typed getters + edge cases */
    OryqReadTxn *rtx = NULL;
    assert(oryq_read_txn_begin(db, &rtx) == ORYQ_OK);
    OryqCursor *cur = NULL;
    assert(oryq_find_json(rtx, "items", "{\"sku\": \"X-1\"}", &cur) == ORYQ_OK);

    /* Calling getter before first advance must fail cleanly with ErrNotFound */
    double pre_price = 99.0;
    assert(oryq_cursor_get_f64(cur, "price", &pre_price) == ORYQ_ERR_NOT_FOUND);
    assert(pre_price == 0.0);

    assert(oryq_cursor_next(cur) == ORYQ_OK);
    const char *sptr = NULL;
    size_t slen = 0;
    assert(oryq_cursor_get_utf8(cur, "sku", &sptr, &slen) == ORYQ_OK);
    assert(slen == 3 && memcmp(sptr, "X-1", 3) == 0);

    /* Empty UTF-8 string check */
    assert(oryq_cursor_get_utf8(cur, "note", &sptr, &slen) == ORYQ_OK);
    assert(slen == 0 && sptr != NULL);

    double price = 0.0;
    assert(oryq_cursor_get_f64(cur, "price", &price) == ORYQ_OK);
    assert(fabs(price - 12.5) < 1e-9);

    bool active = false;
    assert(oryq_cursor_get_bool(cur, "active", &active) == ORYQ_OK);
    assert(active == true);

    assert(oryq_cursor_next(cur) == ORYQ_CURSOR_DONE);
    assert(oryq_cursor_get_bool(cur, "active", &active) == ORYQ_ERR_NOT_FOUND);
    assert(oryq_cursor_close(cur) == ORYQ_OK);

    /* Aggregation pipeline */
    OryqCursor *acur = NULL;
    assert(oryq_aggregate_json(
        rtx,
        "items",
        "[{\"$group\": {\"_id\": null, \"sum_price\": {\"$sum\": \"$price\"}, \"cnt\": {\"$count\": {}}}}]",
        &acur
    ) == ORYQ_OK);
    assert(oryq_cursor_next(acur) == ORYQ_OK);
    double sum_price = 0.0;
    int64_t cnt = 0;
    assert(oryq_cursor_get_f64(acur, "sum_price", &sum_price) == ORYQ_OK);
    assert(fabs(sum_price - 40.0) < 1e-9);
    assert(oryq_cursor_get_i64(acur, "cnt", &cnt) == ORYQ_OK);
    assert(cnt == 2);
    assert(oryq_cursor_close(acur) == ORYQ_OK);
    assert(oryq_read_txn_close(rtx) == ORYQ_OK);

    /* Drop index + missing index populates last_error_message */
    OryqWriteTxn *wtx2 = NULL;
    assert(oryq_write_txn_begin(db, &wtx2) == ORYQ_OK);
    assert(oryq_drop_index(wtx2, "items", "sku_idx") == ORYQ_OK);
    assert(oryq_drop_index(wtx2, "items", "sku_idx") == ORYQ_ERR_INDEX_NOT_FOUND);
    assert(oryq_last_error_message() != NULL);
    assert(oryq_write_txn_commit(wtx2) == ORYQ_OK);

    /* Integrity check + sealed online backup */
    bool ok = false;
    assert(oryq_db_integrity_check(db, &ok) == ORYQ_OK);
    assert(ok == true);
    assert(oryq_db_backup_to(db, bk_path) == ORYQ_OK);
    assert(oryq_db_close(db) == ORYQ_OK);

    printf("C99 ABI v1.1 integration test passed!\n");
    return 0;
}
