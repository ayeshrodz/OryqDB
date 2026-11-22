//! Built-in v1 registry assembly for `oryq-core`.

use std::sync::Arc;

use oryq_document::UpdateRegistry;
use oryq_engine::RecordCompactor;
use oryq_index::IndexProviderRegistry;

use crate::adapters::DocumentRecordCompactor;

/// Immutable collection of domain registries assembled once when opening a [`crate::Database`].
#[derive(Debug)]
pub struct CoreRegistries {
    pub updates: UpdateRegistry,
    pub indexes: IndexProviderRegistry,
    pub compactor: Arc<dyn RecordCompactor>,
}

impl CoreRegistries {
    /// Assembles the standard v1 baseline registries (`$set`/`$unset`/`$inc`/`$push`/`$pull`,
    /// `index.primary`, and `DocumentRecordCompactor`).
    pub fn default_v1() -> Self {
        #[allow(unused_mut)]
        let mut indexes = IndexProviderRegistry::default_v1();
        #[cfg(feature = "fts")]
        {
            indexes.register(Arc::new(oryq_fts::FtsIndexProvider));
        }
        Self {
            updates: UpdateRegistry::default_v1(),
            indexes,
            compactor: Arc::new(DocumentRecordCompactor),
        }
    }
}
