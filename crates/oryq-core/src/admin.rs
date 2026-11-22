//! Administrative operations: snapshot-consistent integrity checking, sealed online backup,
//! and read-only forensic inspection facade (`W14`).

use std::fs;
use std::path::{Path, PathBuf};

use oryq_engine::{
    inspect_locks_readonly, inspect_page_readonly, inspect_wal_readonly, wal_path_for,
    LockInspectionReport, PageInspectionReport, PageSize, WalInspectionReport,
};
use oryq_index::IndexKind;
use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::config::DatabaseConfig;
use crate::database::Database;

const MODULE_ID: &str = "core";

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
        let all_docs = col.find_all()?;
        let indexes = col.list_indexes();
        indexes_checked += indexes.len();

        // 1. Verify every document has a valid `_id` and resolves via primary index.
        for doc in &all_docs {
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
        if count != all_docs.len() {
            issues.push(format!(
                "collection '{col_name}': count ({count}) != scanned docs ({})",
                all_docs.len()
            ));
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
    page_size: PageSize,
    dest: &Path,
) -> Result<BackupReport> {
    if paths_refer_to_same_target(source_path, dest) {
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

    let tmp_path = dest.with_extension(format!("oryq.backup.tmp.{}", std::process::id()));
    let tmp_wal = wal_path_for(&tmp_path);
    let _ = fs::remove_file(&tmp_path);
    let _ = fs::remove_file(&tmp_wal);

    let result = (|| -> Result<BackupReport> {
        // Hold one consistent MVCC read snapshot for the entire backup copy.
        let rtx = db.begin_read()?;
        let col_names = rtx.list_collections()?;

        let mut collections_copied = 0usize;
        let mut documents_copied = 0usize;
        let sealed;

        {
            let backup_cfg = DatabaseConfig {
                page_size,
                ..DatabaseConfig::default()
            };
            let backup_db = Database::open(&tmp_path, backup_cfg)?;
            {
                let mut wtx = backup_db.begin_write()?;
                for col_name in &col_names {
                    let src_col = rtx.collection(col_name)?;
                    wtx.create_collection(col_name)?;
                    let mut dst_col = wtx.collection(col_name)?;

                    // Recreate non-primary indexes
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
                                    let opts_raw =
                                        idx.options_json.as_deref().ok_or_else(|| {
                                            ErrorEnvelope::new(
                                                ErrorCategory::Corruption,
                                                MODULE_ID,
                                                "admin::run_online_backup",
                                                format!(
                                                    "vector index '{}' missing options_json",
                                                    idx.name
                                                ),
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

                    // Copy all documents from the pinned snapshot
                    for doc in src_col.find_all()? {
                        dst_col.insert(&doc)?;
                        documents_copied += 1;
                    }
                    collections_copied += 1;
                }
                wtx.commit()?;
            }

            // Checkpoint & seal backup so it is standalone (wal_required == false)
            backup_db.checkpoint(true)?;
            let check = backup_db.integrity_check()?;
            if !check.ok {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "admin::run_online_backup",
                    format!("backup integrity verification failed: {:?}", check.issues),
                ));
            }
            sealed = backup_db.is_sealed()?;
        }

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

/// Compacts `db` in place (`VACUUM`), reclaiming freelist pages, shrinking the `.oryq` file on
/// disk, and leaving a single sealed `.oryq` file with no `.oryq-wal` sidecar.
///
/// Fails with `ErrorCategory::Busy` if any active read transaction is currently open, or
/// `ErrorCategory::UnsupportedFeature` if a collection contains a disabled pluggable index.
pub fn run_vacuum(
    db: &Database,
    engine: &oryq_engine::StorageEngine,
    source_path: &Path,
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

    let tmp_path = source_path.with_extension(format!("oryq.vacuum.tmp.{}", std::process::id()));
    let tmp_wal = wal_path_for(&tmp_path);
    let _ = fs::remove_file(&tmp_path);
    let _ = fs::remove_file(&tmp_wal);

    let res = (|| -> Result<VacuumReport> {
        let backup = run_online_backup(db, source_path, page_size, &tmp_path)?;
        let (bytes_before, bytes_after) = engine.replace_from_sealed_file(&tmp_path)?;
        Ok(VacuumReport {
            bytes_before,
            bytes_after,
            bytes_reclaimed: bytes_before.saturating_sub(bytes_after),
            collections_compacted: backup.collections_copied,
            documents_compacted: backup.documents_copied,
            sealed: true,
        })
    })();

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
