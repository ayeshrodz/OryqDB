import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const native = require(path.join(__dirname, "build", "oryq_napi.node"));

export const OryqStatus = Object.freeze({
  OK: 0,
  CURSOR_DONE: 1,
  ERR_INVALID_ARGUMENT: 2,
  ERR_IO: 3,
  ERR_CORRUPTION: 4,
  ERR_NOT_FOUND: 5,
  ERR_DUPLICATE_KEY: 6,
  ERR_LOCKED_BY_PROCESS: 7,
  ERR_SNAPSHOT_TOO_OLD: 8,
  ERR_QUERY_MEMORY_LIMIT: 9,
  ERR_PARALLEL_ARRAY_INDEX: 10,
  ERR_UNSUPPORTED_FEATURE: 11,
  ERR_TYPE_MISMATCH: 12,
  ERR_BUSY: 13,
  ERR_COMMIT_OUTCOME_UNKNOWN: 14,
  ERR_WAL_LIMIT: 15,
  ERR_TOO_MANY_READERS: 16,
  ERR_TRANSACTION_MEMORY: 17,
  ERR_NUMERIC_OVERFLOW: 18,
  ERR_NUMERIC_PRECISION: 19,
  ERR_INVALID_QUERY: 20,
  ERR_DOCUMENT_TOO_LARGE: 21,
  ERR_INDEX_KEY_TOO_LARGE: 22,
  ERR_INVALID_ID_TYPE: 23,
  ERR_CONFLICTING_PATH: 24,
  ERR_INDEX_NOT_FOUND: 25,
  ERR_INTERNAL_PANIC: 255,
});

export const VectorMetric = Object.freeze({
  COSINE: 0,
  L2: 1,
  DOT_PRODUCT: 2,
});

export const VectorQuantization = Object.freeze({
  F32: 0,
  INT8: 1,
});

function toJson(val) {
  return typeof val === "string" ? val : JSON.stringify(val);
}

export class Cursor {
  constructor(handle) {
    this._handle = handle;
  }

  _req() {
    if (!this._handle) {
      const err = new Error("cursor is closed");
      err.status = OryqStatus.ERR_INVALID_ARGUMENT;
      throw err;
    }
    return this._handle;
  }

  step() {
    return native.cursorNext(this._req());
  }

  nextDoc() {
    const raw = native.cursorNextJson(this._req());
    return raw === null ? null : JSON.parse(raw);
  }

  *[Symbol.iterator]() {
    while (true) {
      const doc = this.nextDoc();
      if (doc === null) break;
      yield doc;
    }
  }

  getI64(field) {
    return native.cursorGetI64(this._req(), field);
  }

  getF64(field) {
    return native.cursorGetF64(this._req(), field);
  }

  getBool(field) {
    return native.cursorGetBool(this._req(), field);
  }

  getUtf8(field) {
    return native.cursorGetUtf8(this._req(), field);
  }

  getScore() {
    return native.cursorGetScore(this._req());
  }

  close() {
    if (this._handle) {
      const h = this._handle;
      this._handle = null;
      native.cursorClose(h);
    }
  }
}

export class ReadTxn {
  constructor(handle) {
    this._handle = handle;
  }

  _req() {
    if (!this._handle) {
      const err = new Error("read transaction is closed");
      err.status = OryqStatus.ERR_INVALID_ARGUMENT;
      throw err;
    }
    return this._handle;
  }

  find(collection, filter = null) {
    const f = filter === null || filter === undefined ? null : toJson(filter);
    return new Cursor(native.findJson(this._req(), collection, f));
  }

  aggregate(collection, pipeline) {
    return new Cursor(
      native.aggregateJson(this._req(), collection, toJson(pipeline)),
    );
  }

  searchFts(collection, indexName, queryText, limit = 10) {
    if (!Number.isInteger(limit) || limit < 0 || limit > 0xffffffff) {
      const err = new Error("limit must be an integer in range 0..=4294967295");
      err.status = OryqStatus.ERR_INVALID_ARGUMENT;
      throw err;
    }
    return new Cursor(
      native.searchFts(this._req(), collection, indexName, queryText, limit),
    );
  }

  searchVector(collection, indexName, queryVector, topK = 10) {
    if (!Number.isInteger(topK) || topK < 0 || topK > 0xffffffff) {
      const err = new Error("topK must be an integer in range 0..=4294967295");
      err.status = OryqStatus.ERR_INVALID_ARGUMENT;
      throw err;
    }
    const f32 =
      queryVector instanceof Float32Array
        ? queryVector
        : Float32Array.from(queryVector);
    if (f32.length === 0) {
      const err = new Error("queryVector must not be empty");
      err.status = OryqStatus.ERR_INVALID_ARGUMENT;
      throw err;
    }
    return new Cursor(
      native.searchVector(this._req(), collection, indexName, f32, topK),
    );
  }

  close() {
    if (this._handle) {
      const h = this._handle;
      this._handle = null;
      native.readTxnClose(h);
    }
  }
}

export class WriteTxn {
  constructor(handle) {
    this._handle = handle;
  }

  _req() {
    if (!this._handle) {
      const err = new Error("write transaction is finalized");
      err.status = OryqStatus.ERR_INVALID_ARGUMENT;
      throw err;
    }
    return this._handle;
  }

  createCollection(name) {
    native.createCollection(this._req(), name);
  }

  dropCollection(name) {
    native.dropCollection(this._req(), name);
  }

  createIndex(
    collection,
    fieldPath,
    { sortOrder = 1, unique = false, sparse = false, name = null } = {},
  ) {
    native.createIndex(
      this._req(),
      collection,
      fieldPath,
      sortOrder,
      Boolean(unique),
      Boolean(sparse),
      name,
    );
  }

  dropIndex(collection, indexName) {
    native.dropIndex(this._req(), collection, indexName);
  }

  createFtsIndex(collection, fields, name = null) {
    const csv = Array.isArray(fields) ? fields.join(",") : String(fields);
    native.createFtsIndex(this._req(), collection, csv, name);
  }

  createVectorIndex(
    collection,
    fieldPath,
    dimensions,
    {
      metric = VectorMetric.COSINE,
      quantization = VectorQuantization.F32,
      name = null,
    } = {},
  ) {
    if (
      !Number.isInteger(dimensions) ||
      dimensions <= 0 ||
      dimensions > 0xffffffff
    ) {
      const err = new Error(
        "dimensions must be an integer in range 1..=4294967295",
      );
      err.status = OryqStatus.ERR_INVALID_ARGUMENT;
      throw err;
    }
    native.createVectorIndex(
      this._req(),
      collection,
      fieldPath,
      dimensions,
      metric,
      quantization,
      name,
    );
  }

  insert(collection, doc) {
    native.insertJson(this._req(), collection, toJson(doc));
  }

  updateById(collection, id, update) {
    native.updateByIdJson(
      this._req(),
      collection,
      JSON.stringify(id),
      toJson(update),
    );
  }

  deleteById(collection, id) {
    native.deleteByIdJson(this._req(), collection, JSON.stringify(id));
  }

  commit() {
    const h = this._req();
    this._handle = null;
    native.writeTxnCommit(h);
  }

  rollback() {
    if (this._handle) {
      const h = this._handle;
      this._handle = null;
      native.writeTxnRollback(h);
    }
  }
}

export class Database {
  constructor(handle) {
    this._handle = handle;
  }

  static open(dbPath) {
    return new Database(native.dbOpen(String(dbPath)));
  }

  static openInMemory() {
    return Database.open(":memory:");
  }

  static version() {
    return native.version();
  }

  static abiVersion() {
    const packed = native.abiVersion();
    return [packed >>> 16, packed & 0xffff];
  }

  _req() {
    if (!this._handle) {
      const err = new Error("database is closed");
      err.status = OryqStatus.ERR_INVALID_ARGUMENT;
      throw err;
    }
    return this._handle;
  }

  beginRead() {
    return new ReadTxn(native.readTxnBegin(this._req()));
  }

  beginWrite() {
    return new WriteTxn(native.writeTxnBegin(this._req()));
  }

  checkpoint() {
    native.dbCheckpoint(this._req());
  }

  vacuum() {
    native.dbVacuum(this._req());
  }

  backupTo(destPath) {
    native.dbBackupTo(this._req(), String(destPath));
  }

  integrityCheck() {
    return native.dbIntegrityCheck(this._req());
  }

  close() {
    if (this._handle) {
      const h = this._handle;
      native.dbClose(h);
      this._handle = null;
    }
  }
}
