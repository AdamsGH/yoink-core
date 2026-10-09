use super::*;
use std::{
    future::{Future, poll_fn},
    task::Poll,
    time::Duration,
};
use tokio::time::timeout;

const DEADLINE: Duration = Duration::from_secs(10);

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
        Store::open(&dir.path().join("products.db"), true, DEADLINE)
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
    config.import_root = dir.path().join("imports");
    (dir, store, identity, config)
}

#[test]
fn native_product_defaults_do_not_install_opt_in_factories() {
    for (database, telegram) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut keys = Vec::new();
        let mut read = |key: &str| {
            keys.push(key.to_owned());
            Ok(None)
        };
        assert!(
            MusicConfig::from_lookup(&mut read, database, telegram)
                .unwrap()
                .is_none()
        );
        assert!(
            StatsConfig::from_lookup(&mut read, database, telegram)
                .unwrap()
                .is_none()
        );
        assert_eq!(keys, ["MUSIC_ENABLED", "STATS_ENABLED"]);
    }
}

#[test]
fn canceled_actual_stats_factory_retains_queued_index_owner_until_shutdown() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap()
        .block_on(async {
            let (_dir, store, identity, config) = timeout(DEADLINE, fixture()).await.unwrap();
            let mut products = Products::default();
            let tracker = products.search_tasks.clone();
            {
                let startup = products.open_stats(
                    &config,
                    &store,
                    identity.clone(),
                    Arc::new(NoMembership),
                    None,
                );
                tokio::pin!(startup);
                timeout(DEADLINE, async {
                    loop {
                        let registered = poll_fn(|cx| {
                            if let Poll::Ready(result) = startup.as_mut().poll(cx) {
                                panic!("factory finished before queued index work: {result:?}");
                            }
                            Poll::Ready(!tracker.is_empty())
                        })
                        .await;
                        if registered {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
            }
            // No yield since registration: the tracked async owner cannot yet
            // enqueue NativeIndex::open. File verification already completed.
            let (entered, ready) = tokio::sync::oneshot::channel();
            let (release, hold) = std::sync::mpsc::channel();
            let gate = tokio::task::spawn_blocking(move || {
                entered.send(()).unwrap();
                hold.recv_timeout(DEADLINE).unwrap();
            });
            timeout(DEADLINE, ready).await.unwrap().unwrap();
            assert!(products.stats.is_none());
            assert_eq!(tracker.len(), 1);
            assert!(!config.index_dir.exists());
            assert!(
                timeout(Duration::from_millis(20), products.shutdown())
                    .await
                    .is_err()
            );
            assert!(tracker.is_closed());
            assert_eq!(tracker.len(), 1);
            release.send(()).unwrap();
            timeout(DEADLINE, gate).await.unwrap().unwrap();
            timeout(DEADLINE, products.shutdown())
                .await
                .unwrap()
                .unwrap();
            assert!(tracker.is_empty());
            assert!(
                config.index_dir.is_dir(),
                "the canceled factory's real index action ran"
            );
            store.verify_schema().await.unwrap();
            let mut reopened = Products::default();
            timeout(
                DEADLINE,
                reopened.open_stats(&config, &store, identity, Arc::new(NoMembership), None),
            )
            .await
            .unwrap()
            .unwrap();
            timeout(DEADLINE, reopened.shutdown())
                .await
                .unwrap()
                .unwrap();
            store.close().await.unwrap();
        });
}

#[cfg(unix)]
#[tokio::test]
async fn installed_factories_publish_owned_clones_and_close_before_database() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (dir, store, identity, mut config) = timeout(DEADLINE, fixture()).await.unwrap();
    identity
        .register(&yoink_identity::TrustedProfile {
            user_id: 42,
            username: None,
            first_name: None,
            photo_url: None,
        })
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        timeout(DEADLINE, async {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut bytes = [0; 1024];
                let n = socket.read(&mut bytes).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&bytes[..n]);
                assert!(request.len() <= 8192);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            assert!(String::from_utf8(request).unwrap().lines().next().unwrap().to_ascii_lowercase().contains("/bot123456:test/getme "));
            let body = r#"{"ok":true,"result":{"id":123456,"is_bot":true,"first_name":"Fixture","username":"fixture_bot","can_join_groups":false,"can_read_all_group_messages":false,"supports_inline_queries":true,"has_main_web_app":false}}"#;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }).await.unwrap();
    });
    let system = yoink_application::SystemService::new("test");
    let adapter = timeout(
        DEADLINE,
        TelegramAdapter::prepare(
            "123456:test".into(),
            Some(url::Url::parse(&format!("http://{address}")).unwrap()),
            42,
            system.clone(),
            Duration::from_secs(1),
            Duration::from_secs(1),
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .with_identity_store(identity.clone());
    timeout(DEADLINE, server).await.unwrap().unwrap();

    let program = dir.path().join("preflight");
    std::fs::write(&program, b"#!/bin/sh\n[ \"$#\" -eq 1 ] && [ \"$1\" = --check ] || exit 99\nprintf checked > preflight.checked\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut music = MusicConfig::from_lookup(
        &mut |key| Ok((key == "MUSIC_ENABLED").then(|| "true".into())),
        true,
        true,
    )
    .unwrap()
    .unwrap();
    music.executor.program = program;
    music.executor.workspace_root = dir.path().to_path_buf();
    let mut products = Products::default();
    let adapter = timeout(
        DEADLINE,
        products.music(music, store.clone(), identity.clone(), adapter, false),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("preflight.checked")).unwrap(),
        b"checked"
    );
    assert!(products.preflight.is_none());
    assert!(products.executor.is_some());
    assert!(products.music.is_some());
    assert!(products.persistence.is_some());
    assert!(products.music_handlers.is_some());
    config.names.first_delay = Duration::from_secs(3600);
    config.search_interval = Duration::from_secs(3600);
    let adapter = timeout(
        DEADLINE,
        products.stats(config, store.clone(), identity.clone(), None, adapter),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(adapter.bot_id(), 123456);
    let stats = products.stats.as_ref().unwrap().clone();
    let handlers = products.stats_handlers.as_ref().unwrap().clone();
    let imports = products.imports.as_ref().unwrap().clone();
    assert_eq!(imports.config().root, dir.path().join("imports"));
    drop(imports.reserve_input().unwrap());
    let observer = handlers.observer();
    timeout(
        DEADLINE,
        observer(serde_json::json!({
            "update_id": 1,
            "message": {
                "message_id": 1, "date": 1700000000,
                "chat": {"id": -100, "type": "supergroup", "title": "Fixture"},
                "from": {"id": 42, "is_bot": false, "first_name": "Fixture"},
                "text": "factory observation"
            }
        })),
    )
    .await
    .unwrap()
    .unwrap();
    let groups = stats.groups(42).await.unwrap();
    assert_eq!(groups[0]["chat_id"], -100);
    assert_eq!(groups[0]["message_count"], 1);
    timeout(DEADLINE, stats.refresh()).await.unwrap().unwrap();
    let http = yoink_http::LateMusicStats::default();
    http.install_with_stats_imports(
        system.clone(),
        yoink_http::AuthState::new(false),
        products.persistence.clone(),
        Some(stats.clone()),
        yoink_http::activity::Activity::default(),
        Some(imports.clone()),
    )
    .unwrap();
    let cancel = CancellationToken::new();
    let jobs = run_stats_jobs(
        handlers.clone(),
        products.stats_config.take().unwrap(),
        cancel.clone(),
    );
    tokio::pin!(jobs);
    assert!(timeout(Duration::from_millis(20), &mut jobs).await.is_err());
    cancel.cancel();
    timeout(DEADLINE, &mut jobs).await.unwrap().unwrap();
    http.stop_admission();
    timeout(DEADLINE, http.drain()).await.unwrap();
    timeout(DEADLINE, products.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(products.search_tasks.is_closed());
    assert!(products.search_tasks.is_empty());
    assert!(matches!(
        stats.refresh().await,
        Err(yoink_stats::Error::Unavailable)
    ));
    assert!(handlers.recover_search().await.is_err());
    assert!(observer(serde_json::json!({"update_id": 2})).await.is_err());
    assert!(imports.reserve_input().is_err());
    // Retained adapter/HTTP/service clones must not keep the Tantivy writer locked.
    let mut reopened = Products::default();
    let mut config = StatsConfig::from_lookup(
        &mut |key| Ok((key == "STATS_ENABLED").then(|| "true".into())),
        true,
        true,
    )
    .unwrap()
    .unwrap();
    config.index_dir = dir.path().join("index");
    timeout(
        DEADLINE,
        reopened.open_stats(&config, &store, identity, Arc::new(NoMembership), None),
    )
    .await
    .unwrap()
    .unwrap();
    timeout(DEADLINE, reopened.shutdown())
        .await
        .unwrap()
        .unwrap();
    store.verify_schema().await.unwrap();
    store.close().await.unwrap();
    drop((adapter, http, stats, handlers, imports));
}
