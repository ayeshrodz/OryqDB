mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_engine::{EngineConfig, HeaderSlot, OsVfs, Pager, TxId};
use oryq_types::ErrorCategory;
use support::fault_io::{FaultVfs, FaultVfsIo};

const TEST_UUID: [u8; 16] = [0x42; 16];

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("oryqdb_{label}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn os_vfs_rejects_duplicate_and_alias_opens_until_closed() {
    let dir = unique_temp_dir("locking");
    let db_path = dir.join("main.oryq");
    let alias_path = dir.join(".").join("main.oryq");

    let vfs = Arc::new(OsVfs::new());
    let cfg = EngineConfig::default();

    let pager = Pager::create_new(vfs.clone(), &db_path, &cfg, TEST_UUID).unwrap();

    // Second open via exact path fails with LockedByProcess
    let err1 = Pager::open_existing(vfs.clone(), &db_path, &cfg).unwrap_err();
    assert_eq!(err1.category(), ErrorCategory::LockedByProcess);

    // Second open via relative alias fails with LockedByProcess
    let err2 = Pager::open_existing(vfs.clone(), &alias_path, &cfg).unwrap_err();
    assert_eq!(err2.category(), ErrorCategory::LockedByProcess);

    // On Unix, a symlink to the same physical inode must also be rejected
    #[cfg(unix)]
    {
        let link_path = dir.join("symlink.oryq");
        std::os::unix::fs::symlink(&db_path, &link_path).unwrap();
        let err3 = Pager::open_existing(vfs.clone(), &link_path, &cfg).unwrap_err();
        assert_eq!(err3.category(), ErrorCategory::LockedByProcess);
    }

    // Closing (dropping) releases both FileRegistry lease and OS lock
    drop(pager);
    let reopened = Pager::open_existing(vfs, &db_path, &cfg).unwrap();
    assert_eq!(reopened.header().database_uuid, TEST_UUID);
    drop(reopened);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn commit_header_propagates_sync_fault_without_advancing_active_slot() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = std::path::Path::new("/mem/fault_sync.oryq");
    let cfg = EngineConfig::default();

    let mut pager = Pager::create_new(io, path, &cfg, TEST_UUID).unwrap();
    assert_eq!(pager.active_slot(), HeaderSlot::SlotA);

    vfs.fail_next_sync();
    pager.header_mut().checkpointed_tx_id = TxId::new(99);
    let err = pager.commit_header().unwrap_err();
    assert_eq!(err.category(), ErrorCategory::Io);
    // active_slot did not flip because pre-header sync failed
    assert_eq!(pager.active_slot(), HeaderSlot::SlotA);
}
