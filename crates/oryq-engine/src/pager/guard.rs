//! Owned page buffers and RAII page guards with optional memory budget accounting.

use oryq_types::{BudgetHandle, PageId, Reservation, Result};

use crate::config::PageSize;

/// Owned page-sized byte buffer with optional RAII memory budget reservation.
#[derive(Debug)]
pub struct PageBuf {
    bytes: Vec<u8>,
    _reservation: Option<Reservation>,
}

impl PageBuf {
    /// Allocates a zero-initialized buffer of `page_size.bytes()`, charging `budget` if supplied.
    pub fn zeroed(page_size: PageSize, budget: Option<&BudgetHandle>) -> Result<Self> {
        let len = page_size.bytes();
        let reservation = match budget {
            Some(b) => Some(b.reserve(len)?),
            None => None,
        };
        Ok(Self {
            bytes: vec![0u8; len],
            _reservation: reservation,
        })
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// RAII pin tying a [`PageId`] to its validated in-memory [`PageBuf`].
#[derive(Debug)]
pub struct PageGuard {
    page_id: PageId,
    buf: PageBuf,
}

impl PageGuard {
    pub fn new(page_id: PageId, buf: PageBuf) -> Self {
        Self { page_id, buf }
    }

    #[inline]
    pub fn page_id(&self) -> PageId {
        self.page_id
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        self.buf.as_slice()
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.buf.as_mut_slice()
    }

    #[inline]
    pub fn into_buf(self) -> PageBuf {
        self.buf
    }
}
