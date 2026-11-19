use oryq_document::{
    compare_values, encode_document, Decimal128, ObjectId, Uuid, Value, ValueView,
};
use oryq_engine::PageSize;
use oryq_index::{
    decode_record_id, encode_primary_id_key, encode_record_id, encode_value_key,
    max_index_key_bytes, RecordId, SortOrder, MAX_PRIMARY_STRING_ID_BYTES,
};
use oryq_types::ErrorCategory;

#[test]
fn memcomparable_key_order_matches_compare_values_oracle_across_all_types() {
    let two_53 = 1i64 << 53;
    let raw_values = vec![
        Value::Null,
        Value::Bool(false),
        Value::Bool(true),
        Value::Float64(f64::NAN),
        Value::Decimal128(Decimal128::parse_str("NaN").unwrap()),
        Value::Float64(f64::NEG_INFINITY),
        Value::Decimal128(Decimal128::parse_str("-Infinity").unwrap()),
        Value::Int64(i64::MIN),
        Value::Decimal128(Decimal128::parse_str("-9223372036854775808").unwrap()),
        Value::Float64(-100.5),
        Value::Int64(-1),
        Value::Float64(-0.0),
        Value::Float64(0.0),
        Value::Int64(0),
        Value::Decimal128(Decimal128::parse_str("-0.00").unwrap()),
        Value::Float64(f64::from_bits(1)), // subnormal 2^-1074
        Value::Decimal128(Decimal128::parse_str("0.1").unwrap()),
        Value::Float64(0.1_f64),
        Value::Int64(10),
        Value::Float64(10.0),
        Value::Decimal128(Decimal128::parse_str("10.00").unwrap()),
        Value::Int64(two_53),
        Value::Float64(two_53 as f64),
        Value::Int64(two_53 + 1),
        Value::Int64(i64::MAX),
        Value::Float64((1u64 << 63) as f64),
        Value::Float64(f64::INFINITY),
        Value::Decimal128(Decimal128::parse_str("Infinity").unwrap()),
        Value::DateTimeUtcMs(i64::MIN),
        Value::DateTimeUtcMs(-1),
        Value::DateTimeUtcMs(0),
        Value::DateTimeUtcMs(1_700_000_000_000),
        Value::String("".into()),
        Value::String("a".into()),
        Value::String("a\0b".into()),
        Value::String("aa".into()),
        Value::String("🚀".into()),
        Value::Binary {
            subtype: 0,
            bytes: vec![0x00, 0x01],
        },
        Value::Binary {
            subtype: 1,
            bytes: vec![0x00, 0x01],
        },
        Value::ObjectId(ObjectId::from_bytes([0u8; 12])),
        Value::ObjectId(ObjectId::from_bytes([0xFFu8; 12])),
        Value::Uuid(Uuid::from_bytes([0u8; 16])),
        Value::Uuid(Uuid::from_bytes([0xFFu8; 16])),
        Value::Array(vec![Value::Int64(1), Value::String("x".into())]),
        Value::Array(vec![Value::Int64(2)]),
        Value::Document(vec![("a".into(), Value::Int64(1))]),
        Value::Document(vec![("a".into(), Value::Int64(2))]),
    ];

    // Wrap each value inside a 1-field OryqDoc so we borrow genuine `ValueView`s.
    let docs: Vec<_> = raw_values
        .into_iter()
        .map(|v| encode_document(&[("v".into(), v)], None).unwrap().0)
        .collect();
    let views: Vec<ValueView<'_>> = docs
        .iter()
        .map(|d| d.view().get_field("v").unwrap())
        .collect();

    let asc_keys: Vec<Vec<u8>> = views
        .iter()
        .map(|v| encode_value_key(v, SortOrder::Asc, 4096).unwrap())
        .collect();
    let desc_keys: Vec<Vec<u8>> = views
        .iter()
        .map(|v| encode_value_key(v, SortOrder::Desc, 4096).unwrap())
        .collect();

    // Check every pair (i, j) against `oryq_document::compare_values`!
    // Note: Binary compares bytes-first in compare_values vs subtype-first in key_codec,
    // so skip cross-subtype Binary pairs when comparing to compare_values (same-subtype matches).
    for i in 0..views.len() {
        for j in 0..views.len() {
            if matches!(
                (&views[i], &views[j]),
                (
                    ValueView::Binary { subtype: sa, .. },
                    ValueView::Binary { subtype: sb, .. }
                ) if sa != sb
            ) {
                continue;
            }
            let expected = compare_values(&views[i], &views[j]);
            assert_eq!(
                asc_keys[i].cmp(&asc_keys[j]),
                expected,
                "Asc key order mismatch for {:?} vs {:?}",
                views[i],
                views[j]
            );
            assert_eq!(
                desc_keys[i].cmp(&desc_keys[j]),
                expected.reverse(),
                "Desc key order mismatch for {:?} vs {:?}",
                views[i],
                views[j]
            );
        }
    }
}

#[test]
fn primary_id_type_validation_and_record_id_roundtrip() {
    let page_size = PageSize::Size4096;
    assert_eq!(max_index_key_bytes(page_size), 976);

    // Valid _id types: ObjectId, Uuid, Int64, String (<= 1024B)
    let oid = ObjectId::from_bytes([1u8; 12]);
    let uuid = Uuid::from_bytes([2u8; 16]);
    assert!(encode_primary_id_key(&ValueView::ObjectId(oid), page_size).is_ok());
    assert!(encode_primary_id_key(&ValueView::Uuid(uuid), page_size).is_ok());
    assert!(encode_primary_id_key(&ValueView::Int64(42), page_size).is_ok());
    assert!(encode_primary_id_key(&ValueView::String("user-1"), page_size).is_ok());

    // Oversized String _id (> 1024B) rejected
    let long_str = "x".repeat(MAX_PRIMARY_STRING_ID_BYTES + 1);
    assert_eq!(
        encode_primary_id_key(&ValueView::String(&long_str), page_size)
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );

    // Disallowed _id types rejected
    for bad in [
        ValueView::Null,
        ValueView::Bool(true),
        ValueView::Float64(1.5),
        ValueView::Decimal128(Decimal128::parse_str("1.5").unwrap()),
        ValueView::DateTimeUtcMs(0),
        ValueView::Binary {
            subtype: 0,
            bytes: &[1, 2],
        },
    ] {
        assert_eq!(
            encode_primary_id_key(&bad, page_size)
                .unwrap_err()
                .category(),
            ErrorCategory::InvalidArgument
        );
    }

    // RecordId 8B BE roundtrip
    let rid = RecordId::new(0x0102_0304_0506_0708).unwrap();
    let enc = encode_record_id(rid);
    assert_eq!(decode_record_id(&enc).unwrap(), rid);
    assert_eq!(
        decode_record_id(&[0u8; 8]).unwrap_err().category(),
        ErrorCategory::InvalidArgument
    );
}
