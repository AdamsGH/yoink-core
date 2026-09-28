#[cfg(unix)]
fn secure_directory(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) {}

use std::{path::Path, time::Duration};

use yoink_db::{Database, query, raw_sql};
use yoink_identity::IdentityStore;

#[tokio::test]
async fn native_identity_snapshot_uses_a_disk_database() {
    let directory = tempfile::tempdir().unwrap();
    secure_directory(directory.path());
    let database_path = directory.path().join("identity.db");
    let database = Database::open(&database_path, true, Duration::from_secs(2))
        .await
        .unwrap();
    raw_sql(include_str!("../src/sql/test_schema.sql"))
        .execute(&database)
        .await
        .unwrap();
    query("INSERT INTO users (id, role, language) VALUES (?1, ?2, ?3)")
        .bind(101_i64)
        .bind("user")
        .bind("en")
        .execute(&database)
        .await
        .unwrap();

    let snapshot = IdentityStore::new(database, None, false)
        .snapshot(101, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.principal.user_id, 101);
    assert_eq!(snapshot.language, "en");
    assert!(database_path.is_file());
}
