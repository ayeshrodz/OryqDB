#![forbid(unsafe_code)]

//! `oryq`: Command-line administrative, query, backup, and forensic inspection utility for OryqDB.

use std::path::Path;
use std::process::ExitCode;

use oryq_core::{from_json_str, to_json_string, Database, DatabaseConfig, ErrorCategory, Value};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(cmd) = args.first().map(String::as_str) else {
        print_usage();
        return Ok(());
    };

    match cmd {
        "-V" | "--version" | "version" => {
            println!("oryq {}", oryq_core::version());
            Ok(())
        }
        "-h" | "--help" | "help" => {
            print_usage();
            Ok(())
        }
        "info" => {
            let db_path = req_arg(args, 1, "info <db>")?;
            let db = open_db(db_path)?;
            let sealed = db.is_sealed().map_err(fmt_err)?;
            let tx_id = db.latest_committed_tx_id().map_err(fmt_err)?;
            let rtx = db.begin_read().map_err(fmt_err)?;
            let cols = rtx.list_collections().map_err(fmt_err)?;
            println!("database: {db_path}");
            println!("version: {}", oryq_core::version());
            println!("sealed: {sealed}");
            println!("latest_tx_id: {}", tx_id.raw());
            println!("collections: {}", cols.len());
            for c in cols {
                let col = rtx.collection(&c).map_err(fmt_err)?;
                let cnt = col.count().map_err(fmt_err)?;
                let idx_cnt = col.list_indexes().len();
                println!("  - {c}: {cnt} docs, {idx_cnt} indexes");
            }
            Ok(())
        }
        "collections" => {
            let db_path = req_arg(args, 1, "collections <db>")?;
            let db = open_db(db_path)?;
            let rtx = db.begin_read().map_err(fmt_err)?;
            for c in rtx.list_collections().map_err(fmt_err)? {
                println!("{c}");
            }
            Ok(())
        }
        "create-collection" => {
            let db_path = req_arg(args, 1, "create-collection <db> <collection>")?;
            let col_name = req_arg(args, 2, "create-collection <db> <collection>")?;
            let db = open_db(db_path)?;
            let mut wtx = db.begin_write().map_err(fmt_err)?;
            wtx.create_collection(col_name).map_err(fmt_err)?;
            wtx.commit().map_err(fmt_err)?;
            println!("created collection '{col_name}'");
            Ok(())
        }
        "drop-collection" => {
            let db_path = req_arg(args, 1, "drop-collection <db> <collection>")?;
            let col_name = req_arg(args, 2, "drop-collection <db> <collection>")?;
            let db = open_db(db_path)?;
            let mut wtx = db.begin_write().map_err(fmt_err)?;
            wtx.drop_collection(col_name).map_err(fmt_err)?;
            wtx.commit().map_err(fmt_err)?;
            println!("dropped collection '{col_name}'");
            Ok(())
        }
        "insert" => {
            let db_path = req_arg(args, 1, "insert <db> <collection> <json>")?;
            let col_name = req_arg(args, 2, "insert <db> <collection> <json>")?;
            let json = req_arg(args, 3, "insert <db> <collection> <json>")?;
            let db = open_db(db_path)?;
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
            println!("inserted _id={id:?} record_id={}", rid.raw());
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
                    other => return Err(format!("unknown find option '{other}'")),
                }
            }

            let db = open_db(db_path)?;
            let rtx = db.begin_read().map_err(fmt_err)?;
            let col = rtx.collection(col_name).map_err(fmt_err)?;
            let cursor = col
                .find_json(filter, projection, sort, skip, limit)
                .map_err(fmt_err)?;
            for doc in cursor.collect_all() {
                println!("{}", to_json_string(&doc.view()).map_err(fmt_err)?);
            }
            Ok(())
        }
        "update" => {
            let db_path = req_arg(args, 1, "update <db> <collection> <id-json> <update-json>")?;
            let col_name = req_arg(args, 2, "update <db> <collection> <id-json> <update-json>")?;
            let id_json = req_arg(args, 3, "update <db> <collection> <id-json> <update-json>")?;
            let upd_json = req_arg(args, 4, "update <db> <collection> <id-json> <update-json>")?;
            let id_val = parse_id_json(id_json)?;
            let db = open_db(db_path)?;
            let mut wtx = db.begin_write().map_err(fmt_err)?;
            let updated = {
                let mut col = wtx.collection(col_name).map_err(fmt_err)?;
                col.update_by_id(&id_val, upd_json).map_err(fmt_err)?
            };
            wtx.commit().map_err(fmt_err)?;
            println!("updated: {updated}");
            Ok(())
        }
        "delete" => {
            let db_path = req_arg(args, 1, "delete <db> <collection> <id-json>")?;
            let col_name = req_arg(args, 2, "delete <db> <collection> <id-json>")?;
            let id_json = req_arg(args, 3, "delete <db> <collection> <id-json>")?;
            let id_val = parse_id_json(id_json)?;
            let db = open_db(db_path)?;
            let mut wtx = db.begin_write().map_err(fmt_err)?;
            let deleted = {
                let mut col = wtx.collection(col_name).map_err(fmt_err)?;
                col.delete_by_id(&id_val).map_err(fmt_err)?
            };
            wtx.commit().map_err(fmt_err)?;
            println!("deleted: {deleted}");
            Ok(())
        }
        "aggregate" => {
            let db_path = req_arg(args, 1, "aggregate <db> <collection> <pipeline-json>")?;
            let col_name = req_arg(args, 2, "aggregate <db> <collection> <pipeline-json>")?;
            let pipeline = req_arg(args, 3, "aggregate <db> <collection> <pipeline-json>")?;
            let db = open_db(db_path)?;
            let rtx = db.begin_read().map_err(fmt_err)?;
            let col = rtx.collection(col_name).map_err(fmt_err)?;
            let rows = col.aggregate_json(pipeline).map_err(fmt_err)?;
            for doc in rows {
                println!("{}", to_json_string(&doc.view()).map_err(fmt_err)?);
            }
            Ok(())
        }
        "checkpoint" => {
            let db_path = req_arg(args, 1, "checkpoint <db>")?;
            let db = open_db(db_path)?;
            let out = db.checkpoint(true).map_err(fmt_err)?;
            println!(
                "checkpointed tx_id={} pages_copied={} wal_reset={}",
                out.checkpointed_tx_id.raw(),
                out.pages_copied,
                out.wal_reset
            );
            Ok(())
        }
        "vacuum" => {
            let db_path = req_arg(args, 1, "vacuum <db>")?;
            let db = open_db(db_path)?;
            let rep = db.vacuum().map_err(fmt_err)?;
            println!(
                "vacuum complete: bytes_before={} bytes_after={} bytes_reclaimed={} collections={} documents={} sealed={}",
                rep.bytes_before,
                rep.bytes_after,
                rep.bytes_reclaimed,
                rep.collections_compacted,
                rep.documents_compacted,
                rep.sealed
            );
            Ok(())
        }
        "backup" => {
            let db_path = req_arg(args, 1, "backup <db> <dest>")?;
            let dest = req_arg(args, 2, "backup <db> <dest>")?;
            let db = open_db(db_path)?;
            let rep = db.backup_to(Path::new(dest)).map_err(fmt_err)?;
            println!(
                "backup complete: collections={} documents={} sealed={} dest={}",
                rep.collections_copied,
                rep.documents_copied,
                rep.sealed,
                rep.dest_path.display()
            );
            Ok(())
        }
        "integrity-check" => {
            let db_path = req_arg(args, 1, "integrity-check <db>")?;
            let db = open_db(db_path)?;
            let rep = db.integrity_check().map_err(fmt_err)?;
            println!(
                "integrity_check: ok={} collections={} documents={} indexes={}",
                rep.ok, rep.collections_checked, rep.documents_checked, rep.indexes_checked
            );
            if !rep.ok {
                for issue in rep.issues {
                    eprintln!("  issue: {issue}");
                }
                return Err("integrity check detected issues".to_owned());
            }
            Ok(())
        }
        "inspect" => {
            let db_path = req_arg(args, 1, "inspect <db> <page <id> | wal | locks>")?;
            let target = req_arg(args, 2, "inspect <db> <page <id> | wal | locks>")?;
            match target {
                "page" => {
                    let pid_str = req_arg(args, 3, "inspect <db> page <page-id>")?;
                    let pid = pid_str
                        .parse::<u64>()
                        .map_err(|_| format!("invalid page_id '{pid_str}'"))?;
                    let rep = Database::inspect_page(db_path, pid).map_err(fmt_err)?;
                    println!(
                        "page={} size={} type={} slots={} free_bytes={} checksum_valid={}",
                        rep.page_id,
                        rep.page_size,
                        rep.page_type,
                        rep.slot_count,
                        rep.free_bytes,
                        rep.checksum_valid
                    );
                    if let Some(gen) = rep.header_generation {
                        println!(
                            "header_generation={gen} wal_required={}",
                            rep.wal_required.unwrap_or(false)
                        );
                    }
                    Ok(())
                }
                "wal" => {
                    let rep = Database::inspect_wal(db_path).map_err(fmt_err)?;
                    println!(
                        "wal={} exists={} bytes={} committed_txns={} valid_bytes={} trailing_bytes={}",
                        rep.wal_path.display(),
                        rep.wal_exists,
                        rep.wal_bytes,
                        rep.committed_txns,
                        rep.valid_bytes,
                        rep.trailing_bytes
                    );
                    Ok(())
                }
                "locks" => {
                    let rep = Database::inspect_locks(db_path).map_err(fmt_err)?;
                    println!(
                        "db={} exists={} bytes={} exclusive_lock_available={}",
                        rep.db_path.display(),
                        rep.file_exists,
                        rep.file_bytes,
                        rep.exclusive_lock_available
                    );
                    Ok(())
                }
                other => Err(format!(
                    "unknown inspect target '{other}' (expected: page, wal, locks)"
                )),
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
                    other => return Err(format!("unknown fts-index option '{other}'")),
                }
            }
            let fields: Vec<&str> = fields_csv
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            if fields.is_empty() {
                return Err("fields-csv must specify at least one field".to_owned());
            }
            #[cfg(feature = "fts")]
            {
                let db = open_db(db_path)?;
                let mut wtx = db.begin_write().map_err(fmt_err)?;
                let idx_desc = {
                    let mut col = wtx.collection(col_name).map_err(fmt_err)?;
                    col.create_fts_index(&fields, name_opt).map_err(fmt_err)?
                };
                wtx.commit().map_err(fmt_err)?;
                println!("created fts index '{}' on '{col_name}'", idx_desc.name);
                Ok(())
            }
            #[cfg(not(feature = "fts"))]
            {
                let _ = (db_path, col_name, fields, name_opt);
                Err("oryq compiled without 'fts' feature; rebuild with `--features fts`".to_owned())
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
                    other => return Err(format!("unknown fts-search option '{other}'")),
                }
            }
            #[cfg(feature = "fts")]
            {
                let db = open_db(db_path)?;
                let rtx = db.begin_read().map_err(fmt_err)?;
                let col = rtx.collection(col_name).map_err(fmt_err)?;
                let hits = col.search_fts(idx_name, query, limit).map_err(fmt_err)?;
                for (doc, score) in hits {
                    println!(
                        "score={score:.6} {}",
                        to_json_string(&doc.view()).map_err(fmt_err)?
                    );
                }
                Ok(())
            }
            #[cfg(not(feature = "fts"))]
            {
                let _ = (db_path, col_name, idx_name, query, limit);
                Err("oryq compiled without 'fts' feature; rebuild with `--features fts`".to_owned())
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
                return Err("dimensions must be > 0".to_owned());
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
                    other => return Err(format!("unknown vec-index option '{other}'")),
                }
            }
            if !matches!(metric_str, "cosine" | "l2" | "dot") {
                return Err(format!(
                    "unknown --metric '{metric_str}' (expected: cosine, l2, dot)"
                ));
            }
            if !matches!(quant_str, "f32" | "int8") {
                return Err(format!(
                    "unknown --quant '{quant_str}' (expected: f32, int8)"
                ));
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
                        ))
                    }
                };
                let quantization = match quant_str {
                    "f32" => oryq_core::VectorQuantization::F32,
                    "int8" => oryq_core::VectorQuantization::Int8,
                    other => {
                        return Err(format!("unknown --quant '{other}' (expected: f32, int8)"))
                    }
                };
                let dims_usize = usize::try_from(dimensions)
                    .map_err(|_| format!("dimensions {dimensions} exceeds platform usize"))?;
                let opts = oryq_core::VectorIndexOptions {
                    dimensions: dims_usize,
                    metric,
                    quantization,
                };
                let db = open_db(db_path)?;
                let mut wtx = db.begin_write().map_err(fmt_err)?;
                let idx_desc = {
                    let mut col = wtx.collection(col_name).map_err(fmt_err)?;
                    col.create_vector_index(field, opts, name_opt)
                        .map_err(fmt_err)?
                };
                wtx.commit().map_err(fmt_err)?;
                println!(
                    "created vector index '{}' on '{col_name}.{field}'",
                    idx_desc.name
                );
                Ok(())
            }
            #[cfg(not(feature = "vec"))]
            {
                let _ = (
                    db_path, col_name, field, dimensions, metric_str, quant_str, name_opt,
                );
                Err("oryq compiled without 'vec' feature; rebuild with `--features vec`".to_owned())
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
                    other => return Err(format!("unknown vec-search option '{other}'")),
                }
            }
            #[cfg(feature = "vec")]
            {
                let query = parse_vec_json(vec_json)?;
                let db = open_db(db_path)?;
                let rtx = db.begin_read().map_err(fmt_err)?;
                let col = rtx.collection(col_name).map_err(fmt_err)?;
                let hits = col
                    .search_vector(idx_name, &query, top_k)
                    .map_err(fmt_err)?;
                for (doc, dist) in hits {
                    println!(
                        "distance={dist:.6} {}",
                        to_json_string(&doc.view()).map_err(fmt_err)?
                    );
                }
                Ok(())
            }
            #[cfg(not(feature = "vec"))]
            {
                let _ = (db_path, col_name, idx_name, vec_json, top_k);
                Err("oryq compiled without 'vec' feature; rebuild with `--features vec`".to_owned())
            }
        }
        other => Err(format!("unknown command '{other}' (run `oryq --help`)")),
    }
}

fn open_db(path: &str) -> Result<Database, String> {
    Database::open(Path::new(path), DatabaseConfig::default()).map_err(|err| {
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
    let wrapped = format!(r#"{{"_id":{id_json}}}"#);
    let (doc, _) = from_json_str(&wrapped, None).map_err(fmt_err)?;
    doc.view()
        .get_field("_id")
        .map(|v| v.to_owned_value())
        .ok_or_else(|| "invalid id JSON literal".to_owned())
}

#[cfg(feature = "vec")]
fn parse_vec_json(vec_json: &str) -> Result<Vec<f32>, String> {
    let wrapped = format!(r#"{{"v":{vec_json}}}"#);
    let (doc, _) = from_json_str(&wrapped, None).map_err(fmt_err)?;
    let Some(val) = doc.view().get_field("v").map(|v| v.to_owned_value()) else {
        return Err("invalid vector JSON array".to_owned());
    };
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

fn fmt_err(err: oryq_core::ErrorEnvelope) -> String {
    err.to_string()
}

fn print_usage() {
    println!(
        "oryq {} — OryqDB command-line utility\n\n\
         USAGE:\n  \
           oryq <command> [args...]\n\n\
         COMMANDS:\n  \
           info <db>\n  \
           collections <db>\n  \
           create-collection <db> <collection>\n  \
           drop-collection <db> <collection>\n  \
           insert <db> <collection> <json>\n  \
           find <db> <collection> [--filter <json>] [--projection <json>] [--sort <json>] [--skip <n>] [--limit <n>]\n  \
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
    );
}
