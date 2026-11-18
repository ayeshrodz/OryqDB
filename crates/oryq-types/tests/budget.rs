use std::thread;

use oryq_types::{BudgetDomain, BudgetHandle, ErrorCategory};

#[test]
fn reservation_drop_releases_charge_exactly_once() {
    let budget = BudgetHandle::new(BudgetDomain::Query, 1024);
    assert_eq!(budget.used_bytes(), 0);
    assert_eq!(budget.available_bytes(), 1024);

    {
        let r1 = budget.reserve(400).expect("reserve 400");
        assert_eq!(r1.bytes(), 400);
        assert_eq!(budget.used_bytes(), 400);
        assert_eq!(budget.available_bytes(), 624);

        let r2 = budget.reserve(624).expect("reserve remaining 624");
        assert_eq!(budget.used_bytes(), 1024);
        assert_eq!(budget.available_bytes(), 0);

        let err = budget.reserve(1).expect_err("quota should reject +1 byte");
        assert_eq!(err.category(), ErrorCategory::QuotaExceeded);
        assert_eq!(err.module_id(), "types");

        drop(r1);
        assert_eq!(budget.used_bytes(), 624);
        drop(r2);
    }

    assert_eq!(budget.used_bytes(), 0);
    assert_eq!(budget.available_bytes(), 1024);
}

#[test]
fn reservation_split_merge_grow_shrink_preserve_accounting() {
    let budget = BudgetHandle::new(BudgetDomain::Cache, 500);
    let mut r = budget.reserve(300).expect("reserve 300");

    let split = r.split(120).expect("split 120");
    assert_eq!(r.bytes(), 180);
    assert_eq!(split.bytes(), 120);
    assert_eq!(budget.used_bytes(), 300);

    r.try_grow(200).expect("grow to full 500");
    assert_eq!(r.bytes(), 380);
    assert_eq!(budget.used_bytes(), 500);

    let err = r.try_grow(1).expect_err("exceeds 500");
    assert_eq!(err.category(), ErrorCategory::QuotaExceeded);

    r.shrink(80);
    assert_eq!(r.bytes(), 300);
    assert_eq!(budget.used_bytes(), 420);

    r.merge(split).expect("merge back");
    assert_eq!(r.bytes(), 420);
    assert_eq!(budget.used_bytes(), 420);

    let other_budget = BudgetHandle::new(BudgetDomain::Cache, 500);
    let other_res = other_budget.reserve(10).expect("reserve other");
    let err = r
        .merge(other_res)
        .expect_err("cross-budget merge must fail");
    assert_eq!(err.category(), ErrorCategory::InvalidArgument);
    assert_eq!(other_budget.used_bytes(), 0);

    drop(r);
    assert_eq!(budget.used_bytes(), 0);
}

#[test]
fn concurrent_reservations_never_exceed_limit() {
    let budget = BudgetHandle::new(BudgetDomain::Transaction, 1000);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let b = budget.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..200 {
                if let Ok(res) = b.reserve(100) {
                    assert!(b.used_bytes() <= b.limit_bytes());
                    drop(res);
                }
            }
        }));
    }
    for h in handles {
        h.join().expect("thread join");
    }
    assert_eq!(budget.used_bytes(), 0);
}
