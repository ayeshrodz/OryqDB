# `oryq-query`

Storage-independent query engine for OryqDB:

- **AST & JSON Parser (`ast`, `parse`)**: MongoDB-style filters (`$eq`, `$ne`, `$lt`, `$lte`, `$gt`, `$gte`, `$in`, `$nin`, `$exists`, `$startsWith`, `$and`, `$or`, `$nor`, `$not`, `$elemMatch`), Extended-JSON literal preservation, projections, and sorts.
- **Exact Predicate Evaluator (`predicates`)**: Cross-type numeric comparisons via `oryq_document::compare_values`, array element matching, and `Missing` vs `Null` distinction.
- **Rule-Based Planner (`planner`, `explain`)**: Selects `PrimaryIdLookup`, `SecondaryEqScan`, `SecondaryRangeScan`, or `CollectionScan` with sparse-index completeness safety and residual predicate rechecking.
- **Execution (`cursor`, `context`, `ports`)**: Operates solely through the `QuerySource` port under `BudgetHandle` memory quotas and `CancellationToken` checks.
