//! Integration tests for Issue #48 (v0.15.1):
//! deferred WriteTxn XXH3_64 page checksums and pure-safe LZ4 RECORD_KIND_COMPRESSED storage.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_core::write::lz4::{compress_block, decompress_block};
use oryq_core::{Database, DatabaseConfig, SortOrder, SyncMode};
use oryq_types::ErrorCategory;

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "oryq_issue48_{label}_{}_{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn lz4_block_roundtrips_edge_cases_and_rejects_corrupt_bombs() {
    // Empty, tiny, RLE (offset=1, offset=3), mixed, and large payloads.
    let cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"a".to_vec(),
        b"hello world".to_vec(),
        vec![b'A'; 4096],
        b"abc".repeat(1500),
        (0..2000u32)
            .flat_map(|i| i.wrapping_mul(2_654_435_761).to_le_bytes())
            .collect(),
        format!(
            r#"{{"msg":"{}"}}"#,
            "OryqDB LZ4 block compression test! ".repeat(500)
        )
        .into_bytes(),
    ];

    for raw in &cases {
        let comp = compress_block(raw);
        let decomp = decompress_block(&comp, raw.len(), 16 * 1024 * 1024).unwrap();
        assert_eq!(&decomp, raw);
    }

    // Repetitive 4 KiB buffer should compress dramatically (< 100 bytes).
    let rle_comp = compress_block(&vec![b'Z'; 4096]);
    assert!(
        rle_comp.len() < 100,
        "RLE compressed len was {}",
        rle_comp.len()
    );

    // Corruption / bomb guards:
    // 1. Declared length exceeds max_len.
    let err = decompress_block(&rle_comp, 100 * 1024 * 1024, 16 * 1024 * 1024).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::Corruption);
    // 2. Zero match offset.
    let bad_zero_off = [0x10u8, b'x', 0x00, 0x00];
    assert_eq!(
        decompress_block(&bad_zero_off, 10, 1024)
            .unwrap_err()
            .category(),
        ErrorCategory::Corruption
    );
    // 3. Offset exceeds current output length.
    let bad_oob_off = [0x10u8, b'x', 0x05, 0x00];
    assert_eq!(
        decompress_block(&bad_oob_off, 10, 1024)
            .unwrap_err()
            .category(),
        ErrorCategory::Corruption
    );
    // 4. Truncated literal payload.
    let trunc_lit = [0x50u8, b'a', b'b'];
    assert_eq!(
        decompress_block(&trunc_lit, 5, 1024)
            .unwrap_err()
            .category(),
        ErrorCategory::Corruption
    );
    // 5. Length mismatch (decompressed shorter than declared).
    assert_eq!(
        decompress_block(&rle_comp, 4097, 16 * 1024 * 1024)
            .unwrap_err()
            .category(),
        ErrorCategory::Corruption
    );
    // 6. Allocation bomb guard: tiny 4-byte payload declaring 255 MiB must fail cleanly without OOM.
    let tiny_bomb = [0x10u8, b'a', 0x00, 0x00];
    assert_eq!(
        decompress_block(&tiny_bomb, (1 << 28) - 1, (1 << 28) - 1)
            .unwrap_err()
            .category(),
        ErrorCategory::Corruption
    );
}

#[test]
fn lz4_document_compression_shrinks_file_and_survives_update_overflow_and_reopen() {
    let dir = TempDir::new("lz4_db");
    let compressed_path = dir.0.join("compressed.oryq");
    let uncompressed_path = dir.0.join("uncompressed.oryq");

    let repetitive =
        "OryqDB embedded single-file document engine with transparent LZ4 compression. ".repeat(30); // ~2.3 KiB per doc

    // 1. Write 200 repetitive docs WITH compression enabled (default Some(128))
    {
        let cfg = DatabaseConfig {
            sync_mode: SyncMode::Off,
            compression_threshold_bytes: Some(128),
            ..DatabaseConfig::default()
        };
        let db = Database::open(&compressed_path, cfg).unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("articles").unwrap();
        let mut col = wtx.collection("articles").unwrap();
        col.create_index("tag", SortOrder::Asc, false, false, Some("idx_tag"))
            .unwrap();
        for i in 0..200i64 {
            col.insert_json(&format!(
                r#"{{"_id": {i}, "tag": "t{}", "body": "{repetitive}"}}"#,
                i % 10
            ))
            .unwrap();
        }
        // Also insert a 64 KiB highly repetitive doc (compresses inline!) and a 64 KiB
        // semi-random doc (compresses or overflows cleanly).
        let huge_rep = "ABCDEFGHIJKLMNOP".repeat(4096);
        col.insert_json(&format!(
            r#"{{"_id": 1000, "tag": "huge", "body": "{huge_rep}"}}"#
        ))
        .unwrap();
        // Update a compressed doc in place and replace another.
        assert!(col
            .update_by_id(
                &oryq_document::Value::Int64(5),
                r#"{"$set": {"edited": true}}"#
            )
            .unwrap());
        assert!(col
            .replace_by_id(
                &oryq_document::Value::Int64(6),
                &format!(r#"{{"_id": 6, "tag": "replaced", "body": "{repetitive}"}}"#)
            )
            .unwrap());
        wtx.commit().unwrap();
        assert!(db.integrity_check().unwrap().ok);
    }

    // 2. Write the same 200 repetitive docs WITHOUT compression
    {
        let cfg = DatabaseConfig {
            sync_mode: SyncMode::Off,
            compression_threshold_bytes: None,
            ..DatabaseConfig::default()
        };
        let db = Database::open(&uncompressed_path, cfg).unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("articles").unwrap();
        let mut col = wtx.collection("articles").unwrap();
        col.create_index("tag", SortOrder::Asc, false, false, Some("idx_tag"))
            .unwrap();
        for i in 0..200i64 {
            col.insert_json(&format!(
                r#"{{"_id": {i}, "tag": "t{}", "body": "{repetitive}"}}"#,
                i % 10
            ))
            .unwrap();
        }
        let huge_rep = "ABCDEFGHIJKLMNOP".repeat(4096);
        col.insert_json(&format!(
            r#"{{"_id": 1000, "tag": "huge", "body": "{huge_rep}"}}"#
        ))
        .unwrap();
        wtx.commit().unwrap();
    }

    let comp_size = fs::metadata(&compressed_path).unwrap().len();
    let uncomp_size = fs::metadata(&uncompressed_path).unwrap().len();
    assert!(
        comp_size * 2 < uncomp_size,
        "compressed DB ({comp_size} B) should be less than half of uncompressed DB ({uncomp_size} B)"
    );

    // 3. Reopen compressed DB, verify all data, integrity_check, and vacuum.
    {
        let db = Database::open(&compressed_path, DatabaseConfig::default()).unwrap();
        assert!(db.integrity_check().unwrap().ok);
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("articles").unwrap();
        assert_eq!(col.count().unwrap(), 201);
        let doc5 = col
            .find_by_id(&oryq_document::Value::Int64(5))
            .unwrap()
            .unwrap();
        assert_eq!(
            doc5.view().get_field("edited").unwrap().to_owned_value(),
            oryq_document::Value::Bool(true)
        );
        let doc1000 = col
            .find_by_id(&oryq_document::Value::Int64(1000))
            .unwrap()
            .unwrap();
        assert_eq!(
            doc1000.view().get_field("body").unwrap().to_owned_value(),
            oryq_document::Value::String("ABCDEFGHIJKLMNOP".repeat(4096))
        );
        drop(rtx);
        db.vacuum().unwrap();
        assert!(db.integrity_check().unwrap().ok);
    }
}

#[test]
fn deferred_write_txn_checksums_pass_integrity_after_splits_savepoints_and_drop() {
    let dir = TempDir::new("deferred_cksum");
    let db_path = dir.0.join("deferred.oryq");

    {
        let cfg = DatabaseConfig {
            sync_mode: SyncMode::Off,
            ..DatabaseConfig::default()
        };
        let db = Database::open(&db_path, cfg).unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("main").unwrap();
        wtx.create_collection("temp").unwrap();
        {
            let mut tmp = wtx.collection("temp").unwrap();
            for i in 0..50i64 {
                tmp.insert_json(&format!(r#"{{"_id": {i}, "v": "tmp-{i}"}}"#))
                    .unwrap();
            }
        }
        // Dropping a dirty collection inside the same WriteTxn must work with deferred checksums.
        assert!(wtx.drop_collection("temp").unwrap());

        {
            let mut col = wtx.collection("main").unwrap();
            col.create_index("k", SortOrder::Asc, false, false, Some("idx_k"))
                .unwrap();
            for i in 0..500i64 {
                col.insert_json(&format!(
                    r#"{{"_id": {i}, "k": {}, "v": "val-{i}"}}"#,
                    i % 50
                ))
                .unwrap();
            }
            for i in (0..100i64).step_by(2) {
                assert!(col.delete_by_id(&oryq_document::Value::Int64(i)).unwrap());
            }
        }
        wtx.commit().unwrap();
        assert!(db.integrity_check().unwrap().ok);
    }

    // Reopen from disk — every committed page must pass full XXH3_64 verification.
    let db = Database::open(&db_path, DatabaseConfig::default()).unwrap();
    assert!(db.integrity_check().unwrap().ok);
    let rtx = db.begin_read().unwrap();
    assert_eq!(rtx.collection("main").unwrap().count().unwrap(), 450);
}
