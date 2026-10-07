use std::time::Duration;

use yoink_db::{Database, query, raw_sql};

use super::{MIGRATIONS, checksum, schema_hash};
use crate::{StoreError, store::Store};

async fn version_nineteen_store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let database = Database::open(
        &directory.path().join("inbox-upgrade.db"),
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
        for migration in &MIGRATIONS[..19] {
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
    query(include_str!("../sql/test_inbox_upgrade_snapshot.sql"))
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get("snapshot")
        .unwrap()
}

async fn jobs(store: &Store) -> Vec<(i64, i64, String, String, i64, i64)> {
    query("SELECT user_id,item_id,kind,state,available_at,created_at FROM inbox_jobs ORDER BY item_id")
        .fetch_all(store.database())
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            (
                row.try_get("user_id").unwrap(),
                row.try_get("item_id").unwrap(),
                row.try_get("kind").unwrap(),
                row.try_get("state").unwrap(),
                row.try_get("available_at").unwrap(),
                row.try_get("created_at").unwrap(),
            )
        })
        .collect()
}

fn expected_jobs() -> Vec<(i64, i64, String, String, i64, i64)> {
    vec![
        (41, 101, "enrich".into(), "ready".into(), 110, 110),
        (41, 102, "classify".into(), "ready".into(), 120, 120),
    ]
}

#[tokio::test]
async fn inbox_upgrade_preserves_metadata_and_backfills_only_active_work() {
    let (_directory, store) = version_nineteen_store().await;
    raw_sql(include_str!("../sql/test_inbox_upgrade_fixture.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let before = snapshot(&store).await;
    store.migrate().await.unwrap();
    store.verify_schema().await.unwrap();
    assert_eq!(snapshot(&store).await, before);
    assert_eq!(jobs(&store).await, expected_jobs());
    store.migrate().await.unwrap();
    assert_eq!(jobs(&store).await, expected_jobs());
    assert_eq!(snapshot(&store).await, before);
    let versions = query("SELECT version FROM runtime_migrations ORDER BY version")
        .fetch_all(store.database())
        .await
        .unwrap();
    assert_eq!(versions.len(), 22);
    for (row, migration) in versions.iter().zip(&MIGRATIONS) {
        assert_eq!(row.try_get::<i64, _>("version").unwrap(), migration.version);
    }
    raw_sql("DROP INDEX inbox_gh_mutations_active")
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(store.verify_schema().await, Err(StoreError::Schema));
    assert_eq!(store.migrate().await, Err(StoreError::Schema));
}

#[tokio::test]
async fn post_import_admission_runs_after_rows_arrive_and_is_transactional() {
    let (_directory, store) = version_nineteen_store().await;
    store.migrate().await.unwrap();
    assert!(jobs(&store).await.is_empty());
    {
        let mut connection = store.database().connection().await.unwrap();
        let transaction = turso::transaction::Transaction::new(
            &mut connection,
            turso::transaction::TransactionBehavior::Immediate,
        )
        .await
        .unwrap();
        raw_sql(include_str!("../sql/test_inbox_upgrade_fixture.sql"))
            .execute(&*transaction)
            .await
            .unwrap();
        raw_sql(include_str!("../../yoink-inbox/sql/backfill_jobs.sql"))
            .execute(&*transaction)
            .await
            .unwrap();
        transaction.rollback().await.unwrap();
    }
    assert!(jobs(&store).await.is_empty());
    raw_sql(include_str!("../sql/test_inbox_upgrade_fixture.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let before = snapshot(&store).await;
    for _ in 0..2 {
        raw_sql(include_str!("../../yoink-inbox/sql/backfill_jobs.sql"))
            .execute(store.database())
            .await
            .unwrap();
    }
    assert_eq!(jobs(&store).await, expected_jobs());
    assert_eq!(snapshot(&store).await, before);
    store.verify_schema().await.unwrap();
}

#[tokio::test]
async fn invalid_imported_owner_rolls_back_the_entire_inbox_upgrade() {
    let (_directory, store) = version_nineteen_store().await;
    raw_sql(include_str!("../sql/test_inbox_upgrade_fixture.sql"))
        .execute(store.database())
        .await
        .unwrap();
    raw_sql("PRAGMA foreign_keys=OFF; DELETE FROM users WHERE id=41; PRAGMA foreign_keys=ON;")
        .execute(store.database())
        .await
        .unwrap();
    let before = snapshot(&store).await;
    assert_eq!(store.migrate().await, Err(StoreError::Schema));
    assert_eq!(snapshot(&store).await, before);
    let count: i64 = query("SELECT count(*) FROM runtime_migrations")
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(count, 19);
    assert!(
        query("SELECT name FROM sqlite_master WHERE name='inbox_jobs'")
            .fetch_optional(store.database())
            .await
            .unwrap()
            .is_none()
    );
}
