#![forbid(unsafe_code)]

//! `oryq-query`: Filter AST, JSON parser, exact predicate evaluator, rule-based planner,
//! projection/sort/cursor execution, and `explain_analyze` (`docs/modules/query.md`).

pub mod ast;
pub mod context;
pub mod cursor;
pub mod explain;
pub mod parse;
pub mod planner;
pub mod ports;
pub mod predicates;

pub use ast::{FilterExpr, FindOptions, ProjectionSpec, SortSpec};
pub use context::QueryContext;
pub use cursor::{
    apply_projection, compare_docs_by_sort, execute_count, execute_find, explain_analyze,
    sort_documents, QueryCursor,
};
pub use explain::{AccessPlan, ExplainAnalyze, ExplainPlan};
pub use parse::{
    json_to_oryq, parse_filter_json, parse_filter_value, parse_projection_json,
    parse_projection_value, parse_sort_json, parse_sort_value,
};
pub use planner::plan_find;
pub use ports::QuerySource;
pub use predicates::{compare_owned_values, eval_filter, resolve_path};
