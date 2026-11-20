#![forbid(unsafe_code)]

//! `oryq-core`: Public Rust embedded database facade, versioned transactional catalog,
//! atomic statement orchestration, and composition root for OryqDB.

pub mod adapters;
pub mod api;
pub mod catalog;
pub mod composition;
pub mod config;
pub mod database;
pub mod write;

pub use adapters::{CoreQuerySource, DocumentRecordCompactor};
pub use api::{ReadCollection, ReadTransaction, WriteCollection, WriteTransaction};
pub use catalog::{CatalogEntry, CatalogMeta, CATALOG_SCHEMA_VERSION};
pub use composition::CoreRegistries;
pub use config::DatabaseConfig;
pub use database::Database;
pub use oryq_document::{
    from_json_str, to_json_string, DocumentBuf, DocumentView, ObjectId, Uuid, Value, ValueView,
};
pub use oryq_engine::{CheckpointOutcome, CommitReceipt, PageSize, SyncMode};
pub use oryq_index::{IndexDescriptor, IndexKind, SortOrder};
pub use oryq_query::{
    execute_pipeline, execute_pipeline_json, parse_filter_json, parse_pipeline_json,
    parse_projection_json, parse_sort_json, AccessPlan, AccumulatorSpec, ExplainAnalyze,
    ExplainPlan, FilterExpr, FindOptions, PipelineStage, ProjectionSpec, QueryContext, QueryCursor,
    SortSpec,
};
pub use oryq_types::{
    BudgetDomain, BudgetHandle, CancellationSource, CancellationToken, CollectionId, ErrorCategory,
    ErrorEnvelope, IndexId, PageId, RecordId, Result, SnapshotId, TxId,
};

/// Returns the current workspace version string.
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
