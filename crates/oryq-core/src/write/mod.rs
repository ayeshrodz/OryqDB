//! Atomic statement savepoint orchestration across collection data, indexes, and catalog.

pub mod statement;

#[cfg(feature = "fts")]
pub use statement::create_fts_index_stmt;
pub use statement::{
    create_collection_stmt, create_compound_index_stmt, create_secondary_index_stmt,
    decode_collection_record, delete_by_id_stmt, drop_collection_stmt, drop_index_stmt,
    encode_collection_record, generate_object_id, insert_doc_stmt, replace_by_id_stmt,
    update_by_id_stmt,
};
