use std::time::Duration;

use yoink_db::{Database, query, raw_sql};

use super::{MIGRATIONS, checksum, schema_hash};
use crate::{StoreError, store::Store};

async fn version_twenty_three_store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let database = Database::open(
        &directory.path().join("insight-upgrade.db"),
        true,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    {
        let connection = database.connection().await.unwrap();
        raw_sql(include_str!("../sql/create_schema.sql"))
            .execute(&*connection)
            .await
            .unwrap();
        for migration in &MIGRATIONS[..23] {
            raw_sql(migration.sql).execute(&*connection).await.unwrap();
            let fingerprint = schema_hash(&connection, migration.expected_tables)
                .await
                .unwrap();
            query(include_str!("../sql/record_migration.sql"))
                .bind(migration.version)
                .bind(checksum(migration))
                .bind(fingerprint)
                .bind(chrono::Utc::now())
                .execute(&*connection)
                .await
                .unwrap();
        }
    }
    (directory, Store::from_database(database))
}

async fn snapshot(store: &Store) -> String {
    query(include_str!("../sql/test_insight_upgrade_snapshot.sql"))
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get("snapshot")
        .unwrap()
}

#[tokio::test]
async fn insight_upgrade_is_explicit_additive_and_idempotent() {
    let (_directory, store) = version_twenty_three_store().await;
    raw_sql(include_str!("../sql/test_insight_upgrade_fixture.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let before = snapshot(&store).await;
    assert!(matches!(
        store.verify_schema().await,
        Err(StoreError::Schema)
    ));
    store.migrate().await.unwrap();
    store.verify_schema().await.unwrap();
    assert_eq!(snapshot(&store).await, before);
    store.migrate().await.unwrap();
    assert_eq!(snapshot(&store).await, before);
    let versions = query("SELECT version FROM runtime_migrations ORDER BY version")
        .fetch_all(store.database())
        .await
        .unwrap();
    assert_eq!(versions.len(), MIGRATIONS.len());
    for (row, migration) in versions.iter().zip(&MIGRATIONS) {
        assert_eq!(row.try_get::<i64, _>("version").unwrap(), migration.version);
    }
    for table in ["insight_native_cache", "insight_native_reservations"] {
        let exists = query("SELECT name FROM sqlite_schema WHERE type='table' AND name=?")
            .bind(table)
            .fetch_optional(store.database())
            .await
            .unwrap();
        assert!(exists.is_some(), "missing {table}");
    }
    raw_sql("DROP INDEX insight_native_reservations_user")
        .execute(store.database())
        .await
        .unwrap();
    assert!(matches!(
        store.verify_schema().await,
        Err(StoreError::Schema)
    ));
    assert!(matches!(store.migrate().await, Err(StoreError::Schema)));
}
