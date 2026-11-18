# Modular Architecture: Goldfish Review Report

* **Date:** 2026-11-19
* **Target:** Modular technical spec, module guides, development plan and existing architecture/roadmap/ADR consistency.
* **Status:** resolved — 11 Major findings repaired in Rounds 1–3; converged after 5 rounds with consecutive clean Rounds 4 and 5 (0 unresolved Blocker/Major/Nit findings).
* **Workflow:** EGM draft, fresh-context Goldfish review and Elephant resolution. User authorized creation and improvement of the technical spec; this work changes documentation only.

## Reviewer execution

| Round | Reviewer | Persona / standard | Status |
| :--- | :--- | :--- | :--- |
| 1 | modular_design_review | design_critic / feature-spec | completed; three Major findings |
| 1 | modular_plan_review | implementation_readiness / implementation-readiness | completed; four Major findings |
| 1 | modular_consistency_review | design_critic / feature-spec | completed; two Major findings, one overlapping |
| 2 | modular_round2_design | design_critic / feature-spec | completed; two Major findings |
| 2 | modular_round2_ready | implementation_readiness / implementation-readiness | completed; clean |
| 3 | modular_round3_design | design_critic / feature-spec | completed; one Major semantic-binding finding |
| 3 | modular_round3_ready | implementation_readiness / implementation-readiness | completed; same overlapping semantic-binding finding |
| 4 | modular_round4_design | design_critic / feature-spec | completed; clean (0 Blocker, 0 Major, 0 Nit) |
| 4 | modular_round4_ready | implementation_readiness / implementation-readiness | completed; clean (0 Blocker, 0 Major, 0 Nit) |
| 4 | modular_round4_zoom_out | zoom_out / zoom-out-standard | completed; clean (0 Blocker, 0 Major, 0 Nit) |
| 5 | modular_round5_design | design_critic / feature-spec | completed; clean (0 Blocker, 0 Major, 0 Nit) |
| 5 | modular_round5_ready | implementation_readiness / implementation-readiness | completed; clean (0 Blocker, 0 Major, 0 Nit) |
| 5 | modular_round5_details | codebase_details / implementation-readiness | completed; clean (0 Blocker, 0 Major, 0 Nit) |

All reviewers ran in isolated fresh contexts and confirmed reads of persona, standard and target documents. Shared filesystem/host instructions are not claimed isolated. Overlapping E1 dependency findings are combined below, retaining both reviewer sources. Eleven distinct Major findings from Rounds 1–3 were classified PROPOSE FIX and applied, and `docs/research/{architecture-decisions,implementation-roadmap}.md` were synchronized with ADR-007/ADR-008 and `W00–W16`/`X01–X03`. Rounds 4 and 5 independently returned zero Blocker, Major or Nit findings across all six fresh-context reviewers, satisfying the two-consecutive-clean-rounds convergence criterion. No finding is contested or escalated; no runtime code was implemented or marked verified.

## Prioritized findings and resolutions

### D1 — Major [modular_design_review]

* **Affected scope:** Modular technical specification and linked owner/plan documents, as named in the edit.
* **Discrepancy:** Provider search wiring was incomplete.
* **Rationale:** The provider cannot import query, so an unspecified binding would force feature-specific parser/planner edits.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Section 9.1 defines index-owned request/binding/result modes and the query-owned generic factory bound once by core; unknown operations and approximate semantics are explicit.

### D2 — Major [modular_design_review]

* **Affected scope:** Modular technical specification and linked owner/plan documents, as named in the edit.
* **Discrepancy:** Provider-local root/state rollback was undefined.
* **Rationale:** Rolling back pages alone cannot undo a shared mutable provider root/cache.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Section 9.2 makes providers transaction-independent, uses statement-owned arenas, and returns MaintenanceResult roots to the transactional catalog; conformance includes a later successful statement after another provider fails.

### D3 — Major [modular_design_review]

* **Affected scope:** Modular technical specification and linked owner/plan documents, as named in the edit.
* **Discrepancy:** Persisted provider-version enforcement was missing.
* **Rationale:** A present capability bit does not prove a provider can read/write newer data/options.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Section 9.3 specifies supported ranges, post-recovery and per-operation checks, affected-operation refusal and explicit migration/rebuild.

### R1 — Major [modular_plan_review]

* **Affected scope:** Modular technical specification and linked owner/plan documents, as named in the edit.
* **Discrepancy:** New port signatures and supporting ownership were underspecified.
* **Rationale:** Stage/source/provider object safety, lending lifetimes and typed registrations determine whether modules can be independently implemented.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Section 9 defines receiver/lifetime/trait bounds, scoped contexts, request representation, owner entrypoints and assembly; W07/W08/W10 require external-consumer tests.

### C1 — Major [modular_plan_review + modular_consistency_review]

* **Affected scope:** Modular technical specification and linked owner/plan documents, as named in the edit.
* **Discrepancy:** E1 and ADR-005 retained the opposite dependency direction.
* **Rationale:** Old instructions placed provider interfaces in core and let providers import their host.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** ROADMAP E1 and ADR-005 now put provider interfaces in index and optional-provider hosting exclusively in core, explicitly superseding old source placement.

### R3 — Major [modular_plan_review]

* **Affected scope:** Modular technical specification and linked owner/plan documents, as named in the edit.
* **Discrepancy:** Core facade omitted CLI forensic inspection operations.
* **Rationale:** Core-only CLI dependencies would otherwise require inventing or bypassing a boundary.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Section 10 assigns bounded Page/WAL/Locks inspection to core admin/inspect.rs and engine inspect.rs with exclusive read-only/no-recovery semantics; W14 owns the API before W15.

### R4 — Major [modular_plan_review]

* **Affected scope:** Modular technical specification and linked owner/plan documents, as named in the edit.
* **Discrepancy:** Architecture checker manifests/change-note inputs were unspecified.
* **Rationale:** An implementer would need to invent ownership schemas, change comparison and test selection policy.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Section 11 fixes schema/version/fields, contract-file list, command behavior, git/explicit-change comparison, error cases and owner/downstream selections; W01 owns checker self-tests.

### C2 — Major [modular_consistency_review]

* **Affected scope:** Modular technical specification and linked owner/plan documents, as named in the edit.
* **Discrepancy:** Master verification still targeted obsolete owners/paths.
* **Rationale:** Old C source paths, core-only Miri and coverage directories would miss the extracted implementations.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Master verification now selects domain owner tests and canonical FFI C files, distinguishes pure Miri from native process evidence, and names the planned core integration suites.

### D4 — Major [modular_round2_design]

* **Affected scope:** Provider interface, Section 9.1.1 and W07 conformance.
* **Discrepancy:** Index initialization on an empty collection had no provider operation.
* **Rationale:** Core otherwise needed provider-specific root setup or a fake document mutation.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Add create(IndexStorageContext, IndexDefinition), empty-root/version/requirements result, and atomic nonempty build/root publication with rollback.

### D5 — Major [modular_round2_design]

* **Affected scope:** Generic search Section 9.1 and index/query/extension guides.
* **Discrepancy:** Candidate completeness alone had no provider-specific residual evaluator.
* **Rationale:** A generic new predicate could emit false-positive results unless query imported its implementation.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Add pure budgeted provider matches; generic factory rechecks fetched documents. Define unranked index-optional full-scan fallback and reject incompatible ranked/index-optional combinations.

### D6 — Major [modular_round3_design + modular_round3_ready]

* **Affected scope:** Section 9.1, index/extension guides and W07/X01 conformance.
* **Discrepancy:** A shared provider's residual matcher could not see selected per-index field/tokenizer/options semantics.
* **Rationale:** Candidate scan and matches could disagree when two indexes use different analyzers or options.
* **AI suggestion:** PROPOSE FIX — applied.
* **Edit:** Add immutable SearchBindingContext selected from the caller-snapshot descriptor, retained by ValidatedSearch and checked by scan/matches. Require explicit compatible no-index semantics; reject ambiguous selection/options mismatch. Test two configured indexes and candidate supersets.

## Additional boundary clarification

The review also prompted explicit placement of generic byte-tree prefix layout in engine, index-owned compound key semantics, a core-implemented engine batch-command port, and module-local extension metrics. It also makes cursor execution Send, query sources immutable Send + Sync read projections, and FFI mutation BUSY with children, preserving the existing movable-handle contract. This avoids reverse imports and feature-wide metric switches without changing the existing physical format or commit contract.

## Validation

`python3 tools/validate_design_docs.py` passes local document links/fragments/fences, the existing 26,809-case numeric prototype, 100 prefix-bound cases and C/C++ syntax checks of the proposed header. These checks do not compile the new Rust interface declarations or exercise a production engine. The module/API/manifest declarations are technical contracts and remain subject to implementation conformance in W01–W16/X01–X03.

## Remaining evidence gates

No production Cargo scaffold was created. M0 pinned source research, catalog golden fixtures, runtime durability/ownership/budget tests, architecture-checker execution, native linked ABI tests, optional-provider conformance and platform evidence remain explicit development gates. Review convergence concerns document consistency, not runtime correctness or format freeze.
