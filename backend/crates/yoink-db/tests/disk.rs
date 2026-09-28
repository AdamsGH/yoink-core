use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde_json::json;
use yoink_db::{Database, Error, query, raw_sql};

fn private_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
}

#[tokio::test]
async fn creates_reopens_and_closes_shared_database() {
    let dir = private_dir();
    let path = dir.path().join("data.db");
    let db = Database::open(&path, true, Duration::from_secs(2))
        .await
        .unwrap();
    let clone = db.clone();
    raw_sql("CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT); INSERT INTO items(value) VALUES ('ready');")
        .execute(&db)
        .await
        .unwrap();
    let result = query("INSERT INTO items(value) VALUES (?) RETURNING id, value")
        .bind("returned")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].try_get::<i64, _>("id").unwrap(), 2);
    assert_eq!(result[0].try_get::<String, _>("value").unwrap(), "returned");
    assert_eq!(result[0].try_get::<i64, _>(0).unwrap(), 2);
    db.close().await.unwrap();
    assert!(matches!(clone.connection().await, Err(Error::Closed)));
    drop(clone);
    let reopened = Database::open(&path, false, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(
        query("SELECT count(*) AS count FROM items")
            .fetch_one(&reopened)
            .await
            .unwrap()
            .try_get::<i64, _>("count")
            .unwrap(),
        2
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn create_refuses_every_existing_destination_and_ownership_is_exclusive() {
    let dir = private_dir();
    let path = dir.path().join("data.db");
    let first = Database::open(&path, true, Duration::from_secs(2))
        .await
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(format!("{}.yoink-lock", path.display()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(matches!(
        Database::open(&path, false, Duration::from_secs(2)).await,
        Err(Error::AlreadyOwned)
    ));
    first.close().await.unwrap();
    assert!(matches!(
        Database::open(&path, true, Duration::from_secs(2)).await,
        Err(Error::InvalidPath)
    ));
}

#[tokio::test]
async fn dropped_native_transaction_rolls_back_before_next_read() {
    let dir = private_dir();
    let path = dir.path().join("data.db");
    let db = Database::open(&path, true, Duration::from_secs(2))
        .await
        .unwrap();
    raw_sql("CREATE TABLE entries (id INTEGER PRIMARY KEY)")
        .execute(&db)
        .await
        .unwrap();
    {
        let mut connection = db.connection().await.unwrap();
        let transaction = connection.transaction().await.unwrap();
        transaction
            .execute("INSERT INTO entries DEFAULT VALUES", ())
            .await
            .unwrap();
        drop(transaction);
    }
    assert_eq!(
        query("SELECT count(*) AS count FROM entries")
            .fetch_one(&db)
            .await
            .unwrap()
            .try_get::<i64, _>("count")
            .unwrap(),
        0
    );
    db.close().await.unwrap();
}

#[tokio::test]
async fn codecs_preserve_json_uuid_datetime_and_float() {
    let dir = private_dir();
    let path = dir.path().join("data.db");
    let db = Database::open(&path, true, Duration::from_secs(2))
        .await
        .unwrap();
    let uuid = uuid::Uuid::parse_str("a8a0b830-2661-4cd1-9aa3-ec635405968e").unwrap();
    let date = Utc.timestamp_micros(1_700_000_000_123_456).unwrap();
    let row = query("SELECT ? AS json, ? AS uuid, ? AS date, ? AS float")
        .bind(json!({"ok": true, "values": [1, 2]}))
        .bind(uuid)
        .bind(date)
        .bind(1.25_f64)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(
        row.try_get::<serde_json::Value, _>("json").unwrap(),
        json!({"ok": true, "values": [1, 2]})
    );
    assert_eq!(row.try_get::<uuid::Uuid, _>("uuid").unwrap(), uuid);
    assert_eq!(
        row.try_get::<chrono::DateTime<Utc>, _>("date").unwrap(),
        date
    );
    assert_eq!(row.try_get::<f64, _>("float").unwrap(), 1.25);
    assert!(matches!(
        query("SELECT 1").bind(u64::MAX).fetch_one(&db).await,
        Err(Error::Conversion)
    ));
    assert!(matches!(
        query("SELECT 1").bind(f32::NAN).fetch_one(&db).await,
        Err(Error::Conversion)
    ));
    assert!(matches!(
        query("SELECT 1").bind(f32::INFINITY).fetch_one(&db).await,
        Err(Error::Conversion)
    ));
    assert!(matches!(
        query("SELECT 1").bind(f64::NAN).fetch_one(&db).await,
        Err(Error::Conversion)
    ));
    assert!(matches!(
        query("SELECT 1")
            .bind(f64::NEG_INFINITY)
            .fetch_one(&db)
            .await,
        Err(Error::Conversion)
    ));
    db.close().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_insecure_parent_without_creating_database_or_lock() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = dir.path().join("data.db");
    assert!(matches!(
        Database::open(&path, true, Duration::from_secs(2)).await,
        Err(Error::InsecureDirectory)
    ));
    assert!(!path.exists());
    assert!(!std::path::PathBuf::from(format!("{}.yoink-lock", path.display())).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn private_directory_allows_create_and_reopen() {
    let dir = private_dir();
    let path = dir.path().join("data.db");
    let db = Database::open(&path, true, Duration::from_secs(2))
        .await
        .unwrap();
    db.close().await.unwrap();
    let db = Database::open(&path, false, Duration::from_secs(2))
        .await
        .unwrap();
    db.close().await.unwrap();
}

#[tokio::test]
async fn debug_and_engine_errors_do_not_disclose_sql_or_data() {
    let sentinel = "private-sentinel-84721";
    let debug_query = format!(
        "{:?}",
        query(&format!("SELECT '{sentinel}'")).bind(sentinel)
    );
    let debug_bind = format!("{:?}", yoink_db::BindValue::Text(sentinel.to_owned()));
    assert!(!debug_query.contains(sentinel));
    assert!(!debug_bind.contains(sentinel));

    let dir = private_dir();
    let db = Database::open(&dir.path().join("data.db"), true, Duration::from_secs(2))
        .await
        .unwrap();
    let row = query("SELECT ? AS secret")
        .bind(sentinel)
        .fetch_one(&db)
        .await
        .unwrap();
    assert!(!format!("{row:?}").contains(sentinel));
    let error = raw_sql(&format!("SELECT '{sentinel}' FROM missing_table"))
        .execute(&db)
        .await
        .unwrap_err();
    assert!(!format!("{error:?}").contains(sentinel));
    assert!(!error.to_string().contains(sentinel));
    assert!(std::error::Error::source(&error).is_none());
    db.close().await.unwrap();
}

#[tokio::test]
async fn rejects_non_finite_float_bind_values() {
    use yoink_db::ToBindValue;
    for result in [
        f32::NAN.to_bind_value(),
        f32::INFINITY.to_bind_value(),
        f64::NAN.to_bind_value(),
        f64::NEG_INFINITY.to_bind_value(),
    ] {
        assert!(matches!(result, Err(Error::Conversion)));
    }
    assert!(1.25_f32.to_bind_value().is_ok());
    assert!(1.25_f64.to_bind_value().is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_symlink_and_hardlink_database_aliases() {
    let dir = private_dir();
    let path = dir.path().join("data.db");
    let alias = dir.path().join("alias.db");
    std::fs::write(&path, b"existing").unwrap();
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    assert!(matches!(
        Database::open(&alias, false, Duration::from_secs(2)).await,
        Err(Error::InvalidPath)
    ));
    std::fs::remove_file(&alias).unwrap();
    std::fs::hard_link(&path, &alias).unwrap();
    assert!(matches!(
        Database::open(&path, false, Duration::from_secs(2)).await,
        Err(Error::InvalidPath)
    ));
}
