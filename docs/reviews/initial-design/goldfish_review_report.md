# Goldfish Annotated Review Report

**Target:** Master design, eight subsystem specifications, roadmap, ADRs and status/risk trackers (full files).
**Review date:** 2026-11-19 (Pacific/Auckland).
**Status:** resolved — 33 design findings repaired (5 Blocker, 27 Major, 1 Nit); runtime and format-freeze gates remain open.
**Review outcome:** Converged after 5 rounds, with consecutive clean rounds 4 and 5. No unresolved Blocker/Major finding in the reviewed scope.
**Workflow:** EGM Goldfish fresh-context review and Elephant resolution. The user authorized review, improvement and gap closure; document repairs were applied within that scope without additional approval turns. This is an architecture review, not executed-engine verification.

## Reviewer Execution

| Round | Reviewer | Persona / standard | Status |
| :--- | :--- | :--- | :--- |
| 1 | storage_review | design_critic / feature-spec | completed; full target specs, relevant master/draft context |
| 1 | query_ffi_review | implementation_readiness / implementation-readiness | completed; full target specs, relevant master/draft context |
| 1 | plan_review | design_critic / feature-spec | completed; full master, roadmap, trackers and subsystem context |
| 2 | storage_verify | design_critic / feature-spec | completed; full storage targets and master context |
| 2 | api_verify | implementation_readiness / implementation-readiness | completed; full API targets and master context |
| 3 | storage_round3 | design_critic / feature-spec | completed; no Blocker/Major findings |
| 3 | api_round3 | implementation_readiness / implementation-readiness | completed; no Blocker/Major findings |
| 3 | plan_round3 | design_critic / feature-spec | completed; two Major master-summary discrepancies |
| 4 | storage_round4 | design_critic / feature-spec | completed; clean (no Blocker/Major) |
| 4 | api_round4 | implementation_readiness / implementation-readiness | completed; clean (no Blocker/Major) |
| 5 | storage_round5 | design_critic / feature-spec | completed; clean (no Blocker/Major) |
| 5 | api_round5 | implementation_readiness / implementation-readiness | completed; clean (no Blocker/Major) |

Reviewers were launched with `fork_turns: none`; they confirmed required persona, standard and target reads. This isolates conversation history, not shared filesystem or host instructions. Rounds 1–3 were not clean: all their Blocker/Major findings were evaluated before repair. Rounds 4 and 5 independently returned no Blocker/Major findings before any further target edits; their consecutive clean results meet the review-loop convergence criterion. All requested reviewers completed; none was counted as clean after failing or timing out.

## Prioritized Findings & Resolutions

Original scopes refer to the initial audit, with current section references below. Line numbers changed during repairs; stable IDs preserve traceability. All findings were classified PROPOSE FIX and applied; there are no contested, escalated or declined findings. The SHM Nit was worth fixing because it affected layout feasibility. Tests named in design documents remain requirements, not claims of execution.

### api-1-1 — Blocker

* **Affected scope:** `docs/indexing.md §4.1` (full section).
* **Discrepancy:** Nine-byte numeric encoding lost exact large integers.
* **Rationale:** One tail byte cannot distinguish all integers sharing a rounded float prefix.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Replace with exact variable-width decimal significand/exponent encoding; independently compare rational ordering.

### api-1-2 — Blocker

* **Affected scope:** `docs/ffi.md §4.1` (full section).
* **Discrepancy:** Cursors retained database ownership but not their transaction.
* **Rationale:** Completing/freeing a parent could invalidate a live child cursor.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Owned transaction state, active-child count, BUSY completion and separate transaction-close semantics.

### storage-1-1 — Blocker

* **Affected scope:** `docs/transactions.md §4.1` (full section).
* **Discrepancy:** Checkpoint sync/reset ordering was absent.
* **Rationale:** A header could retire redo before data pages became durable.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Define data sync, shadow-header sync, redundant reset intent, WAL retirement and restartable recovery.

### storage-1-2 — Blocker

* **Affected scope:** `docs/transactions.md §4.1` (full section).
* **Discrepancy:** Reader load/store registration raced with reclamation.
* **Rationale:** A reader could register an already-retired snapshot.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Use one admission mutex for snapshot read+token publication and checkpoint horizon capture.

### storage-1-3 — Blocker

* **Affected scope:** `docs/file-format.md §4` (full section).
* **Discrepancy:** Essential tree, overflow, freelist and WAL payloads lacked byte contracts.
* **Rationale:** Independent parsers/writers could not interoperate.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Define envelopes, page-chain entries, WAL delta/commit payloads and root/allocation metadata; retain explicit M3 catalog schema/fixture freeze gate.

### api-1-10 — Major

* **Affected scope:** `docs/indexing.md §4.2` (full section).
* **Discrepancy:** Prefix deltas allowed two incompatible reference keys.
* **Rationale:** Decoders could reconstruct different key bytes.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Specify preceding-key references, LE lengths, validation and deterministic leaf recompression.

### api-1-3 — Major

* **Affected scope:** `docs/query-language.md §4.4` (full section).
* **Discrepancy:** Unicode maximal-scalar prefix upper bound omitted suffixes.
* **Rationale:** The index range could miss matching documents.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Use exclusive successor of escaped encoded prefix bytes, with descending translation and residual checks.

### api-1-4 — Major

* **Affected scope:** `docs/query-language.md §4.4; indexing.md §4.5` (full section).
* **Discrepancy:** Whole-array comparison disagreed with element-only indexes.
* **Rationale:** Index access paths could be incomplete.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Separate scalar element comparisons from whole-array equality and restrict multikey bounds or scan.

### api-1-5 — Major

* **Affected scope:** `docs/query-language.md §4.4; indexing.md §4.5` (full section).
* **Discrepancy:** Missing/Null/negative/sparse semantics were underspecified.
* **Rationale:** Sparse indexes and existential negation could omit matches.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Add truth table, boolean-complement negatives and sparse completeness/recheck rules.

### api-1-6 — Major

* **Affected scope:** `docs/indexing.md §4.5` (full section).
* **Discrepancy:** Arbitrary user IDs could not serve as bitmap integers.
* **Rationale:** Hash collisions or truncation would corrupt dedup/candidate sets.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Persist monotonic non-reused RecordId:u64 identities and use RoaringTreemap.

### api-1-7 — Major

* **Affected scope:** `docs/query-language.md §4.5` (full section).
* **Discrepancy:** Per-stage memory and spill rules did not bound complete queries.
* **Rationale:** Bitmaps, top-K records, joins and one large group could exceed quotas.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Shared reservation accounting, transaction budget, explicit unspillable errors and scratch size/lifecycle rules.

### api-1-8 — Major

* **Affected scope:** `docs/indexing.md §4.5` (full section).
* **Discrepancy:** Normalized index keys could not reconstruct original typed values.
* **Rationale:** Covering projections could erase numeric types or Missing/Null.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Disable covering when representations/presence are erased; require complete index-only predicates/counts.

### api-1-9 — Major

* **Affected scope:** `docs/ffi.md §§4.1,5,6` (full section).
* **Discrepancy:** Panic/diagnostic/output contracts contradicted C signatures.
* **Rationale:** Void functions cannot return status and shared CString pointers could become invalid.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Return-type-specific sentinels, TLS lifetime/NULL-open diagnostics, initialized outputs and pointer preconditions.

### api-2-1 — Major

* **Affected scope:** `docs/query-language.md §4.6` (full section).
* **Discrepancy:** Average result type/division/precision were undefined.
* **Rationale:** Integer average could mean truncation, float or error.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Define exact rational sum/count, one correctly rounded Float64 result, overflow/Null/decimal rules and quota/spill behavior.

### plan-1-1 — Major

* **Affected scope:** `docs/ROADMAP.md M9` (full section).
* **Discrepancy:** Live CLI backup preceded cross-process support.
* **Rationale:** A separate CLI cannot attach to a default exclusive owner.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Test online backup in the owning process; require exclusive offline CLI access until E2.

### plan-1-2 — Major

* **Affected scope:** `docs/ROADMAP.md M10/E1–E3; ADR-007` (full section).
* **Discrepancy:** Optional search and SHM were release prerequisites.
* **Rationale:** This contradicted the initial baseline scope and inflated the release gate.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Separate baseline M10 from independently gated post-baseline extensions.

### plan-1-3 — Major

* **Affected scope:** `docs/ROADMAP.md M8` (full section).
* **Discrepancy:** M8 portability required an M9 CLI command.
* **Rationale:** The sequential acceptance dependency was impossible.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Schedule core integrity-check API/test harness at M8, CLI exposure M9.

### plan-1-4 — Major

* **Affected scope:** `docs/PROJECT_STATUS.md; designs/oryqdb.md` (full section).
* **Discrepancy:** Draft decisions were reported as finalized/verified.
* **Rationale:** Research and implementation evidence did not support readiness claims.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Mark reviewed draft and M0 research outstanding; link traceable dispositions and runtime acceptance gates.

### storage-1-10 — Major

* **Affected scope:** `docs/document-model.md §4.4` (full section).
* **Discrepancy:** Borrowed parent lookup referred to temporary owned materialization.
* **Rationale:** Returned lifetime could outlive the reconstructed buffer.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Top-level normalization makes parent replacements self-contained; overflow/full export uses cursor-owned bounded storage.

### storage-1-11 — Major

* **Affected scope:** `docs/transactions.md §4.1` (full section).
* **Discrepancy:** Online backup had no snapshot-copy procedure.
* **Rationale:** A file copy could mix roots/pages or omit committed WAL.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Pin one snapshot, resolve all live pages, write/sync consistent standalone metadata and publish only verified output.

### storage-1-4 — Major

* **Affected scope:** `docs/document-model.md §4.4` (full section).
* **Discrepancy:** Parent replacement could bypass child bloom checks.
* **Rationale:** Ancestor/descendant deltas could expose old values.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Normalize physical patches to top-level replacements; hash top-level names and apply newest-first shadowing.

### storage-1-5 — Major

* **Affected scope:** `docs/file-format.md §4.4; transactions.md §4.1` (full section).
* **Discrepancy:** A process slot represented only its last reader.
* **Rationale:** Concurrent or nested snapshots could lose retention protection.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Use distinct local reader tokens and a process minimum in the proposed E2 protocol; keep E2 disabled until verified.

### storage-1-6 — Major

* **Affected scope:** `docs/transactions.md §6` (full section).
* **Discrepancy:** Cancellation advanced retention before quiescence.
* **Rationale:** In-flight I/O and borrowed views could read recycled history.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Request cancellation but retain pins through transaction/view release; reject writes at a hard WAL cap.

### storage-1-7 — Major

* **Affected scope:** `docs/transactions.md §§4.2,6` (full section).
* **Discrepancy:** Failed-sync cleanup claimed rollback and reused WAL.
* **Rationale:** A failed sync may have persisted commits, including dependent batched work.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Return CommitOutcomeUnknown, fault the database and require recovery; serialize explicit transactions through sync.

### storage-1-8 — Major

* **Affected scope:** `docs/transactions.md §4.1; file-format.md §6` (full section).
* **Discrepancy:** Truncation and corruption handling conflicted.
* **Rationale:** Recovery could either reject normal incomplete tails or silently discard durable history.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Discard structurally incomplete final transactions; fail closed on a complete invalid frame and record the availability limitation.

### storage-1-9 — Major

* **Affected scope:** `docs/file-format.md §4.4` (full section).
* **Discrepancy:** A latest-only SHM cache could not answer historical reads.
* **Rationale:** Historical versions and 64-bit identities could be lost.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Remove fixed packed hash correctness dependency; reconstruct retained versions; leave cross-process publication as a gated E2 protocol.

### storage-2-1 — Major

* **Affected scope:** `docs/transactions.md §§4.1,4.1.1` (full section).
* **Discrepancy:** Reset omitted truncation of the old WAL frame suffix.
* **Rationale:** New header plus old frames would fail checksums on reopen.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Explicit set_len(64), new-header/file-length sync and final checkpoint sequence validation.

### storage-2-2 — Major

* **Affected scope:** `docs/file-format.md parser rules` (full section).
* **Discrepancy:** Array element descriptor origin/layout was ambiguous.
* **Rationale:** Copied array patches could decode differently.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Specify high-4-bit tag/low-28-bit array-relative offsets, inclusive byte length and strict containment.

### storage-2-3 — Major

* **Affected scope:** `docs/file-format.md parser rules` (full section).
* **Discrepancy:** Recursive parsing lacked termination guarantees.
* **Rationale:** Self-referential offsets or deep inputs could loop/overflow.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Require heap-only strictly contained payloads, checked arithmetic, nonoverlap and depth cap 128 before recursion.

### storage-2-4 — Major

* **Affected scope:** `docs/file-format.md main header; transactions.md §4.1.1` (full section).
* **Discrepancy:** Main-only backup portability conflicted with required WAL presence.
* **Rationale:** Accepting arbitrary missing WAL could silently lose durable commits.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Add durable wal_required flag and sealed-state protocol; unseal both headers before any WAL append.

### storage-1-12 — Nit

* **Affected scope:** `docs/file-format.md §4.4` (full section).
* **Discrepancy:** Sixteen individually cache-line-aligned words could not fit 128 bytes.
* **Rationale:** The stated layout was arithmetically inconsistent.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Clarify block versus word alignment and mark SHM as an unfrozen extension.

### plan-3-1 — Major

* **Affected scope:** `docs/designs/oryqdb.md`, Overflow Handling.
* **Discrepancy:** Master retained selective field overflow and incompatible chunk layout.
* **Rationale:** It contradicted the canonical whole-document envelope and could produce incompatible persistent pages.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Link canonical whole-document overflow layout; budget materialization; defer selective overflow.

### plan-3-2 — Major

* **Affected scope:** Master implementation manifest and architecture crate labels.
* **Discrepancy:** Optional search was still assigned to baseline Phase 3.
* **Rationale:** It contradicted M10 baseline/E1 extension acceptance.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Assign FTS/vector implementation explicitly to post-baseline E1; align RecordId and logged plugin-merge summaries.

## Validation Evidence

Reproduce these checks with `python3 tools/validate_design_docs.py` (Python standard library, C and C++ compilers).

* Exact numeric transform prototype matched an independent `Fraction` ordering oracle for **26,809 distinct rational inputs**, including random f64 bit patterns, decimal values, integer limits and subnormals. 1,024 adjacent integers near 2^62 had distinct keys. This validates the proposed ordering transform, not a Rust codec or Decimal128 parser.
* Encoded prefix-bound prototype passed **100** empty/NUL/Unicode/maximal-scalar combinations against direct string prefix matching.
* The C header extracted from `docs/ffi.md` compiled in **C and C++ syntax-only mode** with `-Wall -Wextra -Werror`. No library was linked or executed.
* Final local Markdown link, heading-fragment and fenced-block checks passed with zero missing targets, zero unresolved fragments and zero unbalanced fences. The planned source tree does not exist, so Cargo/runtime/ABI behavior and crash recovery cannot be tested here.

## Remaining Acceptance Work

The design repairs do not fabricate missing evidence. The following gates remain explicit in [ROADMAP.md](../../ROADMAP.md#review-driven-acceptance-matrix): pinned LiteDB source audit; byte-exact catalog schema and golden fixtures at M3; native filesystem/locking behavior; crash/admission/reset fault schedules; independent Rust codec/parser oracles; memory/spill cleanup; C lifecycle execution; and release/platform verification. Shared-process initialization/admission/death recovery, plugin implementation and queued-command batching remain disabled extensions until their own protocols and tests pass.

Complete-length corrupted WAL tails intentionally fail closed and can require manual diagnosis; this is a documented availability limitation, not a claim that all corrupt logs recover automatically. Snapshot cancellation may stop writers at the hard WAL cap until retained guards are released. Top-level normalized patches and full-document overflow materialization trade some zero-copy/update efficiency for a defined safe baseline. Persistent format v1 remains unfrozen.
