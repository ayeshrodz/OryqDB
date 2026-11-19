//! Atomic statement savepoint orchestration across collection data, indexes, and catalog.

pub mod statement;

pub use statement::{
    create_collection_stmt, decode_collection_record, delete_by_id_stmt, drop_collection_stmt,
    encode_collection_record, generate_object_id, insert_doc_stmt, replace_by_id_stmt,
    update_by_id_stmt,
};
