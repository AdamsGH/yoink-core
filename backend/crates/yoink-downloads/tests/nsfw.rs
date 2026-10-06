use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use yoink_domain::{
    CachedMedia, CachedMediaKind, ContentMetadata, DeliveryTarget, DownloadParams, DownloadRequest,
    MediaMode,
};
use yoink_downloads::{
    DeliveredMedia, Delivery, DeliveryArtifact, DeliveryFuture, DownloadConfig, DownloadService,
};
use yoink_executor::{Executor, ExecutorConfig};
use yoink_identity::IdentityStore;
use yoink_storage::{OperationStatus, Store};

struct Recorder {
    store: Arc<Store>,
    calls: AtomicUsize,
    revoke: bool,
}
impl Delivery for Recorder {
    fn bot_id(&self) -> i64 {
        123456
    }
    fn deliver(&self, _: DeliveryTarget, _: bool, _: Vec<DeliveryArtifact>) -> DeliveryFuture<'_> {
        panic!("must use policy-aware delivery")
    }
    fn deliver_with_policy(
        &self,
        target: DeliveryTarget,
        _: bool,
        spoiler: bool,
        media: Vec<DeliveryArtifact>,
    ) -> DeliveryFuture<'_> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(spoiler, target.chat_id() > 0);
            if self.revoke {
                yoink_db::query(include_str!(
                    "../../yoink-identity/tests/nsfw_change_group.sql"
                ))
                .bind(false)
                .execute(self.store.database())
                .await
                .unwrap();
            }
            Ok(media
                .into_iter()
                .enumerate()
                .map(|(i, _)| DeliveredMedia {
                    message_id: 1000 + (call * 10 + i) as i64,
                    file_id: Some(format!("cached_{i}")),
                })
                .collect())
        })
    }
}
async fn fixture(
    revoke: bool,
) -> (
    tempfile::TempDir,
    Arc<Store>,
    Arc<Recorder>,
    DownloadService,
) {
    fixture_worker(revoke, false).await
}
async fn fixture_worker(
    revoke: bool,
    fresh: bool,
) -> (
    tempfile::TempDir,
    Arc<Store>,
    Arc<Recorder>,
    DownloadService,
) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let workspace = dir.path().join("worker");
    std::fs::create_dir(&workspace).unwrap();
    let store = Arc::new(
        Store::open(&dir.path().join("nsfw.db"), true, Duration::from_secs(2))
            .await
            .unwrap(),
    );
    store.migrate().await.unwrap();
    yoink_db::raw_sql(include_str!("../../yoink-identity/tests/nsfw_fixture.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let delivery = Arc::new(Recorder {
        store: store.clone(),
        calls: AtomicUsize::new(0),
        revoke,
    });
    let identities = Arc::new(IdentityStore::new(
        store.database().clone(),
        Some(42),
        false,
    ));
    let executor = Executor::new(ExecutorConfig {
        program: PathBuf::from(if fresh { "/usr/bin/node" } else { "/bin/false" }),
        args: if fresh {
            vec![
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/nsfw_worker.mjs")
                    .to_string_lossy()
                    .into_owned(),
            ]
        } else {
            vec![]
        },
        workspace_root: workspace,
        timeout: Duration::from_secs(30),
        processing_timeout: Duration::from_secs(20),
        socket_timeout: Duration::from_secs(5),
        max_artifact_bytes: 1024 * 1024,
        max_workspace_bytes: 2 * 1024 * 1024,
        max_output_bytes: 1024 * 1024,
        max_artifacts: 100,
        allowed_proxy_origins: vec![],
        allowed_private_origins: vec![],
    })
    .unwrap();
    let service = DownloadService::new(
        store.clone(),
        identities,
        executor,
        delivery.clone(),
        DownloadConfig::default(),
    )
    .unwrap()
    .with_group_access(true);
    (dir, store, delivery, service)
}
fn request(chat: i64) -> DownloadRequest {
    DownloadRequest::new(
        44,
        "https://source.example/content-page.html",
        MediaMode::Video,
        None,
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(123456, chat, None, None).unwrap())
    .unwrap()
}
async fn warm(store: &Store, service: &DownloadService, metadata: bool, count: usize) {
    service.submit_request(&request(44), "warm").await.unwrap();
    let claim = store
        .claim_for_bot("warm", Duration::from_secs(30), 123456)
        .await
        .unwrap()
        .unwrap();
    store
        .begin_delivery_batch(&claim, count as u32)
        .await
        .unwrap();
    for (ordinal, start) in (0..count).step_by(10).enumerate() {
        let n = (count - start).min(10);
        store
            .begin_delivery_unit(&claim, ordinal as u32, n as u32)
            .await
            .unwrap();
        store
            .complete_delivery_unit(
                &claim,
                ordinal as u32,
                &(0..n).map(|i| 100 + (start + i) as i64).collect::<Vec<_>>(),
            )
            .await
            .unwrap();
    }
    let media = (0..count)
        .map(|i| {
            let m = CachedMedia::new(CachedMediaKind::Video, format!("warm_{i}"), 1024).unwrap();
            if metadata {
                m.with_metadata(ContentMetadata {
                    title: "Fixture sensitivecontent sample".into(),
                    ..Default::default()
                })
                .unwrap()
            } else {
                m
            }
        })
        .collect::<Vec<_>>();
    assert!(
        store
            .publish_media_cache(&claim, &media, Duration::from_secs(120))
            .await
            .unwrap()
    );
}
async fn finish(
    store: &Store,
    service: DownloadService,
    chat: i64,
) -> yoink_storage::OperationRecord {
    let id = service
        .submit_request(&request(chat), "check")
        .await
        .unwrap()
        .operation_id;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let token = shutdown.clone();
    let runner = tokio::spawn(async move { service.run(token).await });
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = store.get(44, id).await.unwrap().unwrap();
            if !matches!(
                record.status,
                OperationStatus::Queued | OperationStatus::Running | OperationStatus::Delivering
            ) {
                break record;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    shutdown.cancel();
    runner.await.unwrap().unwrap();
    result.unwrap()
}
#[tokio::test]
async fn nsfw_cached_metadata_group_block_and_private_spoiler() {
    for chat in [-100, 44] {
        let (_dir, store, delivery, service) = fixture(false).await;
        warm(&store, &service, true, 1).await;
        let record = finish(&store, service, chat).await;
        if chat < 0 {
            assert_eq!(record.status, OperationStatus::Failed);
            assert_eq!(record.content_failure.as_deref(), Some("nsfw_blocked"));
            assert_eq!(delivery.calls.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(record.status, OperationStatus::Succeeded);
            assert_eq!(delivery.calls.load(Ordering::SeqCst), 1);
        }
    }
}
#[tokio::test]
async fn nsfw_policy_rechecked_between_cache_units_preserves_confirmed_prefix() {
    let (_dir, store, delivery, service) = fixture(true).await;
    warm(&store, &service, true, 11).await;
    yoink_db::query(include_str!(
        "../../yoink-identity/tests/nsfw_change_group.sql"
    ))
    .bind(true)
    .execute(store.database())
    .await
    .unwrap();
    let record = finish(&store, service, -100).await;
    assert_eq!(record.status, OperationStatus::DeliveryPartial);
    assert_eq!(record.delivered_artifact_count, 10);
    assert_eq!(record.content_failure.as_deref(), Some("nsfw_blocked"));
    assert_eq!(delivery.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn nsfw_missing_cache_metadata_and_unavailable_rules_never_send() {
    for missing_metadata in [true, false] {
        let (_dir, store, delivery, service) = fixture(false).await;
        warm(&store, &service, !missing_metadata, 1).await;
        if !missing_metadata {
            yoink_db::raw_sql(include_str!("nsfw_drop_rules.sql"))
                .execute(store.database())
                .await
                .unwrap();
        }
        let record = finish(&store, service, -100).await;
        assert_eq!(record.status, OperationStatus::Failed);
        assert_eq!(delivery.calls.load(Ordering::SeqCst), 0);
        if !missing_metadata {
            assert_eq!(
                record.content_failure.as_deref(),
                Some("nsfw_policy_unavailable")
            );
        }
    }
}

#[tokio::test]
async fn nsfw_fresh_source_metadata_blocks_group_and_spoils_private() {
    for chat in [-100, 44] {
        let (_dir, store, delivery, service) = fixture_worker(false, true).await;
        let record = finish(&store, service, chat).await;
        if chat < 0 {
            assert_eq!(record.content_failure.as_deref(), Some("nsfw_blocked"));
            assert_eq!(delivery.calls.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(record.status, OperationStatus::Succeeded);
            assert_eq!(delivery.calls.load(Ordering::SeqCst), 1);
        }
    }
}
#[tokio::test]
async fn nsfw_blur_setting_is_current_versioned_and_preserves_cache_and_preferences() {
    use yoink_downloads::{DownloadError, SettingChange, SettingField};
    let (_dir, store, _delivery, service) = fixture(false).await;
    warm(&store, &service, true, 1).await;
    let target = DeliveryTarget::new(123456, 44, None, None).unwrap();
    let before = service.settings(44, target).await.unwrap();
    assert!(
        before
            .values
            .contains(&(SettingField::NsfwBlur, "true".into()))
    );
    let change = SettingChange::new(SettingField::NsfwBlur, "false").unwrap();
    service
        .change_setting(44, target, change.clone(), Some(before.revision))
        .await
        .unwrap();
    assert_eq!(
        service
            .change_setting(44, target, change, Some(before.revision))
            .await
            .unwrap_err(),
        DownloadError::Conflict
    );
    let after = service.settings(44, target).await.unwrap();
    assert!(
        after
            .values
            .contains(&(SettingField::NsfwBlur, "false".into()))
    );
    for value in before
        .values
        .into_iter()
        .filter(|(field, _)| *field != SettingField::NsfwBlur)
    {
        assert!(after.values.contains(&value));
    }
    assert!(
        store
            .lookup_media_cache(&request(44))
            .await
            .unwrap()
            .is_some()
    );
    let identities = IdentityStore::new(store.database().clone(), Some(42), false);
    assert!(
        !identities
            .content_policy(
                44,
                44,
                "https://source.example/content-page.html",
                &[ContentMetadata {
                    title: "sensitivecontent".into(),
                    ..Default::default()
                }]
            )
            .await
            .unwrap()
            .spoiler
    );
}
