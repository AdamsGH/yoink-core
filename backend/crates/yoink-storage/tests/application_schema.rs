use std::{ops::Deref, time::Duration};

use tempfile::TempDir;
use yoink_storage::Store;

struct Fixture {
    store: Store,
    _directory: TempDir,
}

async fn new_store() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = directory.path().join("storage.db");
    let store = Store::open(&path, true, Duration::from_secs(5))
        .await
        .unwrap();
    Fixture {
        store,
        _directory: directory,
    }
}

impl Deref for Fixture {
    type Target = Store;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}
use yoink_db::{query, raw_sql};
use yoink_storage::StoreError;

async fn exec(store: &yoink_storage::Store, sql: &str) -> Result<(), yoink_db::Error> {
    raw_sql(sql).execute(store.database()).await
}

#[tokio::test]
async fn migration_is_idempotent_and_verification_is_read_only() {
    let fixture = new_store().await;
    fixture.migrate().await.unwrap();
    fixture.verify_schema().await.unwrap();
    fixture.migrate().await.unwrap();
    fixture.verify_schema().await.unwrap();

    let version_count: i64 = query("SELECT count(*) AS count FROM runtime_migrations")
        .fetch_one(fixture.database())
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(version_count, 15);
    let table_count: i64 = query(
        "SELECT count(*) AS count FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT GLOB '__turso_internal_*' AND name <> 'runtime_migrations'",
    ).fetch_one(fixture.database()).await.unwrap().try_get("count").unwrap();
    assert_eq!(table_count, 55);

    exec(&fixture, "CREATE TABLE schema_drift (id INTEGER)")
        .await
        .unwrap();
    assert_eq!(fixture.verify_schema().await, Err(StoreError::Schema));
    let count_after_verify: i64 =
        query("SELECT count(*) AS count FROM sqlite_master WHERE name = 'schema_drift'")
            .fetch_one(fixture.database())
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    assert_eq!(count_after_verify, 1);
    assert!(!StoreError::Schema.to_string().contains("schema_drift"));
}

#[tokio::test]
async fn migration_and_verification_reject_runtime_ledger_drift_without_mutation() {
    let fixture = new_store().await;
    fixture.migrate().await.unwrap();
    exec(
        &fixture,
        "ALTER TABLE runtime_migrations ADD COLUMN unexpected TEXT",
    )
    .await
    .unwrap();

    assert_eq!(fixture.verify_schema().await, Err(StoreError::Schema));
    assert_eq!(fixture.migrate().await, Err(StoreError::Schema));
    let unexpected_columns: i64 = query(
        "SELECT count(*) AS count FROM pragma_table_info('runtime_migrations') WHERE name = 'unexpected'",
    )
    .fetch_one(fixture.database())
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    let versions: i64 = query("SELECT count(*) AS count FROM runtime_migrations")
        .fetch_one(fixture.database())
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(unexpected_columns, 1);
    assert_eq!(versions, 15);
}

#[tokio::test]
async fn malformed_empty_ledger_and_extra_object_are_rejected_atomically() {
    let malformed = new_store().await;
    exec(&malformed, "CREATE TABLE runtime_migrations (version TEXT)")
        .await
        .unwrap();
    assert_eq!(malformed.migrate().await, Err(StoreError::Schema));
    let malformed_history: i64 = query(
        "SELECT count(*) AS count FROM sqlite_master WHERE type = 'table' AND name = 'runtime_migrations'",
    )
    .fetch_one(malformed.database())
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    let malformed_users: i64 = query(
        "SELECT count(*) AS count FROM sqlite_master WHERE type = 'table' AND name = 'users'",
    )
    .fetch_one(malformed.database())
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(malformed_history, 1);
    assert_eq!(malformed_users, 0);

    let extra = new_store().await;
    exec(&extra, "CREATE TABLE unexpected (id INTEGER)")
        .await
        .unwrap();
    assert_eq!(extra.migrate().await, Err(StoreError::Schema));
    let bootstrapped_ledger: i64 = query(
        "SELECT count(*) AS count FROM sqlite_master WHERE type = 'table' AND name = 'runtime_migrations'",
    )
    .fetch_one(extra.database())
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    let app_tables: i64 = query(
        "SELECT count(*) AS count FROM sqlite_master WHERE type = 'table' AND name = 'users'",
    )
    .fetch_one(extra.database())
    .await
    .unwrap()
    .try_get("count")
    .unwrap();
    assert_eq!(bootstrapped_ledger, 0);
    assert_eq!(app_tables, 0);
}

#[tokio::test]
async fn stats_search_triggers_track_raw_writes_tombstones_and_rollback() {
    let fixture = new_store().await;
    fixture.migrate().await.unwrap();
    exec(&fixture, "INSERT INTO stats_messages (id, message_id, chat_id, date, text) VALUES (1, 1, -1, 1, 'first')")
        .await.unwrap();
    exec(
        &fixture,
        "UPDATE stats_messages SET id = 2, message_id = 2, text = 'second' WHERE id = 1",
    )
    .await
    .unwrap();
    exec(&fixture, "DELETE FROM stats_messages WHERE id = 2")
        .await
        .unwrap();
    let dirty_ids = query("SELECT id FROM stats_search_dirty ORDER BY id")
        .fetch_all(fixture.database())
        .await
        .unwrap();
    assert_eq!(dirty_ids.len(), 2);
    assert_eq!(dirty_ids[0].try_get::<i64, _>("id").unwrap(), 1);
    assert_eq!(dirty_ids[1].try_get::<i64, _>("id").unwrap(), 2);
    let revision: i64 = query("SELECT source_revision FROM stats_search_state WHERE singleton = 1")
        .fetch_one(fixture.database())
        .await
        .unwrap()
        .try_get("source_revision")
        .unwrap();
    assert_eq!(revision, 3);

    exec(&fixture, "BEGIN IMMEDIATE; INSERT INTO stats_messages (id, message_id, chat_id, date, text) VALUES (3, 3, -1, 3, 'rolled back'); ROLLBACK;")
        .await.unwrap();
    let revision_after_rollback: i64 =
        query("SELECT source_revision FROM stats_search_state WHERE singleton = 1")
            .fetch_one(fixture.database())
            .await
            .unwrap()
            .try_get("source_revision")
            .unwrap();
    assert_eq!(revision_after_rollback, 3);
}

#[tokio::test]
async fn application_schema_enforces_constraints_views_indexes_and_foreign_keys() {
    let fixture = new_store().await;
    fixture.migrate().await.unwrap();

    assert!(
        exec(
            &fixture,
            "INSERT INTO users (username, created_at, updated_at) VALUES ('missing-id', 1, 1)"
        )
        .await
        .is_err()
    );
    exec(&fixture, "INSERT INTO users (id, username, first_name, created_at, updated_at) VALUES (101, 'alpha', 'Alpha User', 1, 1)").await.unwrap();
    exec(&fixture, "INSERT INTO users (id, username, first_name, created_at, updated_at) VALUES (102, 'beta', 'Beta User', 1, 1)").await.unwrap();

    assert!(
        exec(
            &fixture,
            "INSERT INTO users (id, role, created_at, updated_at) VALUES (103, 'not-a-role', 1, 1)"
        )
        .await
        .is_err()
    );
    assert!(
        exec(
            &fixture,
            "INSERT INTO users (id, is_premium, created_at, updated_at) VALUES (104, 2, 1, 1)"
        )
        .await
        .is_err()
    );
    assert!(exec(&fixture, "INSERT INTO events (plugin, event_type, payload, created_at) VALUES ('test', 'bad-json', '{', 1)").await.is_err());
    assert!(exec(&fixture, "INSERT INTO stats_user_names (id, user_id, date, username, display_name) VALUES (2147483648, 101, 1, 'overflow', 'Overflow')").await.is_err());

    exec(
        &fixture,
        "INSERT INTO groups (id, created_at) VALUES (201, 1)",
    )
    .await
    .unwrap();
    exec(
        &fixture,
        "INSERT INTO thread_policies (group_id, thread_id) VALUES (201, NULL)",
    )
    .await
    .unwrap();
    assert!(
        exec(
            &fixture,
            "INSERT INTO thread_policies (group_id, thread_id) VALUES (201, NULL)"
        )
        .await
        .is_err()
    );

    exec(
        &fixture,
        "INSERT INTO stats_messages (message_id, chat_id, date) VALUES (301, 201, 10)",
    )
    .await
    .unwrap();
    assert!(
        exec(
            &fixture,
            "INSERT INTO stats_messages (message_id, chat_id, date) VALUES (301, 201, 11)"
        )
        .await
        .is_err()
    );

    exec(&fixture, "INSERT INTO stats_user_names (user_id, date, username, display_name) VALUES (101, 10, 'old', 'Old Name')").await.unwrap();
    exec(&fixture, "INSERT INTO stats_user_names (user_id, date, username, display_name) VALUES (101, 20, 'new', 'New Name')").await.unwrap();
    let latest: String =
        query("SELECT display_name FROM stats_user_latest_name WHERE user_id = 101")
            .fetch_one(fixture.database())
            .await
            .unwrap()
            .try_get("display_name")
            .unwrap();
    assert_eq!(latest, "New Name");

    let indexes: i64 = query("SELECT count(*) AS count FROM sqlite_master WHERE type = 'index' AND name IN ('cookies_personal_unique', 'thread_policies_default_unique', 'idx_stats_msg_chat_date')")
        .fetch_one(fixture.database()).await.unwrap().try_get("count").unwrap();
    assert_eq!(indexes, 3);

    exec(&fixture, "INSERT INTO cookies (user_id, domain, content, created_at, updated_at) VALUES (101, 'example.test', 'opaque', 1, 1)").await.unwrap();
    exec(&fixture, "DELETE FROM users WHERE id = 101")
        .await
        .unwrap();
    let cascaded: i64 = query("SELECT count(*) AS count FROM cookies WHERE user_id = 101")
        .fetch_one(fixture.database())
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(cascaded, 0);

    exec(&fixture, "INSERT INTO inbox_categories (owner_user_id, name, normalized_name, slug, kind, parent_id, created_at) VALUES (102, 'Parent', 'parent', 'parent', 'user', NULL, 1)").await.unwrap();
    exec(&fixture, "INSERT INTO inbox_categories (owner_user_id, name, normalized_name, slug, kind, parent_id, created_at) VALUES (102, 'Child', 'child', 'child', 'user', 1, 1)").await.unwrap();
    exec(&fixture, "DELETE FROM inbox_categories WHERE id = 1")
        .await
        .unwrap();
    let parent_id: Option<i64> =
        query("SELECT parent_id FROM inbox_categories WHERE slug = 'child'")
            .fetch_one(fixture.database())
            .await
            .unwrap()
            .try_get("parent_id")
            .unwrap();
    assert_eq!(parent_id, None);
}
