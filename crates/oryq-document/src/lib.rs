#![forbid(unsafe_code)]

//! `oryq-document`: `OryqDoc` v1 binary codec, zero-copy views, exact cross-type comparator,
//! canonical field paths, Extended-JSON conversion, update operators, and state-normalized
//! physical delta patch compaction.

pub mod array;
pub mod codec;
pub mod compare;
pub mod json;
pub mod patch;
pub mod path;
pub mod types;
pub mod updates;
pub mod view;

pub use array::{ArrayIter, ArrayView};
pub use codec::{
    encode_document, field_name_hash_prefix, validate_document, MAX_NESTING_DEPTH, ORYQ_DOC_VERSION,
};
pub use compare::compare_values;
pub use json::{from_json_str, from_json_value, to_json_string, to_json_value};
pub use patch::{
    bloom_mask_for_top_level, compact_document, decode_physical_patch, encode_physical_patch,
    resolve_path_with_patches, PatchEntry, PatchOp, PhysicalPatch, NO_PREV_DELTA_SLOT,
};
pub use path::{FieldPath, MAX_PATH_DEPTH};
pub use types::{
    Decimal128, DecodedDecimal128, DocumentBuf, ObjectId, OryqTypeTag, Uuid, Value, ValueView,
};
pub use updates::{
    apply_update, validate_replacement_doc, NormalizedMutation, UpdateOperator, UpdateRegistry,
    UpdateSpec, UpdateStep,
};
pub use view::{DocumentIter, DocumentView};
