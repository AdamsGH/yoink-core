use std::ops::Deref;
use std::time::Duration;

use tempfile::TempDir;
use yoink_storage::Store;

pub struct DiskStore {
    pub store: Store,
    _directory: TempDir,
}

impl Deref for DiskStore {
    type Target = Store;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl DiskStore {
    #[allow(dead_code)] // Not every integration target tests reopening.
    pub fn path(&self) -> std::path::PathBuf {
        self._directory.path().join("storage.db")
    }
}

pub async fn unmigrated_store() -> DiskStore {
    let directory = tempfile::tempdir().expect("create isolated Turso fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = directory.path().join("storage.db");
    let store = Store::open(&path, true, Duration::from_secs(5))
        .await
        .expect("open isolated Turso fixture");
    DiskStore {
        store,
        _directory: directory,
    }
}

#[allow(dead_code)] // Shared fixture: queue tests exercise unmigrated startup instead.
pub async fn store() -> DiskStore {
    let store = unmigrated_store().await;
    store.migrate().await.expect("initialize native schema");
    store.verify_schema().await.expect("verify native schema");
    store
}
