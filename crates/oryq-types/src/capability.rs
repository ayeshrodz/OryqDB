//! Stable capability identifiers, feature bitmasks, and inclusive version ranges.

use core::fmt;

use crate::error::{ErrorCategory, ErrorEnvelope, Result};

const MODULE_ID: &str = "types";

/// Validated stable string identifier for a module capability (e.g. `oryq.index.vector.v1`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Maximum byte length of a capability identifier.
    pub const MAX_LEN: usize = 128;

    /// Validates that `id` is non-empty, <= 128 bytes, and contains only ASCII lowercase
    /// letters, digits, `.`, `_`, or `-`, without leading/trailing/consecutive dots.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.is_empty() || id.len() > Self::MAX_LEN {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "CapabilityId::new",
                format!(
                    "capability id length must be 1..={} bytes (got {})",
                    Self::MAX_LEN,
                    id.len()
                ),
            ));
        }
        if id.starts_with('.') || id.ends_with('.') || id.contains("..") {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "CapabilityId::new",
                format!("capability id `{id}` has invalid dot placement"),
            ));
        }
        if !id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        }) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "CapabilityId::new",
                format!("capability id `{id}` contains invalid characters"),
            ));
        }
        Ok(Self(id))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 64-bit persistent feature requirement bitmask (`0..=63`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FeatureBitmask(u64);

impl FeatureBitmask {
    pub const EMPTY: Self = Self(0);

    #[inline]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Sets a single bit position in `0..=63`.
    pub fn with_bit(self, bit: u8) -> Result<Self> {
        if bit >= 64 {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "FeatureBitmask::with_bit",
                format!("feature bit position {bit} exceeds maximum 63"),
            ));
        }
        Ok(Self(self.0 | (1u64 << bit)))
    }

    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[inline]
    pub const fn contains_all(self, required: Self) -> bool {
        (self.0 & required.0) == required.0
    }

    /// Returns the bits present in `required` that are missing from `self`.
    #[inline]
    pub const fn missing_from(self, required: Self) -> Self {
        Self(required.0 & !self.0)
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Inclusive version range (`min <= max`) for provider data or options schemas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VersionRange {
    min: u16,
    max: u16,
}

impl VersionRange {
    pub fn new(min: u16, max: u16) -> Result<Self> {
        if min > max {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "VersionRange::new",
                format!("invalid version range: min ({min}) > max ({max})"),
            ));
        }
        Ok(Self { min, max })
    }

    #[inline]
    pub const fn min(self) -> u16 {
        self.min
    }

    #[inline]
    pub const fn max(self) -> u16 {
        self.max
    }

    #[inline]
    pub const fn contains(self, version: u16) -> bool {
        version >= self.min && version <= self.max
    }
}

/// Immutable capability metadata shared between registration and catalog checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDescriptor {
    pub id: CapabilityId,
    pub interface_version: u16,
    pub persistent: bool,
    pub required_read_bits: FeatureBitmask,
    pub required_write_bits: FeatureBitmask,
}
