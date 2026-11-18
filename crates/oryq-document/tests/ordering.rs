use std::cmp::Ordering;

use oryq_document::{
    compare_values, encode_document, Decimal128, ObjectId, Uuid, Value, ValueView,
};

#[test]
fn canonical_cross_type_ordering() {
    let (doc_buf, _) = encode_document(&[("a".to_string(), Value::Int64(1))], None).unwrap();
    let (arr_holder, _) = encode_document(
        &[("arr".to_string(), Value::Array(vec![Value::Int64(1)]))],
        None,
    )
    .unwrap();
    let arr_view = match arr_holder.view().get_field("arr").unwrap() {
        ValueView::Array(a) => a,
        _ => unreachable!(),
    };

    let ordered: Vec<ValueView<'_>> = vec![
        ValueView::Null,
        ValueView::Bool(false),
        ValueView::Bool(true),
        ValueView::Float64(f64::NAN),
        ValueView::Float64(f64::NEG_INFINITY),
        ValueView::Int64(i64::MIN),
        ValueView::Float64(-1.5),
        ValueView::Int64(0),
        ValueView::Float64(1.5),
        ValueView::Int64(i64::MAX),
        ValueView::Float64(f64::INFINITY),
        ValueView::DateTimeUtcMs(-1),
        ValueView::DateTimeUtcMs(0),
        ValueView::String("a"),
        ValueView::String("b"),
        ValueView::Binary {
            subtype: 0,
            bytes: &[0x01],
        },
        ValueView::Binary {
            subtype: 1,
            bytes: &[0x01],
        },
        ValueView::ObjectId(ObjectId::from_bytes([0u8; 12])),
        ValueView::Uuid(Uuid::from_bytes([0u8; 16])),
        ValueView::Array(arr_view),
        ValueView::Document(doc_buf.view()),
    ];

    for i in 0..ordered.len() {
        assert_eq!(compare_values(&ordered[i], &ordered[i]), Ordering::Equal);
        for j in (i + 1)..ordered.len() {
            assert_eq!(
                compare_values(&ordered[i], &ordered[j]),
                Ordering::Less,
                "expected index {i} ({:?}) < index {j} ({:?})",
                ordered[i],
                ordered[j]
            );
            assert_eq!(
                compare_values(&ordered[j], &ordered[i]),
                Ordering::Greater,
                "expected index {j} ({:?}) > index {i} ({:?})",
                ordered[j],
                ordered[i]
            );
        }
    }
}

#[test]
fn exact_numeric_equality_and_inequality() {
    // -0.0 == +0.0 == 0i64 == Decimal("-0.00")
    let neg_zero = ValueView::Float64(-0.0);
    let pos_zero = ValueView::Float64(0.0);
    let int_zero = ValueView::Int64(0);
    let dec_zero = ValueView::Decimal128(Decimal128::parse_str("-0.00").unwrap());
    for a in [&neg_zero, &pos_zero, &int_zero, &dec_zero] {
        for b in [&neg_zero, &pos_zero, &int_zero, &dec_zero] {
            assert_eq!(compare_values(a, b), Ordering::Equal);
        }
    }

    // NaN == NaN (across f64 and Decimal128) and NaN < -Inf
    let f_nan = ValueView::Float64(f64::NAN);
    let d_nan = ValueView::Decimal128(Decimal128::parse_str("NaN").unwrap());
    let f_ninf = ValueView::Float64(f64::NEG_INFINITY);
    let d_ninf = ValueView::Decimal128(Decimal128::parse_str("-Infinity").unwrap());
    let f_pinf = ValueView::Float64(f64::INFINITY);
    let d_pinf = ValueView::Decimal128(Decimal128::parse_str("Infinity").unwrap());

    assert_eq!(compare_values(&f_nan, &d_nan), Ordering::Equal);
    assert_eq!(compare_values(&f_nan, &f_ninf), Ordering::Less);
    assert_eq!(compare_values(&d_nan, &d_ninf), Ordering::Less);
    assert_eq!(compare_values(&f_ninf, &d_ninf), Ordering::Equal);
    assert_eq!(compare_values(&f_pinf, &d_pinf), Ordering::Equal);

    // 10 == 10.0 == Decimal("10.0")
    let i10 = ValueView::Int64(10);
    let f10 = ValueView::Float64(10.0);
    let d10 = ValueView::Decimal128(Decimal128::parse_str("10.0").unwrap());
    assert_eq!(compare_values(&i10, &f10), Ordering::Equal);
    assert_eq!(compare_values(&i10, &d10), Ordering::Equal);
    assert_eq!(compare_values(&f10, &d10), Ordering::Equal);

    // Decimal("0.1") != 0.1_f64 because binary f64 0.1 is 0.10000000000000000555... > 1/10
    let d_tenth = ValueView::Decimal128(Decimal128::parse_str("0.1").unwrap());
    let f_tenth = ValueView::Float64(0.1_f64);
    assert_eq!(compare_values(&d_tenth, &f_tenth), Ordering::Less);
    assert_eq!(compare_values(&f_tenth, &d_tenth), Ordering::Greater);

    // Large i64 values above 2^53 must never lose precision against f64
    let two_53: i64 = 1i64 << 53;
    let two_53_plus_1: i64 = two_53 + 1;
    let f_two_53 = ValueView::Float64(two_53 as f64);
    assert_eq!(
        compare_values(&ValueView::Int64(two_53), &f_two_53),
        Ordering::Equal
    );
    assert_eq!(
        compare_values(&ValueView::Int64(two_53_plus_1), &f_two_53),
        Ordering::Greater
    );

    // i64::MAX vs 2^63 as f64 (9223372036854775808.0 > 9223372036854775807)
    let f_two_63 = ValueView::Float64((1u64 << 63) as f64);
    assert_eq!(
        compare_values(&ValueView::Int64(i64::MAX), &f_two_63),
        Ordering::Less
    );
    assert_eq!(
        compare_values(
            &ValueView::Int64(i64::MAX),
            &ValueView::Decimal128(Decimal128::parse_str("9223372036854775807").unwrap())
        ),
        Ordering::Equal
    );
    assert_eq!(
        compare_values(
            &ValueView::Int64(i64::MIN),
            &ValueView::Decimal128(Decimal128::parse_str("-9223372036854775808").unwrap())
        ),
        Ordering::Equal
    );

    // Subnormal f64 > 0 and < Decimal("1e-300")
    let subnormal = ValueView::Float64(f64::from_bits(1)); // 2^-1074 ~ 5e-324
    assert_eq!(compare_values(&int_zero, &subnormal), Ordering::Less);
    assert_eq!(
        compare_values(
            &subnormal,
            &ValueView::Decimal128(Decimal128::parse_str("1e-300").unwrap())
        ),
        Ordering::Less
    );
}
