#![cfg(unix)]

use std::time::Duration;

use yoink_db::{Database, Error};

#[tokio::test]
async fn health_detects_removed_replaced_files_and_permission_drift() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("data.db");
    let database = Database::open(&path, true, Duration::from_secs(2))
        .await
        .unwrap();
    database.verify_files().await.unwrap();

    let saved = directory.path().join("saved.db");
    std::fs::rename(&path, &saved).unwrap();
    assert!(matches!(
        database.verify_files().await,
        Err(Error::InvalidPath)
    ));
    std::fs::write(&path, []).unwrap();
    assert!(matches!(
        database.verify_files().await,
        Err(Error::InvalidPath)
    ));
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&saved, &path).unwrap();
    database.verify_files().await.unwrap();

    let lock = directory.path().join("data.db.yoink-lock");
    let saved_lock = directory.path().join("saved.lock");
    std::fs::rename(&lock, &saved_lock).unwrap();
    assert!(matches!(
        database.verify_files().await,
        Err(Error::InvalidPath)
    ));
    std::fs::write(&lock, []).unwrap();
    assert!(matches!(
        database.verify_files().await,
        Err(Error::InvalidPath)
    ));
    std::fs::remove_file(&lock).unwrap();
    std::fs::rename(&saved_lock, &lock).unwrap();
    database.verify_files().await.unwrap();

    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        database.verify_files().await,
        Err(Error::InsecureDirectory)
    ));
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    database.verify_files().await.unwrap();
    database.close().await.unwrap();
    assert!(matches!(database.verify_files().await, Err(Error::Closed)));
}
