use super::{NativeIndex, SearchError, test_support::fixture, tracked_blocking};
use std::{sync::mpsc, time::Duration};
use tokio::sync::oneshot;
use tokio_util::task::TaskTracker;

#[tokio::test]
async fn canceled_index_action_remains_owned_until_blocking_worker_finishes() {
    let (_dir, store, search) = fixture().await;
    let tasks = search.tasks.clone();
    let (entered, started) = oneshot::channel();
    let (release, released) = mpsc::channel();
    let running = search.clone();
    let caller = tokio::spawn(async move {
        running
            .blocking(move |_| {
                entered.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(10)).unwrap();
                Ok(())
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), started)
        .await
        .unwrap()
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    tasks.close();
    assert_eq!(tasks.len(), 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), tasks.wait())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), tasks.wait())
        .await
        .unwrap();
    assert!(tasks.is_empty());
    search.refresh().await.unwrap();
    drop(search);
    store.close().await.unwrap();
}

#[tokio::test]
async fn canceled_index_startup_drops_undelivered_writer_before_drain_returns() {
    let (dir, store, _search) = fixture().await;
    let path = dir.path().join("canceled-startup");
    let index_path = path.clone();
    let tasks = TaskTracker::new();
    let running_tasks = tasks.clone();
    let (entered, started) = oneshot::channel();
    let (release, released) = mpsc::channel();
    let caller = tokio::spawn(async move {
        tracked_blocking(&running_tasks, move || {
            let index = NativeIndex::open(&index_path).map_err(SearchError::Index)?;
            entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(index)
        })
        .await
        .map(drop)
    });
    tokio::time::timeout(Duration::from_secs(2), started)
        .await
        .unwrap()
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    tasks.close();
    assert_eq!(tasks.len(), 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), tasks.wait())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), tasks.wait())
        .await
        .unwrap();
    assert!(tasks.is_empty());
    let reopened = tokio::task::spawn_blocking(move || NativeIndex::open(&path))
        .await
        .unwrap()
        .unwrap();
    drop(reopened);
    store.close().await.unwrap();
}
