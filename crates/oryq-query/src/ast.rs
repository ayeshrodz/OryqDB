//! Filter, projection, sort, and find-options AST types (`docs/query-language.md`).

use oryq_document::{FieldPath, Value};
use oryq_index::SortOrder;

/// Canonical filter AST evaluated with `oryq_document::compare_values` semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterExpr {
    MatchAll,
    And(Vec<FilterExpr>),
    Or(Vec<FilterExpr>),
    Nor(Vec<FilterExpr>),
    Not(Box<FilterExpr>),
    Eq(FieldPath, Value),
    Ne(FieldPath, Value),
    Lt(FieldPath, Value),
    Lte(FieldPath, Value),
    Gt(FieldPath, Value),
    Gte(FieldPath, Value),
    In(FieldPath, Vec<Value>),
    Nin(FieldPath, Vec<Value>),
    Exists(FieldPath, bool),
    StartsWith(FieldPath, String),
    ElemMatch(FieldPath, Box<FilterExpr>),
}

/// Inclusion or exclusion projection specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionSpec {
    Include {
        fields: Vec<FieldPath>,
        include_id: bool,
    },
    Exclude {
        fields: Vec<FieldPath>,
    },
}

/// Ordered list of `(field_path, SortOrder)` sort keys.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SortSpec {
    pub keys: Vec<(FieldPath, SortOrder)>,
}

/// High-level `find()` query options.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FindOptions {
    pub filter: Option<FilterExpr>,
    pub projection: Option<ProjectionSpec>,
    pub sort: Option<SortSpec>,
    pub skip: u64,
    pub limit: Option<u64>,
}
