//! Canonical dotted field-path validation shared across filters, projections, and updates.

use core::fmt;

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

const MODULE_ID: &str = "document";

/// Maximum nested path segment depth permitted in OryqDB documents and queries.
pub const MAX_PATH_DEPTH: usize = 128;

/// Validated canonical dotted field path (e.g. `"profile.country"`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldPath(String);

impl FieldPath {
    /// Validates a dotted field path:
    /// - Non-empty, no NUL bytes (`\0`).
    /// - No leading, trailing, or consecutive dots (`""` segments).
    /// - No `$`-prefixed segments (reserved for query/update operators).
    /// - At most [`MAX_PATH_DEPTH`] (128) segments.
    pub fn parse(raw: &str) -> Result<Self> {
        if raw.is_empty() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "FieldPath::parse",
                "field path must not be empty",
            ));
        }
        if raw.as_bytes().contains(&0) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "FieldPath::parse",
                "field path must not contain NUL bytes",
            ));
        }
        let mut count = 0usize;
        for seg in raw.split('.') {
            count += 1;
            if count > MAX_PATH_DEPTH {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "FieldPath::parse",
                    format!("field path depth exceeds maximum {MAX_PATH_DEPTH}"),
                ));
            }
            if seg.is_empty() {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "FieldPath::parse",
                    format!("field path `{raw}` contains an empty segment"),
                ));
            }
            if seg.starts_with('$') {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "FieldPath::parse",
                    format!("field path segment `{seg}` must not start with `$`"),
                ));
            }
        }
        Ok(Self(raw.to_owned()))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[inline]
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('.')
    }

    #[inline]
    pub fn top_level(&self) -> &str {
        self.0.split('.').next().unwrap_or(&self.0)
    }

    #[inline]
    pub fn is_id(&self) -> bool {
        self.top_level() == "_id"
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
