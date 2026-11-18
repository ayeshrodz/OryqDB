#![forbid(unsafe_code)]

//! `oryq-document`: `OryqDoc` v1 binary codec, zero-copy views, exact cross-type comparator,
//! canonical field paths, and Extended-JSON conversion.

pub mod array;
pub mod codec;
pub mod compare;
pub mod json;
pub mod path;
pub mod types;
pub mod view;

pub use array::{ArrayIter, ArrayView};
pub use codec::{
    encode_document, field_name_hash_prefix, validate_document, MAX_NESTING_DEPTH, ORYQ_DOC_VERSION,
};
pub use compare::compare_values;
pub use json::{from_json_str, from_json_value, to_json_string, to_json_value};
pub use path::{FieldPath, MAX_PATH_DEPTH};
pub use types::{
    Decimal128, DecodedDecimal128, DocumentBuf, ObjectId, OryqTypeTag, Uuid, Value, ValueView,
};
pub use view::{DocumentIter, DocumentView};
