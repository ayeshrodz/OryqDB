//! Shared module-tagged error envelope and stable error categories.

use core::fmt;
use std::error::Error;

/// Canonical result alias for cross-crate OryqDB contracts.
pub type Result<T> = core::result::Result<T, ErrorEnvelope>;

/// Stable, coarse-grained error categories shared across domain crates.
///
/// Domain-specific details remain in the owning crate's message or source error
/// rather than growing a global feature enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    InvalidArgument,
    InvalidQuery,
    NotFound,
    DuplicateKey,
    TypeMismatch,
    Overflow,
    QuotaExceeded,
    Cancelled,
    Busy,
    LockedByProcess,
    SnapshotTooOld,
    UnsupportedFeature,
    Io,
    Corruption,
    CommitOutcomeUnknown,
    Internal,
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::InvalidArgument => "InvalidArgument",
            Self::InvalidQuery => "InvalidQuery",
            Self::NotFound => "NotFound",
            Self::DuplicateKey => "DuplicateKey",
            Self::TypeMismatch => "TypeMismatch",
            Self::Overflow => "Overflow",
            Self::QuotaExceeded => "QuotaExceeded",
            Self::Cancelled => "Cancelled",
            Self::Busy => "Busy",
            Self::LockedByProcess => "LockedByProcess",
            Self::SnapshotTooOld => "SnapshotTooOld",
            Self::UnsupportedFeature => "UnsupportedFeature",
            Self::Io => "Io",
            Self::Corruption => "Corruption",
            Self::CommitOutcomeUnknown => "CommitOutcomeUnknown",
            Self::Internal => "Internal",
        };
        f.write_str(label)
    }
}

/// Optional structured forensic location attached to storage or parser errors.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ErrorLocation {
    pub origin: String,
    pub page_or_frame_id: Option<u64>,
    pub byte_offset: Option<u64>,
}

impl ErrorLocation {
    pub fn new(
        origin: impl Into<String>,
        page_or_frame_id: Option<u64>,
        byte_offset: Option<u64>,
    ) -> Self {
        Self {
            origin: origin.into(),
            page_or_frame_id,
            byte_offset,
        }
    }
}

impl fmt::Display for ErrorLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "origin={}", self.origin)?;
        if let Some(id) = self.page_or_frame_id {
            write!(f, ", id={id}")?;
        }
        if let Some(offset) = self.byte_offset {
            write!(f, ", offset={offset}")?;
        }
        Ok(())
    }
}

/// Module-tagged error envelope carrying category, origin, location, and optional source chain.
///
/// Boxed internally so `Result<T, ErrorEnvelope>` remains pointer-sized on hot paths.
pub struct ErrorEnvelope(Box<ErrorEnvelopeInner>);

struct ErrorEnvelopeInner {
    category: ErrorCategory,
    module_id: &'static str,
    operation: &'static str,
    message: String,
    location: Option<ErrorLocation>,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl ErrorEnvelope {
    pub fn new(
        category: ErrorCategory,
        module_id: &'static str,
        operation: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self(Box::new(ErrorEnvelopeInner {
            category,
            module_id,
            operation,
            message: message.into(),
            location: None,
            source: None,
        }))
    }

    pub fn with_location(mut self, location: ErrorLocation) -> Self {
        self.0.location = Some(location);
        self
    }

    pub fn with_source<E>(mut self, source: E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        self.0.source = Some(Box::new(source));
        self
    }

    #[inline]
    pub fn category(&self) -> ErrorCategory {
        self.0.category
    }

    #[inline]
    pub fn module_id(&self) -> &'static str {
        self.0.module_id
    }

    #[inline]
    pub fn operation(&self) -> &'static str {
        self.0.operation
    }

    #[inline]
    pub fn message(&self) -> &str {
        &self.0.message
    }

    #[inline]
    pub fn location(&self) -> Option<&ErrorLocation> {
        self.0.location.as_ref()
    }
}

impl fmt::Debug for ErrorEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErrorEnvelope")
            .field("category", &self.0.category)
            .field("module_id", &self.0.module_id)
            .field("operation", &self.0.operation)
            .field("message", &self.0.message)
            .field("location", &self.0.location)
            .field("has_source", &self.0.source.is_some())
            .finish()
    }
}

impl fmt::Display for ErrorEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}:{}:{}] {}",
            self.0.module_id, self.0.operation, self.0.category, self.0.message
        )?;
        if let Some(loc) = &self.0.location {
            write!(f, " ({loc})")?;
        }
        Ok(())
    }
}

impl Error for ErrorEnvelope {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0
            .source
            .as_deref()
            .map(|err| err as &(dyn Error + 'static))
    }
}
