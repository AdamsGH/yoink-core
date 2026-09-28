use super::{RefreshMode, SearchError, SearchService, test_support::*};
use std::{sync::Arc, time::Duration};
use yoink_db::{query, raw_sql};

#[tokio::test]
async fn cancellation_before_commit_discards_partial_rebuild_and_replays_source() {
    let (_dir, store, search) = fixture().await;
    seed(&store).await;
    search.refresh().await.unwrap();
    query("UPDATE stats_messages SET text = 'new' WHERE id = 1")
        .execute(store.database())
        .await
        .unwrap();
    let pause = search.arm(Stage::AfterBatch);
    let running = search.clone();
    let task = tokio::spawn(async move { running.rebuild().await });
    wait(&pause).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let report = search.refresh().await.unwrap();
    assert_eq!(report.mode, RefreshMode::Rebuild);
    assert_eq!(report.documents, 1);
    assert_eq!(search.search(request("old")).await.unwrap().total, 0);
    assert_eq!(search.search(request("new")).await.unwrap().total, 1);
}

#[tokio::test]
async fn cancelled_incremental_commit_reopens_from_source_not_ahead_index() {
    let (dir, store, search) = fixture().await;
    seed(&store).await;
    let pause = search.arm(Stage::AfterIndexCommit);
    let running = search.clone();
    let task = tokio::spawn(async move { running.refresh().await });
    wait(&pause).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let state = query("SELECT source_revision, acknowledged_revision FROM stats_search_state")
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(state.try_get::<i64, _>("source_revision").unwrap(), 1);
    assert_eq!(state.try_get::<i64, _>("acknowledged_revision").unwrap(), 0);
    drop(search);
    query("UPDATE stats_messages SET text = 'new' WHERE id = 1")
        .execute(store.database())
        .await
        .unwrap();
    let reopened = SearchService::open(store.database().clone(), dir.path().join("index"))
        .await
        .unwrap();
    assert_eq!(reopened.search(request("old")).await.unwrap().total, 0);
    assert_eq!(reopened.search(request("new")).await.unwrap().total, 1);
    assert_eq!(
        reopened.refresh().await.unwrap().mode,
        RefreshMode::Unchanged
    );
}

#[tokio::test]
async fn native_acknowledgement_failure_never_clears_pending_work() {
    let (dir, store, search) = fixture().await;
    seed(&store).await;
    // Fail the real native acknowledgement after index commit and queue deletion.
    raw_sql("CREATE TRIGGER test_ack_failure BEFORE UPDATE OF acknowledged_revision ON stats_search_state BEGIN SELECT RAISE(ABORT, 'forced acknowledgement failure'); END;")
        .execute(store.database()).await.unwrap();
    let result = search.refresh().await;
    assert!(
        matches!(result, Err(SearchError::Database)),
        "unexpected acknowledgement result: {result:?}"
    );
    let dirty = query("SELECT count(*) AS count FROM stats_search_dirty")
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(dirty.try_get::<i64, _>("count").unwrap(), 1);
    let state = query("SELECT acknowledged_revision FROM stats_search_state")
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(state.try_get::<i64, _>("acknowledged_revision").unwrap(), 0);
    raw_sql("DROP TRIGGER test_ack_failure")
        .execute(store.database())
        .await
        .unwrap();
    store.verify_schema().await.unwrap();
    drop(search);
    let reopened = SearchService::open(store.database().clone(), dir.path().join("index"))
        .await
        .unwrap();
    assert_eq!(reopened.search(request("old")).await.unwrap().total, 1);
}

#[tokio::test]
async fn concurrent_mutation_cannot_split_index_query_from_source_hydration() {
    let (_dir, store, search) = fixture().await;
    seed(&store).await;
    let pause = search.arm(Stage::BeforeHydration);
    let running = search.clone();
    let reader = tokio::spawn(async move { running.search(request("old")).await });
    wait(&pause).await;
    let database = store.database().clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let mut writer = tokio::spawn(async move {
        signal.notify_one();
        query("UPDATE stats_messages SET text = 'new', chat_id = -2 WHERE id = 1")
            .execute(&database)
            .await
            .unwrap();
    });
    started.notified().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut writer)
            .await
            .is_err()
    );
    pause.release.notify_one();
    let page = reader.await.unwrap().unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].text.as_deref(), Some("old"));
    assert_eq!(page.items[0].chat_id, -1);
    writer.await.unwrap();
    assert_eq!(search.search(request("old")).await.unwrap().total, 0);
    let mut updated = request("new");
    updated.chat_id = -2;
    assert_eq!(search.search(updated).await.unwrap().total, 1);
}
