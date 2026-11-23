export declare const OryqStatus: Readonly<{
  OK: 0;
  CURSOR_DONE: 1;
  ERR_INVALID_ARGUMENT: 2;
  ERR_IO: 3;
  ERR_CORRUPTION: 4;
  ERR_NOT_FOUND: 5;
  ERR_DUPLICATE_KEY: 6;
  ERR_LOCKED_BY_PROCESS: 7;
  ERR_SNAPSHOT_TOO_OLD: 8;
  ERR_QUERY_MEMORY_LIMIT: 9;
  ERR_PARALLEL_ARRAY_INDEX: 10;
  ERR_UNSUPPORTED_FEATURE: 11;
  ERR_TYPE_MISMATCH: 12;
  ERR_BUSY: 13;
  ERR_COMMIT_OUTCOME_UNKNOWN: 14;
  ERR_WAL_LIMIT: 15;
  ERR_TOO_MANY_READERS: 16;
  ERR_TRANSACTION_MEMORY: 17;
  ERR_NUMERIC_OVERFLOW: 18;
  ERR_NUMERIC_PRECISION: 19;
  ERR_INVALID_QUERY: 20;
  ERR_DOCUMENT_TOO_LARGE: 21;
  ERR_INDEX_KEY_TOO_LARGE: 22;
  ERR_INVALID_ID_TYPE: 23;
  ERR_CONFLICTING_PATH: 24;
  ERR_INDEX_NOT_FOUND: 25;
  ERR_INTERNAL_PANIC: 255;
}>;

export declare const VectorMetric: Readonly<{
  COSINE: 0;
  L2: 1;
  DOT_PRODUCT: 2;
}>;

export declare const VectorQuantization: Readonly<{
  F32: 0;
  INT8: 1;
}>;

export interface OryqError extends Error {
  status: number;
}

export declare class Cursor {
  step(): boolean;
  nextDoc(): Record<string, unknown> | null;
  [Symbol.iterator](): Generator<Record<string, unknown>, void, unknown>;
  getI64(field: string): number;
  getF64(field: string): number;
  getBool(field: string): boolean;
  getUtf8(field: string): string;
  getScore(): number;
  close(): void;
}

export declare class ReadTxn {
  find(collection: string, filter?: Record<string, unknown> | string | null): Cursor;
  aggregate(collection: string, pipeline: unknown[] | string): Cursor;
  searchFts(collection: string, indexName: string, queryText: string, limit?: number): Cursor;
  searchVector(
    collection: string,
    indexName: string,
    queryVector: Float32Array | number[],
    topK?: number,
  ): Cursor;
  close(): void;
}

export declare class WriteTxn {
  createCollection(name: string): void;
  dropCollection(name: string): void;
  createIndex(
    collection: string,
    fieldPath: string,
    options?: { sortOrder?: 1 | -1; unique?: boolean; sparse?: boolean; name?: string | null },
  ): void;
  dropIndex(collection: string, indexName: string): void;
  createFtsIndex(collection: string, fields: string | string[], name?: string | null): void;
  createVectorIndex(
    collection: string,
    fieldPath: string,
    dimensions: number,
    options?: { metric?: number; quantization?: number; name?: string | null },
  ): void;
  insert(collection: string, doc: Record<string, unknown> | string): void;
  updateById(collection: string, id: unknown, update: Record<string, unknown> | string): void;
  deleteById(collection: string, id: unknown): void;
  commit(): void;
  rollback(): void;
}

export declare class Database {
  static open(dbPath: string): Database;
  static openInMemory(): Database;
  static version(): string;
  static abiVersion(): [number, number];
  beginRead(): ReadTxn;
  beginWrite(): WriteTxn;
  checkpoint(): void;
  vacuum(): void;
  backupTo(destPath: string): void;
  integrityCheck(): boolean;
  close(): void;
}
