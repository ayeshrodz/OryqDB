//! Deterministic Unicode-aware alphanumeric lowercase tokenizer for full-text search.

use std::collections::BTreeMap;

/// Maximum UTF-8 byte length of a single indexed token.
pub const MAX_TOKEN_BYTES: usize = 64;

/// Tokenizes `text` into lowercase alphanumeric terms, splitting on non-alphanumeric characters
/// and filtering out tokens exceeding [`MAX_TOKEN_BYTES`].
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                current.push(lc);
            }
        } else if !current.is_empty() {
            push_if_valid(&mut tokens, &mut current);
        }
    }
    if !current.is_empty() {
        push_if_valid(&mut tokens, &mut current);
    }
    tokens
}

fn push_if_valid(out: &mut Vec<String>, buf: &mut String) {
    if buf.len() <= MAX_TOKEN_BYTES {
        out.push(std::mem::take(buf));
    } else {
        buf.clear();
    }
}

/// Computes `(term -> term_frequency)` and total token count for an iterator of text slices.
pub fn count_terms<'a>(texts: impl IntoIterator<Item = &'a str>) -> (BTreeMap<String, u32>, u32) {
    let mut freqs: BTreeMap<String, u32> = BTreeMap::new();
    let mut total: u32 = 0;
    for text in texts {
        for tok in tokenize(text) {
            *freqs.entry(tok).or_insert(0) += 1;
            total = total.saturating_add(1);
        }
    }
    (freqs, total)
}
