/*
 * Native Node-API (N-API v8) bridge for OryqDB C ABI v1.2 (`include/oryq.h`).
 * Zero third-party npm dependencies; GC-finalized handles with double-free protection.
 * License: MIT
 */
#define NAPI_VERSION 8
#include <node_api.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "oryq.h"

typedef enum {
    BOX_DB = 1,
    BOX_READ_TXN = 2,
    BOX_WRITE_TXN = 3,
    BOX_CURSOR = 4
} OryqBoxKind;

typedef struct {
    void *ptr;
    OryqBoxKind kind;
} OryqBox;

static void close_raw_handle(void *ptr, OryqBoxKind kind) {
    if (!ptr) return;
    switch (kind) {
        case BOX_DB:
            oryq_db_close((OryqDb *)ptr);
            break;
        case BOX_READ_TXN:
            oryq_read_txn_close((OryqReadTxn *)ptr);
            break;
        case BOX_WRITE_TXN:
            oryq_write_txn_rollback((OryqWriteTxn *)ptr);
            break;
        case BOX_CURSOR:
            oryq_cursor_close((OryqCursor *)ptr);
            break;
    }
}

static void finalize_box(napi_env env, void *finalize_data, void *finalize_hint) {
    (void)env;
    (void)finalize_hint;
    OryqBox *box = (OryqBox *)finalize_data;
    if (!box) return;
    if (box->ptr) {
        close_raw_handle(box->ptr, box->kind);
        box->ptr = NULL;
    }
    free(box);
}

static napi_value wrap_box(napi_env env, void *ptr, OryqBoxKind kind) {
    OryqBox *box = (OryqBox *)malloc(sizeof(OryqBox));
    if (!box) {
        close_raw_handle(ptr, kind);
        napi_throw_error(env, NULL, "out of memory");
        return NULL;
    }
    box->ptr = ptr;
    box->kind = kind;
    napi_value ext = NULL;
    if (napi_create_external(env, box, finalize_box, NULL, &ext) != napi_ok) {
        close_raw_handle(ptr, kind);
        free(box);
        napi_throw_error(env, NULL, "failed to create native external handle");
        return NULL;
    }
    return ext;
}

static OryqBox *unwrap_box(napi_env env, napi_value val, OryqBoxKind expected) {
    void *raw = NULL;
    if (napi_get_value_external(env, val, &raw) != napi_ok || !raw) {
        napi_throw_type_error(env, NULL, "expected native OryqDB handle");
        return NULL;
    }
    OryqBox *box = (OryqBox *)raw;
    if (box->kind != expected) {
        napi_throw_type_error(env, NULL, "mismatched native OryqDB handle type");
        return NULL;
    }
    if (!box->ptr) {
        napi_value msg_val, err_val, status_val;
        napi_create_string_utf8(env, "handle is already closed", NAPI_AUTO_LENGTH, &msg_val);
        napi_create_error(env, NULL, msg_val, &err_val);
        napi_create_int32(env, (int32_t)ORYQ_ERR_INVALID_ARGUMENT, &status_val);
        napi_set_named_property(env, err_val, "status", status_val);
        napi_throw(env, err_val);
        return NULL;
    }
    return box;
}

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

static bool get_args(napi_env env, napi_callback_info info, size_t expected, napi_value *args) {
    size_t argc = expected;
    if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc < expected) {
        napi_throw_type_error(env, NULL, "insufficient arguments");
        return false;
    }
    return true;
}

static char *dup_utf8_arg(napi_env env, napi_value val, bool allow_null, bool *is_null) {
    napi_valuetype vt;
    if (napi_typeof(env, val, &vt) != napi_ok) {
        napi_throw_type_error(env, NULL, "invalid argument");
        return NULL;
    }
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
    if (napi_get_value_string_utf8(env, val, buf, len + 1, &copied) != napi_ok) {
        free(buf);
        napi_throw_type_error(env, NULL, "failed to read UTF-8 string argument");
        return NULL;
    }
    buf[copied] = '\0';
    if (strlen(buf) != copied) {
        free(buf);
        napi_throw_type_error(env, NULL, "string argument must not contain interior NUL bytes");
        return NULL;
    }
    return buf;
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
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    char *path = dup_utf8_arg(env, args[0], false, NULL);
    if (!path) return NULL;
    OryqDb *db = NULL;
    oryq_status st = oryq_db_open(path, &db);
    free(path);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    return wrap_box(env, db, BOX_DB);
}

static napi_value fn_db_close(napi_env env, napi_callback_info info) {
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_DB);
    if (!box) return NULL;
    oryq_status st = oryq_db_close((OryqDb *)box->ptr);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    box->ptr = NULL;
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_db_checkpoint(napi_env env, napi_callback_info info) {
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_DB);
    if (!box) return NULL;
    oryq_status st = oryq_db_checkpoint((OryqDb *)box->ptr);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_db_vacuum(napi_env env, napi_callback_info info) {
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_DB);
    if (!box) return NULL;
    oryq_status st = oryq_db_vacuum((OryqDb *)box->ptr);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_db_backup_to(napi_env env, napi_callback_info info) {
    napi_value args[2];
    if (!get_args(env, info, 2, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_DB);
    if (!box) return NULL;
    char *dest = dup_utf8_arg(env, args[1], false, NULL);
    if (!dest) return NULL;
    oryq_status st = oryq_db_backup_to((OryqDb *)box->ptr, dest);
    free(dest);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_db_integrity_check(napi_env env, napi_callback_info info) {
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_DB);
    if (!box) return NULL;
    bool ok = false;
    oryq_status st = oryq_db_integrity_check((OryqDb *)box->ptr, &ok);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_get_boolean(env, ok, &out);
    return out;
}

static napi_value fn_read_txn_begin(napi_env env, napi_callback_info info) {
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_DB);
    if (!box) return NULL;
    OryqReadTxn *txn = NULL;
    oryq_status st = oryq_read_txn_begin((OryqDb *)box->ptr, &txn);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    return wrap_box(env, txn, BOX_READ_TXN);
}

static napi_value fn_read_txn_close(napi_env env, napi_callback_info info) {
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_READ_TXN);
    if (!box) return NULL;
    OryqReadTxn *txn = (OryqReadTxn *)box->ptr;
    box->ptr = NULL;
    oryq_status st = oryq_read_txn_close(txn);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_write_txn_begin(napi_env env, napi_callback_info info) {
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_DB);
    if (!box) return NULL;
    OryqWriteTxn *txn = NULL;
    oryq_status st = oryq_write_txn_begin((OryqDb *)box->ptr, &txn);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    return wrap_box(env, txn, BOX_WRITE_TXN);
}

static napi_value fn_write_txn_commit(napi_env env, napi_callback_info info) {
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_WRITE_TXN);
    if (!box) return NULL;
    OryqWriteTxn *txn = (OryqWriteTxn *)box->ptr;
    box->ptr = NULL;
    oryq_status st = oryq_write_txn_commit(txn);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_write_txn_rollback(napi_env env, napi_callback_info info) {
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_WRITE_TXN);
    if (!box) return NULL;
    OryqWriteTxn *txn = (OryqWriteTxn *)box->ptr;
    box->ptr = NULL;
    oryq_status st = oryq_write_txn_rollback(txn);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_create_collection(napi_env env, napi_callback_info info) {
    napi_value args[2];
    if (!get_args(env, info, 2, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_WRITE_TXN);
    if (!box) return NULL;
    char *name = dup_utf8_arg(env, args[1], false, NULL);
    if (!name) return NULL;
    oryq_status st = oryq_create_collection((OryqWriteTxn *)box->ptr, name);
    free(name);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_drop_collection(napi_env env, napi_callback_info info) {
    napi_value args[2];
    if (!get_args(env, info, 2, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_WRITE_TXN);
    if (!box) return NULL;
    char *name = dup_utf8_arg(env, args[1], false, NULL);
    if (!name) return NULL;
    oryq_status st = oryq_drop_collection((OryqWriteTxn *)box->ptr, name);
    free(name);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_create_index(napi_env env, napi_callback_info info) {
    napi_value args[7];
    if (!get_args(env, info, 7, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_WRITE_TXN);
    if (!box) return NULL;
    int32_t sort_order = 1;
    bool unique = false, sparse = false;
    if (napi_get_value_int32(env, args[3], &sort_order) != napi_ok ||
        napi_get_value_bool(env, args[4], &unique) != napi_ok ||
        napi_get_value_bool(env, args[5], &sparse) != napi_ok) {
        napi_throw_type_error(env, NULL, "invalid sortOrder, unique, or sparse argument");
        return NULL;
    }
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *field = dup_utf8_arg(env, args[2], false, NULL);
    if (!field) { free(col); return NULL; }
    bool name_null = false;
    char *name = dup_utf8_arg(env, args[6], true, &name_null);
    if (!name && !name_null) { free(col); free(field); return NULL; }
    oryq_status st = oryq_create_index((OryqWriteTxn *)box->ptr, col, field, sort_order, unique, sparse, name);
    free(col); free(field); free(name);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_drop_index(napi_env env, napi_callback_info info) {
    napi_value args[3];
    if (!get_args(env, info, 3, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_WRITE_TXN);
    if (!box) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *idx = dup_utf8_arg(env, args[2], false, NULL);
    if (!idx) { free(col); return NULL; }
    oryq_status st = oryq_drop_index((OryqWriteTxn *)box->ptr, col, idx);
    free(col); free(idx);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_create_fts_index(napi_env env, napi_callback_info info) {
    napi_value args[4];
    if (!get_args(env, info, 4, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_WRITE_TXN);
    if (!box) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *csv = dup_utf8_arg(env, args[2], false, NULL);
    if (!csv) { free(col); return NULL; }
    bool name_null = false;
    char *name = dup_utf8_arg(env, args[3], true, &name_null);
    if (!name && !name_null) { free(col); free(csv); return NULL; }
    oryq_status st = oryq_create_fts_index((OryqWriteTxn *)box->ptr, col, csv, name);
    free(col); free(csv); free(name);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_create_vector_index(napi_env env, napi_callback_info info) {
    napi_value args[7];
    if (!get_args(env, info, 7, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_WRITE_TXN);
    if (!box) return NULL;
    uint32_t dims = 0;
    int32_t metric = 0, quant = 0;
    if (napi_get_value_uint32(env, args[3], &dims) != napi_ok ||
        napi_get_value_int32(env, args[4], &metric) != napi_ok ||
        napi_get_value_int32(env, args[5], &quant) != napi_ok) {
        napi_throw_type_error(env, NULL, "invalid dimensions, metric, or quantization argument");
        return NULL;
    }
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *field = dup_utf8_arg(env, args[2], false, NULL);
    if (!field) { free(col); return NULL; }
    bool name_null = false;
    char *name = dup_utf8_arg(env, args[6], true, &name_null);
    if (!name && !name_null) { free(col); free(field); return NULL; }
    oryq_status st = oryq_create_vector_index((OryqWriteTxn *)box->ptr, col, field, dims, metric, quant, name);
    free(col); free(field); free(name);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_insert_json(napi_env env, napi_callback_info info) {
    napi_value args[3];
    if (!get_args(env, info, 3, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_WRITE_TXN);
    if (!box) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *doc = dup_utf8_arg(env, args[2], false, NULL);
    if (!doc) { free(col); return NULL; }
    oryq_status st = oryq_insert_json((OryqWriteTxn *)box->ptr, col, doc);
    free(col); free(doc);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_update_by_id_json(napi_env env, napi_callback_info info) {
    napi_value args[4];
    if (!get_args(env, info, 4, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_WRITE_TXN);
    if (!box) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *id_json = dup_utf8_arg(env, args[2], false, NULL);
    if (!id_json) { free(col); return NULL; }
    char *upd = dup_utf8_arg(env, args[3], false, NULL);
    if (!upd) { free(col); free(id_json); return NULL; }
    oryq_status st = oryq_update_by_id_json((OryqWriteTxn *)box->ptr, col, id_json, upd);
    free(col); free(id_json); free(upd);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_delete_by_id_json(napi_env env, napi_callback_info info) {
    napi_value args[3];
    if (!get_args(env, info, 3, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_WRITE_TXN);
    if (!box) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *id_json = dup_utf8_arg(env, args[2], false, NULL);
    if (!id_json) { free(col); return NULL; }
    oryq_status st = oryq_delete_by_id_json((OryqWriteTxn *)box->ptr, col, id_json);
    free(col); free(id_json);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value undef;
    napi_get_undefined(env, &undef);
    return undef;
}

static napi_value fn_find_json(napi_env env, napi_callback_info info) {
    napi_value args[3];
    if (!get_args(env, info, 3, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_READ_TXN);
    if (!box) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    bool filter_null = false;
    char *filter = dup_utf8_arg(env, args[2], true, &filter_null);
    if (!filter && !filter_null) { free(col); return NULL; }
    OryqCursor *cur = NULL;
    oryq_status st = oryq_find_json((OryqReadTxn *)box->ptr, col, filter, &cur);
    free(col); free(filter);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    return wrap_box(env, cur, BOX_CURSOR);
}

static napi_value fn_aggregate_json(napi_env env, napi_callback_info info) {
    napi_value args[3];
    if (!get_args(env, info, 3, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_READ_TXN);
    if (!box) return NULL;
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *pipe = dup_utf8_arg(env, args[2], false, NULL);
    if (!pipe) { free(col); return NULL; }
    OryqCursor *cur = NULL;
    oryq_status st = oryq_aggregate_json((OryqReadTxn *)box->ptr, col, pipe, &cur);
    free(col); free(pipe);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    return wrap_box(env, cur, BOX_CURSOR);
}

static napi_value fn_search_fts(napi_env env, napi_callback_info info) {
    napi_value args[5];
    if (!get_args(env, info, 5, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_READ_TXN);
    if (!box) return NULL;
    uint32_t limit = 0;
    if (napi_get_value_uint32(env, args[4], &limit) != napi_ok) {
        napi_throw_type_error(env, NULL, "limit must be an unsigned 32-bit integer");
        return NULL;
    }
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *idx = dup_utf8_arg(env, args[2], false, NULL);
    if (!idx) { free(col); return NULL; }
    char *query = dup_utf8_arg(env, args[3], false, NULL);
    if (!query) { free(col); free(idx); return NULL; }
    OryqCursor *cur = NULL;
    oryq_status st = oryq_search_fts((OryqReadTxn *)box->ptr, col, idx, query, (size_t)limit, &cur);
    free(col); free(idx); free(query);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    return wrap_box(env, cur, BOX_CURSOR);
}

static napi_value fn_search_vector(napi_env env, napi_callback_info info) {
    napi_value args[5];
    if (!get_args(env, info, 5, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_READ_TXN);
    if (!box) return NULL;
    uint32_t top_k = 0;
    if (napi_get_value_uint32(env, args[4], &top_k) != napi_ok) {
        napi_throw_type_error(env, NULL, "topK must be an unsigned 32-bit integer");
        return NULL;
    }
    napi_typedarray_type ta_type;
    size_t ta_len = 0;
    void *ta_data = NULL;
    if (napi_get_typedarray_info(env, args[3], &ta_type, &ta_len, &ta_data, NULL, NULL) != napi_ok ||
        ta_type != napi_float32_array) {
        napi_throw_type_error(env, NULL, "queryVector must be a Float32Array");
        return NULL;
    }
    char *col = dup_utf8_arg(env, args[1], false, NULL);
    if (!col) return NULL;
    char *idx = dup_utf8_arg(env, args[2], false, NULL);
    if (!idx) { free(col); return NULL; }
    OryqCursor *cur = NULL;
    oryq_status st = oryq_search_vector((OryqReadTxn *)box->ptr, col, idx, (const float *)ta_data, ta_len, (size_t)top_k, &cur);
    free(col); free(idx);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    return wrap_box(env, cur, BOX_CURSOR);
}

static napi_value fn_cursor_next(napi_env env, napi_callback_info info) {
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_CURSOR);
    if (!box) return NULL;
    oryq_status st = oryq_cursor_next((OryqCursor *)box->ptr);
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
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_CURSOR);
    if (!box) return NULL;
    char *raw = NULL;
    oryq_status st = oryq_cursor_next_json((OryqCursor *)box->ptr, &raw);
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
    napi_value args[2];
    if (!get_args(env, info, 2, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_CURSOR);
    if (!box) return NULL;
    char *field = dup_utf8_arg(env, args[1], false, NULL);
    if (!field) return NULL;
    int64_t val = 0;
    oryq_status st = oryq_cursor_get_i64((OryqCursor *)box->ptr, field, &val);
    free(field);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    if (val > 9007199254740991LL || val < -9007199254740991LL) {
        napi_throw_range_error(env, NULL, "int64 value exceeds Number.MAX_SAFE_INTEGER; use getBigInt(field)");
        return NULL;
    }
    napi_value out = NULL;
    if (napi_create_int64(env, val, &out) != napi_ok) return NULL;
    return out;
}

static napi_value fn_cursor_get_bigint(napi_env env, napi_callback_info info) {
    napi_value args[2];
    if (!get_args(env, info, 2, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_CURSOR);
    if (!box) return NULL;
    char *field = dup_utf8_arg(env, args[1], false, NULL);
    if (!field) return NULL;
    int64_t val = 0;
    oryq_status st = oryq_cursor_get_i64((OryqCursor *)box->ptr, field, &val);
    free(field);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_create_bigint_int64(env, val, &out);
    return out;
}

static napi_value fn_cursor_get_f64(napi_env env, napi_callback_info info) {
    napi_value args[2];
    if (!get_args(env, info, 2, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_CURSOR);
    if (!box) return NULL;
    char *field = dup_utf8_arg(env, args[1], false, NULL);
    if (!field) return NULL;
    double val = 0.0;
    oryq_status st = oryq_cursor_get_f64((OryqCursor *)box->ptr, field, &val);
    free(field);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_create_double(env, val, &out);
    return out;
}

static napi_value fn_cursor_get_bool(napi_env env, napi_callback_info info) {
    napi_value args[2];
    if (!get_args(env, info, 2, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_CURSOR);
    if (!box) return NULL;
    char *field = dup_utf8_arg(env, args[1], false, NULL);
    if (!field) return NULL;
    bool val = false;
    oryq_status st = oryq_cursor_get_bool((OryqCursor *)box->ptr, field, &val);
    free(field);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_get_boolean(env, val, &out);
    return out;
}

static napi_value fn_cursor_get_utf8(napi_env env, napi_callback_info info) {
    napi_value args[2];
    if (!get_args(env, info, 2, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_CURSOR);
    if (!box) return NULL;
    char *field = dup_utf8_arg(env, args[1], false, NULL);
    if (!field) return NULL;
    const char *ptr = NULL;
    size_t len = 0;
    oryq_status st = oryq_cursor_get_utf8((OryqCursor *)box->ptr, field, &ptr, &len);
    free(field);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_create_string_utf8(env, len > 0 ? ptr : "", len, &out);
    return out;
}

static napi_value fn_cursor_get_score(napi_env env, napi_callback_info info) {
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_CURSOR);
    if (!box) return NULL;
    double score = 0.0;
    oryq_status st = oryq_cursor_get_score((OryqCursor *)box->ptr, &score);
    if (st != ORYQ_OK) return throw_oryq_error(env, st);
    napi_value out;
    napi_create_double(env, score, &out);
    return out;
}

static napi_value fn_cursor_close(napi_env env, napi_callback_info info) {
    napi_value args[1];
    if (!get_args(env, info, 1, args)) return NULL;
    OryqBox *box = unwrap_box(env, args[0], BOX_CURSOR);
    if (!box) return NULL;
    OryqCursor *cur = (OryqCursor *)box->ptr;
    box->ptr = NULL;
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
        {"cursorGetBigInt", NULL, fn_cursor_get_bigint, NULL, NULL, NULL, napi_default, NULL},
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
