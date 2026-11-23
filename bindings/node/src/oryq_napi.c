/*
 * Native Node-API (N-API v8) bridge for OryqDB C ABI v1.2 (`include/oryq.h`).
 * Zero third-party npm dependencies.
 * License: MIT
 */
#define NAPI_VERSION 8
#include <node_api.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "oryq.h"

static napi_value throw_oryq_error(napi_env env, oryq_status status) {
    const char *msg = oryq_last_error_message();
    if (!msg) {
        msg = "unknown OryqDB error";
    }
    napi_value msg_val, err_val, status_val;
    napi_create_string_utf8(env, msg, NAPI_AUTO_LENGTH, &msg_val);
    napi_create_error(env, NULL, msg_val, &err_val);
    napi_create_int32(env, (int32_t)status, &status_val);
    napi_set_named_property(env, err_val, "status", status_val);
    napi_throw(env, err_val);
    return NULL;
}

static char *dup_utf8_arg(napi_env env, napi_value val, bool allow_null, bool *is_null) {
    napi_valuetype vt;
    napi_typeof(env, val, &vt);
    if (allow_null && (vt == napi_null || vt == napi_undefined)) {
        if (is_null) *is_null = true;
        return NULL;
    }
    if (is_null) *is_null = false;
    size_t len = 0;
    if (napi_get_value_string_utf8(env, val, NULL, 0, &len) != napi_ok) {
        napi_throw_type_error(env, NULL, "expected UTF-8 string argument");
        return NULL;
    }
    char *buf = (char *)malloc(len + 1);
    if (!buf) {
        napi_throw_error(env, NULL, "out of memory");
        return NULL;
    }
    size_t copied = 0;
    napi_get_value_string_utf8(env, val, buf, len + 1, &copied);
    buf[copied] = '\0';
    return buf;
}

static void *get_external_ptr(napi_env env, napi_value val) {
    void *ptr = NULL;
    if (napi_get_value_external(env, val, &ptr) != napi_ok) {
        napi_throw_type_error(env, NULL, "expected native OryqDB handle");
        return NULL;
    }
    return ptr;
}

static napi_value fn_version(napi_env env, napi_callback_info info) {
    (void)info;
    const char *v = oryq_version();
    napi_value out;
    napi_create_string_utf8(env, v ? v : "", NAPI_AUTO_LENGTH, &out);
    return out;
}

static napi_value fn_abi_version(napi_env env, napi_callback_info info) {
    (void)info;
    napi_value out;
    napi_create_uint32(env, oryq_abi_version(), &out);
    return out;
}

static napi_value fn_db_open(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    if (argc < 1) {
        napi_throw_type_error(env, NULL, "dbOpen(path) requires 1 argument");
        return NULL;
    }
    char *path = dup_utf8_arg(env, args[0], false, NULL);
    if (!path) return NULL;
    OryqDb *db = NULL;
    oryq_status st = oryq_db_open(path, &db);
    free(path);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value ext;
    napi_create_external(env, db, NULL, NULL, &ext);
    return ext;
}

static napi_value fn_db_close(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqDb *db = (OryqDb *)get_external_ptr(env, args[0]);
    if (!db) return NULL;
    oryq_status st = oryq_db_close(db);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_db_checkpoint(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqDb *db = (OryqDb *)get_external_ptr(env, args[0]);
    if (!db) return NULL;
    oryq_status st = oryq_db_checkpoint(db);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_db_vacuum(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqDb *db = (OryqDb *)get_external_ptr(env, args[0]);
    if (!db) return NULL;
    oryq_status st = oryq_db_vacuum(db);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_db_backup_to(napi_env env, napi_callback_info info) {
    size_t argc = 2;
    napi_value args[2];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqDb *db = (OryqDb *)get_external_ptr(env, args[0]);
    if (!db) return NULL;
    char *dest = dup_utf8_arg(env, args[1], false, NULL);
    if (!dest) return NULL;
    oryq_status st = oryq_db_backup_to(db, dest);
    free(dest);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_db_integrity_check(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqDb *db = (OryqDb *)get_external_ptr(env, args[0]);
    if (!db) return NULL;
    bool ok = false;
    oryq_status st = oryq_db_integrity_check(db, &ok);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_get_boolean(env, ok, &out);
    return out;
}

static napi_value fn_read_txn_begin(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqDb *db = (OryqDb *)get_external_ptr(env, args[0]);
    if (!db) return NULL;
    OryqReadTxn *txn = NULL;
    oryq_status st = oryq_read_txn_begin(db, &txn);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value ext;
    napi_create_external(env, txn, NULL, NULL, &ext);
    return ext;
}

static napi_value fn_read_txn_close(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqReadTxn *txn = (OryqReadTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    oryq_status st = oryq_read_txn_close(txn);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_write_txn_begin(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqDb *db = (OryqDb *)get_external_ptr(env, args[0]);
    if (!db) return NULL;
    OryqWriteTxn *txn = NULL;
    oryq_status st = oryq_write_txn_begin(db, &txn);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value ext;
    napi_create_external(env, txn, NULL, NULL, &ext);
    return ext;
}

static napi_value fn_write_txn_commit(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqWriteTxn *txn = (OryqWriteTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    oryq_status st = oryq_write_txn_commit(txn);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_write_txn_rollback(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqWriteTxn *txn = (OryqWriteTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    oryq_status st = oryq_write_txn_rollback(txn);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_create_collection(napi_env env, napi_callback_info info) {
    size_t argc = 2;
    napi_value args[2];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqWriteTxn *txn = (OryqWriteTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *name = dup_utf8_arg(env, args[1], false, NULL);
    if (!name) return NULL;
    oryq_status st = oryq_create_collection(txn, name);
    free(name);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_drop_collection(napi_env env, napi_callback_info info) {
    size_t argc = 2;
    napi_value args[2];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqWriteTxn *txn = (OryqWriteTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *name = dup_utf8_arg(env, args[1], false, NULL);
    if (!name) return NULL;
    oryq_status st = oryq_drop_collection(txn, name);
    free(name);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_create_index(napi_env env, napi_callback_info info) {
    size_t argc = 7;
    napi_value args[7];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqWriteTxn *txn = (OryqWriteTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *field = dup_utf8_arg(env, args[2], false, NULL);
    if (!field) { free(col); return NULL; }
    int32_t sort_order = 1;
    bool unique = false, sparse = false;
    napi_get_value_int32(env, args[3], &sort_order);
    napi_get_value_bool(env, args[4], &unique);
    napi_get_value_bool(env, args[5], &sparse);
    bool name_null = false;
    char *name = dup_utf8_arg(env, args[6], true, &name_null);
    if (!name && !name_null) { free(col); free(field); return NULL; }
    oryq_status st = oryq_create_index(txn, col, field, sort_order, unique, sparse, name);
    free(col); free(field); free(name);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_drop_index(napi_env env, napi_callback_info info) {
    size_t argc = 3;
    napi_value args[3];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqWriteTxn *txn = (OryqWriteTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *idx = dup_utf8_arg(env, args[2], false, NULL);
    if (!idx) { free(col); return NULL; }
    oryq_status st = oryq_drop_index(txn, col, idx);
    free(col); free(idx);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_create_fts_index(napi_env env, napi_callback_info info) {
    size_t argc = 4;
    napi_value args[4];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqWriteTxn *txn = (OryqWriteTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *csv = dup_utf8_arg(env, args[2], false, NULL);
    if (!csv) { free(col); return NULL; }
    bool name_null = false;
    char *name = dup_utf8_arg(env, args[3], true, &name_null);
    if (!name && !name_null) { free(col); free(csv); return NULL; }
    oryq_status st = oryq_create_fts_index(txn, col, csv, name);
    free(col); free(csv); free(name);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_create_vector_index(napi_env env, napi_callback_info info) {
    size_t argc = 7;
    napi_value args[7];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqWriteTxn *txn = (OryqWriteTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *field = dup_utf8_arg(env, args[2], false, NULL);
    if (!field) { free(col); return NULL; }
    uint32_t dims = 0;
    int32_t metric = 0, quant = 0;
    napi_get_value_uint32(env, args[3], &dims);
    napi_get_value_int32(env, args[4], &metric);
    napi_get_value_int32(env, args[5], &quant);
    bool name_null = false;
    char *name = dup_utf8_arg(env, args[6], true, &name_null);
    if (!name && !name_null) { free(col); free(field); return NULL; }
    oryq_status st = oryq_create_vector_index(txn, col, field, dims, metric, quant, name);
    free(col); free(field); free(name);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_insert_json(napi_env env, napi_callback_info info) {
    size_t argc = 3;
    napi_value args[3];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqWriteTxn *txn = (OryqWriteTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *doc = dup_utf8_arg(env, args[2], false, NULL);
    if (!doc) { free(col); return NULL; }
    oryq_status st = oryq_insert_json(txn, col, doc);
    free(col); free(doc);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_update_by_id_json(napi_env env, napi_callback_info info) {
    size_t argc = 4;
    napi_value args[4];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqWriteTxn *txn = (OryqWriteTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *id_json = dup_utf8_arg(env, args[2], false, NULL);
    if (!id_json) { free(col); return NULL; }
    char *upd = dup_utf8_arg(env, args[3], false, NULL);
    if (!upd) { free(col); free(id_json); return NULL; }
    oryq_status st = oryq_update_by_id_json(txn, col, id_json, upd);
    free(col); free(id_json); free(upd);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_delete_by_id_json(napi_env env, napi_callback_info info) {
    size_t argc = 3;
    napi_value args[3];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqWriteTxn *txn = (OryqWriteTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *id_json = dup_utf8_arg(env, args[2], false, NULL);
    if (!id_json) { free(col); return NULL; }
    oryq_status st = oryq_delete_by_id_json(txn, col, id_json);
    free(col); free(id_json);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_find_json(napi_env env, napi_callback_info info) {
    size_t argc = 3;
    napi_value args[3];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqReadTxn *txn = (OryqReadTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    bool filter_null = false;
    char *filter = dup_utf8_arg(env, args[2], true, &filter_null);
    if (!filter && !filter_null) { free(col); return NULL; }
    OryqCursor *cur = NULL;
    oryq_status st = oryq_find_json(txn, col, filter, &cur);
    free(col); free(filter);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value ext;
    napi_create_external(env, cur, NULL, NULL, &ext);
    return ext;
}

static napi_value fn_aggregate_json(napi_env env, napi_callback_info info) {
    size_t argc = 3;
    napi_value args[3];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqReadTxn *txn = (OryqReadTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *pipe = dup_utf8_arg(env, args[2], false, NULL);
    if (!pipe) { free(col); return NULL; }
    OryqCursor *cur = NULL;
    oryq_status st = oryq_aggregate_json(txn, col, pipe, &cur);
    free(col); free(pipe);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value ext;
    napi_create_external(env, cur, NULL, NULL, &ext);
    return ext;
}

static napi_value fn_search_fts(napi_env env, napi_callback_info info) {
    size_t argc = 5;
    napi_value args[5];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqReadTxn *txn = (OryqReadTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *idx = dup_utf8_arg(env, args[2], false, NULL);
    if (!idx) { free(col); return NULL; }
    char *query = dup_utf8_arg(env, args[3], false, NULL);
    if (!query) { free(col); free(idx); return NULL; }
    uint32_t limit = 10;
    napi_get_value_uint32(env, args[4], &limit);
    OryqCursor *cur = NULL;
    oryq_status st = oryq_search_fts(txn, col, idx, query, (size_t)limit, &cur);
    free(col); free(idx); free(query);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value ext;
    napi_create_external(env, cur, NULL, NULL, &ext);
    return ext;
}

static napi_value fn_search_vector(napi_env env, napi_callback_info info) {
    size_t argc = 5;
    napi_value args[5];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqReadTxn *txn = (OryqReadTxn *)get_external_ptr(env, args[0]);
    if (!txn) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *idx = dup_utf8_arg(env, args[2], false, NULL);
    if (!idx) { free(col); return NULL; }
    napi_typedarray_type ta_type;
    size_t ta_len = 0;
    void *ta_data = NULL;
    if (napi_get_typedarray_info(env, args[3], &ta_type, &ta_len, &ta_data, NULL, NULL) != napi_ok ||
        ta_type != napi_float32_array) {
        free(col); free(idx);
        napi_throw_type_error(env, NULL, "queryVector must be a Float32Array");
        return NULL;
    }
    uint32_t top_k = 10;
    napi_get_value_uint32(env, args[4], &top_k);
    OryqCursor *cur = NULL;
    oryq_status st = oryq_search_vector(txn, col, idx, (const float *)ta_data, ta_len, (size_t)top_k, &cur);
    free(col); free(idx);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value ext;
    napi_create_external(env, cur, NULL, NULL, &ext);
    return ext;
}

static napi_value fn_cursor_next(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqCursor *cur = (OryqCursor *)get_external_ptr(env, args[0]);
    if (!cur) return NULL;
    oryq_status st = oryq_cursor_next(cur);
    if (st == ORYQ_CURSOR_DONE) {
        napi_value out;
        napi_get_boolean(env, false, &out);
        return out;
    }
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_get_boolean(env, true, &out);
    return out;
}

static napi_value fn_cursor_next_json(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqCursor *cur = (OryqCursor *)get_external_ptr(env, args[0]);
    if (!cur) return NULL;
    char *raw = NULL;
    oryq_status st = oryq_cursor_next_json(cur, &raw);
    if (st == ORYQ_CURSOR_DONE) {
        napi_value n;
        napi_get_null(env, &n);
        return n;
    }
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_create_string_utf8(env, raw ? raw : "", NAPI_AUTO_LENGTH, &out);
    oryq_free_string(raw);
    return out;
}

static napi_value fn_cursor_get_i64(napi_env env, napi_callback_info info) {
    size_t argc = 2;
    napi_value args[2];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqCursor *cur = (OryqCursor *)get_external_ptr(env, args[0]);
    if (!cur) return NULL;
    char *field = dup_utf8_arg(env, args[1], false, NULL);
    if (!field) return NULL;
    int64_t val = 0;
    oryq_status st = oryq_cursor_get_i64(cur, field, &val);
    free(field);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_create_int64(env, val, &out);
    return out;
}

static napi_value fn_cursor_get_f64(napi_env env, napi_callback_info info) {
    size_t argc = 2;
    napi_value args[2];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqCursor *cur = (OryqCursor *)get_external_ptr(env, args[0]);
    if (!cur) return NULL;
    char *field = dup_utf8_arg(env, args[1], false, NULL);
    if (!field) return NULL;
    double val = 0.0;
    oryq_status st = oryq_cursor_get_f64(cur, field, &val);
    free(field);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_create_double(env, val, &out);
    return out;
}

static napi_value fn_cursor_get_bool(napi_env env, napi_callback_info info) {
    size_t argc = 2;
    napi_value args[2];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqCursor *cur = (OryqCursor *)get_external_ptr(env, args[0]);
    if (!cur) return NULL;
    char *field = dup_utf8_arg(env, args[1], false, NULL);
    if (!field) return NULL;
    bool val = false;
    oryq_status st = oryq_cursor_get_bool(cur, field, &val);
    free(field);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_get_boolean(env, val, &out);
    return out;
}

static napi_value fn_cursor_get_utf8(napi_env env, napi_callback_info info) {
    size_t argc = 2;
    napi_value args[2];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqCursor *cur = (OryqCursor *)get_external_ptr(env, args[0]);
    if (!cur) return NULL;
    char *field = dup_utf8_arg(env, args[1], false, NULL);
    if (!field) return NULL;
    const char *ptr = NULL;
    size_t len = 0;
    oryq_status st = oryq_cursor_get_utf8(cur, field, &ptr, &len);
    free(field);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_create_string_utf8(env, len > 0 ? ptr : "", len, &out);
    return out;
}

static napi_value fn_cursor_get_score(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqCursor *cur = (OryqCursor *)get_external_ptr(env, args[0]);
    if (!cur) return NULL;
    double score = 0.0;
    oryq_status st = oryq_cursor_get_score(cur, &score);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_create_double(env, score, &out);
    return out;
}

static napi_value fn_cursor_close(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value args[1];
    napi_get_cb_info(env, info, &argc, args, NULL, NULL);
    OryqCursor *cur = (OryqCursor *)get_external_ptr(env, args[0]);
    if (!cur) return NULL;
    oryq_status st = oryq_cursor_close(cur);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value init_module(napi_env env, napi_value exports) {
    napi_property_descriptor desc[] = {
        {"version", NULL, fn_version, NULL, NULL, NULL, napi_default, NULL},
        {"abiVersion", NULL, fn_abi_version, NULL, NULL, NULL, napi_default, NULL},
        {"dbOpen", NULL, fn_db_open, NULL, NULL, NULL, napi_default, NULL},
        {"dbClose", NULL, fn_db_close, NULL, NULL, NULL, napi_default, NULL},
        {"dbCheckpoint", NULL, fn_db_checkpoint, NULL, NULL, NULL, napi_default, NULL},
        {"dbVacuum", NULL, fn_db_vacuum, NULL, NULL, NULL, napi_default, NULL},
        {"dbBackupTo", NULL, fn_db_backup_to, NULL, NULL, NULL, napi_default, NULL},
        {"dbIntegrityCheck", NULL, fn_db_integrity_check, NULL, NULL, NULL, napi_default, NULL},
        {"readTxnBegin", NULL, fn_read_txn_begin, NULL, NULL, NULL, napi_default, NULL},
        {"readTxnClose", NULL, fn_read_txn_close, NULL, NULL, NULL, napi_default, NULL},
        {"writeTxnBegin", NULL, fn_write_txn_begin, NULL, NULL, NULL, napi_default, NULL},
        {"writeTxnCommit", NULL, fn_write_txn_commit, NULL, NULL, NULL, napi_default, NULL},
        {"writeTxnRollback", NULL, fn_write_txn_rollback, NULL, NULL, NULL, napi_default, NULL},
        {"createCollection", NULL, fn_create_collection, NULL, NULL, NULL, napi_default, NULL},
        {"dropCollection", NULL, fn_drop_collection, NULL, NULL, NULL, napi_default, NULL},
        {"createIndex", NULL, fn_create_index, NULL, NULL, NULL, napi_default, NULL},
        {"dropIndex", NULL, fn_drop_index, NULL, NULL, NULL, napi_default, NULL},
        {"createFtsIndex", NULL, fn_create_fts_index, NULL, NULL, NULL, napi_default, NULL},
        {"createVectorIndex", NULL, fn_create_vector_index, NULL, NULL, NULL, napi_default, NULL},
        {"insertJson", NULL, fn_insert_json, NULL, NULL, NULL, napi_default, NULL},
        {"updateByIdJson", NULL, fn_update_by_id_json, NULL, NULL, NULL, napi_default, NULL},
        {"deleteByIdJson", NULL, fn_delete_by_id_json, NULL, NULL, NULL, napi_default, NULL},
        {"findJson", NULL, fn_find_json, NULL, NULL, NULL, napi_default, NULL},
        {"aggregateJson", NULL, fn_aggregate_json, NULL, NULL, NULL, napi_default, NULL},
        {"searchFts", NULL, fn_search_fts, NULL, NULL, NULL, napi_default, NULL},
        {"searchVector", NULL, fn_search_vector, NULL, NULL, NULL, napi_default, NULL},
        {"cursorNext", NULL, fn_cursor_next, NULL, NULL, NULL, napi_default, NULL},
        {"cursorNextJson", NULL, fn_cursor_next_json, NULL, NULL, NULL, napi_default, NULL},
        {"cursorGetI64", NULL, fn_cursor_get_i64, NULL, NULL, NULL, napi_default, NULL},
        {"cursorGetF64", NULL, fn_cursor_get_f64, NULL, NULL, NULL, napi_default, NULL},
        {"cursorGetBool", NULL, fn_cursor_get_bool, NULL, NULL, NULL, napi_default, NULL},
        {"cursorGetUtf8", NULL, fn_cursor_get_utf8, NULL, NULL, NULL, napi_default, NULL},
        {"cursorGetScore", NULL, fn_cursor_get_score, NULL, NULL, NULL, napi_default, NULL},
        {"cursorClose", NULL, fn_cursor_close, NULL, NULL, NULL, napi_default, NULL},
    };
    napi_define_properties(env, exports, sizeof(desc) / sizeof(desc[0]), desc);
    return exports;
}

NAPI_MODULE(NODE_GYP_MODULE_NAME, init_module)
