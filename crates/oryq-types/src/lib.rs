#![forbid(unsafe_code)]

//! Foundation contracts shared across OryqDB domain crates.
//!
//! This crate intentionally has zero internal or external runtime dependencies.

pub mod budget;
pub mod cancel;
pub mod capability;
pub mod error;
pub mod ids;

pub use budget::{BudgetDomain, BudgetHandle, Reservation};
pub use cancel::{CancellationSource, CancellationToken};
pub use capability::{CapabilityDescriptor, CapabilityId, FeatureBitmask, VersionRange};
pub use error::{ErrorCategory, ErrorEnvelope, ErrorLocation, Result};
pub use ids::{CollectionId, IndexId, NamespaceId, PageId, RecordId, SnapshotId, TxId};
