#![forbid(unsafe_code)]

//! `oryq`: Command-line administrative, query, backup, and forensic inspection utility for OryqDB.

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use oryq_core::{
    parse_json_value_str, to_json_string, Database, DatabaseConfig, ErrorCategory, SortOrder, Value,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::BrokenPipe) => ExitCode::SUCCESS,
        Err(CliError::Message(msg)) => {
            let _ = writeln!(io::stderr(), "error: {msg}");
            ExitCode::FAILURE
        }
    }
}

enum CliError {
    BrokenPipe,
    Message(String),
}

impl From<String> for CliError {
    fn from(s: String) -> Self {
        Self::Message(s)
    }
}

fn out_line(line: &str) -> Result<(), CliError> {
    let mut stdout = io::stdout().lock();
    match writeln!(stdout, "{line}") {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Err(CliError::BrokenPipe),
        Err(e) => Err(CliError::Message(format!("stdout write failed: {e}"))),
    }
}

fn run(args: &[String]) -> Result<(), CliError> {
    let Some(cmd) = args.first().map(String::as_str) else {
        print_usage()?;
        return Ok(());
    };

    // Handle `-h` / `--help` at top level or immediately after a subcommand name (args[1]),
    // avoiding accidental interception of literal "--help" values in query/search arguments.
    if matches!(cmd, "-h" | "--help" | "help")
        || matches!(args.get(1).map(String::as_str), Some("-h" | "--help"))
    {
        print_usage()?;
        return Ok(());
    }

    match cmd {
        "-V" | "--version" | "version" => {
            check_exact_args(args, 1, "--version")?;
            out_line(&format!("oryq {}", oryq_core::version()))?;
            Ok(())
        }
        "info" => {
            check_exact_args(args, 2, "info <db>")?;
            let db_path = req_arg(args, 1, "info <db>")?;
            let db = open_db_readonly(db_path)?;
            let sealed = db.is_sealed().map_err(fmt_err)?;
            let tx_id = db.latest_committed_tx_id().map_err(fmt_err)?;
            let rtx = db.begin_read().map_err(fmt_err)?;
            let cols = rtx.list_collections().map_err(fmt_err)?;
            out_line(&format!("database: {db_path}"))?;
            out_line(&format!("version: {}", oryq_core::version()))?;
            out_line(&format!("sealed: {sealed}"))?;
            out_line(&format!("latest_tx_id: {}", tx_id.raw()))?;
            out_line(&format!("collections: {}", cols.len()))?;
            for c in cols {
                let col = rtx.collection(&c).map_err(fmt_err)?;
                let cnt = col.count().map_err(fmt_err)?;
                let idx_cnt = col.list_indexes().len();
                out_line(&format!("  - {c}: {cnt} docs, {idx_cnt} indexes"))?;
            }
            Ok(())
        }
        "collections" => {
            check_exact_args(args, 2, "collections <db>")?;
            let db_path = req_arg(args, 1, "collections <db>")?;
            let db = open_db_readonly(db_path)?;
            let rtx = db.begin_read().map_err(fmt_err)?;
            for c in rtx.list_collections().map_err(fmt_err)? {
                out_line(&c)?;
            }
            Ok(())
        }
        "indexes" => {
            check_exact_args(args, 3, "indexes <db> <collection>")?;
            let db_path = req_arg(args, 1, "indexes <db> <collection>")?;
            let col_name = req_arg(args, 2, "indexes <db> <collection>")?;
            let db = open_db_readonly(db_path)?;
            let rtx = db.begin_read().map_err(fmt_err)?;
            let col = rtx.collection(col_name).map_err(fmt_err)?;
            for desc in col.list_indexes() {
                let fields: Vec<String> = desc
                    .fields
                    .iter()
                    .map(|f| format!("{}:{:?}", f.path.as_str(), f.order))
                    .collect();
                out_line(&format!(
                    "{} kind={:?} unique={} sparse={} fields=[{}]",
                    desc.name,
                    desc.kind,
                    desc.unique,
                    desc.sparse,
                    fields.join(",")
                ))?;
            }
            Ok(())
        }
        "create-index" => {
            let db_path = req_arg(
                args,
                1,
                "create-index <db> <collection> <field> [--desc] [--unique] [--sparse] [--name <name>]",
            )?;
            let col_name = req_arg(args, 2, "create-index <db> <collection> <field>")?;
            let field = req_arg(args, 3, "create-index <db> <collection> <field>")?;
            let mut order = SortOrder::Asc;
            let mut unique = false;
            let mut sparse = false;
            let mut name_opt: Option<&str> = None;
            let mut i = 4;
            while i < args.len() {
                match args[i].as_str() {
                    "--desc" => {
                        order = SortOrder::Desc;
                        i += 1;
                    }
                    "--unique" => {
                        unique = true;
                        i += 1;
                    }
                    "--sparse" => {
                        sparse = true;
                        i += 1;
                    }
                    "--name" => {
                        name_opt = Some(req_arg(args, i + 1, "--name <name>")?);
                        i += 2;
                    }
                    other => return Err(format!("unknown create-index option '{other}'").into()),
                }
            }
            let db = open_db_existing_rw(db_path)?;
            let mut wtx = db.begin_write().map_err(fmt_err)?;
            let desc = {
                let mut col = wtx.collection(col_name).map_err(fmt_err)?;
                col.create_index(field, order, unique, sparse, name_opt)
                    .map_err(fmt_err)?
            };
            wtx.commit().map_err(fmt_err)?;
            out_line(&format!(
                "created index '{}' on '{col_name}.{field}'",
                desc.name
            ))?;
            Ok(())
        }
        "drop-index" => {
            check_exact_args(args, 4, "drop-index <db> <collection> <index-name>")?;
            let db_path = req_arg(args, 1, "drop-index <db> <collection> <index-name>")?;
            let col_name = req_arg(args, 2, "drop-index <db> <collection> <index-name>")?;
            let idx_name = req_arg(args, 3, "drop-index <db> <collection> <index-name>")?;
            let db = open_db_existing_rw(db_path)?;
            let mut wtx = db.begin_write().map_err(fmt_err)?;
            let dropped = {
                let mut col = wtx.collection(col_name).map_err(fmt_err)?;
                col.drop_index(idx_name).map_err(fmt_err)?
            };
            wtx.commit().map_err(fmt_err)?;
            out_line(&format!("dropped index '{idx_name}': {dropped}"))?;
            Ok(())
        }
        "count" => {
            let db_path = req_arg(args, 1, "count <db> <collection> [--filter <json>]")?;
            let col_name = req_arg(args, 2, "count <db> <collection> [--filter <json>]")?;
            let mut filter: Option<&str> = None;
            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--filter" => {
                        filter = Some(req_arg(args, i + 1, "--filter <json>")?);
                        i += 2;
                    }
                    other => return Err(format!("unknown count option '{other}'").into()),
                }
            }
            let db = open_db_readonly(db_path)?;
            let rtx = db.begin_read().map_err(fmt_err)?;
            let col = rtx.collection(col_name).map_err(fmt_err)?;
            let cnt = match filter {
                Some(f) => {
                    let expr = oryq_core::parse_filter_json(f).map_err(fmt_err)?;
                    col.count_matching(Some(&expr)).map_err(fmt_err)?
                }
                None => col.count().map_err(fmt_err)? as u64,
            };
            out_line(&cnt.to_string())?;
            Ok(())
        }
        "explain" => {
            let db_path = req_arg(args, 1, "explain <db> <collection> [--filter <json>]")?;
            let col_name = req_arg(args, 2, "explain <db> <collection> [--filter <json>]")?;
            let mut filter: Option<&str> = None;
            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--filter" => {
                        filter = Some(req_arg(args, i + 1, "--filter <json>")?);
                        i += 2;
                    }
                    other => return Err(format!("unknown explain option '{other}'").into()),
                }
            }
            let db = open_db_readonly(db_path)?;
            let rtx = db.begin_read().map_err(fmt_err)?;
            let col = rtx.collection(col_name).map_err(fmt_err)?;
            let cursor = col
                .find_json(filter, None, None, 0, None)
                .map_err(fmt_err)?;
            out_line(&format!("{:?}", cursor.plan()))?;
            Ok(())
        }
        "create-collection" => {
            check_exact_args(args, 3, "create-collection <db> <collection>")?;
            let db_path = req_arg(args, 1, "create-collection <db> <collection>")?;
            let col_name = req_arg(args, 2, "create-collection <db> <collection>")?;
            let db = open_db_rw_create(db_path)?;
            let mut wtx = db.begin_write().map_err(fmt_err)?;
            wtx.create_collection(col_name).map_err(fmt_err)?;
            wtx.commit().map_err(fmt_err)?;
            out_line(&format!("created collection '{col_name}'"))?;
            Ok(())
        }
        "drop-collection" => {
            check_exact_args(args, 3, "drop-collection <db> <collection>")?;
            let db_path = req_arg(args, 1, "drop-collection <db> <collection>")?;
            let col_name = req_arg(args, 2, "drop-collection <db> <collection>")?;
            let db = open_db_existing_rw(db_path)?;
            let mut wtx = db.begin_write().map_err(fmt_err)?;
            wtx.drop_collection(col_name).map_err(fmt_err)?;
            wtx.commit().map_err(fmt_err)?;
            out_line(&format!("dropped collection '{col_name}'"))?;
            Ok(())
        }
        "insert" => {
            check_exact_args(args, 4, "insert <db> <collection> <json>")?;
            let db_path = req_arg(args, 1, "insert <db> <collection> <json>")?;
            let col_name = req_arg(args, 2, "insert <db> <collection> <json>")?;
            let json = req_arg(args, 3, "insert <db> <collection> <json>")?;
            let db = open_db_rw_create(db_path)?;
            let mut wtx = db.begin_write().map_err(fmt_err)?;
            if !wtx
                .list_collections()
                .map_err(fmt_err)?
                .iter()
                .any(|c| c == col_name)
            {
                wtx.create_collection(col_name).map_err(fmt_err)?;
            }
            let (id, rid) = {
                let mut col = wtx.collection(col_name).map_err(fmt_err)?;
                col.insert_json(json).map_err(fmt_err)?
            };
            wtx.commit().map_err(fmt_err)?;
            out_line(&format!("inserted _id={id:?} record_id={}", rid.raw()))?;
            Ok(())
        }
        "find" => {
            let db_path = req_arg(args, 1, "find <db> <collection> [options]")?;
            let col_name = req_arg(args, 2, "find <db> <collection> [options]")?;
            let mut filter: Option<&str> = None;
            let mut projection: Option<&str> = None;
            let mut sort: Option<&str> = None;
            let mut skip: u64 = 0;
            let mut limit: Option<u64> = None;

            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--filter" => {
                        filter = Some(req_arg(args, i + 1, "--filter <json>")?);
                        i += 2;
                    }
                    "--projection" => {
                        projection = Some(req_arg(args, i + 1, "--projection <json>")?);
                        i += 2;
                    }
                    "--sort" => {
                        sort = Some(req_arg(args, i + 1, "--sort <json>")?);
                        i += 2;
                    }
                    "--skip" => {
                        let s = req_arg(args, i + 1, "--skip <n>")?;
                        skip = s
                            .parse::<u64>()
                            .map_err(|_| format!("invalid --skip '{s}'"))?;
                        i += 2;
                    }
                    "--limit" => {
                        let s = req_arg(args, i + 1, "--limit <n>")?;
                        limit = Some(
                            s.parse::<u64>()
                                .map_err(|_| format!("invalid --limit '{s}'"))?,
                        );
                        i += 2;
                    }
                    other => return Err(format!("unknown find option '{other}'").into()),
                }
            }

            let db = open_db_readonly(db_path)?;
            let rtx = db.begin_read().map_err(fmt_err)?;
            let col = rtx.collection(col_name).map_err(fmt_err)?;
            let cursor = col
                .find_json(filter, projection, sort, skip, limit)
                .map_err(fmt_err)?;
            for doc in cursor.collect_all() {
                out_line(&to_json_string(&doc.view()).map_err(fmt_err)?)?;
            }
            Ok(())
        }
        "update" => {
            check_exact_args(args, 5, "update <db> <collection> <id-json> <update-json>")?;
            let db_path = req_arg(args, 1, "update <db> <collection> <id-json> <update-json>")?;
            let col_name = req_arg(args, 2, "update <db> <collection> <id-json> <update-json>")?;
            let id_json = req_arg(args, 3, "update <db> <collection> <id-json> <update-json>")?;
            let upd_json = req_arg(args, 4, "update <db> <collection> <id-json> <update-json>")?;
            let id_val = parse_id_json(id_json)?;
            let db = open_db_existing_rw(db_path)?;
            let mut wtx = db.begin_write().map_err(fmt_err)?;
            let updated = {
                let mut col = wtx.collection(col_name).map_err(fmt_err)?;
                col.update_by_id(&id_val, upd_json).map_err(fmt_err)?
            };
            wtx.commit().map_err(fmt_err)?;
            out_line(&format!("updated: {updated}"))?;
            Ok(())
        }
        "delete" => {
            check_exact_args(args, 4, "delete <db> <collection> <id-json>")?;
            let db_path = req_arg(args, 1, "delete <db> <collection> <id-json>")?;
            let col_name = req_arg(args, 2, "delete <db> <collection> <id-json>")?;
            let id_json = req_arg(args, 3, "delete <db> <collection> <id-json>")?;
            let id_val = parse_id_json(id_json)?;
            let db = open_db_existing_rw(db_path)?;
            let mut wtx = db.begin_write().map_err(fmt_err)?;
            let deleted = {
                let mut col = wtx.collection(col_name).map_err(fmt_err)?;
                col.delete_by_id(&id_val).map_err(fmt_err)?
            };
            wtx.commit().map_err(fmt_err)?;
            out_line(&format!("deleted: {deleted}"))?;
            Ok(())
        }
        "aggregate" => {
            check_exact_args(args, 4, "aggregate <db> <collection> <pipeline-json>")?;
            let db_path = req_arg(args, 1, "aggregate <db> <collection> <pipeline-json>")?;
            let col_name = req_arg(args, 2, "aggregate <db> <collection> <pipeline-json>")?;
            let pipeline = req_arg(args, 3, "aggregate <db> <collection> <pipeline-json>")?;
            let db = open_db_readonly(db_path)?;
            let rtx = db.begin_read().map_err(fmt_err)?;
            let col = rtx.collection(col_name).map_err(fmt_err)?;
            let rows = col.aggregate_json(pipeline).map_err(fmt_err)?;
            for doc in rows {
                out_line(&to_json_string(&doc.view()).map_err(fmt_err)?)?;
            }
            Ok(())
        }
        "checkpoint" => {
            check_exact_args(args, 2, "checkpoint <db>")?;
            let db_path = req_arg(args, 1, "checkpoint <db>")?;
            let db = open_db_existing_rw(db_path)?;
            let out = db.checkpoint(true).map_err(fmt_err)?;
            out_line(&format!(
                "checkpointed tx_id={} pages_copied={} wal_reset={}",
                out.checkpointed_tx_id.raw(),
                out.pages_copied,
                out.wal_reset
            ))?;
            Ok(())
        }
        "vacuum" => {
            check_exact_args(args, 2, "vacuum <db>")?;
            let db_path = req_arg(args, 1, "vacuum <db>")?;
            let db = open_db_existing_rw(db_path)?;
            let rep = db.vacuum().map_err(fmt_err)?;
            out_line(&format!(
                "vacuum complete: bytes_before={} bytes_after={} bytes_reclaimed={} collections={} documents={} sealed={}",
                rep.bytes_before,
                rep.bytes_after,
                rep.bytes_reclaimed,
                rep.collections_compacted,
                rep.documents_compacted,
                rep.sealed
            ))?;
            Ok(())
        }
        "backup" => {
            check_exact_args(args, 3, "backup <db> <dest>")?;
            let db_path = req_arg(args, 1, "backup <db> <dest>")?;
            let dest = req_arg(args, 2, "backup <db> <dest>")?;
            let db = open_db_readonly(db_path)?;
            let rep = db.backup_to(Path::new(dest)).map_err(fmt_err)?;
            out_line(&format!(
                "backup complete: collections={} documents={} sealed={} dest={}",
                rep.collections_copied,
                rep.documents_copied,
                rep.sealed,
                rep.dest_path.display()
            ))?;
            Ok(())
        }
        "integrity-check" => {
            check_exact_args(args, 2, "integrity-check <db>")?;
            let db_path = req_arg(args, 1, "integrity-check <db>")?;
            let db = open_db_readonly(db_path)?;
            let rep = db.integrity_check().map_err(fmt_err)?;
            out_line(&format!(
                "integrity_check: ok={} collections={} documents={} indexes={}",
                rep.ok, rep.collections_checked, rep.documents_checked, rep.indexes_checked
            ))?;
            if !rep.ok {
                for issue in rep.issues {
                    let _ = writeln!(io::stderr(), "  issue: {issue}");
                }
                return Err("integrity check detected issues".to_owned().into());
            }
            Ok(())
        }
        "inspect" => {
            let db_path = req_arg(args, 1, "inspect <db> <page <id> | wal | locks>")?;
            let target = req_arg(args, 2, "inspect <db> <page <id> | wal | locks>")?;
            match target {
                "page" => {
                    check_exact_args(args, 4, "inspect <db> page <page-id>")?;
                    let pid_str = req_arg(args, 3, "inspect <db> page <page-id>")?;
                    let pid = pid_str
                        .parse::<u64>()
                        .map_err(|_| format!("invalid page_id '{pid_str}'"))?;
                    let rep = Database::inspect_page(db_path, pid).map_err(fmt_err)?;
                    out_line(&format!(
                        "page={} size={} type={} slots={} free_bytes={} checksum_valid={}",
                        rep.page_id,
                        rep.page_size,
                        rep.page_type,
                        rep.slot_count,
                        rep.free_bytes,
                        rep.checksum_valid
                    ))?;
                    if let Some(gen) = rep.header_generation {
                        out_line(&format!(
                            "header_generation={gen} wal_required={}",
                            rep.wal_required.unwrap_or(false)
                        ))?;
                    }
                    Ok(())
                }
                "wal" => {
                    check_exact_args(args, 3, "inspect <db> wal")?;
                    let rep = Database::inspect_wal(db_path).map_err(fmt_err)?;
                    out_line(&format!(
                        "wal={} exists={} bytes={} committed_txns={} valid_bytes={} trailing_bytes={}",
                        rep.wal_path.display(),
                        rep.wal_exists,
                        rep.wal_bytes,
                        rep.committed_txns,
                        rep.valid_bytes,
                        rep.trailing_bytes
                    ))?;
                    Ok(())
                }
                "locks" => {
                    check_exact_args(args, 3, "inspect <db> locks")?;
                    let rep = Database::inspect_locks(db_path).map_err(fmt_err)?;
                    out_line(&format!(
                        "db={} exists={} bytes={} exclusive_lock_available={}",
                        rep.db_path.display(),
                        rep.file_exists,
                        rep.file_bytes,
                        rep.exclusive_lock_available
                    ))?;
                    Ok(())
                }
                other => Err(format!(
                    "unknown inspect target '{other}' (expected: page, wal, locks)"
                )
                .into()),
            }
        }
        "fts-index" => {
            let db_path = req_arg(
                args,
                1,
                "fts-index <db> <collection> <fields-csv> [--name <name>]",
            )?;
            let col_name = req_arg(
                args,
                2,
                "fts-index <db> <collection> <fields-csv> [--name <name>]",
            )?;
            let fields_csv = req_arg(
                args,
                3,
                "fts-index <db> <collection> <fields-csv> [--name <name>]",
            )?;
            let mut name_opt: Option<&str> = None;
            let mut i = 4;
            while i < args.len() {
                match args[i].as_str() {
                    "--name" => {
                        name_opt = Some(req_arg(args, i + 1, "--name <name>")?);
                        i += 2;
                    }
                    other => return Err(format!("unknown fts-index option '{other}'").into()),
                }
            }
            let fields: Vec<&str> = fields_csv
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            if fields.is_empty() {
                return Err("fields-csv must specify at least one field"
                    .to_owned()
                    .into());
            }
            #[cfg(feature = "fts")]
            {
                let db = open_db_existing_rw(db_path)?;
                let mut wtx = db.begin_write().map_err(fmt_err)?;
                let idx_desc = {
                    let mut col = wtx.collection(col_name).map_err(fmt_err)?;
                    col.create_fts_index(&fields, name_opt).map_err(fmt_err)?
                };
                wtx.commit().map_err(fmt_err)?;
                out_line(&format!(
                    "created fts index '{}' on '{col_name}'",
                    idx_desc.name
                ))?;
                Ok(())
            }
            #[cfg(not(feature = "fts"))]
            {
                let _ = (db_path, col_name, fields, name_opt);
                Err(
                    "oryq compiled without 'fts' feature; rebuild with `--features fts`"
                        .to_owned()
                        .into(),
                )
            }
        }
        "fts-search" => {
            let db_path = req_arg(
                args,
                1,
                "fts-search <db> <collection> <index> <query> [--limit <n>]",
            )?;
            let col_name = req_arg(
                args,
                2,
                "fts-search <db> <collection> <index> <query> [--limit <n>]",
            )?;
            let idx_name = req_arg(
                args,
                3,
                "fts-search <db> <collection> <index> <query> [--limit <n>]",
            )?;
            let query = req_arg(
                args,
                4,
                "fts-search <db> <collection> <index> <query> [--limit <n>]",
            )?;
            let mut limit: usize = 10;
            let mut i = 5;
            while i < args.len() {
                match args[i].as_str() {
                    "--limit" => {
                        let s = req_arg(args, i + 1, "--limit <n>")?;
                        limit = s
                            .parse::<usize>()
                            .map_err(|_| format!("invalid --limit '{s}'"))?;
                        i += 2;
                    }
                    other => return Err(format!("unknown fts-search option '{other}'").into()),
                }
            }
            #[cfg(feature = "fts")]
            {
                let db = open_db_readonly(db_path)?;
                let rtx = db.begin_read().map_err(fmt_err)?;
                let col = rtx.collection(col_name).map_err(fmt_err)?;
                let hits = col.search_fts(idx_name, query, limit).map_err(fmt_err)?;
                for (doc, score) in hits {
                    out_line(&format!(
                        "score={score:.6} {}",
                        to_json_string(&doc.view()).map_err(fmt_err)?
                    ))?;
                }
                Ok(())
            }
            #[cfg(not(feature = "fts"))]
            {
                let _ = (db_path, col_name, idx_name, query, limit);
                Err(
                    "oryq compiled without 'fts' feature; rebuild with `--features fts`"
                        .to_owned()
                        .into(),
                )
            }
        }
        "vec-index" => {
            let db_path = req_arg(args, 1, "vec-index <db> <collection> <field> <dimensions> [--metric cosine|l2|dot] [--quant f32|int8] [--name <name>]")?;
            let col_name = req_arg(args, 2, "vec-index <db> <collection> <field> <dimensions>")?;
            let field = req_arg(args, 3, "vec-index <db> <collection> <field> <dimensions>")?;
            let dims_str = req_arg(args, 4, "vec-index <db> <collection> <field> <dimensions>")?;
            let dimensions = dims_str
                .parse::<u32>()
                .map_err(|_| format!("invalid dimensions '{dims_str}'"))?;
            if dimensions == 0 {
                return Err("dimensions must be > 0".to_owned().into());
            }
            let mut metric_str = "cosine";
            let mut quant_str = "f32";
            let mut name_opt: Option<&str> = None;
            let mut i = 5;
            while i < args.len() {
                match args[i].as_str() {
                    "--metric" => {
                        metric_str = req_arg(args, i + 1, "--metric cosine|l2|dot")?;
                        i += 2;
                    }
                    "--quant" => {
                        quant_str = req_arg(args, i + 1, "--quant f32|int8")?;
                        i += 2;
                    }
                    "--name" => {
                        name_opt = Some(req_arg(args, i + 1, "--name <name>")?);
                        i += 2;
                    }
                    other => return Err(format!("unknown vec-index option '{other}'").into()),
                }
            }
            if !matches!(metric_str, "cosine" | "l2" | "dot") {
                return Err(
                    format!("unknown --metric '{metric_str}' (expected: cosine, l2, dot)").into(),
                );
            }
            if !matches!(quant_str, "f32" | "int8") {
                return Err(format!("unknown --quant '{quant_str}' (expected: f32, int8)").into());
            }
            #[cfg(feature = "vec")]
            {
                let metric = match metric_str {
                    "cosine" => oryq_core::DistanceMetric::Cosine,
                    "l2" => oryq_core::DistanceMetric::L2,
                    "dot" => oryq_core::DistanceMetric::DotProduct,
                    other => {
                        return Err(format!(
                            "unknown --metric '{other}' (expected: cosine, l2, dot)"
                        )
                        .into())
                    }
                };
                let quantization = match quant_str {
                    "f32" => oryq_core::VectorQuantization::F32,
                    "int8" => oryq_core::VectorQuantization::Int8,
                    other => {
                        return Err(
                            format!("unknown --quant '{other}' (expected: f32, int8)").into()
                        )
                    }
                };
                let dims_usize = usize::try_from(dimensions)
                    .map_err(|_| format!("dimensions {dimensions} exceeds platform usize"))?;
                let opts = oryq_core::VectorIndexOptions {
                    dimensions: dims_usize,
                    metric,
                    quantization,
                };
                let db = open_db_existing_rw(db_path)?;
                let mut wtx = db.begin_write().map_err(fmt_err)?;
                let idx_desc = {
                    let mut col = wtx.collection(col_name).map_err(fmt_err)?;
                    col.create_vector_index(field, opts, name_opt)
                        .map_err(fmt_err)?
                };
                wtx.commit().map_err(fmt_err)?;
                out_line(&format!(
                    "created vector index '{}' on '{col_name}.{field}'",
                    idx_desc.name
                ))?;
                Ok(())
            }
            #[cfg(not(feature = "vec"))]
            {
                let _ = (
                    db_path, col_name, field, dimensions, metric_str, quant_str, name_opt,
                );
                Err(
                    "oryq compiled without 'vec' feature; rebuild with `--features vec`"
                        .to_owned()
                        .into(),
                )
            }
        }
        "vec-search" => {
            let db_path = req_arg(
                args,
                1,
                "vec-search <db> <collection> <index> <vec-json> [--top-k <n>]",
            )?;
            let col_name = req_arg(
                args,
                2,
                "vec-search <db> <collection> <index> <vec-json> [--top-k <n>]",
            )?;
            let idx_name = req_arg(
                args,
                3,
                "vec-search <db> <collection> <index> <vec-json> [--top-k <n>]",
            )?;
            let vec_json = req_arg(
                args,
                4,
                "vec-search <db> <collection> <index> <vec-json> [--top-k <n>]",
            )?;
            let mut top_k: usize = 10;
            let mut i = 5;
            while i < args.len() {
                match args[i].as_str() {
                    "--top-k" => {
                        let s = req_arg(args, i + 1, "--top-k <n>")?;
                        top_k = s
                            .parse::<usize>()
                            .map_err(|_| format!("invalid --top-k '{s}'"))?;
                        i += 2;
                    }
                    other => return Err(format!("unknown vec-search option '{other}'").into()),
                }
            }
            #[cfg(feature = "vec")]
            {
                let query = parse_vec_json(vec_json)?;
                let db = open_db_readonly(db_path)?;
                let rtx = db.begin_read().map_err(fmt_err)?;
                let col = rtx.collection(col_name).map_err(fmt_err)?;
                let hits = col
                    .search_vector(idx_name, &query, top_k)
                    .map_err(fmt_err)?;
                for (doc, dist) in hits {
                    out_line(&format!(
                        "distance={dist:.6} {}",
                        to_json_string(&doc.view()).map_err(fmt_err)?
                    ))?;
                }
                Ok(())
            }
            #[cfg(not(feature = "vec"))]
            {
                let _ = (db_path, col_name, idx_name, vec_json, top_k);
                Err(
                    "oryq compiled without 'vec' feature; rebuild with `--features vec`"
                        .to_owned()
                        .into(),
                )
            }
        }
        other => Err(format!("unknown command '{other}' (run `oryq --help`)").into()),
    }
}

fn open_db_readonly(path: &str) -> Result<Database, String> {
    let cfg = DatabaseConfig {
        read_only: true,
        create_if_missing: false,
        auto_checkpoint_on_close: false,
        ..DatabaseConfig::default()
    };
    open_db_with_config(path, cfg)
}

fn open_db_existing_rw(path: &str) -> Result<Database, String> {
    let cfg = DatabaseConfig {
        create_if_missing: false,
        ..DatabaseConfig::default()
    };
    open_db_with_config(path, cfg)
}

fn open_db_rw_create(path: &str) -> Result<Database, String> {
    open_db_with_config(path, DatabaseConfig::default())
}

fn open_db_with_config(path: &str, cfg: DatabaseConfig) -> Result<Database, String> {
    Database::open(Path::new(path), cfg).map_err(|err| {
        if err.category() == ErrorCategory::LockedByProcess {
            format!(
                "database '{path}' is exclusively locked by another process (use `oryq inspect {path} locks` for read-only diagnostics): {err}"
            )
        } else {
            err.to_string()
        }
    })
}

fn parse_id_json(id_json: &str) -> Result<Value, String> {
    parse_json_value_str(id_json).map_err(fmt_err)
}

#[cfg(feature = "vec")]
fn parse_vec_json(vec_json: &str) -> Result<Vec<f32>, String> {
    let val = parse_json_value_str(vec_json).map_err(fmt_err)?;
    let Value::Array(items) = val else {
        return Err("vector query must be a JSON array of numbers".to_owned());
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Value::Float64(f) => out.push(f as f32),
            Value::Int64(n) => out.push(n as f32),
            other => return Err(format!("vector elements must be numeric, found {other:?}")),
        }
    }
    if out.is_empty() {
        return Err("vector query must not be empty".to_owned());
    }
    Ok(out)
}

fn req_arg<'a>(args: &'a [String], idx: usize, usage: &str) -> Result<&'a str, String> {
    args.get(idx)
        .map(String::as_str)
        .ok_or_else(|| format!("missing argument; usage: oryq {usage}"))
}

fn check_exact_args(args: &[String], expected: usize, usage: &str) -> Result<(), String> {
    if args.len() > expected {
        return Err(format!(
            "unexpected argument '{}'; usage: oryq {usage}",
            args[expected]
        ));
    }
    Ok(())
}

fn fmt_err(err: oryq_core::ErrorEnvelope) -> String {
    err.to_string()
}

fn print_usage() -> Result<(), CliError> {
    out_line(&format!(
        "oryq {} — OryqDB command-line utility\n\n\
         USAGE:\n  \
           oryq <command> [args...]\n\n\
         COMMANDS:\n  \
           info <db>\n  \
           collections <db>\n  \
           indexes <db> <collection>\n  \
           create-collection <db> <collection>\n  \
           drop-collection <db> <collection>\n  \
           create-index <db> <collection> <field> [--desc] [--unique] [--sparse] [--name <name>]\n  \
           drop-index <db> <collection> <index-name>\n  \
           insert <db> <collection> <json>\n  \
           find <db> <collection> [--filter <json>] [--projection <json>] [--sort <json>] [--skip <n>] [--limit <n>]\n  \
           count <db> <collection> [--filter <json>]\n  \
           explain <db> <collection> [--filter <json>]\n  \
           update <db> <collection> <id-json> <update-json>\n  \
           delete <db> <collection> <id-json>\n  \
           aggregate <db> <collection> <pipeline-json>\n  \
           fts-index <db> <collection> <fields-csv> [--name <name>]\n  \
           fts-search <db> <collection> <index> <query> [--limit <n>]\n  \
           vec-index <db> <collection> <field> <dimensions> [--metric cosine|l2|dot] [--quant f32|int8] [--name <name>]\n  \
           vec-search <db> <collection> <index> <vec-json> [--top-k <n>]\n  \
           checkpoint <db>\n  \
           vacuum <db>\n  \
           backup <db> <dest>\n  \
           integrity-check <db>\n  \
           inspect <db> <page <id> | wal | locks>\n  \
           --version | --help",
        oryq_core::version()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn cli_help_trailing_args_readonly_and_index_count_explain() {
        let dir = std::env::temp_dir().join(format!(
            "oryq_cli_hardening_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("does_not_exist.oryq");
        let missing_s = missing.to_string_lossy().to_string();

        // Global --help must succeed and never create `./--help` or touch a DB.
        assert!(run(&s(&["drop-collection", "--help"])).is_ok());
        assert!(!Path::new("--help").exists());

        // Read-only `info` on missing path must fail and NOT create an empty DB file.
        assert!(run(&s(&["info", &missing_s])).is_err());
        assert!(!missing.exists());

        // Unexpected trailing arg on fixed-arity command must fail.
        assert!(run(&s(&["info", &missing_s, "--bogus"])).is_err());

        // End-to-end create-index, indexes, count, explain, drop-index.
        let db_path = dir.join("cli.oryq");
        let db_s = db_path.to_string_lossy().to_string();
        assert!(run(&s(&["insert", &db_s, "users", r#"{"_id": 1, "age": 30}"#])).is_ok());
        assert!(run(&s(&[
            "create-index",
            &db_s,
            "users",
            "age",
            "--name",
            "idx_age"
        ]))
        .is_ok());
        assert!(run(&s(&["indexes", &db_s, "users"])).is_ok());
        assert!(run(&s(&["count", &db_s, "users", "--filter", r#"{"age": 30}"#])).is_ok());
        assert!(run(&s(&[
            "explain",
            &db_s,
            "users",
            "--filter",
            r#"{"age": 30}"#
        ]))
        .is_ok());
        assert!(run(&s(&["drop-index", &db_s, "users", "idx_age"])).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
