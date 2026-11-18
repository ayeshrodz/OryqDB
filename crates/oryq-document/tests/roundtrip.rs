use oryq_document::codec::DOC_HEADER_SIZE;
use oryq_document::{
    encode_document, from_json_str, from_json_value, to_json_string, to_json_value,
    validate_document, Decimal128, DecodedDecimal128, DocumentBuf, DocumentView, FieldPath,
    ObjectId, OryqTypeTag, Uuid, Value, ValueView,
};
use oryq_types::{BudgetDomain, BudgetHandle, ErrorCategory};

#[test]
fn roundtrip_all_value_types_and_zero_copy_views() {
    let oid = ObjectId::from_bytes([
        0x66, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
    ]);
    let uuid = Uuid::from_bytes([
        0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00,
        0x00,
    ]);
    let dec = Decimal128::parse_str("12345678901234567890.12345").expect("valid decimal");

    let fields = vec![
        ("null_field".to_string(), Value::Null),
        ("bool_t".to_string(), Value::Bool(true)),
        ("bool_f".to_string(), Value::Bool(false)),
        ("i64_val".to_string(), Value::Int64(-9_007_199_254_740_993)),
        ("f64_val".to_string(), Value::Float64(3.25)),
        ("dec_val".to_string(), Value::Decimal128(dec)),
        ("str_val".to_string(), Value::String("hello 🚀".to_string())),
        (
            "bin_val".to_string(),
            Value::Binary {
                subtype: 0x80,
                bytes: vec![0xDE, 0xAD, 0xBE, 0xEF],
            },
        ),
        ("oid_val".to_string(), Value::ObjectId(oid)),
        ("uuid_val".to_string(), Value::Uuid(uuid)),
        (
            "dt_val".to_string(),
            Value::DateTimeUtcMs(1_700_000_000_123),
        ),
        (
            "arr_val".to_string(),
            Value::Array(vec![
                Value::Int64(10),
                Value::String("nested".to_string()),
                Value::Document(vec![("inner".to_string(), Value::Bool(true))]),
            ]),
        ),
        (
            "doc_val".to_string(),
            Value::Document(vec![
                ("z".to_string(), Value::Int64(1)),
                (
                    "a".to_string(),
                    Value::Document(vec![("leaf".to_string(), Value::String("ok".to_string()))]),
                ),
            ]),
        ),
    ];

    let (doc, reservation): (DocumentBuf, _) =
        encode_document(&fields, None).expect("encode should succeed");
    assert!(reservation.is_none());
    validate_document(doc.as_bytes()).expect("encoded document must validate");

    let view: DocumentView<'_> = doc.view();
    assert_eq!(view.field_count(), fields.len());
    assert_eq!(view.as_bytes().len(), doc.len());
    assert!(doc.len() >= DOC_HEADER_SIZE);

    assert!(matches!(
        view.get_field("null_field"),
        Some(ValueView::Null)
    ));
    assert_eq!(
        view.get_bool(&FieldPath::parse("bool_t").unwrap()),
        Some(true)
    );
    assert_eq!(
        view.get_bool(&FieldPath::parse("bool_f").unwrap()),
        Some(false)
    );
    assert_eq!(
        view.get_i64(&FieldPath::parse("i64_val").unwrap()),
        Some(-9_007_199_254_740_993)
    );
    assert_eq!(
        view.get_f64(&FieldPath::parse("f64_val").unwrap()),
        Some(3.25)
    );
    assert_eq!(
        view.get_decimal128(&FieldPath::parse("dec_val").unwrap()),
        Some(dec)
    );
    assert_eq!(
        view.get_str(&FieldPath::parse("str_val").unwrap()),
        Some("hello 🚀")
    );
    assert!(matches!(
        view.get_field("bin_val"),
        Some(ValueView::Binary {
            subtype: 0x80,
            bytes: &[0xDE, 0xAD, 0xBE, 0xEF],
        })
    ));
    assert_eq!(
        view.get_object_id(&FieldPath::parse("oid_val").unwrap()),
        Some(oid)
    );
    assert_eq!(
        view.get_uuid(&FieldPath::parse("uuid_val").unwrap()),
        Some(uuid)
    );
    assert_eq!(
        view.get_datetime_ms(&FieldPath::parse("dt_val").unwrap()),
        Some(1_700_000_000_123)
    );
    assert!(view.get_field("missing").is_none());

    // ArrayView O(1) random access and ExactSizeIterator
    let arr = match view.get_field("arr_val").unwrap() {
        ValueView::Array(a) => a,
        other => panic!("expected array, got {other:?}"),
    };
    assert_eq!(arr.len(), 3);
    assert!(!arr.is_empty());
    assert!(matches!(arr.get(0), Some(ValueView::Int64(10))));
    assert!(matches!(arr.get(1), Some(ValueView::String("nested"))));
    assert!(arr.get(3).is_none());
    let collected: Vec<ValueView<'_>> = arr.iter().collect();
    assert_eq!(collected.len(), 3);

    // Dotted path lookup across nested documents
    let path = FieldPath::parse("doc_val.a.leaf").expect("valid path");
    assert_eq!(view.get_str(&path), Some("ok"));
    assert!(matches!(
        view.get_path_str("doc_val.z").unwrap(),
        Some(ValueView::Int64(1))
    ));

    // Unaligned byte-offset read (copy into offset +1)
    let mut padded = vec![0xAAu8];
    padded.extend_from_slice(doc.as_bytes());
    let unaligned = DocumentView::from_bytes(&padded[1..]).expect("unaligned view must work");
    assert_eq!(
        unaligned.get_i64(&FieldPath::parse("i64_val").unwrap()),
        Some(-9_007_199_254_740_993)
    );
    assert_eq!(unaligned.get_str(&path), Some("ok"));

    // Type tags match
    assert_eq!(
        view.get_field("dec_val").unwrap().type_tag(),
        OryqTypeTag::Decimal128
    );
}

#[test]
fn extended_json_roundtrip() {
    let input = r#"{
        "_id": {"$oid": "660102030405060708090a0b"},
        "uid": {"$uuid": "550e8400-e29b-41d4-a716-446655440000"},
        "ts": {"$date": 1700000000123},
        "blob": {"$binary": {"base64": "3q2+7w==", "subType": "80"}},
        "big": {"$numberLong": "9223372036854775807"},
        "nan": {"$numberDouble": "NaN"},
        "inf": {"$numberDouble": "-Infinity"},
        "price": {"$numberDecimal": "19.95"},
        "tags": ["nosql", "embedded"],
        "meta": {"ok": true, "n": null}
    }"#;

    let (doc, _) = from_json_str(input, None).expect("extended JSON should encode");
    let view = doc.view();

    assert_eq!(
        view.get_i64(&FieldPath::parse("big").unwrap()),
        Some(i64::MAX)
    );
    assert!(view
        .get_f64(&FieldPath::parse("nan").unwrap())
        .unwrap()
        .is_nan());
    assert_eq!(
        view.get_f64(&FieldPath::parse("inf").unwrap()),
        Some(f64::NEG_INFINITY)
    );
    assert_eq!(
        view.get_decimal128(&FieldPath::parse("price").unwrap()),
        Some(Decimal128::parse_str("19.95").unwrap())
    );

    let json_str = to_json_string(&view).expect("to_json_string should succeed");
    let (doc2, _) = from_json_str(&json_str, None).expect("round-tripped JSON should re-encode");
    let val1 = to_json_value(&view);
    let val2 = to_json_value(&doc2.view());
    assert_eq!(val1, val2);

    // Non-object root is rejected
    let err = from_json_value(&serde_json::json!([1, 2, 3]), None).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::InvalidArgument);
}

#[test]
fn decimal128_special_values_decode_correctly() {
    let nan = Decimal128::parse_str("NaN").unwrap();
    assert_eq!(nan.decode(), DecodedDecimal128::NaN);
    assert_eq!(nan.to_string(), "NaN");

    let pos_inf = Decimal128::parse_str("Infinity").unwrap();
    assert_eq!(
        pos_inf.decode(),
        DecodedDecimal128::Infinity { negative: false }
    );
    assert_eq!(pos_inf.to_string(), "Infinity");

    let neg_inf = Decimal128::parse_str("-Infinity").unwrap();
    assert_eq!(
        neg_inf.decode(),
        DecodedDecimal128::Infinity { negative: true }
    );
    assert_eq!(neg_inf.to_string(), "-Infinity");

    let zero = Decimal128::parse_str("0.00").unwrap();
    assert_eq!(
        zero.decode(),
        DecodedDecimal128::Finite {
            negative: false,
            coefficient: 0,
            exponent: -2,
        }
    );
    assert_eq!(zero.to_string(), "0.00");
}

#[test]
fn encode_document_respects_memory_budget() {
    let small = BudgetHandle::new(BudgetDomain::Query, 32);

    let err = encode_document(
        &[("payload".to_string(), Value::String("x".repeat(128)))],
        Some(&small),
    )
    .unwrap_err();
    assert_eq!(err.category(), ErrorCategory::QuotaExceeded);
    assert_eq!(small.used_bytes(), 0);

    let big = BudgetHandle::new(BudgetDomain::Query, 4096);
    let (doc, reservation) = encode_document(
        &[("payload".to_string(), Value::String("x".repeat(128)))],
        Some(&big),
    )
    .expect("fits in budget");
    assert!(doc.len() > 128);
    assert_eq!(big.used_bytes(), doc.len());
    drop(reservation);
    assert_eq!(big.used_bytes(), 0);
}
