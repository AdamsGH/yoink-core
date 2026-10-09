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
    outcome: AtomicUsize,
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
            match self.outcome.load(Ordering::SeqCst) {
                1 => return Err(yoink_downloads::DeliveryError::Rejected),
                2 => return Err(yoink_downloads::DeliveryError::Unknown),
                _ => {}
            }
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
        outcome: AtomicUsize::new(0),
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
async fn metrics_count_real_cache_fresh_success_and_executor_error_events() {
    use yoink_application::metrics::Metrics;
    let (_dir, store, _, service) = fixture(false).await;
    let metrics = Metrics::default();
    let service = service.with_metrics(metrics.clone());
    warm(&store, &service, true, 1).await;
    assert!(metrics.snapshot().counters.is_empty());
    assert_eq!(
        finish(&store, service, 44).await.status,
        OperationStatus::Succeeded
    );
    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.counters.len(), 1);
    assert_eq!(snapshot.counters["cache_hits"], 1);
    assert!(snapshot.histograms.is_empty());
    store.close().await.unwrap();
    let (_dir, store, _, service) = fixture_worker(false, true).await;
    let metrics = Metrics::default();
    let service = service.with_metrics(metrics.clone());
    assert_eq!(
        finish(&store, service, 44).await.status,
        OperationStatus::Succeeded
    );
    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.counters["downloads_total"], 1);
    assert_eq!(snapshot.counters["downloads_ok"], 1);
    assert_eq!(snapshot.counters["download_bytes_total"], 16);
    assert!(!snapshot.counters.contains_key("downloads_error"));
    assert_eq!(snapshot.histograms["download_duration_seconds"].count, 1);
    store.close().await.unwrap();
    let (_dir, store, _, service) = fixture(false).await;
    let metrics = Metrics::default();
    let service = service.with_metrics(metrics.clone());
    let failed = finish(&store, service, 44).await;
    assert_eq!(failed.status, OperationStatus::Failed);
    assert_eq!(failed.attempt, failed.max_attempts);
    assert!(failed.attempt > 1);
    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counters["downloads_total"],
        u64::from(failed.attempt)
    );
    assert_eq!(
        snapshot.counters["downloads_error"],
        u64::from(failed.attempt)
    );
    assert!(!snapshot.counters.contains_key("downloads_ok"));
    assert!(snapshot.histograms.is_empty());
    store.close().await.unwrap();
}
#[tokio::test]
async fn metrics_count_native_quota_denial_not_admission_or_idempotent_replay() {
    let (_dir, store, _, service) = fixture(false).await;
    yoink_db::query(include_str!(
        "../../yoink-storage/sql/test_downloader_setting.sql"
    ))
    .bind("dl.rate_limit_per_minute")
    .bind("1")
    .execute(store.database())
    .await
    .unwrap();
    let metrics = yoink_application::metrics::Metrics::default();
    let service = service.with_metrics(metrics.clone());
    assert!(
        service
            .submit_request(&request(44), "one")
            .await
            .unwrap()
            .created
    );
    assert!(
        !service
            .submit_request(&request(44), "one")
            .await
            .unwrap()
            .created
    );
    assert!(metrics.snapshot().counters.is_empty());
    assert!(matches!(
        service.submit_request(&request(44), "two").await,
        Err(yoink_downloads::DownloadError::QuotaExceeded { .. })
    ));
    assert_eq!(metrics.snapshot().counters["rate_limited"], 1);
    assert_eq!(metrics.snapshot().counters.len(), 1);
    store.close().await.unwrap();
}
#[tokio::test]
async fn metrics_count_confirmed_rejection_but_not_unknown_delivery_as_error() {
    for outcome in [1, 2] {
        let (_dir, store, delivery, service) = fixture_worker(false, true).await;
        delivery.outcome.store(outcome, Ordering::SeqCst);
        let metrics = yoink_application::metrics::Metrics::default();
        let service = service.with_metrics(metrics.clone());
        let record = finish(&store, service, 44).await;
        assert_ne!(record.status, OperationStatus::Succeeded);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.counters["downloads_total"], 1);
        assert!(!snapshot.counters.contains_key("downloads_ok"));
        assert!(!snapshot.counters.contains_key("download_bytes_total"));
        assert_eq!(
            snapshot.counters.get("downloads_error").copied(),
            if outcome == 1 { Some(1) } else { None }
        );
        assert!(snapshot.histograms.is_empty());
        store.close().await.unwrap();
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
            .lookup_media_cache(&request(44).freeze_provider_routing())
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

#[tokio::test]
async fn nsfw_mark_command_replay_denial_and_clean_cached_forced_delivery() {
    use yoink_downloads::DownloadError;
    for chat in [-100, 44] {
        let (_dir, store, delivery, service) = fixture(false).await;
        warm(&store, &service, true, 1).await;
        // Cache metadata is now unclassified. The arm, not metadata, forces policy.
        yoink_db::raw_sql(include_str!("nsfw_clean_rules.sql"))
            .execute(store.database())
            .await
            .unwrap();
        let target = DeliveryTarget::new(123456, chat, None, None).unwrap();
        assert_eq!(
            service.mark_next_nsfw(45, target, 1).await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service.mark_next_nsfw(44, target, 0).await,
            Err(DownloadError::InvalidPreferences)
        );
        service.mark_next_nsfw(44, target, 10).await.unwrap();
        service.mark_next_nsfw(44, target, 10).await.unwrap();
        let req = request(chat);
        assert!(
            service
                .submit_request(&request(-101), "denied-target")
                .await
                .is_err()
        );
        assert!(service.submit_request(&req, "").await.is_err());
        let pending: i64 = yoink_db::query(include_str!("nsfw_mark_state.sql"))
            .bind(44_i64)
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        assert_eq!(
            pending, 1,
            "denied and invalid service admissions leave the arm intact"
        );
        let first = service
            .submit_request(&req, "mark-admission")
            .await
            .unwrap();
        assert!(
            !service
                .submit_request(&req, "mark-admission")
                .await
                .unwrap()
                .created
        );
        // An old Telegram update, retried after consumption, must not re-arm.
        service.mark_next_nsfw(44, target, 10).await.unwrap();
        service.mark_next_nsfw(44, target, 9).await.unwrap();
        let count: i64 = yoink_db::query(include_str!("nsfw_mark_state.sql"))
            .bind(44_i64)
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        assert_eq!(count, 0);
        // Re-arm with genuinely newer intent; the pending first request remains forced.
        service.mark_next_nsfw(44, target, 11).await.unwrap();
        let record = finish(&store, service, chat).await;
        if chat < 0 {
            assert_eq!(record.content_failure.as_deref(), Some("nsfw_blocked"));
            assert_eq!(delivery.calls.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(record.status, OperationStatus::Succeeded);
            assert_eq!(delivery.calls.load(Ordering::SeqCst), 2);
        }
        assert!(store.get(44, first.operation_id).await.unwrap().is_some());
        let cached = store
            .lookup_media_cache(&req.clone().freeze_provider_routing())
            .await
            .unwrap()
            .unwrap();
        assert!(cached.media.iter().all(|m| m.metadata().is_some()));
    }
}

#[tokio::test]
async fn nsfw_mark_forces_fresh_clean_metadata_and_current_group_policy_between_units() {
    for fresh in [true, false] {
        let (_dir, store, delivery, service) = fixture_worker(!fresh, fresh).await;
        if !fresh {
            warm(&store, &service, true, 11).await;
            yoink_db::query(include_str!(
                "../../yoink-identity/tests/nsfw_change_group.sql"
            ))
            .bind(true)
            .execute(store.database())
            .await
            .unwrap();
        }
        yoink_db::raw_sql(include_str!("nsfw_clean_rules.sql"))
            .execute(store.database())
            .await
            .unwrap();
        let chat = if fresh { 44 } else { -100 };
        let target = DeliveryTarget::new(123456, chat, None, None).unwrap();
        service.mark_next_nsfw(44, target, 1).await.unwrap();
        let record = finish(&store, service, chat).await;
        if fresh {
            assert_eq!(record.status, OperationStatus::Succeeded);
        } else {
            assert_eq!(record.status, OperationStatus::DeliveryPartial);
            assert_eq!(record.delivered_artifact_count, 10);
            assert_eq!(record.content_failure.as_deref(), Some("nsfw_blocked"));
        }
        assert_eq!(delivery.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn nsfw_reload_current_role_and_forced_intent_never_overrides_private_blur() {
    let (_dir, store, _delivery, service) = fixture(false).await;
    let target = |actor| DeliveryTarget::new(123456, actor, None, None).unwrap();
    assert!(service.reload_nsfw(44, target(44)).await.is_err());
    service.reload_nsfw(43, target(43)).await.unwrap();
    yoink_db::raw_sql(include_str!("../../yoink-identity/tests/nsfw_demote.sql"))
        .execute(store.database())
        .await
        .unwrap();
    assert!(service.reload_nsfw(43, target(43)).await.is_err());
    service
        .change_setting(
            44,
            target(44),
            yoink_downloads::SettingChange::new(yoink_downloads::SettingField::NsfwBlur, "false")
                .unwrap(),
            None,
        )
        .await
        .unwrap();
    let identities = IdentityStore::new(store.database().clone(), Some(42), false);
    let decision = identities
        .content_policy_for_request(
            44,
            44,
            "https://clean.example/test",
            &[ContentMetadata::default()],
            true,
        )
        .await
        .unwrap();
    assert!(!decision.spoiler);
    assert!(
        identities
            .content_policy_for_request(
                44,
                -100,
                "https://clean.example/test",
                &[ContentMetadata::default()],
                true
            )
            .await
            .unwrap()
            .blocked
    );
}
