//! Opaque FFI handle wrappers with child-reference lifecycle enforcement.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use oryq_core::{
    Database, DocumentBuf, ErrorCategory, ErrorEnvelope, QueryCursor, ReadTransaction,
    WriteTransaction,
};

const CLOSING_BIT: usize = 1usize << (usize::BITS - 1);
const COUNT_MASK: usize = !CLOSING_BIT;

/// RAII guard holding one active child slot on an [`OryqDb`] until transferred to a live
/// transaction handle or dropped on error.
struct ChildSlotGuard {
    counter: Arc<AtomicUsize>,
    armed: bool,
}

impl ChildSlotGuard {
    fn disarm(mut self) -> Arc<AtomicUsize> {
        self.armed = false;
        Arc::clone(&self.counter)
    }
}

impl Drop for ChildSlotGuard {
    fn drop(&mut self) {
        if self.armed {
            self.counter.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Opaque C handle representing an open OryqDB database.
#[derive(Debug)]
pub struct OryqDb {
    pub(crate) db: Arc<Database>,
    pub(crate) active_children: Arc<AtomicUsize>,
}

impl OryqDb {
    pub(crate) fn new(db: Database) -> Self {
        Self {
            db: Arc::new(db),
            active_children: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(crate) fn has_live_children(&self) -> bool {
        (self.active_children.load(Ordering::Acquire) & COUNT_MASK) > 0
    }

    /// Atomically transitions `active_children` from `0` to `CLOSING_BIT`, preventing any
    /// concurrent `begin_read` / `begin_write` from acquiring a new child slot while closing.
    pub(crate) fn try_begin_close(&self) -> bool {
        self.active_children
            .compare_exchange(0, CLOSING_BIT, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Atomically acquires a child slot BEFORE blocking in `begin_read` / `begin_write`.
    /// Fails with `Busy` if `oryq_db_close` has already initiated closing.
    fn try_acquire_child_slot(&self) -> oryq_core::Result<ChildSlotGuard> {
        let mut cur = self.active_children.load(Ordering::Acquire);
        loop {
            if (cur & CLOSING_BIT) != 0 {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Busy,
                    "ffi",
                    "OryqDb::try_acquire_child_slot",
                    "database handle is closing",
                ));
            }
            let next = cur.checked_add(1).ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Busy,
                    "ffi",
                    "OryqDb::try_acquire_child_slot",
                    "active child transaction count overflow",
                )
            })?;
            if (next & CLOSING_BIT) != 0 {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Busy,
                    "ffi",
                    "OryqDb::try_acquire_child_slot",
                    "active child transaction count overflow",
                ));
            }
            match self.active_children.compare_exchange_weak(
                cur,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(ChildSlotGuard {
                        counter: Arc::clone(&self.active_children),
                        armed: true,
                    });
                }
                Err(actual) => cur = actual,
            }
        }
    }
}

/// Opaque C handle representing a snapshot-isolated read transaction.
///
/// Field drop order is strictly `tx` BEFORE `_db_owner` so the `Database` behind the
/// `'static` reference outlives `ReadTransaction` for the entire lifetime of this struct.
#[derive(Debug)]
pub struct OryqReadTxn {
    pub(crate) tx: Option<ReadTransaction<'static>>,
    _db_owner: Arc<Database>,
    parent_children: Arc<AtomicUsize>,
}

impl OryqReadTxn {
    pub(crate) fn begin(parent: &OryqDb) -> oryq_core::Result<Self> {
        let slot = parent.try_acquire_child_slot()?;
        let db_owner = Arc::clone(&parent.db);
        let db_ptr: *const Database = Arc::as_ptr(&db_owner);
        // SAFETY: `db_owner` is moved into `Self` and dropped strictly AFTER `self.tx` in `Drop`.
        // `Arc<Database>` keeps the heap allocation at a stable address for the lifetime of `Self`.
        let db_ref: &'static Database = unsafe { &*db_ptr };
        let tx = db_ref.begin_read()?;
        let parent_children = slot.disarm();
        Ok(Self {
            tx: Some(tx),
            _db_owner: db_owner,
            parent_children,
        })
    }
}

impl Drop for OryqReadTxn {
    fn drop(&mut self) {
        self.tx.take();
        self.parent_children.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Opaque C handle representing an exclusive write transaction.
///
/// Field drop order is strictly `tx` BEFORE `_db_owner`.
#[derive(Debug)]
pub struct OryqWriteTxn {
    pub(crate) tx: Option<WriteTransaction<'static>>,
    _db_owner: Arc<Database>,
    parent_children: Arc<AtomicUsize>,
}

impl OryqWriteTxn {
    pub(crate) fn begin(parent: &OryqDb) -> oryq_core::Result<Self> {
        let slot = parent.try_acquire_child_slot()?;
        let db_owner = Arc::clone(&parent.db);
        let db_ptr: *const Database = Arc::as_ptr(&db_owner);
        // SAFETY: `db_owner` is moved into `Self` and dropped strictly AFTER `self.tx` in `Drop`.
        let db_ref: &'static Database = unsafe { &*db_ptr };
        let tx = db_ref.begin_write()?;
        let parent_children = slot.disarm();
        Ok(Self {
            tx: Some(tx),
            _db_owner: db_owner,
            parent_children,
        })
    }

    pub(crate) fn try_begin(parent: &OryqDb) -> oryq_core::Result<Self> {
        let slot = parent.try_acquire_child_slot()?;
        let db_owner = Arc::clone(&parent.db);
        let db_ptr: *const Database = Arc::as_ptr(&db_owner);
        // SAFETY: `db_owner` is moved into `Self` and dropped strictly AFTER `self.tx` in `Drop`.
        let db_ref: &'static Database = unsafe { &*db_ptr };
        let tx = db_ref.try_begin_write()?;
        let parent_children = slot.disarm();
        Ok(Self {
            tx: Some(tx),
            _db_owner: db_owner,
            parent_children,
        })
    }
}

impl Drop for OryqWriteTxn {
    fn drop(&mut self) {
        self.tx.take();
        self.parent_children.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug)]
pub(crate) enum CursorBacking {
    Query(QueryCursor),
    Docs(std::vec::IntoIter<DocumentBuf>),
    #[cfg(any(feature = "fts", feature = "vec"))]
    ScoredDocs(std::vec::IntoIter<(DocumentBuf, f64)>),
}

impl CursorBacking {
    pub(crate) fn next_row(&mut self) -> Option<(DocumentBuf, Option<f64>)> {
        match self {
            Self::Query(q) => q.next_doc().map(|d| (d, None)),
            Self::Docs(it) => it.next().map(|d| (d, None)),
            #[cfg(any(feature = "fts", feature = "vec"))]
            Self::ScoredDocs(it) => it.next().map(|(d, s)| (d, Some(s))),
        }
    }
}

/// Opaque C handle representing a query, aggregation, FTS, or vector search result cursor.
#[derive(Debug)]
pub struct OryqCursor {
    pub(crate) backing: CursorBacking,
    pub(crate) current: Option<DocumentBuf>,
    pub(crate) current_score: Option<f64>,
}

impl OryqCursor {
    pub(crate) fn new(cursor: QueryCursor) -> Self {
        Self {
            backing: CursorBacking::Query(cursor),
            current: None,
            current_score: None,
        }
    }

    pub(crate) fn from_docs(docs: Vec<DocumentBuf>) -> Self {
        Self {
            backing: CursorBacking::Docs(docs.into_iter()),
            current: None,
            current_score: None,
        }
    }

    #[cfg(any(feature = "fts", feature = "vec"))]
    pub(crate) fn from_scored_docs(docs: Vec<(DocumentBuf, f64)>) -> Self {
        Self {
            backing: CursorBacking::ScoredDocs(docs.into_iter()),
            current: None,
            current_score: None,
        }
    }
}
