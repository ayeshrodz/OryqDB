#![cfg(not(feature = "fts"))]

//! Verifies reduced-build fail-closed behavior: when `oryq-core` is compiled WITHOUT
//! `--features fts`, `CoreRegistries::default_v1()` does NOT register `index.fts` and
//! `require("index.fts")` fails closed with `ErrorCategory::UnsupportedFeature`.

use oryq_core::{CoreRegistries, ErrorCategory};
use oryq_types::CapabilityId;

#[test]
fn reduced_build_without_fts_rejects_fts_provider_fail_closed() {
    let registries = CoreRegistries::default_v1();
    let fts_id = CapabilityId::new("index.fts").unwrap();
    assert!(!registries.indexes.contains(&fts_id));
    let err = registries.indexes.require(&fts_id).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::UnsupportedFeature);
}
