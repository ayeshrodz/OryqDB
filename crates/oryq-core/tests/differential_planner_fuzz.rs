//! Randomized differential query-planner oracle suite for Issue #49 (`v1.0.0` GA).
//!
//! Verifies that indexed access plans (`PrimaryIdLookup`, `SecondaryEqScan`,
//! `SecondaryRangeScan` across `Secondary`, `Compound`, `Multikey`, `Sparse`,
//! `Asc`/`Desc`, and nested dotted-path indexes) return the exact same document
//! set and count as an unindexed `CollectionScan` oracle across 300 randomized
//! filters, updates, and deletes.

use std::collections::{BTreeMap, BTreeSet};

use oryq_core::{AccessPlan, Database, DatabaseConfig, SortOrder, SyncMode};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn range(&mut self, upper: usize) -> usize {
        (self.next_u64() as usize) % upper
    }

    fn pick<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
        &slice[self.range(slice.len())]
    }
}

fn gen_doc_json(rng: &mut Rng, id: i64) -> String {
    let mut fields = Vec::new();
    fields.push(format!(r#""_id":{id}"#));

    // `score`: mix of i64, f64, null, or missing
    match rng.range(10) {
        0 => {}
        1 => fields.push(r#""score":null"#.to_owned()),
        2..=4 => {
            let v = (rng.range(60) as i64) - 10;
            fields.push(format!(r#""score":{v}"#));
        }
        _ => {
            let v = ((rng.range(120) as i64) - 20) as f64 / 2.0;
            fields.push(format!(r#""score":{v}"#));
        }
    }

    // `rank`: sparse Desc indexed field
    match rng.range(6) {
        0 | 1 => {}
        2 => fields.push(r#""rank":null"#.to_owned()),
        _ => {
            let r = (rng.range(25) as i64) - 5;
            fields.push(format!(r#""rank":{r}"#));
        }
    }

    // `cat`: leading compound key
    match rng.range(6) {
        0 => {}
        1 => fields.push(r#""cat":null"#.to_owned()),
        _ => {
            let c = *rng.pick(&["alpha", "beta", "gamma", "delta"]);
            fields.push(format!(r#""cat":"{c}""#));
        }
    }

    // `tags`: multikey array / empty array / scalar / null / missing
    match rng.range(8) {
        0 => {}
        1 => fields.push(r#""tags":null"#.to_owned()),
        2 => fields.push(r#""tags":[]"#.to_owned()),
        3 => {
            let n = rng.range(12) as i64;
            fields.push(format!(r#""tags":{n}"#));
        }
        _ => {
            let len = 1 + rng.range(4);
            let mut elems = Vec::with_capacity(len);
            for _ in 0..len {
                match rng.range(5) {
                    0 => elems.push("null".to_owned()),
                    1 => elems.push(format!(r#""{}""#, rng.pick(&["x", "y"]))),
                    _ => elems.push((rng.range(12) as i64).to_string()),
                }
            }
            fields.push(format!(r#""tags":[{}]"#, elems.join(",")));
        }
    }

    // `meta.tier`: nested object or array of objects or missing
    match rng.range(7) {
        0 => {}
        1 => fields.push(r#""meta":{}"#.to_owned()),
        2 => fields.push(r#""meta":null"#.to_owned()),
        3 | 4 => {
            let t = rng.range(5) as i64;
            fields.push(format!(r#""meta":{{"tier":{t}}}"#));
        }
        _ => {
            let t1 = rng.range(5) as i64;
            let t2 = rng.range(5) as i64;
            fields.push(format!(r#""meta":[{{"tier":{t1}}},{{"tier":{t2}}}]"#));
        }
    }

    // `opt`: sparse optional field
    match rng.range(4) {
        0 | 1 => {}
        2 => fields.push(r#""opt":null"#.to_owned()),
        _ => {
            let v = rng.range(8) as i64;
            fields.push(format!(r#""opt":{v}"#));
        }
    }

    format!("{{{}}}", fields.join(","))
}

fn gen_filter_json(rng: &mut Rng) -> String {
    match rng.range(14) {
        // Primary _id lookup
        0 => {
            let id = 1 + rng.range(160) as i64;
            format!(r#"{{"_id":{id}}}"#)
        }
        // Secondary equality on score (including null)
        1 => match rng.range(4) {
            0 => r#"{"score":null}"#.to_owned(),
            1 => format!(r#"{{"score":{}}}"#, (rng.range(40) as i64) - 5),
            _ => format!(
                r#"{{"score":{}}}"#,
                ((rng.range(60) as i64) - 10) as f64 / 2.0
            ),
        },
        // Secondary two-sided range on score
        2 => {
            let lo = (rng.range(30) as i64) - 5;
            let hi = lo + 1 + (rng.range(25) as i64);
            let op_lo = *rng.pick(&["$gt", "$gte"]);
            let op_hi = *rng.pick(&["$lt", "$lte"]);
            format!(r#"{{"score":{{"{op_lo}":{lo},"{op_hi}":{hi}}}}}"#)
        }
        // Sparse Desc range on rank
        3 => {
            let v = (rng.range(25) as i64) - 5;
            let op = *rng.pick(&["$gt", "$gte", "$lt", "$lte"]);
            format!(r#"{{"rank":{{"{op}":{v}}}}}"#)
        }
        // Multikey array element equality or empty-array equality
        4 => match rng.range(5) {
            0 => r#"{"tags":[]}"#.to_owned(),
            1 => r#"{"tags":null}"#.to_owned(),
            2 => format!(r#"{{"tags":"{}"}}"#, rng.pick(&["x", "y"])),
            _ => format!(r#"{{"tags":{}}}"#, rng.range(12)),
        },
        // Multikey two-sided range on tags (critical multikey soundness case!)
        5 => {
            let lo = rng.range(6) as i64;
            let hi = lo + 1 + (rng.range(6) as i64);
            format!(r#"{{"tags":{{"$gt":{lo},"$lt":{hi}}}}}"#)
        }
        // Compound prefix eq + range
        6 => {
            let c = *rng.pick(&["alpha", "beta", "gamma", "delta"]);
            let lo = (rng.range(30) as i64) - 5;
            let hi = lo + 5 + (rng.range(20) as i64);
            format!(r#"{{"cat":"{c}","score":{{"$gte":{lo},"$lte":{hi}}}}}"#)
        }
        // Compound prefix eq + null
        7 => {
            let c = *rng.pick(&["alpha", "beta", "gamma"]);
            format!(r#"{{"cat":"{c}","score":null}}"#)
        }
        // Nested dotted path eq / range
        8 => match rng.range(2) {
            0 => format!(r#"{{"meta.tier":{}}}"#, rng.range(5)),
            _ => format!(r#"{{"meta.tier":{{"$gte":{}}}}}"#, rng.range(5)),
        },
        // Sparse field eq / null / $exists
        9 => match rng.range(4) {
            0 => r#"{"opt":null}"#.to_owned(),
            1 => r#"{"opt":{"$exists":false}}"#.to_owned(),
            2 => r#"{"opt":{"$exists":true}}"#.to_owned(),
            _ => format!(r#"{{"opt":{}}}"#, rng.range(8)),
        },
        // $in on indexed field
        10 => {
            let a = rng.range(12);
            let b = rng.range(12);
            format!(r#"{{"tags":{{"$in":[{a},{b}]}}}}"#)
        }
        // $or combining indexed and sparse fields
        11 => {
            let c = *rng.pick(&["alpha", "beta"]);
            let o = rng.range(8);
            format!(r#"{{"$or":[{{"cat":"{c}"}},{{"opt":{o}}}]}}"#)
        }
        // $ne / $not
        12 => {
            let c = *rng.pick(&["alpha", "beta", "gamma"]);
            format!(r#"{{"cat":{{"$ne":"{c}"}}}}"#)
        }
        // Compound equality + multikey conjunction
        _ => {
            let c = *rng.pick(&["alpha", "beta", "gamma", "delta"]);
            let t = rng.range(12);
            format!(r#"{{"cat":"{c}","tags":{t}}}"#)
        }
    }
}

fn collect_id_set(docs: &[oryq_document::DocumentBuf]) -> BTreeSet<i64> {
    docs.iter()
        .map(
            |d| match d.view().get_field("_id").unwrap().to_owned_value() {
                oryq_document::Value::Int64(n) => n,
                other => panic!("unexpected _id type: {other:?}"),
            },
        )
        .collect()
}

fn collect_id_vec(docs: &[oryq_document::DocumentBuf]) -> Vec<i64> {
    docs.iter()
        .map(
            |d| match d.view().get_field("_id").unwrap().to_owned_value() {
                oryq_document::Value::Int64(n) => n,
                other => panic!("unexpected _id type: {other:?}"),
            },
        )
        .collect()
}

#[test]
fn differential_planner_oracle_matches_collection_scan_across_300_random_queries() {
    let db = Database::open_with_io(
        std::sync::Arc::new(oryq_engine::MemoryVfs::new()),
        std::path::Path::new("diff_planner_oracle.oryq"),
        DatabaseConfig {
            sync_mode: SyncMode::Off,
            ..DatabaseConfig::default()
        },
    )
    .unwrap();

    let mut rng = Rng::new(0x0123_4567_89AB_CDEF);

    // Populate identical `oracle` (unindexed) and `indexed` (6 diverse indexes) collections.
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("oracle").unwrap();
        wtx.create_collection("indexed").unwrap();
        {
            let mut idx_col = wtx.collection("indexed").unwrap();
            idx_col
                .create_index("score", SortOrder::Asc, false, false, Some("idx_score_asc"))
                .unwrap();
            idx_col
                .create_index(
                    "rank",
                    SortOrder::Desc,
                    false,
                    true,
                    Some("idx_rank_desc_sparse"),
                )
                .unwrap();
            idx_col
                .create_index("tags", SortOrder::Asc, false, false, Some("idx_tags_mk"))
                .unwrap();
            idx_col
                .create_compound_index(
                    &[("cat", SortOrder::Asc), ("score", SortOrder::Desc)],
                    false,
                    false,
                    Some("idx_cat_score"),
                )
                .unwrap();
            idx_col
                .create_index(
                    "meta.tier",
                    SortOrder::Asc,
                    false,
                    false,
                    Some("idx_nested"),
                )
                .unwrap();
            idx_col
                .create_index("opt", SortOrder::Asc, false, true, Some("idx_opt_sparse"))
                .unwrap();
        }

        for id in 1..=150i64 {
            let doc_json = gen_doc_json(&mut rng, id);
            wtx.collection("oracle")
                .unwrap()
                .insert_json(&doc_json)
                .unwrap();
            wtx.collection("indexed")
                .unwrap()
                .insert_json(&doc_json)
                .unwrap();
        }

        // Apply 30 identical random updates and 20 random deletes to verify index maintenance.
        for _ in 0..30 {
            let id = oryq_document::Value::Int64(1 + rng.range(150) as i64);
            let new_score = (rng.range(50) as i64) - 10;
            let upd = format!(
                r#"{{"$set":{{"score":{new_score},"opt":{}}}}}"#,
                rng.range(8)
            );
            let r1 = wtx
                .collection("oracle")
                .unwrap()
                .update_by_id(&id, &upd)
                .unwrap();
            let r2 = wtx
                .collection("indexed")
                .unwrap()
                .update_by_id(&id, &upd)
                .unwrap();
            assert_eq!(r1, r2);
        }
        for _ in 0..20 {
            let id = oryq_document::Value::Int64(1 + rng.range(150) as i64);
            let r1 = wtx.collection("oracle").unwrap().delete_by_id(&id).unwrap();
            let r2 = wtx
                .collection("indexed")
                .unwrap()
                .delete_by_id(&id)
                .unwrap();
            assert_eq!(r1, r2);
        }

        wtx.commit().unwrap();
    }

    assert!(db.integrity_check().unwrap().ok);

    let rtx = db.begin_read().unwrap();
    let oracle = rtx.collection("oracle").unwrap();
    let indexed = rtx.collection("indexed").unwrap();

    let mut plan_counts: BTreeMap<&'static str, usize> = BTreeMap::new();

    for trial in 0..300usize {
        let filter_json = gen_filter_json(&mut rng);
        let filter = oryq_core::parse_filter_json(&filter_json).unwrap();

        let o_cur = oracle
            .find_json(Some(&filter_json), None, None, 0, None)
            .unwrap();
        let i_cur = indexed
            .find_json(Some(&filter_json), None, None, 0, None)
            .unwrap();

        let plan_kind = match &i_cur.plan().access {
            AccessPlan::PrimaryIdLookup { .. } => "PrimaryIdLookup",
            AccessPlan::SecondaryEqScan { .. } => "SecondaryEqScan",
            AccessPlan::SecondaryRangeScan { .. } => "SecondaryRangeScan",
            AccessPlan::CollectionScan => "CollectionScan",
        };
        *plan_counts.entry(plan_kind).or_insert(0) += 1;

        let o_ids = collect_id_set(&o_cur.collect_all());
        let i_ids = collect_id_set(&i_cur.collect_all());
        assert_eq!(
            i_ids, o_ids,
            "trial {trial}: indexed ({plan_kind}) diverged from CollectionScan on filter {filter_json}"
        );

        let o_cnt = oracle.count_matching(Some(&filter)).unwrap();
        let i_cnt = indexed.count_matching(Some(&filter)).unwrap();
        assert_eq!(
            i_cnt, o_cnt,
            "trial {trial}: count_matching diverged on filter {filter_json}"
        );
        assert_eq!(i_cnt as usize, i_ids.len());

        // Every 5th trial also verifies deterministic sort + skip + limit parity.
        if trial % 5 == 0 {
            let sort = r#"{"_id": 1}"#;
            let skip = rng.range(3) as u64;
            let limit = Some(1 + rng.range(5) as u64);
            let o_page = collect_id_vec(
                &oracle
                    .find_json(Some(&filter_json), None, Some(sort), skip, limit)
                    .unwrap()
                    .collect_all(),
            );
            let i_page = collect_id_vec(
                &indexed
                    .find_json(Some(&filter_json), None, Some(sort), skip, limit)
                    .unwrap()
                    .collect_all(),
            );
            assert_eq!(
                i_page, o_page,
                "trial {trial}: sorted paginated find diverged on filter {filter_json}"
            );
        }
    }

    // Confirm all 4 access plan families were exercised.
    for required in [
        "PrimaryIdLookup",
        "SecondaryEqScan",
        "SecondaryRangeScan",
        "CollectionScan",
    ] {
        assert!(
            plan_counts.get(required).copied().unwrap_or(0) > 0,
            "expected plan family {required} to be exercised, got {plan_counts:?}"
        );
    }
}
