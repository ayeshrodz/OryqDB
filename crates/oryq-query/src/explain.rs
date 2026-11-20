//! Query plan and `explain_analyze` diagnostic structures (`docs/query-language.md` §5).

use oryq_document::Value;
use oryq_types::IndexId;

use crate::ast::FilterExpr;

/// Chosen physical access path for a `find()` or `count()` operation.
#[derive(Debug, Clone, PartialEq)]
pub enum AccessPlan {
    /// Direct point lookup on the mandatory `_id` primary index.
    PrimaryIdLookup { target: Value },
    /// Exact equality scan on a secondary or compound index.
    SecondaryEqScan {
        index_id: IndexId,
        index_name: String,
        target: Value,
    },
    /// Ordered range scan on a secondary or compound index.
    SecondaryRangeScan {
        index_id: IndexId,
        index_name: String,
        lower: Option<(Value, bool)>,
        upper: Option<(Value, bool)>,
    },
    /// Full sequential scan of the collection's data B+ tree.
    CollectionScan,
}

/// Static logical/physical plan produced by `plan_find`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExplainPlan {
    pub collection: String,
    pub access: AccessPlan,
    pub residual_filter: Option<FilterExpr>,
}

/// Runtime execution metrics paired with the chosen [`ExplainPlan`].
#[derive(Debug, Clone, PartialEq)]
pub struct ExplainAnalyze {
    pub plan: ExplainPlan,
    pub keys_examined: u64,
    pub docs_examined: u64,
    pub docs_returned: u64,
    pub execution_time_us: u64,
}
