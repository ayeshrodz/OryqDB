// Package oryqdb provides official cgo bindings for OryqDB (C ABI v1.2, include/oryq.h).
//
// License: MIT
package oryqdb

/*
#cgo CFLAGS: -I${SRCDIR}/../../include
#cgo linux LDFLAGS: -L${SRCDIR}/../../target/release -L${SRCDIR}/../../target/debug -loryq -Wl,-rpath,${SRCDIR}/../../target/release -Wl,-rpath,${SRCDIR}/../../target/debug -lm
#cgo darwin LDFLAGS: -L${SRCDIR}/../../target/release -L${SRCDIR}/../../target/debug -loryq -Wl,-rpath,${SRCDIR}/../../target/release -Wl,-rpath,${SRCDIR}/../../target/debug -lm
#cgo windows LDFLAGS: -L${SRCDIR}/../../target/release -L${SRCDIR}/../../target/debug -loryq
#include <stdlib.h>
#include <stdint.h>
#include <stdbool.h>
#include "oryq.h"
*/
import "C"

import (
	"errors"
	"fmt"
	"math"
	"runtime"
	"strings"
	"sync"
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

// callStatus pins the goroutine to the current OS thread across the FFI call and
// `oryq_last_error_message()` lookup so thread-local error diagnostics are never racing.
func callStatus(fn func() C.oryq_status) error {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	s := Status(fn())
	if s == StatusOK {
		return nil
	}
	msg := "unknown OryqDB error"
	if raw := C.oryq_last_error_message(); raw != nil {
		msg = C.GoString(raw)
	}
	return &Error{Status: s, Message: msg}
}

// callCursorStatus pins the OS thread for cursor-advancing calls where StatusCursorDone is normal.
func callCursorStatus(fn func() C.oryq_status) (bool, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	s := Status(fn())
	if s == StatusOK {
		return true, nil
	}
	if s == StatusCursorDone {
		return false, nil
	}
	msg := "unknown OryqDB error"
	if raw := C.oryq_last_error_message(); raw != nil {
		msg = C.GoString(raw)
	}
	return false, &Error{Status: s, Message: msg}
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

func cStringChecked(s string) (*C.char, error) {
	if strings.IndexByte(s, 0) >= 0 {
		return nil, &Error{
			Status:  StatusErrInvalidArgument,
			Message: "string argument must not contain interior NUL bytes",
		}
	}
	return C.CString(s), nil
}

// Database wraps an open OryqDb* handle with goroutine-safe lifecycle locking.
type Database struct {
	mu  sync.RWMutex
	ptr *C.OryqDb
}

// Open opens or creates a database at path (pass ":memory:" for an ephemeral in-memory DB).
func Open(path string) (*Database, error) {
	cPath, err := cStringChecked(path)
	if err != nil {
		return nil, err
	}
	defer C.free(unsafe.Pointer(cPath))
	var out *C.OryqDb
	if err := callStatus(func() C.oryq_status { return C.oryq_db_open(cPath, &out) }); err != nil {
		return nil, err
	}
	return &Database{ptr: out}, nil
}

// OpenInMemory opens a fresh, isolated, zero-disk in-memory database (":memory:").
func OpenInMemory() (*Database, error) {
	return Open(":memory:")
}

func (db *Database) requireOpenLocked() (*C.OryqDb, error) {
	if db == nil || db.ptr == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "database is closed"}
	}
	return db.ptr, nil
}

// Close closes the database handle. Returns StatusErrBusy if any child transaction is still open.
func (db *Database) Close() error {
	if db == nil {
		return nil
	}
	db.mu.Lock()
	defer db.mu.Unlock()
	if db.ptr == nil {
		return nil
	}
	ptr := db.ptr
	if err := callStatus(func() C.oryq_status { return C.oryq_db_close(ptr) }); err != nil {
		return err
	}
	db.ptr = nil
	return nil
}

// Checkpoint runs a WAL checkpoint and seals the database if idle.
func (db *Database) Checkpoint() error {
	if db == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "database is closed"}
	}
	db.mu.RLock()
	defer db.mu.RUnlock()
	ptr, err := db.requireOpenLocked()
	if err != nil {
		return err
	}
	return callStatus(func() C.oryq_status { return C.oryq_db_checkpoint(ptr) })
}

// Vacuum compacts the database in place, reclaiming free pages and shrinking the .oryq file.
func (db *Database) Vacuum() error {
	if db == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "database is closed"}
	}
	db.mu.RLock()
	defer db.mu.RUnlock()
	ptr, err := db.requireOpenLocked()
	if err != nil {
		return err
	}
	return callStatus(func() C.oryq_status { return C.oryq_db_vacuum(ptr) })
}

// BackupTo creates a standalone sealed online backup at destPath.
func (db *Database) BackupTo(destPath string) error {
	if db == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "database is closed"}
	}
	cDest, err := cStringChecked(destPath)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cDest))
	db.mu.RLock()
	defer db.mu.RUnlock()
	ptr, err := db.requireOpenLocked()
	if err != nil {
		return err
	}
	return callStatus(func() C.oryq_status { return C.oryq_db_backup_to(ptr, cDest) })
}

// IntegrityCheck verifies snapshot-consistent collection and index integrity.
func (db *Database) IntegrityCheck() (bool, error) {
	if db == nil {
		return false, &Error{Status: StatusErrInvalidArgument, Message: "database is closed"}
	}
	db.mu.RLock()
	defer db.mu.RUnlock()
	ptr, err := db.requireOpenLocked()
	if err != nil {
		return false, err
	}
	var ok C.bool
	if err := callStatus(func() C.oryq_status { return C.oryq_db_integrity_check(ptr, &ok) }); err != nil {
		return false, err
	}
	return bool(ok), nil
}

// BeginRead starts a snapshot-isolated ReadTxn.
func (db *Database) BeginRead() (*ReadTxn, error) {
	if db == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "database is closed"}
	}
	db.mu.RLock()
	defer db.mu.RUnlock()
	ptr, err := db.requireOpenLocked()
	if err != nil {
		return nil, err
	}
	var out *C.OryqReadTxn
	if err := callStatus(func() C.oryq_status { return C.oryq_read_txn_begin(ptr, &out) }); err != nil {
		return nil, err
	}
	return &ReadTxn{ptr: out, parent: db}, nil
}

// BeginWrite starts an exclusive ACID WriteTxn.
func (db *Database) BeginWrite() (*WriteTxn, error) {
	if db == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "database is closed"}
	}
	db.mu.RLock()
	defer db.mu.RUnlock()
	ptr, err := db.requireOpenLocked()
	if err != nil {
		return nil, err
	}
	var out *C.OryqWriteTxn
	if err := callStatus(func() C.oryq_status { return C.oryq_write_txn_begin(ptr, &out) }); err != nil {
		return nil, err
	}
	return &WriteTxn{ptr: out, parent: db}, nil
}

// WriteTxn wraps an exclusive OryqWriteTxn* handle with mutex synchronization.
type WriteTxn struct {
	mu     sync.Mutex
	ptr    *C.OryqWriteTxn
	parent *Database
}

func (tx *WriteTxn) requireOpen() (*C.OryqWriteTxn, error) {
	if tx == nil || tx.ptr == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "write transaction is finalized"}
	}
	return tx.ptr, nil
}

// CreateCollection creates a new collection.
func (tx *WriteTxn) CreateCollection(name string) error {
	if tx == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "write transaction is finalized"}
	}
	cName, err := cStringChecked(name)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cName))
	tx.mu.Lock()
	defer tx.mu.Unlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	return callStatus(func() C.oryq_status { return C.oryq_create_collection(ptr, cName) })
}

// DropCollection drops an existing collection.
func (tx *WriteTxn) DropCollection(name string) error {
	if tx == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "write transaction is finalized"}
	}
	cName, err := cStringChecked(name)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cName))
	tx.mu.Lock()
	defer tx.mu.Unlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	return callStatus(func() C.oryq_status { return C.oryq_drop_collection(ptr, cName) })
}

// CreateIndex creates a secondary index on fieldPath (sortOrder: 1 for asc, -1 for desc).
func (tx *WriteTxn) CreateIndex(collection, fieldPath string, sortOrder int32, unique, sparse bool, name string) error {
	if tx == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "write transaction is finalized"}
	}
	cCol, err := cStringChecked(collection)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cCol))
	cField, err := cStringChecked(fieldPath)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cField))
	var cName *C.char
	if name != "" {
		cName, err = cStringChecked(name)
		if err != nil {
			return err
		}
		defer C.free(unsafe.Pointer(cName))
	}
	tx.mu.Lock()
	defer tx.mu.Unlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	return callStatus(func() C.oryq_status {
		return C.oryq_create_index(ptr, cCol, cField, C.int32_t(sortOrder), C.bool(unique), C.bool(sparse), cName)
	})
}

// DropIndex drops a non-primary index by name.
func (tx *WriteTxn) DropIndex(collection, indexName string) error {
	if tx == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "write transaction is finalized"}
	}
	cCol, err := cStringChecked(collection)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cCol))
	cIdx, err := cStringChecked(indexName)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cIdx))
	tx.mu.Lock()
	defer tx.mu.Unlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	return callStatus(func() C.oryq_status { return C.oryq_drop_index(ptr, cCol, cIdx) })
}

// CreateFtsIndex creates a BM25 full-text index (requires liboryq built with --features fts).
func (tx *WriteTxn) CreateFtsIndex(collection string, fields []string, name string) error {
	if tx == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "write transaction is finalized"}
	}
	cCol, err := cStringChecked(collection)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cCol))
	cFields, err := cStringChecked(strings.Join(fields, ","))
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cFields))
	var cName *C.char
	if name != "" {
		cName, err = cStringChecked(name)
		if err != nil {
			return err
		}
		defer C.free(unsafe.Pointer(cName))
	}
	tx.mu.Lock()
	defer tx.mu.Unlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	return callStatus(func() C.oryq_status { return C.oryq_create_fts_index(ptr, cCol, cFields, cName) })
}

// CreateVectorIndex creates a vector similarity index (requires liboryq built with --features vec).
func (tx *WriteTxn) CreateVectorIndex(collection, fieldPath string, dimensions uint32, metric VectorMetric, quant VectorQuantization, name string) error {
	if tx == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "write transaction is finalized"}
	}
	cCol, err := cStringChecked(collection)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cCol))
	cField, err := cStringChecked(fieldPath)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cField))
	var cName *C.char
	if name != "" {
		cName, err = cStringChecked(name)
		if err != nil {
			return err
		}
		defer C.free(unsafe.Pointer(cName))
	}
	tx.mu.Lock()
	defer tx.mu.Unlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	return callStatus(func() C.oryq_status {
		return C.oryq_create_vector_index(ptr, cCol, cField, C.uint32_t(dimensions), C.int32_t(metric), C.int32_t(quant), cName)
	})
}

// InsertJSON inserts a JSON document into collection.
func (tx *WriteTxn) InsertJSON(collection, docJSON string) error {
	if tx == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "write transaction is finalized"}
	}
	cCol, err := cStringChecked(collection)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cCol))
	cDoc, err := cStringChecked(docJSON)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cDoc))
	tx.mu.Lock()
	defer tx.mu.Unlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	return callStatus(func() C.oryq_status { return C.oryq_insert_json(ptr, cCol, cDoc) })
}

// UpdateByIDJSON applies a MongoDB-style update operator JSON to the document with _id=idJSON.
func (tx *WriteTxn) UpdateByIDJSON(collection, idJSON, updateJSON string) error {
	if tx == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "write transaction is finalized"}
	}
	cCol, err := cStringChecked(collection)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cCol))
	cID, err := cStringChecked(idJSON)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cID))
	cUpd, err := cStringChecked(updateJSON)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cUpd))
	tx.mu.Lock()
	defer tx.mu.Unlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	return callStatus(func() C.oryq_status { return C.oryq_update_by_id_json(ptr, cCol, cID, cUpd) })
}

// DeleteByIDJSON deletes the document with _id=idJSON.
func (tx *WriteTxn) DeleteByIDJSON(collection, idJSON string) error {
	if tx == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "write transaction is finalized"}
	}
	cCol, err := cStringChecked(collection)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cCol))
	cID, err := cStringChecked(idJSON)
	if err != nil {
		return err
	}
	defer C.free(unsafe.Pointer(cID))
	tx.mu.Lock()
	defer tx.mu.Unlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	return callStatus(func() C.oryq_status { return C.oryq_delete_by_id_json(ptr, cCol, cID) })
}

// Commit commits the write transaction and consumes the handle.
func (tx *WriteTxn) Commit() error {
	if tx == nil {
		return &Error{Status: StatusErrInvalidArgument, Message: "write transaction is finalized"}
	}
	tx.mu.Lock()
	defer tx.mu.Unlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return err
	}
	tx.ptr = nil
	tx.parent = nil
	return callStatus(func() C.oryq_status { return C.oryq_write_txn_commit(ptr) })
}

// Rollback aborts the write transaction and consumes the handle (idempotent if already finalized).
func (tx *WriteTxn) Rollback() error {
	if tx == nil {
		return nil
	}
	tx.mu.Lock()
	defer tx.mu.Unlock()
	if tx.ptr == nil {
		return nil
	}
	ptr := tx.ptr
	tx.ptr = nil
	tx.parent = nil
	return callStatus(func() C.oryq_status { return C.oryq_write_txn_rollback(ptr) })
}

// ReadTxn wraps a snapshot-isolated OryqReadTxn* handle with goroutine-safe locking.
type ReadTxn struct {
	mu     sync.RWMutex
	ptr    *C.OryqReadTxn
	parent *Database
}

func (tx *ReadTxn) requireOpen() (*C.OryqReadTxn, error) {
	if tx == nil || tx.ptr == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "read transaction is closed"}
	}
	return tx.ptr, nil
}

// Close closes the read transaction handle.
func (tx *ReadTxn) Close() error {
	if tx == nil {
		return nil
	}
	tx.mu.Lock()
	defer tx.mu.Unlock()
	if tx.ptr == nil {
		return nil
	}
	ptr := tx.ptr
	tx.ptr = nil
	tx.parent = nil
	return callStatus(func() C.oryq_status { return C.oryq_read_txn_close(ptr) })
}

// FindJSON queries collection with an optional JSON filter (empty string matches all documents).
func (tx *ReadTxn) FindJSON(collection, filterJSON string) (*Cursor, error) {
	if tx == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "read transaction is closed"}
	}
	cCol, err := cStringChecked(collection)
	if err != nil {
		return nil, err
	}
	defer C.free(unsafe.Pointer(cCol))
	var cFilter *C.char
	if filterJSON != "" {
		cFilter, err = cStringChecked(filterJSON)
		if err != nil {
			return nil, err
		}
		defer C.free(unsafe.Pointer(cFilter))
	}
	tx.mu.RLock()
	defer tx.mu.RUnlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return nil, err
	}
	var out *C.OryqCursor
	if err := callStatus(func() C.oryq_status { return C.oryq_find_json(ptr, cCol, cFilter, &out) }); err != nil {
		return nil, err
	}
	return &Cursor{ptr: out, parent: tx}, nil
}

// AggregateJSON executes a JSON aggregation pipeline on collection.
func (tx *ReadTxn) AggregateJSON(collection, pipelineJSON string) (*Cursor, error) {
	if tx == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "read transaction is closed"}
	}
	cCol, err := cStringChecked(collection)
	if err != nil {
		return nil, err
	}
	defer C.free(unsafe.Pointer(cCol))
	cPipe, err := cStringChecked(pipelineJSON)
	if err != nil {
		return nil, err
	}
	defer C.free(unsafe.Pointer(cPipe))
	tx.mu.RLock()
	defer tx.mu.RUnlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return nil, err
	}
	var out *C.OryqCursor
	if err := callStatus(func() C.oryq_status { return C.oryq_aggregate_json(ptr, cCol, cPipe, &out) }); err != nil {
		return nil, err
	}
	return &Cursor{ptr: out, parent: tx}, nil
}

// SearchFts executes a BM25 full-text search (requires liboryq built with --features fts).
func (tx *ReadTxn) SearchFts(collection, indexName, queryText string, limit int) (*Cursor, error) {
	if tx == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "read transaction is closed"}
	}
	if limit < 0 {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "limit must be >= 0"}
	}
	cCol, err := cStringChecked(collection)
	if err != nil {
		return nil, err
	}
	defer C.free(unsafe.Pointer(cCol))
	cIdx, err := cStringChecked(indexName)
	if err != nil {
		return nil, err
	}
	defer C.free(unsafe.Pointer(cIdx))
	cQuery, err := cStringChecked(queryText)
	if err != nil {
		return nil, err
	}
	defer C.free(unsafe.Pointer(cQuery))
	tx.mu.RLock()
	defer tx.mu.RUnlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return nil, err
	}
	var out *C.OryqCursor
	if err := callStatus(func() C.oryq_status {
		return C.oryq_search_fts(ptr, cCol, cIdx, cQuery, C.size_t(limit), &out)
	}); err != nil {
		return nil, err
	}
	return &Cursor{ptr: out, parent: tx}, nil
}

// SearchVector executes a top-k vector similarity search (requires liboryq built with --features vec).
func (tx *ReadTxn) SearchVector(collection, indexName string, queryVec []float32, topK int) (*Cursor, error) {
	if tx == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "read transaction is closed"}
	}
	if len(queryVec) == 0 {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "queryVec must not be empty"}
	}
	if topK < 0 {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "topK must be >= 0"}
	}
	cCol, err := cStringChecked(collection)
	if err != nil {
		return nil, err
	}
	defer C.free(unsafe.Pointer(cCol))
	cIdx, err := cStringChecked(indexName)
	if err != nil {
		return nil, err
	}
	defer C.free(unsafe.Pointer(cIdx))
	tx.mu.RLock()
	defer tx.mu.RUnlock()
	ptr, err := tx.requireOpen()
	if err != nil {
		return nil, err
	}
	var out *C.OryqCursor
	if err := callStatus(func() C.oryq_status {
		return C.oryq_search_vector(
			ptr,
			cCol,
			cIdx,
			(*C.float)(unsafe.Pointer(&queryVec[0])),
			C.size_t(len(queryVec)),
			C.size_t(topK),
			&out,
		)
	}); err != nil {
		return nil, err
	}
	return &Cursor{ptr: out, parent: tx}, nil
}

// Cursor wraps a streaming OryqCursor* result handle with mutex synchronization.
type Cursor struct {
	mu     sync.Mutex
	ptr    *C.OryqCursor
	parent *ReadTxn
}

func (c *Cursor) requireOpen() (*C.OryqCursor, error) {
	if c == nil || c.ptr == nil {
		return nil, &Error{Status: StatusErrInvalidArgument, Message: "cursor is closed"}
	}
	return c.ptr, nil
}

// Close frees the cursor handle.
func (c *Cursor) Close() error {
	if c == nil {
		return nil
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.ptr == nil {
		return nil
	}
	ptr := c.ptr
	c.ptr = nil
	c.parent = nil
	return callStatus(func() C.oryq_status { return C.oryq_cursor_close(ptr) })
}

// Next advances the cursor without JSON serialization (use with Get* accessors).
// Returns (false, nil) when exhausted.
func (c *Cursor) Next() (bool, error) {
	if c == nil {
		return false, &Error{Status: StatusErrInvalidArgument, Message: "cursor is closed"}
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	ptr, err := c.requireOpen()
	if err != nil {
		return false, err
	}
	return callCursorStatus(func() C.oryq_status { return C.oryq_cursor_next(ptr) })
}

// NextJSON advances the cursor and returns the next document as a JSON string.
// Returns ("", false, nil) when exhausted.
func (c *Cursor) NextJSON() (string, bool, error) {
	if c == nil {
		return "", false, &Error{Status: StatusErrInvalidArgument, Message: "cursor is closed"}
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	ptr, err := c.requireOpen()
	if err != nil {
		return "", false, err
	}
	var raw *C.char
	hasRow, err := callCursorStatus(func() C.oryq_status { return C.oryq_cursor_next_json(ptr, &raw) })
	if err != nil || !hasRow || raw == nil {
		return "", false, err
	}
	defer C.oryq_free_string(raw)
	return C.GoString(raw), true, nil
}

// GetI64 reads a top-level int64 field from the current cursor document.
func (c *Cursor) GetI64(field string) (int64, error) {
	if c == nil {
		return 0, &Error{Status: StatusErrInvalidArgument, Message: "cursor is closed"}
	}
	cField, err := cStringChecked(field)
	if err != nil {
		return 0, err
	}
	defer C.free(unsafe.Pointer(cField))
	c.mu.Lock()
	defer c.mu.Unlock()
	ptr, err := c.requireOpen()
	if err != nil {
		return 0, err
	}
	var out C.int64_t
	if err := callStatus(func() C.oryq_status { return C.oryq_cursor_get_i64(ptr, cField, &out) }); err != nil {
		return 0, err
	}
	return int64(out), nil
}

// GetF64 reads a top-level float64 field from the current cursor document.
func (c *Cursor) GetF64(field string) (float64, error) {
	if c == nil {
		return 0, &Error{Status: StatusErrInvalidArgument, Message: "cursor is closed"}
	}
	cField, err := cStringChecked(field)
	if err != nil {
		return 0, err
	}
	defer C.free(unsafe.Pointer(cField))
	c.mu.Lock()
	defer c.mu.Unlock()
	ptr, err := c.requireOpen()
	if err != nil {
		return 0, err
	}
	var out C.double
	if err := callStatus(func() C.oryq_status { return C.oryq_cursor_get_f64(ptr, cField, &out) }); err != nil {
		return 0, err
	}
	return float64(out), nil
}

// GetBool reads a top-level bool field from the current cursor document.
func (c *Cursor) GetBool(field string) (bool, error) {
	if c == nil {
		return false, &Error{Status: StatusErrInvalidArgument, Message: "cursor is closed"}
	}
	cField, err := cStringChecked(field)
	if err != nil {
		return false, err
	}
	defer C.free(unsafe.Pointer(cField))
	c.mu.Lock()
	defer c.mu.Unlock()
	ptr, err := c.requireOpen()
	if err != nil {
		return false, err
	}
	var out C.bool
	if err := callStatus(func() C.oryq_status { return C.oryq_cursor_get_bool(ptr, cField, &out) }); err != nil {
		return false, err
	}
	return bool(out), nil
}

// GetUTF8 reads a top-level UTF-8 string field from the current cursor document.
func (c *Cursor) GetUTF8(field string) (string, error) {
	if c == nil {
		return "", &Error{Status: StatusErrInvalidArgument, Message: "cursor is closed"}
	}
	cField, err := cStringChecked(field)
	if err != nil {
		return "", err
	}
	defer C.free(unsafe.Pointer(cField))
	c.mu.Lock()
	defer c.mu.Unlock()
	ptr, err := c.requireOpen()
	if err != nil {
		return "", err
	}
	var raw *C.char
	var length C.size_t
	if err := callStatus(func() C.oryq_status { return C.oryq_cursor_get_utf8(ptr, cField, &raw, &length) }); err != nil {
		return "", err
	}
	if length == 0 {
		return "", nil
	}
	if raw == nil || uint64(length) > math.MaxInt32 {
		return "", &Error{Status: StatusErrNumericOverflow, Message: "UTF-8 field length exceeds C.int range"}
	}
	return C.GoStringN(raw, C.int(length)), nil
}

// GetScore returns the BM25 score (FTS) or distance (Vector) for the current scored row.
func (c *Cursor) GetScore() (float64, error) {
	if c == nil {
		return 0, &Error{Status: StatusErrInvalidArgument, Message: "cursor is closed"}
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	ptr, err := c.requireOpen()
	if err != nil {
		return 0, err
	}
	var out C.double
	if err := callStatus(func() C.oryq_status { return C.oryq_cursor_get_score(ptr, &out) }); err != nil {
		return 0, err
	}
	return float64(out), nil
}

// IsStatus checks whether err is an *Error with the expected Status code.
func IsStatus(err error, status Status) bool {
	var oe *Error
	return errors.As(err, &oe) && oe.Status == status
}
