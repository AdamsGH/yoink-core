use super::*;
use std::time::Duration;
use tokio::time::timeout;

struct NoMembership;
impl yoink_stats::MembershipProvider for NoMembership {
    fn lookup(&self, _: i64, _: i64) -> yoink_stats::MembershipFuture<'_> {
        Box::pin(async { Ok(yoink_stats::MemberAccess::default()) })
    }
    fn lookup_name(&self, _: Option<i64>, _: i64) -> yoink_stats::NameFuture<'_> {
        Box::pin(async { Ok(None) })
    }
}

async fn fixture() -> (
    tempfile::TempDir,
    Arc<Store>,
    Arc<IdentityStore>,
    StatsConfig,
) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Arc::new(
        Store::open(
            &dir.path().join("products.db"),
            true,
            Duration::from_secs(2),
        )
        .await
        .unwrap(),
    );
    store.migrate().await.unwrap();
    let identity = Arc::new(IdentityStore::new(
        store.database().clone(),
        Some(42),
        false,
    ));
    let mut config = StatsConfig::from_lookup(
        &mut |key| Ok((key == "STATS_ENABLED").then(|| "true".into())),
        true,
        true,
    )
    .unwrap()
    .unwrap();
    config.index_dir = dir.path().join("index");
    (dir, store, identity, config)
}

#[tokio::test]
async fn real_stats_factory_shutdown_releases_index_before_database_close() {
    let (_dir, store, identity, config) = fixture().await;
    let mut products = Products::default();
    products
        .open_stats(
            &config,
            &store,
            identity.clone(),
            Arc::new(NoMembership),
            None,
        )
        .await
        .unwrap();
    assert!(products.stats.is_some());
    products.shutdown().await.unwrap();
    assert!(products.search_tasks.is_closed());
    assert!(products.search_tasks.is_empty());
    let mut reopened = Products::default();
    reopened
        .open_stats(&config, &store, identity, Arc::new(NoMembership), None)
        .await
        .unwrap();
    reopened.shutdown().await.unwrap();
    store.verify_schema().await.unwrap();
    store.close().await.unwrap();
}

#[tokio::test]
async fn failed_real_stats_factory_drains_unpublished_search_before_database_close() {
    let (_dir, store, identity, config) = fixture().await;
    std::fs::write(&config.index_dir, b"not an index directory").unwrap();
    let mut products = Products::default();
    assert_eq!(
        products
            .open_stats(&config, &store, identity, Arc::new(NoMembership), None)
            .await,
        Err(ServerError::StatsStartup)
    );
    assert!(products.stats.is_none());
    products.shutdown().await.unwrap();
    assert!(products.search_tasks.is_closed());
    assert!(products.search_tasks.is_empty());
    store.verify_schema().await.unwrap();
    store.close().await.unwrap();
}

#[tokio::test]
async fn canceled_real_stats_factory_closes_tracker_and_leaves_database_usable() {
    let (_dir, store, identity, config) = fixture().await;
    let connection = store.database().connection().await.unwrap();
    let mut products = Products::default();
    assert!(
        timeout(
            Duration::from_millis(20),
            products.open_stats(&config, &store, identity, Arc::new(NoMembership), None)
        )
        .await
        .is_err()
    );
    assert!(products.stats.is_none());
    drop(connection);
    products.shutdown().await.unwrap();
    assert!(products.search_tasks.is_closed());
    assert!(products.search_tasks.is_empty());
    store.verify_schema().await.unwrap();
    store.close().await.unwrap();
}

#[tokio::test]
async fn unpublished_blocking_search_ownership_survives_factory_cancellation() {
    let mut products = Products::default();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, hold) = std::sync::mpsc::channel();
    let task = products.search_tasks.spawn_blocking(move || {
        entered.send(()).unwrap();
        hold.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    timeout(Duration::from_secs(2), ready)
        .await
        .unwrap()
        .unwrap();
    drop(task);
    assert!(
        timeout(Duration::from_millis(20), products.shutdown())
            .await
            .is_err()
    );
    assert!(products.search_tasks.is_closed());
    assert_eq!(products.search_tasks.len(), 1);
    release.send(()).unwrap();
    timeout(Duration::from_secs(2), products.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(products.search_tasks.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn canceled_music_factory_preflight_is_killed_and_joined() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("worker");
    std::fs::write(&program, b"#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = MusicConfig::from_lookup(
        &mut |key| Ok((key == "MUSIC_ENABLED").then(|| "true".into())),
        true,
        true,
    )
    .unwrap()
    .unwrap();
    config.executor.program = program;
    config.executor.workspace_root = dir.path().to_path_buf();
    let mut products = Products::default();
    assert!(
        timeout(
            Duration::from_millis(20),
            products.check_worker(&config.executor)
        )
        .await
        .is_err()
    );
    let pid = products.preflight.as_ref().unwrap().id().unwrap();
    timeout(Duration::from_secs(2), products.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(products.preflight.is_none());
    #[cfg(target_os = "linux")]
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    #[cfg(not(target_os = "linux"))]
    let _ = pid;
}
