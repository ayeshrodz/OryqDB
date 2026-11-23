//! Opaque FFI handle wrappers with child-reference lifecycle enforcement.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use oryq_core::{Database, DocumentBuf, QueryCursor, ReadTransaction, WriteTransaction};

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
        self.active_children.load(Ordering::Acquire) > 0
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
        let db_owner = Arc::clone(&parent.db);
        let db_ptr: *const Database = Arc::as_ptr(&db_owner);
        // SAFETY: `db_owner` is moved into `Self` and dropped strictly AFTER `self.tx` in `Drop`.
        // `Arc<Database>` keeps the heap allocation at a stable address for the lifetime of `Self`.
        let db_ref: &'static Database = unsafe { &*db_ptr };
        let tx = db_ref.begin_read()?;
        parent.active_children.fetch_add(1, Ordering::AcqRel);
        Ok(Self {
            tx: Some(tx),
            _db_owner: db_owner,
            parent_children: Arc::clone(&parent.active_children),
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
        let db_owner = Arc::clone(&parent.db);
        let db_ptr: *const Database = Arc::as_ptr(&db_owner);
        // SAFETY: `db_owner` is moved into `Self` and dropped strictly AFTER `self.tx` in `Drop`.
        let db_ref: &'static Database = unsafe { &*db_ptr };
        let tx = db_ref.begin_write()?;
        parent.active_children.fetch_add(1, Ordering::AcqRel);
        Ok(Self {
            tx: Some(tx),
            _db_owner: db_owner,
            parent_children: Arc::clone(&parent.active_children),
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
