use sha2::{Digest, Sha256};
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_db::{Database, query, raw_sql};

use crate::StoreError;

struct Migration {
    version: i64,
    sql: &'static str,
    expected_tables: usize,
}

const MIGRATIONS: [Migration; 6] = [
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
    async fn valid_version_one_history_upgrades_to_latest() {
        let (_directory, store) = version_one_store().await;
        store.migrate().await.unwrap();
        store.verify_schema().await.unwrap();
        let versions = query("SELECT version FROM runtime_migrations ORDER BY version")
            .fetch_all(store.database())
            .await
            .unwrap();
        assert_eq!(versions.len(), 6);
        assert_eq!(versions[0].try_get::<i64, _>("version").unwrap(), 1);
        assert_eq!(versions[1].try_get::<i64, _>("version").unwrap(), 2);
        assert_eq!(versions[2].try_get::<i64, _>("version").unwrap(), 3);
        assert_eq!(versions[3].try_get::<i64, _>("version").unwrap(), 4);
        assert_eq!(versions[4].try_get::<i64, _>("version").unwrap(), 5);
        assert_eq!(versions[5].try_get::<i64, _>("version").unwrap(), 6);
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
