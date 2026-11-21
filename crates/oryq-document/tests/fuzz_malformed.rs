//! Deterministic fuzz/property suite asserting malformed, truncated, and bitflipped
//! `OryqDoc` and physical delta patch buffers never panic and fail cleanly.

use oryq_document::{decode_physical_patch, from_json_str, validate_document, DocumentBuf};

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

    // 1. Every truncation prefix of every valid seed must fail cleanly without panicking.
    for seed in &seeds {
        for cut in 0..seed.len() {
            let res = validate_document(&seed[..cut]);
            assert!(
                res.is_err(),
                "truncated doc of len {cut} unexpectedly passed"
            );
            assert!(DocumentBuf::from_bytes(&seed[..cut]).is_err());
        }
    }

    // 2. 600 deterministic single- and multi-byte mutations on valid documents.
    for _ in 0..600 {
        let base = &seeds[rng.next_usize(seeds.len())];
        let mut mutated = base.clone();
        let flips = 1 + rng.next_usize(4);
        for _ in 0..flips {
            let idx = rng.next_usize(mutated.len());
            mutated[idx] ^= (1u8 << (rng.next() % 8)) | ((rng.next() & 0xFF) as u8);
        }
        // Must never panic; if it happens to still decode as valid, iterating its view must also not panic.
        if let Ok(doc) = DocumentBuf::from_bytes(mutated.as_slice()) {
            let view = doc.view();
            for (_k, v) in view.iter() {
                let _ = v.to_owned_value();
            }
        }
    }

    // 3. 300 completely random byte buffers fed into validate_document & decode_physical_patch.
    for _ in 0..300 {
        let len = rng.next_usize(256);
        let mut buf = vec![0u8; len];
        for b in &mut buf {
            *b = (rng.next() & 0xFF) as u8;
        }
        let _ = validate_document(&buf);
        let _ = decode_physical_patch(&buf);
    }
}
