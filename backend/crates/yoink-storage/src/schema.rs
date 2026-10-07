use sha2::{Digest, Sha256};
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_db::{Database, query, raw_sql};

use crate::StoreError;

struct Migration {
    version: i64,
    sql: &'static str,
    expected_tables: usize,
}

const MIGRATIONS: [Migration; 22] = [
    Migration {
        version: 1,
        sql: include_str!("../migrations-turso/0001_runtime.sql"),
        expected_tables: 4,
    },
    Migration {
        version: 2,
        sql: include_str!("../migrations-turso/0002_application.sql"),
        expected_tables: 45,
    },
    Migration {
        version: 3,
        sql: include_str!("../migrations-turso/0003_stats_search.sql"),
        expected_tables: 47,
    },
    Migration {
        version: 4,
        sql: include_str!("../migrations-turso/0004_fk_rowid_indexes.sql"),
        expected_tables: 47,
    },
    Migration {
        version: 5,
        sql: include_str!("../migrations-turso/0005_progress_notices.sql"),
        expected_tables: 48,
    },
    Migration {
        version: 6,
        sql: include_str!("../migrations-turso/0006_cut_sessions.sql"),
        expected_tables: 49,
    },
    Migration {
        version: 7,
        sql: include_str!("../migrations-turso/0007_media_cache_clear.sql"),
        expected_tables: 50,
    },
    Migration {
        version: 8,
        sql: include_str!("../migrations-turso/0008_personal_cookie_mutation.sql"),
        expected_tables: 51,
    },
    Migration {
        version: 9,
        sql: include_str!("../migrations-turso/0009_quality_sessions.sql"),
        expected_tables: 52,
    },
    Migration {
        version: 10,
        sql: include_str!("../migrations-turso/0010_cut_quality.sql"),
        expected_tables: 52,
    },
    Migration {
        version: 11,
        sql: include_str!("../migrations-turso/0011_quality_audio.sql"),
        expected_tables: 52,
    },
    Migration {
        version: 12,
        sql: include_str!("../migrations-turso/0012_quality_cut_handoff.sql"),
        expected_tables: 52,
    },
    Migration {
        version: 13,
        sql: include_str!("../migrations-turso/0013_cut_segments.sql"),
        expected_tables: 52,
    },
    Migration {
        version: 14,
        sql: include_str!("../migrations-turso/0014_native_membership.sql"),
        expected_tables: 56,
    },
    Migration {
        version: 15,
        sql: include_str!("../migrations-turso/0015_download_quotas.sql"),
        expected_tables: 56,
    },
    Migration {
        version: 16,
        sql: include_str!("../migrations-turso/0016_content_policy.sql"),
        expected_tables: 56,
    },
    Migration {
        version: 17,
        sql: include_str!("../migrations-turso/0017_nsfw_marks.sql"),
        expected_tables: 58,
    },
    Migration {
        version: 18,
        sql: include_str!("../migrations-turso/0018_deferred_media.sql"),
        expected_tables: 61,
    },
    Migration {
        version: 19,
        sql: include_str!("../migrations-turso/0019_cookie_web.sql"),
        expected_tables: 63,
    },
    Migration {
        version: 20,
        sql: concat!(
            include_str!("../../yoink-inbox/sql/durable_jobs.sql"),
            "\n",
            include_str!("../../yoink-inbox/sql/backfill_jobs.sql"),
        ),
        expected_tables: 64,
    },
    Migration {
        version: 21,
        sql: include_str!("../../yoink-inbox/sql/github_mutations.sql"),
        expected_tables: 65,
    },
    Migration {
        version: 22,
        sql: include_str!("../../yoink-inbox/sql/notification_outbox.sql"),
        expected_tables: 66,
    },
];

pub(crate) async fn migrate(database: &Database) -> Result<(), StoreError> {
    let mut connection = database
        .connection()
        .await
        .map_err(|_| StoreError::Database)?;
    let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
        .await
        .map_err(|_| StoreError::Database)?;

    raw_sql(include_str!("../sql/create_schema.sql"))
        .execute(&*tx)
        .await
        .map_err(|_| StoreError::Schema)?;
    validate_ledger_schema(&tx).await?;
    let history = read_history(&tx).await?;
    let applied = validate_history(&history)?;
    if applied == 0 {
        validate_empty_schema(&tx).await?;
    }
    if applied > 0 {
        validate_fingerprint(&tx, &history[applied - 1], &MIGRATIONS[applied - 1]).await?;
    }

    for migration in &MIGRATIONS[applied..] {
        raw_sql(migration.sql)
            .execute(&*tx)
            .await
            .map_err(|_| StoreError::Schema)?;
        let fingerprint = schema_hash(&tx, migration.expected_tables).await?;
        query(include_str!("../sql/record_migration.sql"))
            .bind(migration.version)
            .bind(checksum(migration))
            .bind(fingerprint)
            .bind(chrono::Utc::now())
            .execute(&*tx)
            .await
            .map_err(|_| StoreError::Schema)?;
    }
    verify_connection(&tx).await?;
    tx.commit().await.map_err(|_| StoreError::Database)
}

pub(crate) async fn verify(database: &Database) -> Result<(), StoreError> {
    let mut connection = database
        .connection()
        .await
        .map_err(|_| StoreError::Database)?;
    let tx = Transaction::new(&mut connection, TransactionBehavior::Deferred)
        .await
        .map_err(|_| StoreError::Database)?;
    verify_connection(&tx).await?;
    tx.commit().await.map_err(|_| StoreError::Database)
}

async fn read_history(connection: &turso::Connection) -> Result<Vec<yoink_db::Row>, StoreError> {
    query(include_str!("../sql/verify_migration_history.sql"))
        .fetch_all(connection)
        .await
        .map_err(|_| StoreError::Schema)
}

fn validate_history(history: &[yoink_db::Row]) -> Result<usize, StoreError> {
    if history.len() > MIGRATIONS.len() {
        return Err(StoreError::Schema);
    }
    for (index, row) in history.iter().enumerate() {
        let migration = &MIGRATIONS[index];
        if row
            .try_get::<i64, _>("version")
            .map_err(|_| StoreError::Schema)?
            != migration.version
            || !row
                .try_get::<bool, _>("success")
                .map_err(|_| StoreError::Schema)?
            || row
                .try_get::<String, _>("checksum")
                .map_err(|_| StoreError::Schema)?
                != checksum(migration)
        {
            return Err(StoreError::Schema);
        }
    }
    Ok(history.len())
}

async fn verify_connection(connection: &turso::Connection) -> Result<(), StoreError> {
    validate_ledger_schema(connection).await?;
    let history = read_history(connection).await?;
    let applied = validate_history(&history)?;
    if applied != MIGRATIONS.len() {
        return Err(StoreError::Schema);
    }
    validate_fingerprint(connection, &history[applied - 1], &MIGRATIONS[applied - 1]).await
}

async fn validate_ledger_schema(connection: &turso::Connection) -> Result<(), StoreError> {
    let sql: Option<String> =
        query("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'runtime_migrations'")
            .fetch_optional(connection)
            .await
            .map_err(|_| StoreError::Schema)?
            .and_then(|row| row.try_get("sql").ok());
    let Some(sql) = sql else {
        return Err(StoreError::Schema);
    };
    let expected = include_str!("../sql/create_schema.sql");
    let expected = expected
        .split_once('(')
        .and_then(|(prefix, rest)| {
            rest.rsplit_once(')')
                .map(|(body, _)| format!("{prefix}({body})"))
        })
        .ok_or(StoreError::Schema)?;
    if normalize_ddl(&sql) != normalize_ddl(&expected) {
        return Err(StoreError::Schema);
    }
    Ok(())
}

fn normalize_ddl(sql: &str) -> String {
    sql.replace("IF NOT EXISTS", "")
        .chars()
        .filter(|character| !character.is_whitespace() && *character != ';')
        .collect()
}

async fn validate_empty_schema(connection: &turso::Connection) -> Result<(), StoreError> {
    let rows = query(include_str!("../sql/verify_schema.sql"))
        .fetch_all(connection)
        .await
        .map_err(|_| StoreError::Schema)?;
    if rows.len() != 1
        || rows[0]
            .try_get::<String, _>("type")
            .map_err(|_| StoreError::Schema)?
            != "table"
        || rows[0]
            .try_get::<String, _>("name")
            .map_err(|_| StoreError::Schema)?
            != "runtime_migrations"
    {
        return Err(StoreError::Schema);
    }
    Ok(())
}

async fn validate_fingerprint(
    connection: &turso::Connection,
    row: &yoink_db::Row,
    migration: &Migration,
) -> Result<(), StoreError> {
    let expected = row
        .try_get::<String, _>("schema_hash")
        .map_err(|_| StoreError::Schema)?;
    if expected != schema_hash(connection, migration.expected_tables).await? {
        return Err(StoreError::Schema);
    }
    Ok(())
}

fn checksum(migration: &Migration) -> String {
    format!("{:x}", Sha256::digest(migration.sql.as_bytes()))
}

async fn schema_hash(
    connection: &turso::Connection,
    expected_tables: usize,
) -> Result<String, StoreError> {
    let rows = query(include_str!("../sql/verify_schema.sql"))
        .fetch_all(connection)
        .await
        .map_err(|_| StoreError::Schema)?;
    let mut digest = Sha256::new();
    let mut tables = 0;
    for row in rows {
        let kind: String = row.try_get("type").map_err(|_| StoreError::Schema)?;
        tables += usize::from(kind == "table");
        for column in ["type", "name", "tbl_name", "sql"] {
            let value: Option<String> = row.try_get(column).map_err(|_| StoreError::Schema)?;
            match value {
                Some(value) => {
                    digest.update([1]);
                    digest.update((value.len() as u64).to_le_bytes());
                    digest.update(value.as_bytes());
                }
                None => digest.update([0]),
            }
        }
    }
    if tables != expected_tables {
        return Err(StoreError::Schema);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
#[path = "schema_search_tests.rs"]
mod search_tests;

#[cfg(test)]
#[path = "schema_inbox_tests.rs"]
mod inbox_tests;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use yoink_db::{Database, query, raw_sql};

    use super::*;
    use crate::store::Store;

    async fn version_one_store() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let path = directory.path().join("schema.db");
        let database = Database::open(&path, true, Duration::from_secs(5))
            .await
            .unwrap();
        let connection = database.connection().await.unwrap();
        raw_sql(include_str!("../sql/create_schema.sql"))
            .execute(&*connection)
            .await
            .unwrap();
        raw_sql(MIGRATIONS[0].sql)
            .execute(&*connection)
            .await
            .unwrap();
        let fingerprint = schema_hash(&connection, MIGRATIONS[0].expected_tables)
            .await
            .unwrap();
        query(include_str!("../sql/record_migration.sql"))
            .bind(MIGRATIONS[0].version)
            .bind(checksum(&MIGRATIONS[0]))
            .bind(fingerprint)
            .bind(chrono::Utc::now())
            .execute(&*connection)
            .await
            .unwrap();
        drop(connection);
        (directory, Store::from_database(database))
    }

    #[tokio::test]
    async fn nsfw_mark_upgrade_from_16_preserves_operation_snapshots() {
        let (_directory, store) = version_one_store().await;
        {
            let c = store.database().connection().await.unwrap();
            for migration in &MIGRATIONS[1..16] {
                raw_sql(migration.sql).execute(&*c).await.unwrap();
                let fingerprint = schema_hash(&c, migration.expected_tables).await.unwrap();
                query(include_str!("../sql/record_migration.sql"))
                    .bind(migration.version)
                    .bind(checksum(migration))
                    .bind(fingerprint)
                    .bind(chrono::Utc::now())
                    .execute(&*c)
                    .await
                    .unwrap();
            }
        }
        raw_sql(include_str!(
            "../sql/test_content_policy_upgrade_fixture.sql"
        ))
        .execute(store.database())
        .await
        .unwrap();
        let sql = include_str!("../sql/test_content_policy_operations_rows.sql");
        let before = query(sql)
            .fetch_all(store.database())
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.try_get::<String, _>("snapshot").unwrap())
            .collect::<Vec<_>>();
        store.migrate().await.unwrap();
        store.verify_schema().await.unwrap();
        let after = query(sql)
            .fetch_all(store.database())
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.try_get::<String, _>("snapshot").unwrap())
            .collect::<Vec<_>>();
        assert_eq!(before, after);
        store.migrate().await.unwrap();
    }

    #[tokio::test]
    async fn content_policy_rebuild_preserves_runtime_data_children_indexes_and_fks() {
        let (_directory, store) = version_one_store().await;
        {
            let connection = store.database().connection().await.unwrap();
            for migration in &MIGRATIONS[1..15] {
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
        raw_sql(include_str!(
            "../sql/test_content_policy_upgrade_fixture.sql"
        ))
        .execute(store.database())
        .await
        .unwrap();
        async fn rows(store: &Store, sql: &str) -> Vec<String> {
            query(sql)
                .fetch_all(store.database())
                .await
                .unwrap()
                .into_iter()
                .map(|r| r.try_get("snapshot").unwrap())
                .collect()
        }
        async fn indexes(store: &Store) -> Vec<(String, Option<String>)> {
            query(include_str!("../sql/test_content_policy_indexes.sql"))
                .fetch_all(store.database())
                .await
                .unwrap()
                .into_iter()
                .map(|r| (r.try_get("name").unwrap(), r.try_get("sql").unwrap()))
                .collect()
        }
        let queries = [
            include_str!("../sql/test_content_policy_operations_rows.sql"),
            include_str!("../sql/test_content_policy_units_rows.sql"),
            include_str!("../sql/test_content_policy_notices_rows.sql"),
        ];
        let mut before = Vec::new();
        for sql in queries {
            before.push(rows(&store, sql).await);
        }
        let before_indexes = indexes(&store).await;
        assert_eq!(before[0].len(), 8);
        assert_eq!(before[1].len(), 32);
        assert_eq!(before[2].len(), 8);
        assert_eq!(
            before_indexes
                .iter()
                .filter(|(_, sql)| sql.is_some())
                .count(),
            6
        );
        store.migrate().await.unwrap();
        store.migrate().await.unwrap();
        store.verify_schema().await.unwrap();
        for (sql, expected) in queries.into_iter().zip(&before) {
            assert_eq!(&rows(&store, sql).await, expected);
        }
        assert_eq!(indexes(&store).await, before_indexes);
        for row in query(include_str!("../sql/test_content_policy_failures.sql"))
            .fetch_all(store.database())
            .await
            .unwrap()
        {
            assert_eq!(
                row.try_get::<Option<String>, _>("content_failure").unwrap(),
                None
            );
        }
        for failure in ["nsfw_blocked", "nsfw_policy_unavailable"] {
            query(include_str!("../sql/test_content_policy_set_failure.sql"))
                .bind(failure)
                .execute(store.database())
                .await
                .unwrap();
        }
        assert!(
            query(include_str!("../sql/test_content_policy_set_failure.sql"))
                .bind("invalid")
                .execute(store.database())
                .await
                .is_err()
        );
        assert!(
            raw_sql(include_str!("../sql/test_content_policy_update_parent.sql"))
                .execute(store.database())
                .await
                .is_err(),
            "parent updates must not orphan FK children"
        );
        raw_sql(include_str!("../sql/test_content_policy_delete_parent.sql"))
            .execute(store.database())
            .await
            .unwrap();
        assert_eq!(rows(&store, queries[0]).await.len(), 7);
        assert_eq!(rows(&store, queries[1]).await.len(), 28);
        assert_eq!(rows(&store, queries[2]).await.len(), 7);
        store.verify_schema().await.unwrap();
    }

    #[tokio::test]
    async fn quota_upgrade_preserves_operations_and_imported_counters_and_fingerprints_index() {
        let (_dir, store) = version_one_store().await;
        {
            let connection = store.database().connection().await.unwrap();
            for migration in &MIGRATIONS[1..14] {
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
        raw_sql("INSERT INTO runtime_operations(id,actor_id,idempotency_key,request,max_attempts) VALUES('fixture',7,'old','{\"old\":true}',3); INSERT INTO rate_limits VALUES(7,'minute',4,1700000060000000),(7,'hour',29,1700003600000000),(7,'day',99,1700086400000000),(7,'other',8,1);")
            .execute(store.database()).await.unwrap();
        let snapshot = |sql: &str| query(sql);
        let operations_sql = "SELECT json_object('id',id,'actor_id',actor_id,'key',idempotency_key,'request',request,'status',status,'created_at',created_at) FROM runtime_operations";
        let counters_sql = "SELECT json_object('user_id',user_id,'window',\"window\",'count',count,'reset_at',reset_at) FROM rate_limits ORDER BY user_id,\"window\"";
        let before_ops: String = snapshot(operations_sql)
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        let before_counters = snapshot(counters_sql)
            .fetch_all(store.database())
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get::<String, _>(0).unwrap())
            .collect::<Vec<_>>();
        store.migrate().await.unwrap();
        store.migrate().await.unwrap();
        store.verify_schema().await.unwrap();
        let after_ops: String = snapshot(operations_sql)
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        let after_counters = snapshot(counters_sql)
            .fetch_all(store.database())
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get::<String, _>(0).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(before_ops, after_ops);
        assert_eq!(before_counters, after_counters);
        raw_sql("DROP INDEX operations_active_actor")
            .execute(store.database())
            .await
            .unwrap();
        assert_eq!(store.verify_schema().await, Err(StoreError::Schema));
        assert_eq!(store.migrate().await, Err(StoreError::Schema));
    }

    #[tokio::test]
    async fn membership_upgrade_preserves_imported_state_and_fingerprints_indexes() {
        let (_directory, store) = version_one_store().await;
        let before = {
            let connection = store.database().connection().await.unwrap();
            for migration in &MIGRATIONS[1..13] {
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
            raw_sql(include_str!("../sql/test_membership_upgrade_fixture.sql"))
                .execute(&*connection)
                .await
                .unwrap();
            query(include_str!("../sql/test_membership_upgrade_state.sql"))
                .fetch_one(&*connection)
                .await
                .unwrap()
                .try_get::<String, _>(0)
                .unwrap()
        };
        store.migrate().await.unwrap();
        store.migrate().await.unwrap();
        store.verify_schema().await.unwrap();
        let after = query(include_str!("../sql/test_membership_upgrade_state.sql"))
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap();
        assert_eq!(before, after);
        let objects = query(include_str!("../sql/test_membership_upgrade_objects.sql"))
            .fetch_all(store.database())
            .await
            .unwrap();
        let names = objects
            .iter()
            .map(|r| r.try_get::<String, _>(0).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "membership_bots",
                "membership_bots_group",
                "membership_feature_suppressions",
                "membership_group_suppressions",
                "membership_users",
                "membership_users_group",
                "membership_users_user"
            ]
        );
        raw_sql(include_str!("../sql/test_membership_drop_index.sql"))
            .execute(store.database())
            .await
            .unwrap();
        assert_eq!(store.verify_schema().await, Err(StoreError::Schema));
        assert_eq!(store.migrate().await, Err(StoreError::Schema));
    }

    #[tokio::test]
    async fn migration_twelve_preserves_all_legacy_quality_and_cut_columns() {
        let (_directory, store) = version_one_store().await;
        let before_quality;
        let before_cut;
        {
            let connection = store.database().connection().await.unwrap();
            for migration in &MIGRATIONS[1..11] {
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
            raw_sql(include_str!(
                "../sql/test_quality_cut_migration_fixture.sql"
            ))
            .execute(&*connection)
            .await
            .unwrap();
            before_quality = query(include_str!("../sql/test_quality_cut_migration_rows.sql"))
                .fetch_all(&*connection)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.try_get::<String, _>("snapshot").unwrap())
                .collect::<Vec<_>>();
            before_cut = query(include_str!(
                "../sql/test_quality_cut_migration_cut_rows.sql"
            ))
            .fetch_all(&*connection)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.try_get::<String, _>("snapshot").unwrap())
            .collect::<Vec<_>>();
        }
        store.migrate().await.unwrap();
        let after_quality = query(include_str!("../sql/test_quality_cut_migration_rows.sql"))
            .fetch_all(store.database())
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.try_get::<String, _>("snapshot").unwrap())
            .collect::<Vec<_>>();
        let after_cut = query(include_str!(
            "../sql/test_quality_cut_migration_cut_rows.sql"
        ))
        .fetch_all(store.database())
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.try_get::<String, _>("snapshot").unwrap())
        .collect::<Vec<_>>();
        assert_eq!(after_quality, before_quality);
        assert_eq!(after_cut, before_cut);
    }

    #[tokio::test]
    async fn valid_version_one_history_upgrades_to_latest() {
        let (_directory, store) = version_one_store().await;
        store.migrate().await.unwrap();
        store.verify_schema().await.unwrap();
        let versions = query("SELECT version FROM runtime_migrations ORDER BY version")
            .fetch_all(store.database())
            .await
            .unwrap();
        assert_eq!(versions.len(), MIGRATIONS.len());
        for (row, migration) in versions.iter().zip(&MIGRATIONS) {
            assert_eq!(row.try_get::<i64, _>("version").unwrap(), migration.version);
        }
    }

    #[tokio::test]
    async fn quality_audio_migration_preserves_every_existing_stage() {
        let (_directory, store) = version_one_store().await;
        let before = {
            let connection = store.database().connection().await.unwrap();
            for migration in &MIGRATIONS[1..10] {
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
            raw_sql(include_str!("../sql/test_quality_audio_migration_rows.sql"))
                .execute(&*connection)
                .await
                .unwrap();
            query(include_str!(
                "../sql/test_quality_audio_migration_verify.sql"
            ))
            .fetch_all(&*connection)
            .await
            .unwrap()
            .iter()
            .map(|row| row.try_get::<String, _>("snapshot").unwrap())
            .collect::<Vec<_>>()
        };
        store.migrate().await.unwrap();
        let rows = query(include_str!(
            "../sql/test_quality_audio_migration_verify.sql"
        ))
        .fetch_all(store.database())
        .await
        .unwrap();
        assert_eq!(rows.len(), 4);
        for (row, snapshot) in rows.iter().zip(before) {
            assert_eq!(row.try_get::<String, _>("snapshot").unwrap(), snapshot);
        }
        for (row, (actor, stage, revision, selected)) in rows.iter().zip([
            (301_i64, "choosing", 1_i64, None),
            (302, "confirming", 2, Some("best")),
            (303, "submitted", 3, Some("720")),
            (304, "cancelled", 2, None),
        ]) {
            assert_eq!(row.try_get::<i64, _>("actor_id").unwrap(), actor);
            assert_eq!(row.try_get::<String, _>("stage").unwrap(), stage);
            assert_eq!(row.try_get::<i64, _>("revision").unwrap(), revision);
            assert_eq!(
                row.try_get::<Option<String>, _>("selected")
                    .unwrap()
                    .as_deref(),
                selected
            );
        }
    }

    #[tokio::test]
    async fn version_nine_cut_rows_survive_quality_migration_without_new_identity() {
        let (_directory, store) = version_one_store().await;
        {
            let connection = store.database().connection().await.unwrap();
            for migration in &MIGRATIONS[1..9] {
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
            raw_sql(include_str!("../sql/test_cut_v9_quality_migration.sql"))
                .execute(&*connection)
                .await
                .unwrap();
        }
        store.migrate().await.unwrap();
        let rows = query(include_str!("../sql/test_cut_quality_migration_rows.sql"))
            .fetch_all(store.database())
            .await
            .unwrap();
        assert_eq!(rows.len(), 7);
        let expected = [
            (
                7,
                "38e4f36a-14da-4f7b-8d1d-12d2191d8db1",
                "confirming",
                4,
                11,
                13,
                1.0,
                Some(2.0),
                Some(8.0),
                None,
            ),
            (
                8,
                "48e4f36a-14da-4f7b-8d1d-12d2191d8db2",
                "ready",
                5,
                21,
                23,
                2.0,
                Some(3.0),
                Some(9.0),
                None,
            ),
            (
                10,
                "58e4f36a-14da-4f7b-8d1d-12d2191d8db3",
                "submitted",
                6,
                31,
                33,
                3.0,
                Some(4.0),
                Some(10.0),
                Some("68e4f36a-14da-4f7b-8d1d-12d2191d8db4"),
            ),
            (
                12,
                "78e4f36a-14da-4f7b-8d1d-12d2191d8db5",
                "cancelled",
                7,
                41,
                43,
                4.0,
                None,
                None,
                None,
            ),
        ];
        for (row, (actor, id, stage, revision, origin, last, begin, start, end, operation)) in
            rows.iter().zip(expected)
        {
            assert_eq!(row.try_get::<i64, _>("actor_id").unwrap(), actor);
            assert_eq!(row.try_get::<i64, _>("bot_id").unwrap(), 9);
            assert_eq!(row.try_get::<i64, _>("chat_id").unwrap(), actor);
            assert_eq!(row.try_get::<String, _>("id").unwrap(), id);
            assert_eq!(row.try_get::<i64, _>("revision").unwrap(), revision);
            assert_eq!(row.try_get::<String, _>("stage").unwrap(), stage);
            assert_eq!(
                row.try_get::<Option<String>, _>("operation_id")
                    .unwrap()
                    .as_deref(),
                operation
            );
            assert_eq!(row.try_get::<Option<String>, _>("selected").unwrap(), None);
            assert_eq!(row.try_get::<i64, _>("origin_message_id").unwrap(), origin);
            assert_eq!(row.try_get::<i64, _>("last_message_id").unwrap(), last);
            assert_eq!(
                row.try_get::<String, _>("source_url").unwrap(),
                format!("https://example.test/{stage}")
            );
            assert_eq!(
                row.try_get::<Option<f64>, _>("begin_start").unwrap(),
                Some(begin)
            );
            assert_eq!(row.try_get::<Option<f64>, _>("start").unwrap(), start);
            assert_eq!(row.try_get::<Option<f64>, _>("end").unwrap(), end);
        }
        for (actor_id, expected_stage, expected_end) in [
            (13, "start", -1.0),
            (14, "end", -2.0),
            (15, "cancelled", -3.0),
        ] {
            let row = query("SELECT stage,end FROM runtime_cut_sessions WHERE actor_id=?1")
                .bind(actor_id)
                .fetch_one(store.database())
                .await
                .unwrap();
            assert_eq!(
                row.try_get::<Option<f64>, _>("end").unwrap(),
                Some(expected_end)
            );
            assert_eq!(row.try_get::<String, _>("stage").unwrap(), expected_stage);
        }
        store.verify_schema().await.unwrap();
    }

    #[tokio::test]
    async fn cut_quality_migration_preserves_nonnegative_range_constraints() {
        let (_directory, store) = version_one_store().await;
        store.migrate().await.unwrap();
        let connection = store.database().connection().await.unwrap();
        assert!(
            raw_sql(include_str!(
                "../sql/test_cut_negative_start_constraint.sql"
            ))
            .execute(&*connection)
            .await
            .is_err()
        );
        drop(connection);
        store.verify_schema().await.unwrap();
    }

    #[tokio::test]
    async fn version_three_upgrade_preserves_rows_and_protects_all_rowid_foreign_keys() {
        let (_directory, store) = version_one_store().await;
        {
            let connection = store.database().connection().await.unwrap();
            for migration in &MIGRATIONS[1..3] {
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
        let rows = [
            (
                "dl_user_settings",
                "INSERT INTO dl_user_settings(user_id,created_at,updated_at) VALUES(1,1,1)",
            ),
            (
                "inbox_gh_sync_state",
                "INSERT INTO inbox_gh_sync_state(user_id) VALUES(2)",
            ),
            (
                "inbox_user_settings",
                "INSERT INTO inbox_user_settings(user_id) VALUES(3)",
            ),
            (
                "insight_access",
                "INSERT INTO insight_access(user_id,granted_by,granted_at) VALUES(4,1,1)",
            ),
            (
                "insight_user_byok",
                "INSERT INTO insight_user_byok(user_id,provider,api_key,model,created_at,updated_at) VALUES(5,'test','synthetic','test',1,1)",
            ),
            (
                "insight_user_settings",
                "INSERT INTO insight_user_settings(user_id) VALUES(6)",
            ),
        ];
        for (i, (_, sql)) in rows.iter().enumerate() {
            query("INSERT INTO users(id,created_at,updated_at) VALUES(?1,1,1)")
                .bind((i + 1) as i64)
                .execute(store.database())
                .await
                .unwrap();
            raw_sql(sql).execute(store.database()).await.unwrap();
        }
        store.migrate().await.unwrap();
        store.verify_schema().await.unwrap();
        for (i, (table, _)) in rows.iter().enumerate() {
            let id = (i + 1) as i64;
            assert!(
                query("UPDATE users SET id=?1 WHERE id=?2")
                    .bind(id + 100)
                    .bind(id)
                    .execute(store.database())
                    .await
                    .is_err(),
                "parent update must not orphan {table}"
            );
            let count: i64 = query(&format!(
                "SELECT count(*) AS n FROM {table} WHERE user_id={id}"
            ))
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get("n")
            .unwrap();
            assert_eq!(count, 1);
            query("DELETE FROM users WHERE id=?1")
                .bind(id)
                .execute(store.database())
                .await
                .unwrap();
            let count: i64 = query(&format!("SELECT count(*) AS n FROM {table}"))
                .fetch_one(store.database())
                .await
                .unwrap()
                .try_get("n")
                .unwrap();
            assert_eq!(count, 0, "CASCADE must preserve its behavior for {table}");
        }
    }

    #[tokio::test]
    async fn malformed_or_unknown_history_is_rejected_without_upgrade_mutation() {
        for history_sql in [
            "UPDATE runtime_migrations SET checksum = 'wrong' WHERE version = 1",
            "INSERT INTO runtime_migrations (version, checksum, schema_hash, success, installed_at) VALUES (99, 'future', 'future', 1, 1)",
        ] {
            let (_directory, store) = version_one_store().await;
            raw_sql(history_sql)
                .execute(store.database())
                .await
                .unwrap();
            assert_eq!(store.migrate().await, Err(StoreError::Schema));
            let versions: i64 = query("SELECT count(*) AS count FROM runtime_migrations")
                .fetch_one(store.database())
                .await
                .unwrap()
                .try_get("count")
                .unwrap();
            assert_eq!(
                versions,
                if history_sql.starts_with("UPDATE") {
                    1
                } else {
                    2
                }
            );
            let app_tables: i64 = query("SELECT count(*) AS count FROM sqlite_master WHERE type = 'table' AND name = 'users'")
                .fetch_one(store.database()).await.unwrap().try_get("count").unwrap();
            assert_eq!(app_tables, 0);
        }
    }

    #[tokio::test]
    async fn drifted_version_one_is_rejected_without_upgrade_mutation() {
        let (_directory, store) = version_one_store().await;
        raw_sql("ALTER TABLE runtime_operations ADD COLUMN unexpected TEXT")
            .execute(store.database())
            .await
            .unwrap();
        assert_eq!(store.migrate().await, Err(StoreError::Schema));
        let versions: i64 = query("SELECT count(*) AS count FROM runtime_migrations")
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
        assert_eq!(versions, 1);
        let app_tables: i64 = query(
            "SELECT count(*) AS count FROM sqlite_master WHERE type = 'table' AND name = 'users'",
        )
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
        assert_eq!(app_tables, 0);
    }
}
