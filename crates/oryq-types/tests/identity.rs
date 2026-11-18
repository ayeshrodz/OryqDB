use oryq_types::{
    CapabilityDescriptor, CapabilityId, CollectionId, ErrorCategory, FeatureBitmask, IndexId,
    NamespaceId, PageId, RecordId, SnapshotId, TxId, VersionRange,
};

#[test]
fn checked_ids_reject_zero_and_overflow() {
    assert_eq!(PageId::HEADER.raw(), 0);
    assert_eq!(PageId::HEADER.checked_next().unwrap().raw(), 1);
    assert_eq!(
        PageId::new(u64::MAX).checked_next().unwrap_err().category(),
        ErrorCategory::Overflow
    );

    let snap: SnapshotId = TxId::INITIAL;
    assert_eq!(snap.raw(), 0);
    assert_eq!(
        TxId::new(u64::MAX).checked_next().unwrap_err().category(),
        ErrorCategory::Overflow
    );

    assert_eq!(
        CollectionId::new(0).unwrap_err().category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(
        IndexId::new(0).unwrap_err().category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(
        NamespaceId::new(0).unwrap_err().category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(
        RecordId::new(0).unwrap_err().category(),
        ErrorCategory::InvalidArgument
    );

    assert_eq!(RecordId::new(1).unwrap().checked_next().unwrap().raw(), 2);
    assert_eq!(
        RecordId::new(u64::MAX)
            .unwrap()
            .checked_next()
            .unwrap_err()
            .category(),
        ErrorCategory::Overflow
    );
}

#[test]
fn capability_ids_bitmasks_and_version_ranges_validate_bounds() {
    let cap = CapabilityId::new("oryq.index.vector.v1").expect("valid id");
    assert_eq!(cap.as_str(), "oryq.index.vector.v1");

    for bad in [
        "",
        ".leading",
        "trailing.",
        "double..dot",
        "UpperCase",
        "bad/slash",
    ] {
        assert_eq!(
            CapabilityId::new(bad).unwrap_err().category(),
            ErrorCategory::InvalidArgument,
            "expected invalid capability id: {bad}"
        );
    }

    let mask = FeatureBitmask::EMPTY
        .with_bit(0)
        .unwrap()
        .with_bit(1)
        .unwrap();
    assert!(mask.contains_all(FeatureBitmask::EMPTY.with_bit(0).unwrap()));
    assert_eq!(
        FeatureBitmask::EMPTY.with_bit(64).unwrap_err().category(),
        ErrorCategory::InvalidArgument
    );

    let range = VersionRange::new(1, 3).unwrap();
    assert!(range.contains(1));
    assert!(range.contains(2));
    assert!(range.contains(3));
    assert!(!range.contains(4));
    assert_eq!(
        VersionRange::new(4, 2).unwrap_err().category(),
        ErrorCategory::InvalidArgument
    );

    let desc = CapabilityDescriptor {
        id: cap,
        interface_version: 1,
        persistent: true,
        required_read_bits: FeatureBitmask::EMPTY,
        required_write_bits: mask,
    };
    assert_eq!(desc.required_write_bits.raw(), 0b11);
}
