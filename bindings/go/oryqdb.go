// Package oryqdb provides official cgo bindings for OryqDB (C ABI v1.2, include/oryq.h).
//
// License: MIT
package oryqdb

/*
#cgo CFLAGS: -I${SRCDIR}/../../include
#cgo linux LDFLAGS: -L${SRCDIR}/../../target/debug -L${SRCDIR}/../../target/release -loryq -Wl,-rpath,${SRCDIR}/../../target/debug -Wl,-rpath,${SRCDIR}/../../target/release -lm
#cgo darwin LDFLAGS: -L${SRCDIR}/../../target/debug -L${SRCDIR}/../../target/release -loryq -Wl,-rpath,${SRCDIR}/../../target/debug -Wl,-rpath,${SRCDIR}/../../target/release -lm
#cgo windows LDFLAGS: -L${SRCDIR}/../../target/debug -L${SRCDIR}/../../target/release -loryq
#include <stdlib.h>
#include <stdint.h>
#include <stdbool.h>
#include "oryq.h"
*/
import "C"

import (
	"errors"
	"fmt"
	"strings"
	"unsafe"
)

// Status mirrors oryq_status in include/oryq.h (ABI v1.2).
type Status int32

const (
	StatusOK                    Status = 0
	StatusCursorDone            Status = 1
	StatusErrInvalidArgument    Status = 2
	StatusErrIO                 Status = 3
	StatusErrCorruption         Status = 4
	StatusErrNotFound           Status = 5
	StatusErrDuplicateKey       Status = 6
	StatusErrLockedByProcess    Status = 7
	StatusErrSnapshotTooOld     Status = 8
	StatusErrQueryMemoryLimit   Status = 9
	StatusErrParallelArrayIndex Status = 10
	StatusErrUnsupportedFeature Status = 11
	StatusErrTypeMismatch       Status = 12
	StatusErrBusy               Status = 13
	StatusErrCommitUnknown      Status = 14
	StatusErrWalLimit           Status = 15
	StatusErrTooManyReaders     Status = 16
	StatusErrTransactionMemory  Status = 17
	StatusErrNumericOverflow    Status = 18
	StatusErrNumericPrecision   Status = 19
	StatusErrInvalidQuery       Status = 20
	StatusErrDocumentTooLarge   Status = 21
	StatusErrIndexKeyTooLarge   Status = 22
	StatusErrInvalidIDType      Status = 23
	StatusErrConflictingPath    Status = 24
	StatusErrIndexNotFound      Status = 25
	StatusErrInternalPanic      Status = 255
)

// VectorMetric selects the distance function for vector similarity indexes.
type VectorMetric int32

const (
	VectorMetricCosine     VectorMetric = 0
	VectorMetricL2         VectorMetric = 1
	VectorMetricDotProduct VectorMetric = 2
)

// VectorQuantization selects vector storage quantization.
type VectorQuantization int32

const (
	VectorQuantF32  VectorQuantization = 0
	VectorQuantInt8 VectorQuantization = 1
)

// Error represents a non-OK status returned by liboryq.
type Error struct {
	Status  Status
	Message string
}

func (e *Error) Error() string {
	return fmt.Sprintf("oryqdb error (status=%d): %s", e.Status, e.Message)
}

func checkStatus(st C.oryq_status) error {
	s := Status(st)
	if s == StatusOK {
		return nil
	}
	msg := "unknown OryqDB error"
	if raw := C.oryq_last_error_message(); raw != nil {
		msg = C.GoString(raw)
	}
	return &Error{Status: s, Message: msg}
}

// Version returns the native liboryq version string.
func Version() string {
	if raw := C.oryq_version(); raw != nil {
		return C.GoString(raw)
	}
	return ""
}

// ABIVersion returns the (major, minor) C ABI version pair.
func ABIVersion() (uint16, uint16) {
	packed := uint32(C.oryq_abi_version())
	return uint16(packed >> 16), uint16(packed & 0xFFFF)
}

// Database wraps an open OryqDb* handle.
type Database struct {
	ptr *C.OryqDb
}

// Open opens or creates a database at path (pass ":memory:" for an ephemeral in-memory DB).
func Open(path string) (*Database, error) {
	cPath := C.CString(path)
	defer C.free(unsafe.Pointer(cPath))
	var out *C.OryqDb
	if err := checkStatus(C.oryq_db_open(cPath, &out)); err != nil {
		return nil, err
	}
	return &Database{ptr: out}, nil
}

// OpenInMemory opens a fresh, isolated, zero-disk in-memory database (":memory:").
func OpenInMemory() (*Database, error) {
	return Open(":memory:")
}

func (db *Database) requireOpen() (*C.OryqDb, error) {
	if db == nil || db.ptr == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "database is closed"}
	}
	return db.ptr, nil
}

// Close closes the database handle. Returns StatusErrBusy if any child transaction is still open.
func (db *Database) Close() error {
	if db == nil || db.ptr == nil {
		return nil
	}
	if err := checkStatus(C.oryq_db_close(db.ptr)); err != nil {
		return err
	}
	db.ptr = nil
	return nil
}

// Checkpoint runs a WAL checkpoint and seals the database if idle.
func (db *Database) Checkpoint() error {
	ptr, err := db.requireOpen()
	if err != nil {
		return err
	}
	return checkStatus(C.oryq_db_checkpoint(ptr))
}

// Vacuum compacts the database in place, reclaiming free pages and shrinking the .oryq file.
func (db *Database) Vacuum() error {
	ptr, err := db.requireOpen()
	if err != nil {
		return err
	}
	return checkStatus(C.oryq_db_vacuum(ptr))
}

// BackupTo creates a standalone sealed online backup at destPath.
func (db *Database) BackupTo(destPath string) error {
	ptr, err := db.requireOpen()
	if err != nil {
		return err
	}
	cDest := C.CString(destPath)
	defer C.free(unsafe.Pointer(cDest))
	return checkStatus(C.oryq_db_backup_to(ptr, cDest))
}

// IntegrityCheck verifies snapshot-consistent collection and index integrity.
func (db *Database) IntegrityCheck() (bool, error) {
	ptr, err := db.requireOpen()
	if err != nil {
		return false, err
	}
	var ok C.bool
	if err := checkStatus(C.oryq_db_integrity_check(ptr, &ok)); err != nil {
		return false, err
	}
	return bool(ok), nil
}

// BeginRead starts a snapshot-isolated ReadTxn.
func (db *Database) BeginRead() (*ReadTxn, error) {
	ptr, err := db.requireOpen()
	if err != nil {
		return nil, err
	}
	var out *C.OryqReadTxn
	if err := checkStatus(C.oryq_read_txn_begin(ptr, &out)); err != nil {
		return nil, err
	}
	return &ReadTxn{ptr: out}, nil
}

// BeginWrite starts an exclusive ACID WriteTxn.
func (db *Database) BeginWrite() (*WriteTxn, error) {
	ptr, err := db.requireOpen()
	if err != nil {
		return nil, err
	}
	var out *C.OryqWriteTxn
	if err := checkStatus(C.oryq_write_txn_begin(ptr, &out)); err != nil {
		return nil, err
	}
	return &WriteTxn{ptr: out}, nil
}

// WriteTxn wraps an exclusive OryqWriteTxn* handle.
type WriteTxn struct {
	ptr *C.OryqWriteTxn
}

func (tx *WriteTxn) requireOpen() (*C.OryqWriteTxn, error) {
	if tx == nil || tx.ptr == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "write transaction is finalized"}
	}
	return tx.ptr, nil
}

// CreateCollection creates a new collection.
func (tx *WriteTxn) CreateCollection(name string) error {
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	cName := C.CString(name)
	defer C.free(unsafe.Pointer(cName))
	return checkStatus(C.oryq_create_collection(ptr, cName))
}

// DropCollection drops an existing collection.
func (tx *WriteTxn) DropCollection(name string) error {
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	cName := C.CString(name)
	defer C.free(unsafe.Pointer(cName))
	return checkStatus(C.oryq_drop_collection(ptr, cName))
}

// CreateIndex creates a secondary index on fieldPath (sortOrder: 1 for asc, -1 for desc).
func (tx *WriteTxn) CreateIndex(collection, fieldPath string, sortOrder int32, unique, sparse bool, name string) error {
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	cCol := C.CString(collection)
	defer C.free(unsafe.Pointer(cCol))
	cField := C.CString(fieldPath)
	defer C.free(unsafe.Pointer(cField))
	var cName *C.char
	if name != "" {
		cName = C.CString(name)
		defer C.free(unsafe.Pointer(cName))
	}
	return checkStatus(C.oryq_create_index(ptr, cCol, cField, C.int32_t(sortOrder), C.bool(unique), C.bool(sparse), cName))
}

// DropIndex drops a non-primary index by name.
func (tx *WriteTxn) DropIndex(collection, indexName string) error {
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	cCol := C.CString(collection)
	defer C.free(unsafe.Pointer(cCol))
	cIdx := C.CString(indexName)
	defer C.free(unsafe.Pointer(cIdx))
	return checkStatus(C.oryq_drop_index(ptr, cCol, cIdx))
}

// CreateFtsIndex creates a BM25 full-text index (requires liboryq built with --features fts).
func (tx *WriteTxn) CreateFtsIndex(collection string, fields []string, name string) error {
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	cCol := C.CString(collection)
	defer C.free(unsafe.Pointer(cCol))
	cFields := C.CString(strings.Join(fields, ","))
	defer C.free(unsafe.Pointer(cFields))
	var cName *C.char
	if name != "" {
		cName = C.CString(name)
		defer C.free(unsafe.Pointer(cName))
	}
	return checkStatus(C.oryq_create_fts_index(ptr, cCol, cFields, cName))
}

// CreateVectorIndex creates a vector similarity index (requires liboryq built with --features vec).
func (tx *WriteTxn) CreateVectorIndex(collection, fieldPath string, dimensions uint32, metric VectorMetric, quant VectorQuantization, name string) error {
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	cCol := C.CString(collection)
	defer C.free(unsafe.Pointer(cCol))
	cField := C.CString(fieldPath)
	defer C.free(unsafe.Pointer(cField))
	var cName *C.char
	if name != "" {
		cName = C.CString(name)
		defer C.free(unsafe.Pointer(cName))
	}
	return checkStatus(C.oryq_create_vector_index(ptr, cCol, cField, C.uint32_t(dimensions), C.int32_t(metric), C.int32_t(quant), cName))
}

// InsertJSON inserts a JSON document into collection.
func (tx *WriteTxn) InsertJSON(collection, docJSON string) error {
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	cCol := C.CString(collection)
	defer C.free(unsafe.Pointer(cCol))
	cDoc := C.CString(docJSON)
	defer C.free(unsafe.Pointer(cDoc))
	return checkStatus(C.oryq_insert_json(ptr, cCol, cDoc))
}

// UpdateByIDJSON applies a MongoDB-style update operator JSON to the document with _id=idJSON.
func (tx *WriteTxn) UpdateByIDJSON(collection, idJSON, updateJSON string) error {
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	cCol := C.CString(collection)
	defer C.free(unsafe.Pointer(cCol))
	cID := C.CString(idJSON)
	defer C.free(unsafe.Pointer(cID))
	cUpd := C.CString(updateJSON)
	defer C.free(unsafe.Pointer(cUpd))
	return checkStatus(C.oryq_update_by_id_json(ptr, cCol, cID, cUpd))
}

// DeleteByIDJSON deletes the document with _id=idJSON.
func (tx *WriteTxn) DeleteByIDJSON(collection, idJSON string) error {
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	cCol := C.CString(collection)
	defer C.free(unsafe.Pointer(cCol))
	cID := C.CString(idJSON)
	defer C.free(unsafe.Pointer(cID))
	return checkStatus(C.oryq_delete_by_id_json(ptr, cCol, cID))
}

// Commit commits the write transaction and consumes the handle.
func (tx *WriteTxn) Commit() error {
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	tx.ptr = nil
	return checkStatus(C.oryq_write_txn_commit(ptr))
}

// Rollback aborts the write transaction and consumes the handle (idempotent if already finalized).
func (tx *WriteTxn) Rollback() error {
	if tx == nil || tx.ptr == nil {
		return nil
	}
	ptr := tx.ptr
	tx.ptr = nil
	return checkStatus(C.oryq_write_txn_rollback(ptr))
}

// ReadTxn wraps a snapshot-isolated OryqReadTxn* handle.
type ReadTxn struct {
	ptr *C.OryqReadTxn
}

func (tx *ReadTxn) requireOpen() (*C.OryqReadTxn, error) {
	if tx == nil || tx.ptr == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "read transaction is closed"}
	}
	return tx.ptr, nil
}

// Close closes the read transaction handle.
func (tx *ReadTxn) Close() error {
	if tx == nil || tx.ptr == nil {
		return nil
	}
	ptr := tx.ptr
	tx.ptr = nil
	return checkStatus(C.oryq_read_txn_close(ptr))
}

// FindJSON queries collection with an optional JSON filter (empty string matches all documents).
func (tx *ReadTxn) FindJSON(collection, filterJSON string) (*Cursor, error) {
	ptr, err := tx.requireOpen()
	if err != nil {
		return nil, err
	}
	cCol := C.CString(collection)
	defer C.free(unsafe.Pointer(cCol))
	var cFilter *C.char
	if filterJSON != "" {
		cFilter = C.CString(filterJSON)
		defer C.free(unsafe.Pointer(cFilter))
	}
	var out *C.OryqCursor
	if err := checkStatus(C.oryq_find_json(ptr, cCol, cFilter, &out)); err != nil {
		return nil, err
	}
	return &Cursor{ptr: out}, nil
}

// AggregateJSON executes a JSON aggregation pipeline on collection.
func (tx *ReadTxn) AggregateJSON(collection, pipelineJSON string) (*Cursor, error) {
	ptr, err := tx.requireOpen()
	if err != nil {
		return nil, err
	}
	cCol := C.CString(collection)
	defer C.free(unsafe.Pointer(cCol))
	cPipe := C.CString(pipelineJSON)
	defer C.free(unsafe.Pointer(cPipe))
	var out *C.OryqCursor
	if err := checkStatus(C.oryq_aggregate_json(ptr, cCol, cPipe, &out)); err != nil {
		return nil, err
	}
	return &Cursor{ptr: out}, nil
}

// SearchFts executes a BM25 full-text search (requires liboryq built with --features fts).
func (tx *ReadTxn) SearchFts(collection, indexName, queryText string, limit int) (*Cursor, error) {
	ptr, err := tx.requireOpen()
	if err != nil {
		return nil, err
	}
	if limit < 0 {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "limit must be >= 0"}
	}
	cCol := C.CString(collection)
	defer C.free(unsafe.Pointer(cCol))
	cIdx := C.CString(indexName)
	defer C.free(unsafe.Pointer(cIdx))
	cQuery := C.CString(queryText)
	defer C.free(unsafe.Pointer(cQuery))
	var out *C.OryqCursor
	if err := checkStatus(C.oryq_search_fts(ptr, cCol, cIdx, cQuery, C.size_t(limit), &out)); err != nil {
		return nil, err
	}
	return &Cursor{ptr: out}, nil
}

// SearchVector executes a top-k vector similarity search (requires liboryq built with --features vec).
func (tx *ReadTxn) SearchVector(collection, indexName string, queryVec []float32, topK int) (*Cursor, error) {
	ptr, err := tx.requireOpen()
	if err != nil {
		return nil, err
	}
	if len(queryVec) == 0 {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "queryVec must not be empty"}
	}
	if topK < 0 {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "topK must be >= 0"}
	}
	cCol := C.CString(collection)
	defer C.free(unsafe.Pointer(cCol))
	cIdx := C.CString(indexName)
	defer C.free(unsafe.Pointer(cIdx))
	var out *C.OryqCursor
	if err := checkStatus(C.oryq_search_vector(
		ptr,
		cCol,
		cIdx,
		(*C.float)(unsafe.Pointer(&queryVec[0])),
		C.size_t(len(queryVec)),
		C.size_t(topK),
		&out,
	)); err != nil {
		return nil, err
	}
	return &Cursor{ptr: out}, nil
}

// Cursor wraps a streaming OryqCursor* result handle.
type Cursor struct {
	ptr *C.OryqCursor
}

func (c *Cursor) requireOpen() (*C.OryqCursor, error) {
	if c == nil || c.ptr == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "cursor is closed"}
	}
	return c.ptr, nil
}

// Close frees the cursor handle.
func (c *Cursor) Close() error {
	if c == nil || c.ptr == nil {
		return nil
	}
	ptr := c.ptr
	c.ptr = nil
	return checkStatus(C.oryq_cursor_close(ptr))
}

// Next advances the cursor without JSON serialization (use with Get* accessors).
// Returns (false, nil) when exhausted.
func (c *Cursor) Next() (bool, error) {
	ptr, err := c.requireOpen()
	if err != nil {
		return false, err
	}
	st := C.oryq_cursor_next(ptr)
	if Status(st) == StatusCursorDone {
		return false, nil
	}
	if err := checkStatus(st); err != nil {
		return false, err
	}
	return true, nil
}

// NextJSON advances the cursor and returns the next document as a JSON string.
// Returns ("", false, nil) when exhausted.
func (c *Cursor) NextJSON() (string, bool, error) {
	ptr, err := c.requireOpen()
	if err != nil {
		return "", false, err
	}
	var raw *C.char
	st := C.oryq_cursor_next_json(ptr, &raw)
	if Status(st) == StatusCursorDone {
		return "", false, nil
	}
	if err := checkStatus(st); err != nil {
		return "", false, err
	}
	if raw == nil {
		return "", false, nil
	}
	defer C.oryq_free_string(raw)
	return C.GoString(raw), true, nil
}

// GetI64 reads a top-level int64 field from the current cursor document.
func (c *Cursor) GetI64(field string) (int64, error) {
	ptr, err := c.requireOpen()
	if err != nil {
		return 0, err
	}
	cField := C.CString(field)
	defer C.free(unsafe.Pointer(cField))
	var out C.int64_t
	if err := checkStatus(C.oryq_cursor_get_i64(ptr, cField, &out)); err != nil {
		return 0, err
	}
	return int64(out), nil
}

// GetF64 reads a top-level float64 field from the current cursor document.
func (c *Cursor) GetF64(field string) (float64, error) {
	ptr, err := c.requireOpen()
	if err != nil {
		return 0, err
	}
	cField := C.CString(field)
	defer C.free(unsafe.Pointer(cField))
	var out C.double
	if err := checkStatus(C.oryq_cursor_get_f64(ptr, cField, &out)); err != nil {
		return 0, err
	}
	return float64(out), nil
}

// GetBool reads a top-level bool field from the current cursor document.
func (c *Cursor) GetBool(field string) (bool, error) {
	ptr, err := c.requireOpen()
	if err != nil {
		return false, err
	}
	cField := C.CString(field)
	defer C.free(unsafe.Pointer(cField))
	var out C.bool
	if err := checkStatus(C.oryq_cursor_get_bool(ptr, cField, &out)); err != nil {
		return false, err
	}
	return bool(out), nil
}

// GetUTF8 reads a top-level UTF-8 string field from the current cursor document.
func (c *Cursor) GetUTF8(field string) (string, error) {
	ptr, err := c.requireOpen()
	if err != nil {
		return "", err
	}
	cField := C.CString(field)
	defer C.free(unsafe.Pointer(cField))
	var raw *C.char
	var length C.size_t
	if err := checkStatus(C.oryq_cursor_get_utf8(ptr, cField, &raw, &length)); err != nil {
		return "", err
	}
	if length == 0 {
		return "", nil
	}
	return C.GoStringN(raw, C.int(length)), nil
}

// GetScore returns the BM25 score (FTS) or distance (Vector) for the current scored row.
func (c *Cursor) GetScore() (float64, error) {
	ptr, err := c.requireOpen()
	if err != nil {
		return 0, err
	}
	var out C.double
	if err := checkStatus(C.oryq_cursor_get_score(ptr, &out)); err != nil {
		return 0, err
	}
	return float64(out), nil
}

// IsStatus checks whether err is an *Error with the expected Status code.
func IsStatus(err error, status Status) bool {
	var oe *Error
	return errors.As(err, &oe) && oe.Status == status
}
