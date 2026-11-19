//! Owner-local registry of immutable [`IndexProvider`] factories (`docs/modules/index.md` §4).

use std::collections::HashMap;
use std::sync::Arc;

use oryq_types::{CapabilityId, ErrorCategory, ErrorEnvelope, Result};

use crate::primary::PrimaryIndexProvider;
use crate::provider::IndexProvider;

const MODULE_ID: &str = "index";

/// Immutable registry mapping [`CapabilityId`] to [`IndexProvider`] implementations.
#[derive(Debug, Default)]
pub struct IndexProviderRegistry {
    providers: HashMap<CapabilityId, Arc<dyn IndexProvider>>,
}

impl IndexProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a registry populated with all baseline built-in providers (`index.primary`).
    pub fn default_v1() -> Self {
        let mut reg = Self::new();
        reg.register(Arc::new(PrimaryIndexProvider));
        reg
    }

    pub fn register(&mut self, provider: Arc<dyn IndexProvider>) {
        self.providers.insert(provider.provider_id(), provider);
    }

    pub fn get(&self, id: &CapabilityId) -> Option<&Arc<dyn IndexProvider>> {
        self.providers.get(id)
    }

    pub fn require(&self, id: &CapabilityId) -> Result<&Arc<dyn IndexProvider>> {
        self.providers.get(id).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "IndexProviderRegistry::require",
                format!("index provider '{id}' is not registered in this build"),
            )
        })
    }

    pub fn contains(&self, id: &CapabilityId) -> bool {
        self.providers.contains_key(id)
    }
}
