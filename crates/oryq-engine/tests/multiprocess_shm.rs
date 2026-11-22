//! Multi-process `.oryq-shm` coordination, WAL-reset exclusion, and real child-process
//! crash recovery integration tests (`X02` / Issue #17).
//!
//! Runs identically on Linux, macOS, and Windows using the self-re-exec pattern
//! (`std::env::current_exe()` + `ORYQ_SHM_WORKER_ROLE`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oryq_engine::{
    shm_path_for, BTree, EngineConfig, OsVfs, PageSize, SharedProcessConfig,
    SharedProcessCoordinator, StorageEngine, TreeKind,
};
use oryq_types::{ErrorCategory, TxId};

static DIR_SEQ: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "oryqdb_shm_{label}_{}_{seq}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn spawn_worker(role: &str, db_path: &Path, extra_arg: &str) -> std::process::Child {
    let exe = std::env::current_exe().expect("current test exe");
    Command::new(exe)
        .arg("--exact")
        .arg("multiprocess_worker_entrypoint")
        .arg("--nocapture")
        .env("ORYQ_SHM_WORKER_ROLE", role)
        .env("ORYQ_SHM_DB_PATH", db_path)
        .env("ORYQ_SHM_EXTRA", extra_arg)
        .spawn()
        .expect("spawn child test worker")
}

/// Child-process entrypoint invoked only when `ORYQ_SHM_WORKER_ROLE` is set.
#[test]
fn multiprocess_worker_entrypoint() {
    let Ok(role) = std::env::var("ORYQ_SHM_WORKER_ROLE") else {
        return;
    };
    let db_path = PathBuf::from(std::env::var("ORYQ_SHM_DB_PATH").expect("db path"));
    let extra = std::env::var("ORYQ_SHM_EXTRA").unwrap_or_default();

    let cfg = SharedProcessConfig {
        max_readers: 32,
        peer_lease_timeout: Duration::from_secs(10),
        lock_retry_interval: Duration::from_millis(2),
        lock_timeout: Duration::from_secs(10),
    };
    let coord = SharedProcessCoordinator::open(&db_path, TxId::INITIAL, cfg).unwrap();

    match role.as_str() {
        "writer" => {
            let rounds: u64 = extra.parse().unwrap_or(10);
            for _ in 0..rounds {
                let permit = coord.acquire_writer().unwrap();
                let next = TxId::new(permit.base_tx_id().raw() + 1);
                coord.publish_commit(&permit, next).unwrap();
                drop(permit);
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        "reader" => {
            let rounds: u64 = extra.parse().unwrap_or(10);
            let mut last_snap = 0u64;
            for _ in 0..rounds {
                let guard = coord.acquire_read_snapshot().unwrap();
                let snap = guard.snapshot_id().raw();
                assert!(
                    snap >= last_snap,
                    "snapshot regressed: {snap} < {last_snap}"
                );
                last_snap = snap;
                let horizon = coord.retention_horizon().unwrap().raw();
                assert!(
                    horizon <= snap,
                    "retention_horizon {horizon} exceeded pinned snapshot {snap}"
                );
                guard.refresh_lease().unwrap();
                std::thread::sleep(Duration::from_millis(2));
                drop(guard);
            }
        }
        "crash_writer" => {
            let permit = coord.acquire_writer().unwrap();
            let target_tx: u64 = extra.parse().unwrap_or(5);
            coord.publish_commit(&permit, TxId::new(target_tx)).unwrap();
            // Intentionally leak permit & exit abruptly without running Drop (simulates crash / SIGKILL)
            std::mem::forget(permit);
            std::process::exit(99);
        }
        "crash_reader" => {
            let guard = coord.acquire_read_snapshot().unwrap();
            let _ = guard.snapshot_id();
            // Intentionally leak guard & exit abruptly without running Drop (simulates crash / SIGKILL)
            std::mem::forget(guard);
            std::process::exit(99);
        }
        "crash_wal_reset" => {
            let permit = coord.acquire_writer().unwrap();
            let reset = coord
                .try_acquire_wal_reset(&permit)
                .unwrap()
                .expect("child should acquire WAL reset");
            // Intentionally leak both guards & exit abruptly (simulates crash mid-WAL-reset)
            std::mem::forget(reset);
            std::mem::forget(permit);
            std::process::exit(99);
        }
        "db_writer" => {
            let permit = coord.acquire_writer().unwrap();
            let vfs = Arc::new(OsVfs::new());
            let engine = StorageEngine::open(
                vfs,
                &db_path,
                EngineConfig {
                    page_size: PageSize::Size4096,
                    ..EngineConfig::default()
                },
                [0x44; 16],
            )
            .unwrap();
            let mut wtx = engine.begin_write().unwrap();
            let mut tree = BTree::open(wtx.catalog_root_page_id(), TreeKind::Collection);
            tree.insert(&mut wtx, b"child_key", extra.as_bytes())
                .unwrap();
            wtx.set_catalog_root_page_id(tree.root());
            let receipt = wtx.commit().unwrap();
            engine.checkpoint(true).unwrap();
            drop(engine);
            coord.publish_commit(&permit, receipt.tx_id).unwrap();
            drop(permit);
        }
        other => panic!("unknown worker role '{other}'"),
    }
}

#[test]
fn shm_reader_slots_horizon_wal_reset_exclusion_and_corrupt_recovery() {
    let dir = TempDir::new("invariants");
    let db_path = dir.path().join("inv.oryq");

    let cfg = SharedProcessConfig {
        max_readers: 2,
        peer_lease_timeout: Duration::from_secs(10),
        lock_retry_interval: Duration::from_millis(2),
        lock_timeout: Duration::from_millis(80),
    };
    let coord = SharedProcessCoordinator::open(&db_path, TxId::new(1), cfg.clone()).unwrap();

    // 1. Reader slot table bounds + horizon pinning
    let r1 = coord.acquire_read_snapshot().unwrap();
    assert_eq!(r1.snapshot_id(), TxId::new(1));
    assert_eq!(coord.active_reader_count().unwrap(), 1);

    // Writer commits TxId(2) while r1 pins TxId(1)
    let w1 = coord.acquire_writer().unwrap();
    assert_eq!(
        coord.try_acquire_writer().unwrap_err().category(),
        ErrorCategory::Busy
    );
    coord.publish_commit(&w1, TxId::new(2)).unwrap();
    assert_eq!(coord.latest_committed().unwrap(), TxId::new(2));
    assert_eq!(coord.retention_horizon().unwrap(), TxId::new(1));

    // Second reader pins TxId(2); 3rd reader hits max_readers=2 -> Busy
    let r2 = coord.acquire_read_snapshot().unwrap();
    assert_eq!(r2.snapshot_id(), TxId::new(2));
    assert_eq!(
        coord.acquire_read_snapshot().unwrap_err().category(),
        ErrorCategory::Busy
    );

    // 2. WAL reset excluded while any reader is active
    assert!(coord.try_acquire_wal_reset(&w1).unwrap().is_none());

    // Drop r1 -> horizon advances to r2 (TxId(2)), still blocked from WAL reset
    drop(r1);
    assert_eq!(coord.retention_horizon().unwrap(), TxId::new(2));
    assert!(coord.try_acquire_wal_reset(&w1).unwrap().is_none());

    // Drop r2 -> zero readers -> WAL reset succeeds and blocks new readers until dropped
    drop(r2);
    let reset_guard = coord
        .try_acquire_wal_reset(&w1)
        .unwrap()
        .expect("WAL reset should succeed with zero active readers");
    assert_eq!(reset_guard.wal_reset_epoch(), 1);
    assert_eq!(
        coord.acquire_read_snapshot().unwrap_err().category(),
        ErrorCategory::Busy
    );
    let info = coord.inspect().unwrap();
    assert_eq!(info.wal_reset_pid, Some(std::process::id()));
    assert_eq!(info.writer_pid, Some(std::process::id()));

    drop(reset_guard);
    drop(w1);

    // New reader succeeds immediately after WAL reset completes
    let r3 = coord.acquire_read_snapshot().unwrap();
    assert_eq!(r3.snapshot_id(), TxId::new(2));
    drop(r3);

    // 3. Corrupt `.oryq-shm` file is deterministically reinitialized when zero peers hold locks,
    //    and fails closed with Corruption when a live peer holds a lock.
    {
        let live_reader = coord.acquire_read_snapshot().unwrap();
        std::fs::write(shm_path_for(&db_path), b"CORRUPT_WHILE_ACTIVE").unwrap();
        assert_eq!(
            SharedProcessCoordinator::open(&db_path, TxId::new(2), cfg.clone())
                .unwrap_err()
                .category(),
            ErrorCategory::Corruption
        );
        drop(live_reader);
    }
    let recovered = SharedProcessCoordinator::open(&db_path, TxId::new(2), cfg.clone()).unwrap();
    assert_eq!(recovered.latest_committed().unwrap(), TxId::new(2));
    assert_eq!(recovered.active_reader_count().unwrap(), 0);

    // 4. `publish_commit` rejects tx_id regression and cross-database writer permits
    let other_path = dir.path().join("other.oryq");
    let other_coord =
        SharedProcessCoordinator::open(&other_path, TxId::new(1), cfg.clone()).unwrap();
    let other_permit = other_coord.acquire_writer().unwrap();
    assert_eq!(
        recovered
            .publish_commit(&other_permit, TxId::new(3))
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(
        recovered
            .try_acquire_wal_reset(&other_permit)
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    drop(other_permit);

    let p = recovered.acquire_writer().unwrap();
    assert_eq!(
        recovered
            .publish_commit(&p, TxId::new(1))
            .unwrap_err()
            .category(),
        ErrorCategory::InvalidArgument
    );
    drop(p);

    // 5. Evicted reader `refresh_lease()` fails closed with `SnapshotTooOld`
    let short_lease_path = dir.path().join("short_lease.oryq");
    let short_cfg = SharedProcessConfig {
        max_readers: 4,
        peer_lease_timeout: Duration::from_millis(25),
        lock_retry_interval: Duration::from_millis(2),
        lock_timeout: Duration::from_secs(2),
    };
    let short_coord =
        SharedProcessCoordinator::open(&short_lease_path, TxId::new(1), short_cfg).unwrap();
    let stale_guard = short_coord.acquire_read_snapshot().unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let report = short_coord.reclaim_stale_peers().unwrap();
    assert_eq!(report.reclaimed_readers, 1);
    assert_eq!(
        stale_guard.refresh_lease().unwrap_err().category(),
        ErrorCategory::SnapshotTooOld
    );
    drop(stale_guard);
}

#[test]
fn multiprocess_concurrent_readers_writers_and_wal_reset() {
    let dir = TempDir::new("mp_rw");
    let db_path = dir.path().join("shared.oryq");

    let cfg = SharedProcessConfig {
        max_readers: 32,
        peer_lease_timeout: Duration::from_secs(10),
        lock_retry_interval: Duration::from_millis(2),
        lock_timeout: Duration::from_secs(10),
    };
    let coord = SharedProcessCoordinator::open(&db_path, TxId::INITIAL, cfg).unwrap();

    let mut children = vec![
        spawn_worker("writer", &db_path, "10"),
        spawn_worker("writer", &db_path, "10"),
        spawn_worker("reader", &db_path, "10"),
        spawn_worker("reader", &db_path, "10"),
    ];

    for child in &mut children {
        let status = child.wait().expect("wait child");
        assert!(status.success(), "child exited with status: {status}");
    }

    let info = coord.inspect().unwrap();
    assert_eq!(info.latest_committed_tx_id, TxId::new(20));
    assert_eq!(info.active_readers, 0);
    assert_eq!(info.writer_pid, None);
    assert_eq!(info.retention_horizon, TxId::new(20));

    // After all peer processes finish, WAL reset acquires cleanly
    let permit = coord.acquire_writer().unwrap();
    let reset = coord
        .try_acquire_wal_reset(&permit)
        .unwrap()
        .expect("WAL reset should acquire cleanly after all readers exit");
    assert_eq!(reset.wal_reset_epoch(), 1);
    drop(reset);
    drop(permit);
}

#[test]
fn multiprocess_crash_recovery_reclaims_dead_writer_and_reader() {
    let dir = TempDir::new("mp_crash");
    let db_path = dir.path().join("crash.oryq");

    let cfg = SharedProcessConfig {
        max_readers: 32,
        peer_lease_timeout: Duration::from_secs(10),
        lock_retry_interval: Duration::from_millis(2),
        lock_timeout: Duration::from_secs(10),
    };
    let coord = SharedProcessCoordinator::open(&db_path, TxId::INITIAL, cfg).unwrap();

    // 1. Spawn a writer child that publishes TxId(5) and crashes abruptly (exit 99, no Drop)
    let mut crashed_writer = spawn_worker("crash_writer", &db_path, "5");
    let writer_pid = crashed_writer.id();
    let status = crashed_writer.wait().unwrap();
    assert_eq!(status.code(), Some(99));

    // Before reclamation, `.oryq-shm` still shows the dead writer's PID
    let before = coord.inspect().unwrap();
    assert_eq!(before.writer_pid, Some(writer_pid));
    assert_eq!(before.latest_committed_tx_id, TxId::new(5));

    // Reclaim dead writer via OS kernel lock probe -> deterministic recovery
    let report = coord.reclaim_stale_peers().unwrap();
    assert!(report.reclaimed_writer);
    assert_eq!(coord.inspect().unwrap().writer_pid, None);

    // Parent can immediately acquire writer lock and publish TxId(6)
    let permit = coord.try_acquire_writer().unwrap();
    coord.publish_commit(&permit, TxId::new(6)).unwrap();
    drop(permit);

    // 2. Spawn a reader child that pins TxId(6) and crashes abruptly (exit 99, no Drop)
    let mut crashed_reader = spawn_worker("crash_reader", &db_path, "");
    let status = crashed_reader.wait().unwrap();
    assert_eq!(status.code(), Some(99));

    // Publish TxId(7) -> horizon is still pinned at TxId(6) by the dead reader slot
    let permit = coord.acquire_writer().unwrap();
    coord.publish_commit(&permit, TxId::new(7)).unwrap();
    drop(permit);
    assert_eq!(coord.active_reader_count().unwrap(), 1);
    assert_eq!(coord.retention_horizon().unwrap(), TxId::new(6));

    // Reclaim dead reader -> slot cleared, horizon advances to TxId(7), WAL reset succeeds
    let report = coord.reclaim_stale_peers().unwrap();
    assert_eq!(report.reclaimed_readers, 1);
    assert_eq!(coord.active_reader_count().unwrap(), 0);
    assert_eq!(coord.retention_horizon().unwrap(), TxId::new(7));

    let permit = coord.acquire_writer().unwrap();
    assert!(coord.try_acquire_wal_reset(&permit).unwrap().is_some());
    drop(permit);

    // 2b. Spawn a child that crashes mid-WAL-reset (leaking both wlock & rlock metadata)
    let mut crashed_reset = spawn_worker("crash_wal_reset", &db_path, "");
    assert_eq!(crashed_reset.wait().unwrap().code(), Some(99));
    let before_reset = coord.inspect().unwrap();
    assert!(before_reset.writer_pid.is_some());
    assert!(before_reset.wal_reset_pid.is_some());
    let reset_report = coord.reclaim_stale_peers().unwrap();
    assert!(reset_report.reclaimed_writer);
    assert!(reset_report.reclaimed_wal_reset);
    assert_eq!(coord.inspect().unwrap().wal_reset_pid, None);

    // 2c. Crashed reader alongside a LIVE reader: live reader survives while crashed reader is reclaimed
    let live_cfg = SharedProcessConfig {
        max_readers: 32,
        peer_lease_timeout: Duration::from_millis(40),
        lock_retry_interval: Duration::from_millis(2),
        lock_timeout: Duration::from_secs(5),
    };
    let live_coord = SharedProcessCoordinator::open(&db_path, TxId::INITIAL, live_cfg).unwrap();
    let live_reader = live_coord.acquire_read_snapshot().unwrap();
    let mut dead_reader = spawn_worker("crash_reader", &db_path, "");
    assert_eq!(dead_reader.wait().unwrap().code(), Some(99));
    std::thread::sleep(Duration::from_millis(60));
    live_reader.refresh_lease().unwrap();
    let partial_report = live_coord.reclaim_stale_peers().unwrap();
    assert_eq!(partial_report.reclaimed_readers, 1);
    assert_eq!(live_coord.active_reader_count().unwrap(), 1);
    drop(live_reader);
    assert_eq!(live_coord.active_reader_count().unwrap(), 0);

    // 3. Real `.oryq` DB handoff across processes under SharedWriterPermit
    let handoff_path = dir.path().join("handoff.oryq");
    let handoff_coord = SharedProcessCoordinator::open(
        &handoff_path,
        TxId::INITIAL,
        SharedProcessConfig::default(),
    )
    .unwrap();
    let mut db_child = spawn_worker("db_writer", &handoff_path, "hello_from_child_proc");
    assert!(db_child.wait().unwrap().success());
    assert_eq!(handoff_coord.latest_committed().unwrap(), TxId::new(1));

    let permit = handoff_coord.acquire_writer().unwrap();
    let vfs = Arc::new(OsVfs::new());
    let engine = StorageEngine::open(
        vfs,
        &handoff_path,
        EngineConfig {
            page_size: PageSize::Size4096,
            ..EngineConfig::default()
        },
        [0x44; 16],
    )
    .unwrap();
    let rtx = engine.begin_read().unwrap();
    let tree = BTree::open(rtx.catalog_root_page_id(), TreeKind::Collection);
    assert_eq!(
        tree.get(&rtx, b"child_key").unwrap().as_deref(),
        Some(b"hello_from_child_proc".as_slice())
    );
    drop(rtx);
    drop(engine);
    drop(permit);
}
