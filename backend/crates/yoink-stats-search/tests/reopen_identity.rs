mod common;

use common::{fixture, insert, request};
use std::{fs, time::Duration};
use yoink_stats_search::{RefreshMode, SearchService};
use yoink_storage::Store;

#[tokio::test]
async fn missing_index_rebuilds_after_source_acknowledgement_is_current() {
    let f = fixture().await;
    insert(&f.store, 21, -21, 1, Some("persisted evidence"), "text").await;
    assert_eq!(
        f.search
            .search(request(-21, "persisted"))
            .await
            .unwrap()
            .total,
        1
    );
    let index = f.directory.path().join("index");
    drop(f.search);
    f.store.close().await.unwrap();
    drop(f.store);
    fs::remove_dir_all(&index).unwrap();

    let reopened_store = Store::open(
        &f.directory.path().join("source.db"),
        false,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let reopened = SearchService::open(reopened_store.database().clone(), &index)
        .await
        .unwrap();
    let page = reopened.search(request(-21, "persisted")).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].id, 21);
}

#[tokio::test]
async fn index_from_another_database_identity_rebuilds_even_at_equal_revision() {
    let f = fixture().await;
    insert(&f.store, 1, -31, 1, Some("first database token"), "text").await;
    assert_eq!(
        f.search.search(request(-31, "first")).await.unwrap().total,
        1
    );
    let index = f.directory.path().join("index");
    drop(f.search);
    let other = Store::open(
        &f.directory.path().join("other.db"),
        true,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    other.migrate().await.unwrap();
    insert(&other, 2, -31, 2, Some("second database token"), "text").await;
    let scratch = SearchService::open(
        other.database().clone(),
        f.directory.path().join("scratch-index"),
    )
    .await
    .unwrap();
    drop(scratch);
    assert_current(&f.store, 1).await;
    assert_current(&other, 1).await;
    let old = payload(&index);
    let search = SearchService::open(other.database().clone(), &index)
        .await
        .unwrap();
    let page = search.search(request(-31, "second")).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].id, 2);
    assert_eq!(search.search(request(-31, "first")).await.unwrap().total, 0);
    let new = payload(&index);
    assert_ne!(old["database_id"], new["database_id"]);
    assert_eq!(old["revision"], new["revision"]);
    assert_eq!(search.refresh().await.unwrap().mode, RefreshMode::Unchanged);
}

#[cfg(unix)]
#[tokio::test]
async fn same_sql_identity_with_replaced_inode_invalidates_old_index() {
    use std::os::unix::fs::MetadataExt;
    let f = fixture().await;
    insert(&f.store, 1, -41, 1, Some("initial source payload"), "text").await;
    f.search.refresh().await.unwrap();
    let index = f.directory.path().join("index");
    let source = f.directory.path().join("source.db");
    let backup = f.directory.path().join("backup.db");
    drop(f.search);
    checkpoint_and_close(&f.store).await;
    drop(f.store);
    fs::copy(&source, &backup).unwrap();

    let current = Store::open(&source, false, Duration::from_secs(5))
        .await
        .unwrap();
    yoink_db::query("UPDATE stats_messages SET text = 'current content' WHERE id = 1")
        .execute(current.database())
        .await
        .unwrap();
    let current_index = SearchService::open(current.database().clone(), &index)
        .await
        .unwrap();
    assert_eq!(
        current_index
            .search(request(-41, "current"))
            .await
            .unwrap()
            .total,
        1
    );
    assert_current(&current, 2).await;
    let previous = payload(&index);
    drop(current_index);
    checkpoint_and_close(&current).await;
    drop(current);

    let snapshot = Store::open(&backup, false, Duration::from_secs(5))
        .await
        .unwrap();
    yoink_db::query("UPDATE stats_messages SET text = 'restored content' WHERE id = 1")
        .execute(snapshot.database())
        .await
        .unwrap();
    let scratch = SearchService::open(
        snapshot.database().clone(),
        f.directory.path().join("backup-index"),
    )
    .await
    .unwrap();
    assert_current(&snapshot, 2).await;
    drop(scratch);
    checkpoint_and_close(&snapshot).await;
    drop(snapshot);
    let old_inode = fs::metadata(&source).unwrap().ino();
    assert_ne!(old_inode, fs::metadata(&backup).unwrap().ino());
    fs::rename(&backup, &source).unwrap();

    let restored = Store::open(&source, false, Duration::from_secs(5))
        .await
        .unwrap();
    assert_current(&restored, 2).await;
    let search = SearchService::open(restored.database().clone(), &index)
        .await
        .unwrap();
    assert_eq!(
        search.search(request(-41, "current")).await.unwrap().total,
        0
    );
    assert_eq!(
        search.search(request(-41, "restored")).await.unwrap().total,
        1
    );
    let rebound = payload(&index);
    assert_eq!(previous["database_id"], rebound["database_id"]);
    assert_eq!(previous["revision"], rebound["revision"]);
    assert_eq!(previous["path"], rebound["path"]);
    assert_ne!(previous["inode"], rebound["inode"]);
    assert_eq!(
        rebound["inode"].as_u64(),
        Some(fs::metadata(&source).unwrap().ino())
    );
}

async fn checkpoint_and_close(store: &Store) {
    let connection = store.database().connection().await.unwrap();
    let mut rows = connection
        .query("PRAGMA wal_checkpoint(TRUNCATE)", ())
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert!(matches!(
        row.get_value(0).unwrap(),
        turso::Value::Integer(0)
    ));
    assert_eq!(row.get_value(1).unwrap(), row.get_value(2).unwrap());
    assert!(rows.next().await.unwrap().is_none());
    drop(rows);
    drop(connection);
    store.close().await.unwrap();
}

async fn assert_current(store: &Store, revision: i64) {
    let state = yoink_db::query("SELECT source_revision, acknowledged_revision, (SELECT count(*) FROM stats_search_dirty) AS dirty FROM stats_search_state").fetch_one(store.database()).await.unwrap();
    assert_eq!(
        state.try_get::<i64, _>("source_revision").unwrap(),
        revision
    );
    assert_eq!(
        state.try_get::<i64, _>("acknowledged_revision").unwrap(),
        revision
    );
    assert_eq!(state.try_get::<i64, _>("dirty").unwrap(), 0);
}

fn payload(index: &std::path::Path) -> serde_json::Value {
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(index.join("meta.json")).unwrap()).unwrap();
    serde_json::from_str(metadata["payload"].as_str().unwrap()).unwrap()
}
