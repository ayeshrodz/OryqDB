//! Distinct, checked identity newtypes shared across OryqDB domains.

use core::fmt;

use crate::error::{ErrorCategory, ErrorEnvelope, Result};

const MODULE_ID: &str = "types";

/// Logical page identifier within a database file (`0` is Page 0 header).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageId(u64);

impl PageId {
    /// Page 0 holds the dual shadow database headers.
    pub const HEADER: Self = Self(0);

    #[inline]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Returns the next sequential page identifier or an overflow error at `u64::MAX`.
    pub fn checked_next(self) -> Result<Self> {
        self.0.checked_add(1).map(Self).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "PageId::checked_next",
                "page identifier counter reached u64::MAX",
            )
        })
    }
}

impl fmt::Display for PageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PageId({})", self.0)
    }
}

/// Monotonic transaction identifier (`0` denotes the initial empty database state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TxId(u64);

/// Alias for transaction-pinned snapshot identifiers (see modular specification Section 4).
pub type SnapshotId = TxId;

impl TxId {
    /// Initial baseline transaction identifier before any committed write.
    pub const INITIAL: Self = Self(0);

    #[inline]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Returns the next monotonic transaction identifier or an overflow error at `u64::MAX`.
    pub fn checked_next(self) -> Result<Self> {
        self.0.checked_add(1).map(Self).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "TxId::checked_next",
                "transaction identifier counter reached u64::MAX",
            )
        })
    }
}

impl fmt::Display for TxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TxId({})", self.0)
    }
}

macro_rules! define_nonzero_id {
    ($(#[$meta:meta])* $name:ident, $label:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            /// Constructs a checked non-zero identifier (`raw >= 1`).
            pub fn new(raw: u64) -> Result<Self> {
                if raw == 0 {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::InvalidArgument,
                        MODULE_ID,
                        concat!(stringify!($name), "::new"),
                        concat!($label, " must be non-zero"),
                    ));
                }
                Ok(Self(raw))
            }

            #[inline]
            pub const fn raw(self) -> u64 {
                self.0
            }

            /// Returns the next monotonic identifier or an overflow error at `u64::MAX`.
            pub fn checked_next(self) -> Result<Self> {
                self.0.checked_add(1).map(Self).ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::Overflow,
                        MODULE_ID,
                        concat!(stringify!($name), "::checked_next"),
                        concat!($label, " counter reached u64::MAX"),
                    )
                })
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
    };
}

define_nonzero_id!(
    /// Stable non-zero collection catalog identifier.
    CollectionId,
    "collection identifier"
);

define_nonzero_id!(
    /// Stable non-zero index descriptor identifier.
    IndexId,
    "index identifier"
);

define_nonzero_id!(
    /// Stable non-zero storage namespace identifier for scoped provider trees/pages.
    NamespaceId,
    "namespace identifier"
);

define_nonzero_id!(
    /// Monotonic, never-reused internal document record identifier (`u64 >= 1`).
    RecordId,
    "record identifier"
);
