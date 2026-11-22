//! Deterministic fuzz/property suite asserting malformed, truncated, bitflipped, and
//! adversarially structured `OryqDoc` and physical delta patch buffers never panic and
//! fail cleanly with `ErrorCategory::Corruption`.

use oryq_document::{
    decode_physical_patch, encode_physical_patch, from_json_str, validate_document, DocumentBuf,
    PatchEntry, PatchOp, Value,
};
use oryq_types::ErrorCategory;

struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn next_usize(&mut self, max: usize) -> usize {
        if max == 0 {
            0
        } else {
            (self.next() as usize) % max
        }
    }
}

#[test]
fn fuzz_malformed_bitflipped_and_truncated_oryqdoc_never_panics() {
    let seed_jsons = [
        r#"{"_id": 1, "name": "Alice", "age": 30, "active": true}"#,
        r#"{"_id": "abc", "nested": {"a": [1, 2, 3], "b": null}, "score": 99.5}"#,
        r#"{"_id": 42, "tags": ["rust", "db", "embedded"], "meta": {"v": 1}}"#,
    ];
    let seeds: Vec<Vec<u8>> = seed_jsons
        .iter()
        .map(|j| from_json_str(j, None).unwrap().0.as_bytes().to_vec())
        .collect();

    let mut rng = XorShift64(0xDEAD_BEEF_CAFE_BABE);

    // 1. Every truncation prefix of every valid seed must fail cleanly with Corruption.
    for seed in &seeds {
        for cut in 0..seed.len() {
            let err = validate_document(&seed[..cut]).unwrap_err();
            assert_eq!(
                err.category(),
                ErrorCategory::Corruption,
                "truncated doc of len {cut} returned unexpected category"
            );
            assert_eq!(
                DocumentBuf::from_bytes(&seed[..cut])
                    .unwrap_err()
                    .category(),
                ErrorCategory::Corruption
            );
        }
    }

    // 2. 200 true single-bit flips + 200 multi-byte mutations on valid documents.
    for round in 0..400 {
        let base = &seeds[rng.next_usize(seeds.len())];
        let mut mutated = base.clone();
        if round < 200 {
            let idx = rng.next_usize(mutated.len());
            mutated[idx] ^= 1u8 << (rng.next() % 8);
        } else {
            let flips = 2 + rng.next_usize(3);
            for _ in 0..flips {
                let idx = rng.next_usize(mutated.len());
                mutated[idx] ^= (rng.next() & 0xFF) as u8;
            }
        }
        match DocumentBuf::from_bytes(mutated.as_slice()) {
            Ok(doc) => {
                for (_k, v) in doc.view().iter() {
                    let _ = v.to_owned_value();
                }
            }
            Err(err) => {
                assert!(
                    matches!(
                        err.category(),
                        ErrorCategory::Corruption | ErrorCategory::UnsupportedFeature
                    ),
                    "unexpected error category on mutated doc: {:?}",
                    err.category()
                );
            }
        }
    }

    // 3. Adversarial structural payloads: u32::MAX counts/lengths & excessive binary nesting.
    let huge_len_doc = [
        0x01u8, 0x00, 0xFF, 0xFF, // version=1, flags=0, field_count=u16::MAX
        0xFF, 0xFF, 0xFF, 0xFF, // total_size=u32::MAX
        0x00, 0x00, 0x00, 0x00,
    ];
    assert_eq!(
        validate_document(&huge_len_doc).unwrap_err().category(),
        ErrorCategory::Corruption
    );

    // 4. Valid PhysicalPatch truncation & single-bitflip fuzzing.
    let (valid_patch, _) = encode_physical_patch(
        0xFFFF,
        &[
            PatchEntry {
                top_level_field: "score".to_owned(),
                op: PatchOp::ReplaceValue,
                value: Some(Value::Int64(99)),
            },
            PatchEntry {
                top_level_field: "obsolete".to_owned(),
                op: PatchOp::TombstoneUnset,
                value: None,
            },
        ],
    )
    .unwrap();
    for cut in 0..valid_patch.len() {
        let err = decode_physical_patch(&valid_patch[..cut]).unwrap_err();
        assert_eq!(err.category(), ErrorCategory::Corruption);
    }
    for _ in 0..100 {
        let mut flipped = valid_patch.clone();
        let idx = rng.next_usize(flipped.len());
        flipped[idx] ^= 1u8 << (rng.next() % 8);
        let _ = decode_physical_patch(&flipped);
    }

    // 5. 150 completely random byte buffers fed into validate_document & decode_physical_patch.
    for _ in 0..150 {
        let len = rng.next_usize(256);
        let mut buf = vec![0u8; len];
        for b in &mut buf {
            *b = (rng.next() & 0xFF) as u8;
        }
        let _ = validate_document(&buf);
        let _ = decode_physical_patch(&buf);
    }
}
