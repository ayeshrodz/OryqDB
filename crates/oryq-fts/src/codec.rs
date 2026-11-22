//! B+ tree key/value encoding for the FTS inverted index (`TreeKind::Index`).
//!
//! Key namespaces (single leading discriminant byte):
//! - `0x00`: Corpus stats -> `[doc_count: u64 LE][total_tokens: u64 LE]` (16 bytes)
//! - `0x01`: Per-doc token length `[0x01][record_id: u64 BE]` -> `[token_count: u32 LE]` (4 bytes)
//! - `0x02`: Term posting `[0x02][term_utf8][0x00][record_id: u64 BE]` -> `[tf: u32 LE]` (4 bytes)

use oryq_types::{ErrorCategory, ErrorEnvelope, RecordId, Result};

const MODULE_ID: &str = "fts";

pub const KEY_STATS: &[u8] = &[0x00];
pub const PREFIX_DOC_LEN: u8 = 0x01;
pub const PREFIX_POSTING: u8 = 0x02;

/// Corpus-wide statistics persisted under [`KEY_STATS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CorpusStats {
    pub doc_count: u64,
    pub total_tokens: u64,
}

impl CorpusStats {
    pub fn encode(self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..8].copy_from_slice(&self.doc_count.to_le_bytes());
        out[8..16].copy_from_slice(&self.total_tokens.to_le_bytes());
        out
    }

    pub fn decode(raw: &[u8]) -> Result<Self> {
        if raw.len() != 16 {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "CorpusStats::decode",
                format!("expected 16-byte FTS corpus stats, got {}", raw.len()),
            ));
        }
        Ok(Self {
            doc_count: u64::from_le_bytes(raw[0..8].try_into().unwrap()),
            total_tokens: u64::from_le_bytes(raw[8..16].try_into().unwrap()),
        })
    }
}

pub fn encode_doc_len_key(record_id: RecordId) -> [u8; 9] {
    let mut out = [0u8; 9];
    out[0] = PREFIX_DOC_LEN;
    out[1..9].copy_from_slice(&record_id.raw().to_be_bytes());
    out
}

pub fn encode_posting_prefix(term: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + term.len() + 1);
    out.push(PREFIX_POSTING);
    out.extend_from_slice(term.as_bytes());
    out.push(0x00);
    out
}

pub fn encode_posting_key(term: &str, record_id: RecordId) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + term.len() + 1 + 8);
    out.push(PREFIX_POSTING);
    out.extend_from_slice(term.as_bytes());
    out.push(0x00);
    out.extend_from_slice(&record_id.raw().to_be_bytes());
    out
}

pub fn decode_u32_le(raw: &[u8], op: &'static str) -> Result<u32> {
    if raw.len() != 4 {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            op,
            format!("expected 4-byte u32 value, got {}", raw.len()),
        ));
    }
    Ok(u32::from_le_bytes(raw[0..4].try_into().unwrap()))
}

pub fn decode_posting_record_id(key: &[u8], prefix: &[u8]) -> Result<RecordId> {
    if key.len() != prefix.len() + 8 || !key.starts_with(prefix) {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "codec::decode_posting_record_id",
            "malformed FTS posting key",
        ));
    }
    let raw = u64::from_be_bytes(key[prefix.len()..prefix.len() + 8].try_into().unwrap());
    RecordId::new(raw)
}
