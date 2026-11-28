//! Administrative operations: snapshot-consistent integrity checking, sealed online backup,
//! and read-only forensic inspection facade (`W14`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use oryq_engine::{
    inspect_locks_readonly, inspect_page_readonly, inspect_wal_readonly, wal_path_for, BTree,
    LockInspectionReport, PageInspectionReport, PageSize, PageStore, StorageIo, TreeKind,
    WalInspectionReport,
};
use oryq_index::{
    decode_record_id, extract_compound_key_prefixes, extract_secondary_key_prefixes,
    scan_primary_record_ids, split_record_id_suffix, IndexKind,
};
use oryq_types::{ErrorCategory, ErrorEnvelope, RecordId, Result};

use crate::config::DatabaseConfig;
use crate::database::Database;
use crate::write::decode_collection_record_from_store;

const MODULE_ID: &str = "core";
static TMP_SEQ: AtomicU64 = AtomicU64::new(1);

/// Summary report returned by [`Database::integrity_check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityReport {
    pub ok: bool,
    pub collections_checked: usize,
    pub documents_checked: usize,
    pub indexes_checked: usize,
    pub issues: Vec<String>,
}

/// Summary report returned by [`Database::backup_to`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupReport {
    pub source_path: PathBuf,
    pub dest_path: PathBuf,
    pub collections_copied: usize,
    pub documents_copied: usize,
    pub sealed: bool,
}

/// Summary report returned by [`Database::vacuum`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VacuumReport {
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub bytes_reclaimed: u64,
    pub collections_compacted: usize,
    pub documents_compacted: usize,
    pub sealed: bool,
}

/// Runs a full snapshot-consistent integrity check across every collection and index in `db`.
pub fn run_integrity_check(db: &Database) -> Result<IntegrityReport> {
    let rtx = db.begin_read()?;
    let col_names = rtx.list_collections()?;
    let mut issues = Vec::new();
    let mut docs_checked = 0usize;
    let mut indexes_checked = 0usize;

    for col_name in &col_names {
        let col = rtx.collection(col_name)?;
        let tx = col.raw_tx();
        let entry = col.catalog_entry();
        let data_tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
        let raw_rows = data_tree.scan_all(tx)?;

        let mut docs_with_rids = Vec::with_capacity(raw_rows.len());
        let mut live_rids = BTreeSet::new();
        for (raw_k, raw_v) in raw_rows {
            let rid = decode_record_id(&raw_k)?;
            let doc = decode_collection_record_from_store(tx, &raw_v)?;
            live_rids.insert(rid);
            docs_with_rids.push((rid, doc));
        }

        let indexes = col.list_indexes();
        indexes_checked += indexes.len();

        // 1. Verify every document has a valid `_id` and resolves via primary index.
        for (_, doc) in &docs_with_rids {
            docs_checked += 1;
            let Some(id_view) = doc.view().get_field("_id") else {
                issues.push(format!("collection '{col_name}': document missing '_id'"));
                continue;
            };
            let id_val = id_view.to_owned_value();
            match col.find_by_id(&id_val)? {
                Some(found) => {
                    if found.as_bytes() != doc.as_bytes() {
                        issues.push(format!(
                            "collection '{col_name}': primary index lookup mismatch for _id={id_val:?}"
                        ));
                    }
                }
                None => {
                    issues.push(format!(
                        "collection '{col_name}': document _id={id_val:?} missing from primary index"
                    ));
                }
            }
        }

        // 2. Verify collection count matches scanned documents.
        let count = col.count()?;
        if count != docs_with_rids.len() {
            issues.push(format!(
                "collection '{col_name}': count ({count}) != scanned docs ({})",
                docs_with_rids.len()
            ));
        }

        // 3. Verify primary index reverse consistency (no orphan/extra primary entries).
        if let Some(primary) = indexes.iter().find(|d| d.kind == IndexKind::Primary) {
            let primary_rids = scan_primary_record_ids(tx, primary.root_page_id)?;
            if primary_rids.len() != docs_with_rids.len() {
                issues.push(format!(
                    "collection '{col_name}': primary index entry count ({}) != collection doc count ({})",
                    primary_rids.len(),
                    docs_with_rids.len()
                ));
            }
            for rid in primary_rids {
                if !live_rids.contains(&rid) {
                    issues.push(format!(
                        "collection '{col_name}': orphan RecordId {rid:?} in primary index"
                    ));
                }
            }
        }

        // 4. Verify secondary and compound indexes match collection documents exactly.
        let page_size = tx.page_size();
        for desc in indexes {
            if !matches!(desc.kind, IndexKind::Secondary | IndexKind::Compound) {
                continue;
            }
            let idx_tree = BTree::open(desc.root_page_id, TreeKind::Index);
            let actual_pairs = idx_tree.scan_all(tx)?;
            let mut actual_set: BTreeSet<(Vec<u8>, RecordId)> = BTreeSet::new();
            let mut unique_check: BTreeMap<Vec<u8>, RecordId> = BTreeMap::new();

            for (raw_k, _) in actual_pairs {
                let (prefix, rid) = split_record_id_suffix(&raw_k)?;
                let prefix_vec = prefix.to_vec();
                if desc.unique {
                    if let Some(&prev_rid) = unique_check.get(&prefix_vec) {
                        if prev_rid != rid {
                            issues.push(format!(
                                "collection '{col_name}': unique index '{}' has duplicate prefix for {prev_rid:?} and {rid:?}",
                                desc.name
                            ));
                        }
                    } else {
                        unique_check.insert(prefix_vec.clone(), rid);
                    }
                }
                actual_set.insert((prefix_vec, rid));
            }

            let mut expected_set: BTreeSet<(Vec<u8>, RecordId)> = BTreeSet::new();
            for (rid, doc) in &docs_with_rids {
                let prefixes = match desc.kind {
                    IndexKind::Secondary => {
                        extract_secondary_key_prefixes(desc, &doc.view(), page_size)?
                    }
                    IndexKind::Compound => {
                        extract_compound_key_prefixes(desc, &doc.view(), page_size)?
                    }
                    _ => Vec::new(),
                };
                for p in prefixes {
                    expected_set.insert((p, *rid));
                }
            }

            let missing = expected_set.difference(&actual_set).count();
            let extra = actual_set.difference(&expected_set).count();
            if missing > 0 || extra > 0 {
                issues.push(format!(
                    "collection '{col_name}': index '{}' mismatch (missing={missing}, extra={extra})",
                    desc.name
                ));
            }
        }
    }

    Ok(IntegrityReport {
        ok: issues.is_empty(),
        collections_checked: col_names.len(),
        documents_checked: docs_checked,
        indexes_checked,
        issues,
    })
}

/// Produces a standalone, sealed (`.wal`-free) online backup of `db` at `dest`.
///
/// - Rejects `dest` matching the live source database path.
/// - Reads under a single pinned MVCC read snapshot so concurrent readers remain unblocked.
/// - Builds a fresh database at `<dest>.tmp.<pid>`, checkpoints and seals it, verifies integrity,
///   and atomically renames it into place (removing temp artifacts on any failure).
pub fn run_online_backup(
    db: &Database,
    source_path: &Path,
    source_in_memory: bool,
    page_size: PageSize,
    dest: &Path,
) -> Result<BackupReport> {
    if dest == Path::new(Database::IN_MEMORY_PATH) {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "admin::run_online_backup",
            "backup destination must be a real file path, not ':memory:'",
        ));
    }
    if !source_in_memory && paths_refer_to_same_target(source_path, dest) {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "admin::run_online_backup",
            "backup destination must not be the same file as the live source database",
        ));
    }

    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| {
                ErrorEnvelope::new(
                    ErrorCategory::Io,
                    MODULE_ID,
                    "admin::run_online_backup",
                    format!("failed to create backup parent directory: {err}"),
                )
                .with_source(err)
            })?;
        }
    }

    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = dest.with_extension(format!("oryq.backup.tmp.{}.{seq}", std::process::id()));
    let tmp_wal = wal_path_for(&tmp_path);
    let _ = fs::remove_file(&tmp_path);
    let _ = fs::remove_file(&tmp_wal);

    let result = (|| -> Result<BackupReport> {
        let (collections_copied, documents_copied, sealed) = {
            let backup_cfg = DatabaseConfig {
                page_size,
                max_document_bytes: db.max_document_bytes(),
                max_wal_hard_bytes: u64::MAX / 2,
                ..DatabaseConfig::default()
            };
            let backup_db = Database::open(&tmp_path, backup_cfg)?;
            copy_snapshot_into_db(db, &backup_db)?
        };

        // Remove any empty companion WAL before atomic publish
        let _ = fs::remove_file(&tmp_wal);
        fs::rename(&tmp_path, dest).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "admin::run_online_backup",
                format!(
                    "failed to atomically publish backup to '{}': {err}",
                    dest.display()
                ),
            )
            .with_source(err)
        })?;

        Ok(BackupReport {
            source_path: source_path.to_path_buf(),
            dest_path: dest.to_path_buf(),
            collections_copied,
            documents_copied,
            sealed,
        })
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
        let _ = fs::remove_file(&tmp_wal);
    }

    result
}

const BACKUP_CHUNK_DOCS: usize = 1_000;
const BACKUP_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const BACKUP_CHECKPOINT_DOCS: usize = 10_000;
const BACKUP_CHECKPOINT_BYTES: usize = 64 * 1024 * 1024;

fn flush_backup_batch(
    target_db: &Database,
    col_name: &str,
    batch: &mut Vec<oryq_document::DocumentBuf>,
    batch_bytes: &mut usize,
    docs_since_ckpt: &mut usize,
    bytes_since_ckpt: &mut usize,
) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let mut wtx = target_db.begin_write()?;
    {
        let mut dst_col = wtx.collection(col_name)?;
        for doc in batch.drain(..) {
            dst_col.insert(&doc)?;
        }
    }
    wtx.commit()?;
    *batch_bytes = 0;

    if *docs_since_ckpt >= BACKUP_CHECKPOINT_DOCS || *bytes_since_ckpt >= BACKUP_CHECKPOINT_BYTES {
        target_db.checkpoint(true)?;
        *docs_since_ckpt = 0;
        *bytes_since_ckpt = 0;
    }
    Ok(())
}

fn copy_snapshot_into_db(db: &Database, target_db: &Database) -> Result<(usize, usize, bool)> {
    // Hold one consistent MVCC read snapshot for the entire backup/compaction copy.
    let rtx = db.begin_read()?;
    let col_names = rtx.list_collections()?;

    let mut collections_copied = 0usize;
    let mut documents_copied = 0usize;
    let mut docs_since_ckpt = 0usize;
    let mut bytes_since_ckpt = 0usize;

    for col_name in &col_names {
        let src_col = rtx.collection(col_name)?;

        // Create collection and all non-primary indexes in a dedicated DDL transaction.
        {
            let mut wtx = target_db.begin_write()?;
            wtx.create_collection(col_name)?;
            let mut dst_col = wtx.collection(col_name)?;

            for idx in src_col.list_indexes() {
                match idx.kind {
                    IndexKind::Primary => {}
                    IndexKind::Secondary => {
                        if let Some(f) = idx.fields.first() {
                            dst_col.create_index(
                                f.path.as_str(),
                                f.order,
                                idx.unique,
                                idx.sparse,
                                Some(&idx.name),
                            )?;
                        }
                    }
                    IndexKind::Compound => {
                        let specs: Vec<(&str, oryq_index::SortOrder)> = idx
                            .fields
                            .iter()
                            .map(|f| (f.path.as_str(), f.order))
                            .collect();
                        dst_col.create_compound_index(
                            &specs,
                            idx.unique,
                            idx.sparse,
                            Some(&idx.name),
                        )?;
                    }
                    IndexKind::Pluggable(ref kind_name) => {
                        #[cfg(feature = "fts")]
                        if kind_name == "fts" {
                            let field_paths: Vec<&str> =
                                idx.fields.iter().map(|f| f.path.as_str()).collect();
                            dst_col.create_fts_index(&field_paths, Some(&idx.name))?;
                            continue;
                        }
                        #[cfg(feature = "vec")]
                        if kind_name == "vec" {
                            let field = idx.fields.first().ok_or_else(|| {
                                ErrorEnvelope::new(
                                    ErrorCategory::Corruption,
                                    MODULE_ID,
                                    "admin::run_online_backup",
                                    format!(
                                        "vector index '{}' missing field specification",
                                        idx.name
                                    ),
                                )
                            })?;
                            let opts_raw = idx.options_json.as_deref().ok_or_else(|| {
                                ErrorEnvelope::new(
                                    ErrorCategory::Corruption,
                                    MODULE_ID,
                                    "admin::run_online_backup",
                                    format!("vector index '{}' missing options_json", idx.name),
                                )
                            })?;
                            let opts = oryq_vec::VectorIndexOptions::from_json(opts_raw)?;
                            dst_col.create_vector_index(
                                field.path.as_str(),
                                opts,
                                Some(&idx.name),
                            )?;
                            continue;
                        }
                        return Err(ErrorEnvelope::new(
                            ErrorCategory::UnsupportedFeature,
                            MODULE_ID,
                            "admin::run_online_backup",
                            format!(
                                "cannot back up pluggable index '{}' (kind '{kind_name}') without its provider enabled",
                                idx.name
                            ),
                        ));
                    }
                }
            }
            wtx.commit()?;
        }

        // Stream all documents in bounded-memory chunks and checkpoint periodically so
        // large databases (>256 MiB) never OOM RAM or grow the target WAL unboundedly.
        let mut batch = Vec::with_capacity(BACKUP_CHUNK_DOCS.min(256));
        let mut batch_bytes = 0usize;
        src_col.for_each_document(|doc| {
            let doc_len = doc.as_bytes().len();
            batch_bytes = batch_bytes.saturating_add(doc_len);
            documents_copied += 1;
            docs_since_ckpt += 1;
            bytes_since_ckpt = bytes_since_ckpt.saturating_add(doc_len);
            batch.push(doc);

            if batch.len() >= BACKUP_CHUNK_DOCS || batch_bytes >= BACKUP_CHUNK_BYTES {
                flush_backup_batch(
                    target_db,
                    col_name,
                    &mut batch,
                    &mut batch_bytes,
                    &mut docs_since_ckpt,
                    &mut bytes_since_ckpt,
                )?;
            }
            Ok(true)
        })?;

        flush_backup_batch(
            target_db,
            col_name,
            &mut batch,
            &mut batch_bytes,
            &mut docs_since_ckpt,
            &mut bytes_since_ckpt,
        )?;
        collections_copied += 1;
    }

    // Checkpoint & seal target so it is standalone (wal_required == false)
    target_db.checkpoint(true)?;
    let check = target_db.integrity_check()?;
    if !check.ok {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "admin::run_online_backup",
            format!("backup integrity verification failed: {:?}", check.issues),
        ));
    }
    let sealed = target_db.is_sealed()?;
    Ok((collections_copied, documents_copied, sealed))
}

fn build_in_memory_compacted_image(
    db: &Database,
    io: &Arc<dyn StorageIo>,
    page_size: PageSize,
    tmp_path: &Path,
) -> Result<(usize, usize)> {
    let tmp_wal = wal_path_for(tmp_path);
    let _ = io.remove_file(tmp_path);
    let _ = io.remove_file(&tmp_wal);

    let res = (|| -> Result<(usize, usize)> {
        let backup_cfg = DatabaseConfig {
            page_size,
            max_document_bytes: db.max_document_bytes(),
            max_wal_hard_bytes: u64::MAX / 2,
            ..DatabaseConfig::default()
        };
        let target_db = Database::open_with_io(Arc::clone(io), tmp_path, backup_cfg)?;
        let (collections_copied, documents_copied, _) = copy_snapshot_into_db(db, &target_db)?;
        Ok((collections_copied, documents_copied))
    })();

    let _ = io.remove_file(&tmp_wal);
    if res.is_err() {
        let _ = io.remove_file(tmp_path);
    }
    res
}

/// Compacts `db` in place (`VACUUM`), reclaiming freelist pages, shrinking the `.oryq` file on
/// disk (or in memory for `:memory:`), and leaving a single sealed `.oryq` file with no `.oryq-wal`
/// sidecar.
///
/// Fails with `ErrorCategory::Busy` if any active read transaction is currently open, or
/// `ErrorCategory::UnsupportedFeature` if a collection contains a disabled pluggable index.
pub fn run_vacuum(
    db: &Database,
    engine: &oryq_engine::StorageEngine,
    source_path: &Path,
    in_memory: bool,
    page_size: PageSize,
) -> Result<VacuumReport> {
    if engine.is_read_only() {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "admin::run_vacuum",
            "cannot VACUUM a read-only database",
        ));
    }
    if engine.is_poisoned() {
        return Err(ErrorEnvelope::new(
            ErrorCategory::CommitOutcomeUnknown,
            MODULE_ID,
            "admin::run_vacuum",
            "database engine is poisoned; close and reopen to recover before VACUUM",
        ));
    }

    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    if in_memory {
        let tmp_path = PathBuf::from(format!(
            "{}.vacuum.tmp.{}.{seq}",
            source_path.display(),
            std::process::id()
        ));
        let tmp_wal = wal_path_for(&tmp_path);
        let io = Arc::clone(engine.io());
        let res = engine
            .run_exclusive_vacuum(&tmp_path, |p| {
                build_in_memory_compacted_image(db, &io, page_size, p)
            })
            .map(
                |((collections_compacted, documents_compacted), bytes_before, bytes_after)| {
                    VacuumReport {
                        bytes_before,
                        bytes_after,
                        bytes_reclaimed: bytes_before.saturating_sub(bytes_after),
                        collections_compacted,
                        documents_compacted,
                        sealed: true,
                    }
                },
            );
        let _ = engine.io().remove_file(&tmp_path);
        let _ = engine.io().remove_file(&tmp_wal);
        return res;
    }

    let tmp_path =
        source_path.with_extension(format!("oryq.vacuum.tmp.{}.{seq}", std::process::id()));
    let tmp_wal = wal_path_for(&tmp_path);
    let _ = fs::remove_file(&tmp_path);
    let _ = fs::remove_file(&tmp_wal);

    let res = engine
        .run_exclusive_vacuum(&tmp_path, |p| {
            run_online_backup(db, source_path, false, page_size, p)
        })
        .map(|(backup, bytes_before, bytes_after)| VacuumReport {
            bytes_before,
            bytes_after,
            bytes_reclaimed: bytes_before.saturating_sub(bytes_after),
            collections_compacted: backup.collections_copied,
            documents_compacted: backup.documents_copied,
            sealed: true,
        });

    let _ = fs::remove_file(&tmp_path);
    let _ = fs::remove_file(&tmp_wal);
    res
}

fn paths_refer_to_same_target(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

pub fn inspect_page(path: &Path, page_id: u64) -> Result<PageInspectionReport> {
    inspect_page_readonly(path, page_id)
}

pub fn inspect_wal(path: &Path) -> Result<WalInspectionReport> {
    inspect_wal_readonly(path)
}

pub fn inspect_locks(path: &Path) -> Result<LockInspectionReport> {
    inspect_locks_readonly(path)
}
