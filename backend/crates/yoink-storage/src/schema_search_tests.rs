use super::{MIGRATIONS, checksum, schema_hash};
use crate::{Store, StoreError};
use std::time::Duration;
use yoink_db::{Database, query, raw_sql};

#[tokio::test]
async fn version_two_messages_are_backfilled_and_triggers_are_fingerprinted() {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let database = Database::open(&dir.path().join("v2.db"), true, Duration::from_secs(5))
        .await
        .unwrap();
    let connection = database.connection().await.unwrap();
    raw_sql(include_str!("../sql/create_schema.sql"))
        .execute(&*connection)
        .await
        .unwrap();
    for migration in MIGRATIONS.iter().take(2) {
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
    drop(connection);
    let store = Store::from_database(database);
    raw_sql("INSERT INTO stats_messages(id, message_id, chat_id, date, text) VALUES (-2147483648, -9223372036854775808, -1, -1, 'before'), (2147483647, 9223372036854775807, -1, 1, 'after');")
        .execute(store.database()).await.unwrap();
    assert_eq!(store.verify_schema().await, Err(StoreError::Schema));
    store.migrate().await.unwrap();
    store.verify_schema().await.unwrap();
    let rows = query("SELECT id FROM stats_search_dirty ORDER BY id")
        .fetch_all(store.database())
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].try_get::<i64, _>("id").unwrap(),
        i64::from(i32::MIN)
    );
    assert_eq!(
        rows[1].try_get::<i64, _>("id").unwrap(),
        i64::from(i32::MAX)
    );
    store.migrate().await.unwrap();
    raw_sql("DROP TRIGGER stats_messages_search_update")
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(store.verify_schema().await, Err(StoreError::Schema));
}
