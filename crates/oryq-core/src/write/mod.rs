//! Atomic statement savepoint orchestration across collection data, indexes, and catalog.

pub mod lz4;
pub mod statement;

#[cfg(feature = "fts")]
pub use statement::create_fts_index_stmt;
#[cfg(feature = "vec")]
pub use statement::create_vector_index_stmt;
pub use statement::{
    create_collection_stmt, create_compound_index_stmt, create_secondary_index_stmt,
    decode_collection_record, decode_collection_record_from_store, delete_by_id_stmt,
    drop_collection_stmt, drop_index_stmt, encode_collection_record, generate_object_id,
    insert_doc_stmt, max_inline_record_bytes, replace_by_id_stmt, update_by_id_stmt,
};
pub(crate) use statement::{
    delete_by_id_with_entry, insert_doc_with_entry, replace_by_id_with_entry,
    update_by_id_with_entry,
};
