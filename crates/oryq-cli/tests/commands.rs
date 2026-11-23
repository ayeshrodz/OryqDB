use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn oryq_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oryq"))
}

#[test]
fn cli_end_to_end_crud_aggregate_backup_integrity_and_inspect() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("oryqdb_cli_e2e_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("cli.oryq");
    let backup = dir.join("cli_backup.oryq");
    let db_s = db.to_str().unwrap();
    let bk_s = backup.to_str().unwrap();

    // --version
    let out = oryq_bin().arg("--version").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("oryq "));

    // insert (auto-creates collection)
    let out = oryq_bin()
        .args([
            "insert",
            db_s,
            "users",
            r#"{"_id": 1, "name": "Alice", "age": 30}"#,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = oryq_bin()
        .args([
            "insert",
            db_s,
            "users",
            r#"{"_id": 2, "name": "Bob", "age": 20}"#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success());

    // update
    let out = oryq_bin()
        .args(["update", db_s, "users", "1", r#"{"$inc": {"age": 5}}"#])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("updated: true"));

    // find with filter & sort
    let out = oryq_bin()
        .args([
            "find",
            db_s,
            "users",
            "--filter",
            r#"{"age": {"$gte": 30}}"#,
            "--sort",
            r#"{"age": -1}"#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Alice") && stdout.contains("35"));

    // aggregate
    let out = oryq_bin()
        .args([
            "aggregate",
            db_s,
            "users",
            r#"[{"$group": {"_id": null, "total_age": {"$sum": "$age"}}}]"#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("55"));

    // integrity-check
    let out = oryq_bin().args(["integrity-check", db_s]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("ok=true"));

    // vacuum
    let out = oryq_bin().args(["vacuum", db_s]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("sealed=true"));

    // backup
    let out = oryq_bin().args(["backup", db_s, bk_s]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("sealed=true"));

    // inspect page 0, wal, locks
    let out = oryq_bin()
        .args(["inspect", db_s, "page", "0"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("StorageHeader"));

    let out = oryq_bin()
        .args(["inspect", db_s, "locks"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("exclusive_lock_available=true"));

    // info
    let out = oryq_bin().args(["info", bk_s]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("sealed: true"));

    // delete
    let out = oryq_bin()
        .args(["delete", db_s, "users", "2"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("deleted: true"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cli_hybrid_fts_and_vector_commands_or_feature_gating() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("oryqdb_cli_hybrid_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("hybrid.oryq");
    let db_s = db.to_str().unwrap();

    let out = oryq_bin()
        .args([
            "insert",
            db_s,
            "docs",
            r#"{"_id": 1, "title": "Rust Engine", "emb": [1.0, 0.0]}"#,
        ])
        .output()
        .unwrap();
    assert!(out.status.success());

    let fts_idx = oryq_bin()
        .args(["fts-index", db_s, "docs", "title", "--name", "t_idx"])
        .output()
        .unwrap();
    #[cfg(feature = "fts")]
    {
        assert!(fts_idx.status.success());
        let fts_q = oryq_bin()
            .args(["fts-search", db_s, "docs", "t_idx", "rust", "--limit", "5"])
            .output()
            .unwrap();
        assert!(fts_q.status.success());
        let s = String::from_utf8_lossy(&fts_q.stdout);
        assert!(s.contains("score=") && s.contains("Rust Engine"));
    }
    #[cfg(not(feature = "fts"))]
    {
        assert!(!fts_idx.status.success());
        assert!(String::from_utf8_lossy(&fts_idx.stderr).contains("fts"));
    }

    let vec_idx = oryq_bin()
        .args([
            "vec-index",
            db_s,
            "docs",
            "emb",
            "2",
            "--metric",
            "cosine",
            "--quant",
            "f32",
            "--name",
            "e_idx",
        ])
        .output()
        .unwrap();
    #[cfg(feature = "vec")]
    {
        assert!(vec_idx.status.success());
        let vec_q = oryq_bin()
            .args([
                "vec-search",
                db_s,
                "docs",
                "e_idx",
                "[0.9, 0.1]",
                "--top-k",
                "3",
            ])
            .output()
            .unwrap();
        assert!(vec_q.status.success());
        let s = String::from_utf8_lossy(&vec_q.stdout);
        assert!(s.contains("distance=") && s.contains("Rust Engine"));
    }
    #[cfg(not(feature = "vec"))]
    {
        assert!(!vec_idx.status.success());
        assert!(String::from_utf8_lossy(&vec_idx.stderr).contains("vec"));
    }

    let _ = std::fs::remove_dir_all(&dir);
}
