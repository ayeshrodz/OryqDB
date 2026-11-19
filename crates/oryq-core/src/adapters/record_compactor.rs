//! Adapter implementing `oryq_engine::RecordCompactor` using `oryq_document::compact_document`.

use oryq_document::compact_document;
use oryq_engine::RecordCompactor;
use oryq_types::{BudgetHandle, Result};

/// Stateless compactor bridging `oryq-engine` slotted-page compaction to `oryq-document`.
#[derive(Debug, Default)]
pub struct DocumentRecordCompactor;

impl RecordCompactor for DocumentRecordCompactor {
    fn compact(&self, base: &[u8], patches: &[&[u8]], budget: &BudgetHandle) -> Result<Vec<u8>> {
        let (bytes, _reservation) = compact_document(base, patches, Some(budget))?;
        Ok(bytes)
    }
}
