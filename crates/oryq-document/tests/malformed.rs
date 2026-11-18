use oryq_document::codec::DOC_HEADER_SIZE;
use oryq_document::{
    encode_document, validate_document, DocumentView, FieldPath, Value, MAX_PATH_DEPTH,
};
use oryq_types::ErrorCategory;

const FLAG_HAS_OVERFLOW_POINTERS: u8 = 0x02;

#[test]
fn rejects_truncated_and_wrong_version_headers() {
    for len in 0..DOC_HEADER_SIZE {
        let bytes = vec![0u8; len];
        let err = validate_document(&bytes).unwrap_err();
        assert_eq!(err.category(), ErrorCategory::Corruption);
    }

    let (doc, _) = encode_document(&[("k".to_string(), Value::Int64(1))], None).unwrap();
    // Byte 0 is version
    let mut bad_ver = doc.as_bytes().to_vec();
    bad_ver[0] = 0x02;
    assert_eq!(
        validate_document(&bad_ver).unwrap_err().category(),
        ErrorCategory::Corruption
    );

    // Bytes 4..8 are total_byte_len (u32 LE)
    let mut bad_len = doc.as_bytes().to_vec();
    bad_len[4..8].copy_from_slice(&(doc.len() as u32 + 4).to_le_bytes());
    assert_eq!(
        validate_document(&bad_len).unwrap_err().category(),
        ErrorCategory::Corruption
    );
}

#[test]
fn rejects_overflow_flag_and_reserved_flags() {
    let (doc, _) = encode_document(&[("k".to_string(), Value::Int64(1))], None).unwrap();

    // Byte 1 is flags
    let mut with_overflow = doc.as_bytes().to_vec();
    with_overflow[1] = FLAG_HAS_OVERFLOW_POINTERS;
    let err = validate_document(&with_overflow).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::Corruption);

    let mut with_reserved = doc.as_bytes().to_vec();
    with_reserved[1] = 0x04;
    let err = validate_document(&with_reserved).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::Corruption);
}

#[test]
fn rejects_corrupted_directory_entries() {
    let (doc, _) = encode_document(
        &[
            ("alpha".to_string(), Value::Int64(1)),
            ("beta".to_string(), Value::Int64(2)),
        ],
        None,
    )
    .unwrap();

    // Corrupt hash prefix of entry 0
    let mut bad_hash = doc.as_bytes().to_vec();
    bad_hash[DOC_HEADER_SIZE] ^= 0xFF;
    assert_eq!(
        validate_document(&bad_hash).unwrap_err().category(),
        ErrorCategory::Corruption
    );

    // Swap entries 0 and 1 so directory is unsorted
    let mut swapped = doc.as_bytes().to_vec();
    let e0 = swapped[DOC_HEADER_SIZE..DOC_HEADER_SIZE + 12].to_vec();
    let e1 = swapped[DOC_HEADER_SIZE + 12..DOC_HEADER_SIZE + 24].to_vec();
    swapped[DOC_HEADER_SIZE..DOC_HEADER_SIZE + 12].copy_from_slice(&e1);
    swapped[DOC_HEADER_SIZE + 12..DOC_HEADER_SIZE + 24].copy_from_slice(&e0);
    assert_eq!(
        validate_document(&swapped).unwrap_err().category(),
        ErrorCategory::Corruption
    );

    // Set reserved type tag 0xE in high nibble of val_descriptor (u32 LE -> top nibble of byte +11)
    let mut bad_tag = doc.as_bytes().to_vec();
    bad_tag[DOC_HEADER_SIZE + 11] = (bad_tag[DOC_HEADER_SIZE + 11] & 0x0F) | 0xE0;
    assert_eq!(
        validate_document(&bad_tag).unwrap_err().category(),
        ErrorCategory::Corruption
    );

    // Set OverflowRef tag 0xF on entry 0 -> Corruption (forbidden in exported OryqDoc bytes)
    let mut overflow_tag = doc.as_bytes().to_vec();
    overflow_tag[DOC_HEADER_SIZE + 11] = (overflow_tag[DOC_HEADER_SIZE + 11] & 0x0F) | 0xF0;
    assert_eq!(
        validate_document(&overflow_tag).unwrap_err().category(),
        ErrorCategory::Corruption
    );

    // Make entry 1 val_descriptor point to entry 0's value span (overlapping spans)
    let mut overlap = doc.as_bytes().to_vec();
    let e0_desc = overlap[DOC_HEADER_SIZE + 8..DOC_HEADER_SIZE + 12].to_vec();
    overlap[DOC_HEADER_SIZE + 12 + 8..DOC_HEADER_SIZE + 12 + 12].copy_from_slice(&e0_desc);
    assert_eq!(
        validate_document(&overlap).unwrap_err().category(),
        ErrorCategory::Corruption
    );
}

#[test]
fn rejects_invalid_utf8_and_nul_field_names() {
    let err = encode_document(&[("bad\0key".to_string(), Value::Null)], None).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::InvalidArgument);

    let err = encode_document(
        &[
            ("dup".to_string(), Value::Int64(1)),
            ("dup".to_string(), Value::Int64(2)),
        ],
        None,
    )
    .unwrap_err();
    assert_eq!(err.category(), ErrorCategory::InvalidArgument);

    // Corrupt a valid string's UTF-8 payload byte
    let (doc, _) =
        encode_document(&[("s".to_string(), Value::String("abc".to_string()))], None).unwrap();
    let mut bad_utf8 = doc.as_bytes().to_vec();
    let last = bad_utf8.len() - 1;
    bad_utf8[last] = 0xFF;
    assert_eq!(
        DocumentView::from_bytes(&bad_utf8).unwrap_err().category(),
        ErrorCategory::Corruption
    );
}

#[test]
fn enforces_nesting_depth_and_field_path_rules() {
    // Build a document nested MAX_PATH_DEPTH + 1 levels deep
    let mut val = Value::Int64(42);
    for _ in 0..=MAX_PATH_DEPTH {
        val = Value::Document(vec![("n".to_string(), val)]);
    }
    let err = encode_document(&[("root".to_string(), val)], None).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::InvalidArgument);

    // FieldPath validation
    assert!(FieldPath::parse("").is_err());
    assert!(FieldPath::parse(".a").is_err());
    assert!(FieldPath::parse("a.").is_err());
    assert!(FieldPath::parse("a..b").is_err());
    assert!(FieldPath::parse("a.$b").is_err());
    assert!(FieldPath::parse("a.b\0c").is_err());

    let deep = vec!["x"; MAX_PATH_DEPTH + 1].join(".");
    assert!(FieldPath::parse(&deep).is_err());

    let ok_deep = vec!["x"; MAX_PATH_DEPTH].join(".");
    assert_eq!(
        FieldPath::parse(&ok_deep).unwrap().segments().count(),
        MAX_PATH_DEPTH
    );
}
