"""Official Python ctypes bindings for OryqDB (C ABI v1.3).

License: MIT
"""

from __future__ import annotations

import ctypes
import enum
import json
import os
import sys
from pathlib import Path
from typing import Any, Iterator, Optional, Sequence, Tuple, Union

__version__ = "0.15.0"
__all__ = [
    "OryqStatus",
    "OryqError",
    "VectorMetric",
    "VectorQuantization",
    "Database",
    "ReadTxn",
    "WriteTxn",
    "Cursor",
    "load_library",
]


class VectorMetric(enum.IntEnum):
    """Distance metric for vector similarity indexes (`oryq_create_vector_index`)."""

    COSINE = 0
    L2 = 1
    DOT_PRODUCT = 2


class VectorQuantization(enum.IntEnum):
    """Storage quantization for vector similarity indexes (`oryq_create_vector_index`)."""

    F32 = 0
    INT8 = 1


class OryqStatus(enum.IntEnum):
    """Status codes matching `oryq_status` in `include/oryq.h` (ABI v1.2)."""

    OK = 0
    CURSOR_DONE = 1
    ERR_INVALID_ARGUMENT = 2
    ERR_IO = 3
    ERR_CORRUPTION = 4
    ERR_NOT_FOUND = 5
    ERR_DUPLICATE_KEY = 6
    ERR_LOCKED_BY_PROCESS = 7
    ERR_SNAPSHOT_TOO_OLD = 8
    ERR_QUERY_MEMORY_LIMIT = 9
    ERR_PARALLEL_ARRAY_INDEX = 10
    ERR_UNSUPPORTED_FEATURE = 11
    ERR_TYPE_MISMATCH = 12
    ERR_BUSY = 13
    ERR_COMMIT_OUTCOME_UNKNOWN = 14
    ERR_WAL_LIMIT = 15
    ERR_TOO_MANY_READERS = 16
    ERR_TRANSACTION_MEMORY = 17
    ERR_NUMERIC_OVERFLOW = 18
    ERR_NUMERIC_PRECISION = 19
    ERR_INVALID_QUERY = 20
    ERR_DOCUMENT_TOO_LARGE = 21
    ERR_INDEX_KEY_TOO_LARGE = 22
    ERR_INVALID_ID_TYPE = 23
    ERR_CONFLICTING_PATH = 24
    ERR_INDEX_NOT_FOUND = 25
    ERR_INTERNAL_PANIC = 255


class OryqError(RuntimeError):
    """Exception raised when a `liboryq` C ABI call returns a non-OK status."""

    def __init__(self, status: int, message: str) -> None:
        try:
            self.status: Union[OryqStatus, int] = OryqStatus(status)
        except ValueError:
            self.status = status
        self.message = message
        super().__init__(f"{self.status!r}: {message}")


def _platform_lib_names() -> Tuple[str, ...]:
    if sys.platform.startswith("win"):
        return ("oryq.dll", "liboryq.dll")
    if sys.platform == "darwin":
        return ("liboryq.dylib",)
    return ("liboryq.so",)


def _discover_library_path() -> Optional[Path]:
    env_path = os.environ.get("ORYQ_LIB_PATH")
    if env_path:
        return Path(env_path)

    # Only probe workspace target/{release,debug} when running inside an actual OryqDB checkout.
    parents = Path(__file__).resolve().parents
    if len(parents) > 3:
        repo_root = parents[3]
        if (repo_root / "crates" / "oryq-ffi" / "Cargo.toml").is_file():
            candidates = []
            for profile in ("release", "debug"):
                for name in _platform_lib_names():
                    candidate = repo_root / "target" / profile / name
                    if candidate.is_file():
                        candidates.append(candidate)
            if candidates:
                candidates.sort(key=lambda p: p.stat().st_mtime, reverse=True)
                return candidates[0]
    return None


_DEFAULT_LIB: Optional[ctypes.CDLL] = None


def _configure_prototypes(lib: ctypes.CDLL) -> ctypes.CDLL:
    c_void_p = ctypes.c_void_p
    c_char_p = ctypes.c_char_p
    c_int = ctypes.c_int
    c_int32 = ctypes.c_int32
    c_int64 = ctypes.c_int64
    c_uint32 = ctypes.c_uint32
    c_double = ctypes.c_double
    c_bool = ctypes.c_bool
    c_size_t = ctypes.c_size_t
    POINTER = ctypes.POINTER

    # Diagnostics & Version
    lib.oryq_version.argtypes = []
    lib.oryq_version.restype = c_char_p
    lib.oryq_abi_version.argtypes = []
    lib.oryq_abi_version.restype = c_uint32
    lib.oryq_last_error_message.argtypes = []
    lib.oryq_last_error_message.restype = c_char_p
    lib.oryq_free_string.argtypes = [c_void_p]
    lib.oryq_free_string.restype = None

    # Database Lifecycle & Administration
    lib.oryq_db_open.argtypes = [c_char_p, POINTER(c_void_p)]
    lib.oryq_db_open.restype = c_int
    lib.oryq_db_open_with_config.argtypes = [c_char_p, c_char_p, POINTER(c_void_p)]
    lib.oryq_db_open_with_config.restype = c_int
    lib.oryq_db_close.argtypes = [c_void_p]
    lib.oryq_db_close.restype = c_int
    lib.oryq_db_checkpoint.argtypes = [c_void_p]
    lib.oryq_db_checkpoint.restype = c_int
    lib.oryq_db_vacuum.argtypes = [c_void_p]
    lib.oryq_db_vacuum.restype = c_int
    lib.oryq_db_backup_to.argtypes = [c_void_p, c_char_p]
    lib.oryq_db_backup_to.restype = c_int
    lib.oryq_db_integrity_check.argtypes = [c_void_p, POINTER(c_bool)]
    lib.oryq_db_integrity_check.restype = c_int

    # Transactions
    lib.oryq_read_txn_begin.argtypes = [c_void_p, POINTER(c_void_p)]
    lib.oryq_read_txn_begin.restype = c_int
    lib.oryq_read_txn_close.argtypes = [c_void_p]
    lib.oryq_read_txn_close.restype = c_int
    lib.oryq_write_txn_begin.argtypes = [c_void_p, POINTER(c_void_p)]
    lib.oryq_write_txn_begin.restype = c_int
    lib.oryq_write_txn_try_begin.argtypes = [c_void_p, POINTER(c_void_p)]
    lib.oryq_write_txn_try_begin.restype = c_int
    lib.oryq_write_txn_commit.argtypes = [c_void_p]
    lib.oryq_write_txn_commit.restype = c_int
    lib.oryq_write_txn_rollback.argtypes = [c_void_p]
    lib.oryq_write_txn_rollback.restype = c_int

    # Collections, Indexes & CRUD
    lib.oryq_create_collection.argtypes = [c_void_p, c_char_p]
    lib.oryq_create_collection.restype = c_int
    lib.oryq_drop_collection.argtypes = [c_void_p, c_char_p]
    lib.oryq_drop_collection.restype = c_int
    lib.oryq_create_index.argtypes = [
        c_void_p,
        c_char_p,
        c_char_p,
        c_int32,
        c_bool,
        c_bool,
        c_char_p,
    ]
    lib.oryq_create_index.restype = c_int
    lib.oryq_drop_index.argtypes = [c_void_p, c_char_p, c_char_p]
    lib.oryq_drop_index.restype = c_int
    lib.oryq_insert_json.argtypes = [c_void_p, c_char_p, c_char_p]
    lib.oryq_insert_json.restype = c_int
    lib.oryq_insert_many_json.argtypes = [c_void_p, c_char_p, c_char_p, POINTER(c_size_t)]
    lib.oryq_insert_many_json.restype = c_int
    lib.oryq_update_by_id_json.argtypes = [c_void_p, c_char_p, c_char_p, c_char_p]
    lib.oryq_update_by_id_json.restype = c_int
    lib.oryq_update_many_json.argtypes = [
        c_void_p,
        c_char_p,
        c_char_p,
        c_char_p,
        POINTER(c_size_t),
    ]
    lib.oryq_update_many_json.restype = c_int
    lib.oryq_delete_by_id_json.argtypes = [c_void_p, c_char_p, c_char_p]
    lib.oryq_delete_by_id_json.restype = c_int
    lib.oryq_delete_many_json.argtypes = [c_void_p, c_char_p, c_char_p, POINTER(c_size_t)]
    lib.oryq_delete_many_json.restype = c_int
    lib.oryq_upsert_json.argtypes = [c_void_p, c_char_p, c_char_p, POINTER(c_bool)]
    lib.oryq_upsert_json.restype = c_int

    # Query, Aggregation & Zero-Copy Cursor Accessors
    lib.oryq_find_json.argtypes = [c_void_p, c_char_p, c_char_p, POINTER(c_void_p)]
    lib.oryq_find_json.restype = c_int
    lib.oryq_find_with_options_json.argtypes = [
        c_void_p,
        c_char_p,
        c_char_p,
        c_char_p,
        c_char_p,
        c_size_t,
        c_int64,
        POINTER(c_void_p),
    ]
    lib.oryq_find_with_options_json.restype = c_int
    lib.oryq_count_json.argtypes = [c_void_p, c_char_p, c_char_p, POINTER(ctypes.c_uint64)]
    lib.oryq_count_json.restype = c_int
    lib.oryq_aggregate_json.argtypes = [c_void_p, c_char_p, c_char_p, POINTER(c_void_p)]
    lib.oryq_aggregate_json.restype = c_int
    lib.oryq_cursor_next.argtypes = [c_void_p]
    lib.oryq_cursor_next.restype = c_int
    # Use POINTER(c_void_p) so we preserve the raw pointer address for `oryq_free_string`.
    lib.oryq_cursor_next_json.argtypes = [c_void_p, POINTER(c_void_p)]
    lib.oryq_cursor_next_json.restype = c_int
    lib.oryq_cursor_get_i64.argtypes = [c_void_p, c_char_p, POINTER(c_int64)]
    lib.oryq_cursor_get_i64.restype = c_int
    lib.oryq_cursor_get_f64.argtypes = [c_void_p, c_char_p, POINTER(c_double)]
    lib.oryq_cursor_get_f64.restype = c_int
    lib.oryq_cursor_get_bool.argtypes = [c_void_p, c_char_p, POINTER(c_bool)]
    lib.oryq_cursor_get_bool.restype = c_int
    lib.oryq_cursor_get_utf8.argtypes = [
        c_void_p,
        c_char_p,
        POINTER(c_void_p),
        POINTER(c_size_t),
    ]
    lib.oryq_cursor_get_utf8.restype = c_int
    lib.oryq_cursor_get_score.argtypes = [c_void_p, POINTER(c_double)]
    lib.oryq_cursor_get_score.restype = c_int
    lib.oryq_cursor_close.argtypes = [c_void_p]
    lib.oryq_cursor_close.restype = c_int

    # Optional Hybrid FTS & Vector Search (ABI v1.2)
    lib.oryq_create_fts_index.argtypes = [c_void_p, c_char_p, c_char_p, c_char_p]
    lib.oryq_create_fts_index.restype = c_int
    lib.oryq_search_fts.argtypes = [c_void_p, c_char_p, c_char_p, c_char_p, c_size_t, POINTER(c_void_p)]
    lib.oryq_search_fts.restype = c_int
    lib.oryq_create_vector_index.argtypes = [
        c_void_p,
        c_char_p,
        c_char_p,
        c_uint32,
        c_int32,
        c_int32,
        c_char_p,
    ]
    lib.oryq_create_vector_index.restype = c_int
    lib.oryq_search_vector.argtypes = [
        c_void_p,
        c_char_p,
        c_char_p,
        POINTER(ctypes.c_float),
        c_size_t,
        c_size_t,
        POINTER(c_void_p),
    ]
    lib.oryq_search_vector.restype = c_int
    return lib


def load_library(lib_path: Optional[Union[str, os.PathLike[str]]] = None) -> ctypes.CDLL:
    """Loads `liboryq` and configures all C ABI v1.2 prototypes."""
    global _DEFAULT_LIB
    if lib_path is not None:
        return _configure_prototypes(ctypes.CDLL(str(lib_path)))
    if _DEFAULT_LIB is not None:
        return _DEFAULT_LIB

    discovered = _discover_library_path()
    if discovered is not None:
        _DEFAULT_LIB = _configure_prototypes(ctypes.CDLL(str(discovered)))
        return _DEFAULT_LIB

    last_err: Optional[OSError] = None
    for name in _platform_lib_names():
        try:
            _DEFAULT_LIB = _configure_prototypes(ctypes.CDLL(name))
            return _DEFAULT_LIB
        except OSError as exc:
            last_err = exc
    raise OSError(
        f"Could not locate liboryq ({_platform_lib_names()}). "
        "Build with `cargo build -p oryq-ffi` or set ORYQ_LIB_PATH."
    ) from last_err


def _check(lib: ctypes.CDLL, status: int) -> None:
    if status != OryqStatus.OK:
        raw = lib.oryq_last_error_message()
        msg = raw.decode("utf-8", errors="replace") if raw else "unknown OryqDB error"
        raise OryqError(status, msg)


def _cstr(value: str, name: str = "string") -> bytes:
    if "\x00" in value:
        raise OryqError(
            OryqStatus.ERR_INVALID_ARGUMENT,
            f"{name} must not contain interior NUL bytes",
        )
    return value.encode("utf-8")


_MAX_SIZE_T = (1 << (ctypes.sizeof(ctypes.c_size_t) * 8)) - 1


def _require_nonneg_int(value: int, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0 or value > _MAX_SIZE_T:
        raise OryqError(
            OryqStatus.ERR_INVALID_ARGUMENT,
            f"{name} must be an integer in range 0..={_MAX_SIZE_T}",
        )
    return value


def _to_json_bytes(value: Any) -> bytes:
    if isinstance(value, str):
        return _cstr(value, "json")
    raw = json.dumps(value, separators=(",", ":"))
    return _cstr(raw, "json")


class Cursor:
    """Streaming query or aggregation cursor backed by `OryqCursor*`."""

    def __init__(
        self,
        lib: ctypes.CDLL,
        handle: ctypes.c_void_p,
        parent: Optional[ReadTxn] = None,
    ) -> None:
        self._lib = lib
        self._handle: Optional[ctypes.c_void_p] = handle
        self._parent = parent

    def _require_open(self) -> ctypes.c_void_p:
        if self._handle is None:
            raise OryqError(OryqStatus.ERR_INVALID_ARGUMENT, "cursor is already closed")
        return self._handle

    def close(self) -> None:
        if self._handle is not None:
            h = self._handle
            self._handle = None
            self._parent = None
            _check(self._lib, self._lib.oryq_cursor_close(h))

    def __enter__(self) -> Cursor:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def __del__(self) -> None:
        if getattr(self, "_handle", None) is not None:
            try:
                self.close()
            except Exception:
                pass

    def step(self) -> bool:
        """Advances to the next document without JSON serialization (use with `get_*`)."""
        h = self._require_open()
        status = self._lib.oryq_cursor_next(h)
        if status == OryqStatus.CURSOR_DONE:
            return False
        _check(self._lib, status)
        return True

    def next_doc(self) -> Optional[dict[str, Any]]:
        """Advances and returns the next document decoded as a Python `dict`, or `None`."""
        h = self._require_open()
        out_ptr = ctypes.c_void_p()
        status = self._lib.oryq_cursor_next_json(h, ctypes.byref(out_ptr))
        if status == OryqStatus.CURSOR_DONE:
            return None
        _check(self._lib, status)
        if not out_ptr.value:
            return None
        try:
            raw = ctypes.string_at(out_ptr.value)
            return json.loads(raw.decode("utf-8"))
        finally:
            self._lib.oryq_free_string(out_ptr)

    def __iter__(self) -> Iterator[dict[str, Any]]:
        return self

    def __next__(self) -> dict[str, Any]:
        doc = self.next_doc()
        if doc is None:
            raise StopIteration
        return doc

    def get_i64(self, field: str) -> int:
        h = self._require_open()
        out = ctypes.c_int64()
        _check(self._lib, self._lib.oryq_cursor_get_i64(h, _cstr(field, "field"), ctypes.byref(out)))
        return int(out.value)

    def get_f64(self, field: str) -> float:
        h = self._require_open()
        out = ctypes.c_double()
        _check(self._lib, self._lib.oryq_cursor_get_f64(h, _cstr(field, "field"), ctypes.byref(out)))
        return float(out.value)

    def get_bool(self, field: str) -> bool:
        h = self._require_open()
        out = ctypes.c_bool()
        _check(self._lib, self._lib.oryq_cursor_get_bool(h, _cstr(field, "field"), ctypes.byref(out)))
        return bool(out.value)

    def get_utf8(self, field: str) -> str:
        """Reads a top-level UTF-8 string field via zero-copy borrowed slice."""
        h = self._require_open()
        ptr = ctypes.c_void_p()
        length = ctypes.c_size_t()
        _check(
            self._lib,
            self._lib.oryq_cursor_get_utf8(
                h, _cstr(field, "field"), ctypes.byref(ptr), ctypes.byref(length)
            ),
        )
        if length.value == 0:
            return ""
        return ctypes.string_at(ptr.value, length.value).decode("utf-8")

    def get_score(self) -> float:
        """Returns the BM25 relevance score (FTS) or distance (Vector) for the current row."""
        h = self._require_open()
        out = ctypes.c_double()
        _check(self._lib, self._lib.oryq_cursor_get_score(h, ctypes.byref(out)))
        return float(out.value)


class ReadTxn:
    """Snapshot-isolated read transaction backed by `OryqReadTxn*`."""

    def __init__(
        self,
        lib: ctypes.CDLL,
        handle: ctypes.c_void_p,
        parent: Optional[Database] = None,
    ) -> None:
        self._lib = lib
        self._handle: Optional[ctypes.c_void_p] = handle
        self._parent = parent

    def _require_open(self) -> ctypes.c_void_p:
        if self._handle is None:
            raise OryqError(OryqStatus.ERR_INVALID_ARGUMENT, "read transaction is already closed")
        return self._handle

    def find(
        self,
        collection: str,
        filter_doc: Optional[Any] = None,
        *,
        projection: Optional[Any] = None,
        sort: Optional[Any] = None,
        skip: int = 0,
        limit: Optional[int] = None,
    ) -> Cursor:
        h = self._require_open()
        skip = _require_nonneg_int(skip, "skip")
        limit_val = -1 if limit is None else _require_nonneg_int(limit, "limit")
        filter_bytes = _to_json_bytes(filter_doc) if filter_doc is not None else None
        proj_bytes = _to_json_bytes(projection) if projection is not None else None
        sort_bytes = _to_json_bytes(sort) if sort is not None else None
        out_cursor = ctypes.c_void_p()
        _check(
            self._lib,
            self._lib.oryq_find_with_options_json(
                h,
                _cstr(collection, "collection"),
                filter_bytes,
                proj_bytes,
                sort_bytes,
                skip,
                limit_val,
                ctypes.byref(out_cursor),
            ),
        )
        return Cursor(self._lib, out_cursor, parent=self)

    def count(self, collection: str, filter_doc: Optional[Any] = None) -> int:
        """Counts documents in `collection` matching optional `filter_doc` in O(1) memory."""
        h = self._require_open()
        filter_bytes = _to_json_bytes(filter_doc) if filter_doc is not None else None
        out_count = ctypes.c_uint64(0)
        _check(
            self._lib,
            self._lib.oryq_count_json(
                h, _cstr(collection, "collection"), filter_bytes, ctypes.byref(out_count)
            ),
        )
        return int(out_count.value)

    def aggregate(self, collection: str, pipeline: Any) -> Cursor:
        h = self._require_open()
        out_cursor = ctypes.c_void_p()
        _check(
            self._lib,
            self._lib.oryq_aggregate_json(
                h,
                _cstr(collection, "collection"),
                _to_json_bytes(pipeline),
                ctypes.byref(out_cursor),
            ),
        )
        return Cursor(self._lib, out_cursor, parent=self)

    def search_fts(
        self,
        collection: str,
        index_name: str,
        query_text: str,
        *,
        limit: int = 10,
    ) -> Cursor:
        """Executes an Okapi BM25 full-text search (requires `oryq-ffi --features fts`)."""
        limit = _require_nonneg_int(limit, "limit")
        h = self._require_open()
        out_cursor = ctypes.c_void_p()
        _check(
            self._lib,
            self._lib.oryq_search_fts(
                h,
                _cstr(collection, "collection"),
                _cstr(index_name, "index_name"),
                _cstr(query_text, "query_text"),
                limit,
                ctypes.byref(out_cursor),
            ),
        )
        return Cursor(self._lib, out_cursor, parent=self)

    def search_vector(
        self,
        collection: str,
        index_name: str,
        query_vector: Sequence[float],
        *,
        top_k: int = 10,
    ) -> Cursor:
        """Executes a top-k vector similarity search (requires `oryq-ffi --features vec`)."""
        top_k = _require_nonneg_int(top_k, "top_k")
        h = self._require_open()
        n = len(query_vector)
        if n == 0:
            raise OryqError(OryqStatus.ERR_INVALID_ARGUMENT, "query_vector must not be empty")
        arr_type = ctypes.c_float * n
        arr = arr_type(*(float(x) for x in query_vector))
        out_cursor = ctypes.c_void_p()
        _check(
            self._lib,
            self._lib.oryq_search_vector(
                h,
                _cstr(collection, "collection"),
                _cstr(index_name, "index_name"),
                arr,
                n,
                top_k,
                ctypes.byref(out_cursor),
            ),
        )
        return Cursor(self._lib, out_cursor, parent=self)

    def close(self) -> None:
        if self._handle is not None:
            h = self._handle
            self._handle = None
            self._parent = None
            _check(self._lib, self._lib.oryq_read_txn_close(h))

    def __enter__(self) -> ReadTxn:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def __del__(self) -> None:
        if getattr(self, "_handle", None) is not None:
            try:
                self.close()
            except Exception:
                pass


class WriteTxn:
    """Exclusive ACID write transaction backed by `OryqWriteTxn*`.

    Exiting the context manager without calling `commit()` automatically rolls back.
    """

    def __init__(
        self,
        lib: ctypes.CDLL,
        handle: ctypes.c_void_p,
        parent: Optional[Database] = None,
    ) -> None:
        self._lib = lib
        self._handle: Optional[ctypes.c_void_p] = handle
        self._parent = parent

    def _require_open(self) -> ctypes.c_void_p:
        if self._handle is None:
            raise OryqError(OryqStatus.ERR_INVALID_ARGUMENT, "write transaction is already finished")
        return self._handle

    def create_collection(self, name: str) -> None:
        _check(self._lib, self._lib.oryq_create_collection(self._require_open(), _cstr(name, "name")))

    def drop_collection(self, name: str) -> None:
        _check(self._lib, self._lib.oryq_drop_collection(self._require_open(), _cstr(name, "name")))

    def create_index(
        self,
        collection: str,
        field_path: str,
        *,
        sort_order: int = 1,
        unique: bool = False,
        sparse: bool = False,
        name: Optional[str] = None,
    ) -> None:
        _check(
            self._lib,
            self._lib.oryq_create_index(
                self._require_open(),
                _cstr(collection, "collection"),
                _cstr(field_path, "field_path"),
                int(sort_order),
                bool(unique),
                bool(sparse),
                _cstr(name, "name") if name is not None else None,
            ),
        )

    def drop_index(self, collection: str, index_name: str) -> None:
        _check(
            self._lib,
            self._lib.oryq_drop_index(
                self._require_open(),
                _cstr(collection, "collection"),
                _cstr(index_name, "index_name"),
            ),
        )

    def create_fts_index(
        self,
        collection: str,
        fields: Union[str, Sequence[str]],
        *,
        name: Optional[str] = None,
    ) -> None:
        """Creates a BM25 full-text search index (requires `oryq-ffi --features fts`)."""
        fields_csv = fields if isinstance(fields, str) else ",".join(fields)
        _check(
            self._lib,
            self._lib.oryq_create_fts_index(
                self._require_open(),
                _cstr(collection, "collection"),
                _cstr(fields_csv, "fields"),
                _cstr(name, "name") if name is not None else None,
            ),
        )

    def create_vector_index(
        self,
        collection: str,
        field_path: str,
        dimensions: int,
        *,
        metric: VectorMetric = VectorMetric.COSINE,
        quantization: VectorQuantization = VectorQuantization.F32,
        name: Optional[str] = None,
    ) -> None:
        """Creates a dense vector similarity index (requires `oryq-ffi --features vec`)."""
        if isinstance(dimensions, bool) or not isinstance(dimensions, int) or not (0 < dimensions <= 0xFFFF_FFFF):
            raise OryqError(
                OryqStatus.ERR_INVALID_ARGUMENT,
                "dimensions must be an integer in range 1..=4294967295",
            )
        _check(
            self._lib,
            self._lib.oryq_create_vector_index(
                self._require_open(),
                _cstr(collection, "collection"),
                _cstr(field_path, "field_path"),
                dimensions,
                int(metric),
                int(quantization),
                _cstr(name, "name") if name is not None else None,
            ),
        )

    def insert(self, collection: str, doc: Any) -> None:
        _check(
            self._lib,
            self._lib.oryq_insert_json(
                self._require_open(), _cstr(collection, "collection"), _to_json_bytes(doc)
            ),
        )

    def insert_many(self, collection: str, docs: Sequence[Any]) -> int:
        """Inserts a sequence of document objects and returns the number inserted."""
        out_inserted = ctypes.c_size_t(0)
        _check(
            self._lib,
            self._lib.oryq_insert_many_json(
                self._require_open(),
                _cstr(collection, "collection"),
                _to_json_bytes(list(docs)),
                ctypes.byref(out_inserted),
            ),
        )
        return int(out_inserted.value)

    def update_by_id(self, collection: str, doc_id: Any, update: Any) -> None:
        _check(
            self._lib,
            self._lib.oryq_update_by_id_json(
                self._require_open(),
                _cstr(collection, "collection"),
                _cstr(json.dumps(doc_id, separators=(",", ":")), "doc_id"),
                _to_json_bytes(update),
            ),
        )

    def update_many(self, collection: str, filter_doc: Any, update: Any) -> int:
        """Updates all documents matching `filter_doc` and returns the number updated."""
        out_updated = ctypes.c_size_t(0)
        _check(
            self._lib,
            self._lib.oryq_update_many_json(
                self._require_open(),
                _cstr(collection, "collection"),
                _to_json_bytes(filter_doc),
                _to_json_bytes(update),
                ctypes.byref(out_updated),
            ),
        )
        return int(out_updated.value)

    def delete_by_id(self, collection: str, doc_id: Any) -> None:
        _check(
            self._lib,
            self._lib.oryq_delete_by_id_json(
                self._require_open(),
                _cstr(collection, "collection"),
                _cstr(json.dumps(doc_id, separators=(",", ":")), "doc_id"),
            ),
        )

    def delete_many(self, collection: str, filter_doc: Any) -> int:
        """Deletes all documents matching `filter_doc` and returns the number deleted."""
        out_deleted = ctypes.c_size_t(0)
        _check(
            self._lib,
            self._lib.oryq_delete_many_json(
                self._require_open(),
                _cstr(collection, "collection"),
                _to_json_bytes(filter_doc),
                ctypes.byref(out_deleted),
            ),
        )
        return int(out_deleted.value)

    def upsert(self, collection: str, doc: Any) -> bool:
        """Upserts `doc` by `_id`; returns True if inserted, False if an existing doc was replaced."""
        out_inserted = ctypes.c_bool(False)
        _check(
            self._lib,
            self._lib.oryq_upsert_json(
                self._require_open(),
                _cstr(collection, "collection"),
                _to_json_bytes(doc),
                ctypes.byref(out_inserted),
            ),
        )
        return bool(out_inserted.value)

    def commit(self) -> None:
        h = self._require_open()
        self._handle = None
        self._parent = None
        _check(self._lib, self._lib.oryq_write_txn_commit(h))

    def rollback(self) -> None:
        if self._handle is not None:
            h = self._handle
            self._handle = None
            self._parent = None
            _check(self._lib, self._lib.oryq_write_txn_rollback(h))

    def __enter__(self) -> WriteTxn:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        if self._handle is not None:
            self.rollback()

    def __del__(self) -> None:
        if getattr(self, "_handle", None) is not None:
            try:
                self.rollback()
            except Exception:
                pass


class Database:
    """Embedded OryqDB database handle backed by `OryqDb*`."""

    def __init__(
        self,
        lib: ctypes.CDLL,
        handle: ctypes.c_void_p,
        parent: Optional[Database] = None,
    ) -> None:
        self._lib = lib
        self._handle: Optional[ctypes.c_void_p] = handle
        self._parent = parent

    @classmethod
    def open(
        cls,
        path: Union[str, os.PathLike[str]],
        *,
        config: Optional[Any] = None,
        lib_path: Optional[Union[str, os.PathLike[str]]] = None,
    ) -> Database:
        """Opens or creates an OryqDB database (pass `":memory:"` for an ephemeral in-memory DB)."""
        lib = load_library(lib_path)
        out_db = ctypes.c_void_p()
        if config is None:
            _check(lib, lib.oryq_db_open(_cstr(str(path), "path"), ctypes.byref(out_db)))
        else:
            _check(
                lib,
                lib.oryq_db_open_with_config(
                    _cstr(str(path), "path"), _to_json_bytes(config), ctypes.byref(out_db)
                ),
            )
        return cls(lib, out_db)

    @classmethod
    def open_in_memory(
        cls,
        *,
        lib_path: Optional[Union[str, os.PathLike[str]]] = None,
    ) -> Database:
        """Opens a fresh, isolated, zero-disk in-memory database (`":memory:"`)."""
        return cls.open(":memory:", lib_path=lib_path)

    def _require_open(self) -> ctypes.c_void_p:
        if self._handle is None:
            raise OryqError(OryqStatus.ERR_INVALID_ARGUMENT, "database is already closed")
        return self._handle

    def version(self) -> str:
        raw = self._lib.oryq_version()
        return raw.decode("utf-8") if raw else ""

    def abi_version(self) -> Tuple[int, int]:
        packed = int(self._lib.oryq_abi_version())
        return (packed >> 16, packed & 0xFFFF)

    def read_txn(self) -> ReadTxn:
        out_txn = ctypes.c_void_p()
        _check(self._lib, self._lib.oryq_read_txn_begin(self._require_open(), ctypes.byref(out_txn)))
        return ReadTxn(self._lib, out_txn, parent=self)

    def write_txn(self) -> WriteTxn:
        out_txn = ctypes.c_void_p()
        _check(self._lib, self._lib.oryq_write_txn_begin(self._require_open(), ctypes.byref(out_txn)))
        return WriteTxn(self._lib, out_txn, parent=self)

    def try_write_txn(self) -> WriteTxn:
        """Non-blocking exclusive write transaction acquisition (raises ERR_BUSY if contended)."""
        out_txn = ctypes.c_void_p()
        _check(
            self._lib,
            self._lib.oryq_write_txn_try_begin(self._require_open(), ctypes.byref(out_txn)),
        )
        return WriteTxn(self._lib, out_txn, parent=self)

    def checkpoint(self) -> None:
        _check(self._lib, self._lib.oryq_db_checkpoint(self._require_open()))

    def vacuum(self) -> None:
        """Compacts the database in place, reclaiming free space and shrinking the `.oryq` file."""
        _check(self._lib, self._lib.oryq_db_vacuum(self._require_open()))

    def backup_to(self, dest_path: Union[str, os.PathLike[str]]) -> None:
        _check(
            self._lib,
            self._lib.oryq_db_backup_to(self._require_open(), _cstr(str(dest_path), "dest_path")),
        )

    def integrity_check(self) -> bool:
        out_ok = ctypes.c_bool()
        _check(
            self._lib,
            self._lib.oryq_db_integrity_check(self._require_open(), ctypes.byref(out_ok)),
        )
        return bool(out_ok.value)

    def close(self) -> None:
        if self._handle is not None:
            _check(self._lib, self._lib.oryq_db_close(self._handle))
            self._handle = None

    def __enter__(self) -> Database:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def __del__(self) -> None:
        if getattr(self, "_handle", None) is not None:
            try:
                self.close()
            except Exception:
                pass
