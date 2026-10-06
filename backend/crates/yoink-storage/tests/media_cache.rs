mod common;
use common::store;
use std::time::Duration;

use uuid::Uuid;
use yoink_domain::{
    CachedMedia, CachedMediaKind, DeliveryTarget, DownloadParams, DownloadRequest, MediaMode,
};
use yoink_storage::{Store, StoreError};

fn request(actor: u64, bot: i64, url: &str) -> DownloadRequest {
    DownloadRequest::new(
        actor,
        url,
        MediaMode::Video,
        None,
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(bot, actor as i64, None, None).unwrap())
    .unwrap()
}

async fn delivered_claim(store: &Store, request: &DownloadRequest) -> yoink_storage::Claim {
    let admitted = store
        .admit(request, "media-cache-operation", 1)
        .await
        .unwrap();
    let claim = store
        .claim_for_bot(
            "media-cache-worker",
            Duration::from_secs(30),
            request.target().unwrap().bot_id(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.operation_id, admitted.operation_id);
    assert!(!claim.cache_bypassed);
    assert!(store.begin_delivery_batch(&claim, 1).await.unwrap());
    assert!(store.begin_delivery_unit(&claim, 0, 1).await.unwrap());
    assert!(
        store
            .complete_delivery_unit(&claim, 0, &[70001])
            .await
            .unwrap()
    );
    claim
}

#[tokio::test]
async fn cache_cleanup_and_revision_invalidation_contract() {
    let store = store().await;
    let request = request(73_010, 821, "https://example.com/cache-cleanup");
    let claim = delivered_claim(&store, &request).await;
    let media = CachedMedia::new(CachedMediaKind::Video, "fresh-upload-id".into(), 1024).unwrap();
    let expired_revision = Uuid::new_v4();
    yoink_db::query(include_str!("fixtures/seed_expired_media_cache.sql"))
        .bind(expired_revision)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        store
            .publish_media_cache(
                &claim,
                std::slice::from_ref(&media),
                Duration::from_secs(120)
            )
            .await
            .unwrap()
    );
    let remaining: i64 = yoink_db::query(include_str!("fixtures/count_media_cache_revision.sql"))
        .bind(expired_revision)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(remaining, 8, "publication prunes at most 32 expired rows");
    let first = store.lookup_media_cache(&request).await.unwrap().unwrap();
    assert!(
        !store
            .invalidate_media_cache(&request, Uuid::new_v4())
            .await
            .unwrap()
    );
    assert!(
        store
            .publish_media_cache(
                &claim,
                std::slice::from_ref(&media),
                Duration::from_secs(120)
            )
            .await
            .unwrap()
    );
    let replacement = store.lookup_media_cache(&request).await.unwrap().unwrap();
    assert_ne!(first.revision, replacement.revision);
    assert!(
        !store
            .invalidate_media_cache(&request, first.revision)
            .await
            .unwrap()
    );
    let foreign = request
        .clone()
        .with_target(DeliveryTarget::new(822, 73_010, None, None).unwrap())
        .unwrap();
    assert!(
        !store
            .invalidate_media_cache(&foreign, replacement.revision)
            .await
            .unwrap()
    );
    assert!(
        store
            .invalidate_media_cache(&request, replacement.revision)
            .await
            .unwrap()
    );
    assert!(store.lookup_media_cache(&request).await.unwrap().is_none());
    store.close().await.unwrap();
}

#[tokio::test]
async fn cache_publication_partition_expiry_and_invalid_recovery_contract() {
    let store = store().await;
    let base_request = request(73_001, 811, "https://example.com/cache-a");
    let media =
        CachedMedia::new(CachedMediaKind::Video, "opaque-cache-file-id".into(), 88).unwrap();
    assert!(
        !store
            .publish_media_cache(
                &yoink_storage::Claim {
                    operation_id: Uuid::nil(),
                    attempt: 1,
                    lease_token: Uuid::nil(),
                    worker_id: "worker".into(),
                    request: base_request.clone(),
                    cancel_requested: false,
                    cache_bypassed: false,
                    lease_expires_at: chrono::Utc::now(),
                },
                std::slice::from_ref(&media),
                Duration::from_secs(1),
            )
            .await
            .unwrap()
    );

    let claim = delivered_claim(&store, &base_request).await;
    let stored_key: String =
        yoink_db::query(include_str!("fixtures/select_operation_cache_key.sql"))
            .bind(claim.operation_id)
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get("cache_key")
            .unwrap();
    assert_eq!(
        stored_key,
        yoink_storage::media_cache_key(&base_request)
            .unwrap()
            .as_str()
    );
    assert!(
        store
            .publish_media_cache(
                &claim,
                std::slice::from_ref(&media),
                Duration::from_secs(120)
            )
            .await
            .unwrap()
    );
    let cached = store
        .lookup_media_cache(&base_request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cached.media, vec![media]);
    assert!(!format!("{cached:?}").contains("opaque-cache-file-id"));

    let other_actor = request(73_002, 811, "https://example.com/cache-a");
    let other_bot = request(73_001, 812, "https://example.com/cache-a");
    assert!(
        store
            .lookup_media_cache(&other_actor)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .lookup_media_cache(&other_bot)
            .await
            .unwrap()
            .is_none()
    );
    yoink_db::query(include_str!("fixtures/expire_media_cache.sql"))
        .bind(cached.revision)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        store
            .lookup_media_cache(&base_request)
            .await
            .unwrap()
            .is_none()
    );

    let recovery_request = request(73_003, 813, "https://example.com/recover-cache");
    let admission = store
        .admit(&recovery_request, "recover-cache", 1)
        .await
        .unwrap();
    let recovery_claim = store
        .claim_for_bot("recovery-worker", Duration::from_secs(30), 813)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovery_claim.operation_id, admission.operation_id);
    assert!(
        store
            .begin_delivery_batch(&recovery_claim, 1)
            .await
            .unwrap()
    );
    assert!(
        store
            .begin_delivery_unit(&recovery_claim, 0, 1)
            .await
            .unwrap()
    );
    let recovery_key = yoink_storage::media_cache_key(&recovery_request).unwrap();
    yoink_db::query(include_str!("fixtures/insert_media_cache.sql"))
        .bind(73_003_i64)
        .bind(813_i64)
        .bind(recovery_key.as_str())
        .bind(cached.revision)
        .bind(serde_json::json!([{"kind":"video","file_id":"seed-cache-id","size_bytes":88}]))
        .execute(store.database())
        .await
        .unwrap();
    let mut foreign = recovery_claim.clone();
    foreign.request = request(73_004, 814, "https://example.com/foreign-claim");
    assert!(
        !store
            .requeue_invalid_cache(&foreign, 0, cached.revision)
            .await
            .unwrap()
    );
    assert!(
        !store
            .requeue_invalid_cache(&recovery_claim, 1, cached.revision)
            .await
            .unwrap()
    );
    let mut stale = recovery_claim.clone();
    stale.lease_token = Uuid::new_v4();
    assert!(
        !store
            .requeue_invalid_cache(&stale, 0, cached.revision)
            .await
            .unwrap()
    );
    let replacement = Uuid::new_v4();
    yoink_db::query(include_str!("fixtures/replace_media_cache_revision.sql"))
        .bind(73_003_i64)
        .bind(813_i64)
        .bind(replacement)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        store
            .requeue_invalid_cache(&recovery_claim, 0, cached.revision)
            .await
            .unwrap()
    );
    let record = store
        .get(73_003, admission.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.max_attempts, 2);
    assert!(record.cache_bypassed);
    assert_eq!(
        yoink_db::query(include_str!("fixtures/count_media_cache_revision.sql"))
            .bind(replacement)
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get::<i64, _>("count")
            .unwrap(),
        1
    );
    let retried = store
        .claim_for_bot("retry-worker", Duration::from_secs(30), 813)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retried.attempt, 2);
    assert!(retried.cache_bypassed);
    assert!(store.begin_delivery_batch(&retried, 1).await.unwrap());
    assert!(store.begin_delivery_unit(&retried, 0, 1).await.unwrap());
    assert!(
        !store
            .requeue_invalid_cache(&retried, 0, replacement)
            .await
            .unwrap(),
        "a cache-bypassed sending attempt cannot gain another attempt"
    );
    assert!(
        store
            .complete_delivery_unit(&retried, 0, &[70002])
            .await
            .unwrap()
    );
    let fresh = CachedMedia::new(CachedMediaKind::Video, "fresh-upload-id".into(), 1024).unwrap();
    assert!(
        store
            .publish_media_cache(
                &retried,
                std::slice::from_ref(&fresh),
                Duration::from_secs(120)
            )
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .lookup_media_cache(&recovery_request)
            .await
            .unwrap()
            .unwrap()
            .media,
        vec![fresh]
    );
    assert!(
        !store
            .requeue_invalid_cache(&retried, 0, replacement)
            .await
            .unwrap()
    );

    store.close().await.unwrap();
}

#[tokio::test]
async fn cache_refuses_confirmed_prefix_and_cancelled_recovery() {
    let store = store().await;
    let no_recovery_request = request(73_101, 821, "https://example.com/no-recovery");
    let admission = store
        .admit(&no_recovery_request, "no-recovery", 1)
        .await
        .unwrap();
    let claim = store
        .claim_for_bot("worker", Duration::from_secs(30), 821)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.operation_id, admission.operation_id);
    assert!(store.begin_delivery_batch(&claim, 2).await.unwrap());
    assert!(store.begin_delivery_unit(&claim, 0, 1).await.unwrap());
    assert!(
        store
            .complete_delivery_unit(&claim, 0, &[70011])
            .await
            .unwrap()
    );
    assert!(store.begin_delivery_unit(&claim, 1, 1).await.unwrap());
    assert!(
        !store
            .requeue_invalid_cache(&claim, 0, Uuid::new_v4())
            .await
            .unwrap()
    );

    let cancelled = request(73_102, 822, "https://example.com/cancel-recovery");
    let admission = store.admit(&cancelled, "cancel-recovery", 1).await.unwrap();
    let cancel_claim = store
        .claim_for_bot("cancel-worker", Duration::from_secs(30), 822)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cancel_claim.operation_id, admission.operation_id);
    assert!(store.begin_delivery_batch(&cancel_claim, 1).await.unwrap());
    assert!(
        store
            .begin_delivery_unit(&cancel_claim, 0, 1)
            .await
            .unwrap()
    );
    assert!(
        store
            .request_cancel(73_102, admission.operation_id)
            .await
            .unwrap()
    );
    assert!(
        !store
            .requeue_invalid_cache(&cancel_claim, 0, Uuid::new_v4())
            .await
            .unwrap()
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn cache_lookup_rejects_corrupt_bounded_payload_as_a_whole() {
    let store = store().await;
    let request = request(73_201, 831, "https://example.com/corruption");
    let claim = delivered_claim(&store, &request).await;
    let media = CachedMedia::new(CachedMediaKind::Photo, "known-good-id".into(), 1).unwrap();
    assert!(
        store
            .publish_media_cache(&claim, &[media], Duration::from_secs(60))
            .await
            .unwrap()
    );
    let key = yoink_storage::media_cache_key(&request).unwrap();
    yoink_db::query(include_str!("fixtures/corrupt_media_cache.sql"))
        .bind(request.actor_id() as i64)
        .bind(831_i64)
        .bind(key.as_str())
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        store.lookup_media_cache(&request).await.unwrap_err(),
        StoreError::InvalidStoredData
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn scoped_clear_deletes_all_url_formats_and_replays_stay_noops_after_reopen() {
    use yoink_domain::DownloadParams;

    let disk = common::unmigrated_store().await;
    disk.store.migrate().await.unwrap();
    let actor = 73_301_u64;
    let bot = 841_i64;
    let url = "https://example.com/media?part=1";
    yoink_db::query(include_str!("fixtures/seed_cache_clear_owner.sql"))
        .bind(actor as i64)
        .execute(disk.database())
        .await
        .unwrap();

    async fn add_cache(
        store: &Store,
        actor: u64,
        bot: i64,
        url: &str,
        mode: MediaMode,
        key_id: &str,
    ) -> DownloadRequest {
        let request = DownloadRequest::new(actor, url, mode, None, DownloadParams::default())
            .unwrap()
            .with_target(DeliveryTarget::new(bot, actor as i64, None, None).unwrap())
            .unwrap();
        store.admit(&request, key_id, 1).await.unwrap();
        let cache_key = yoink_storage::media_cache_key(&request).unwrap();
        yoink_db::query(include_str!("fixtures/insert_media_cache.sql"))
            .bind(actor as i64)
            .bind(bot)
            .bind(cache_key.as_str())
            .bind(Uuid::new_v4())
            .bind("[{\"kind\":\"video\",\"file_id\":\"opaque\",\"size_bytes\":1}]")
            .execute(store.database())
            .await
            .unwrap();
        request
    }

    let formats = [MediaMode::Video, MediaMode::Audio, MediaMode::Image];
    let mut requests = Vec::new();
    for (index, mode) in formats.into_iter().enumerate() {
        requests.push(
            add_cache(
                &disk.store,
                actor,
                bot,
                url,
                mode,
                &format!("clear-format-{index}"),
            )
            .await,
        );
    }
    let other_actor = add_cache(
        &disk.store,
        actor + 1,
        bot,
        url,
        MediaMode::Video,
        "clear-other-actor",
    )
    .await;
    let other_bot = add_cache(
        &disk.store,
        actor,
        bot + 1,
        url,
        MediaMode::Video,
        "clear-other-bot",
    )
    .await;
    let other_url = add_cache(
        &disk.store,
        actor,
        bot,
        "https://example.com/media?part=2",
        MediaMode::Video,
        "clear-other-url",
    )
    .await;
    let repopulated_request = requests[0].clone();

    let before = yoink_db::query(include_str!("fixtures/count_cache_clear_rows.sql"))
        .bind(actor as i64)
        .fetch_one(disk.database())
        .await
        .unwrap();
    let operations_before: i64 = before.try_get("operations").unwrap();
    let deliveries_before: i64 = before.try_get("deliveries").unwrap();
    assert_eq!(
        disk.store
            .clear_media_cache(actor, bot, 500, url, true)
            .await
            .unwrap(),
        Some(3)
    );
    for request in requests {
        assert!(
            disk.store
                .lookup_media_cache(&request)
                .await
                .unwrap()
                .is_none()
        );
    }
    for request in [&other_actor, &other_bot, &other_url] {
        assert!(
            disk.store
                .lookup_media_cache(request)
                .await
                .unwrap()
                .is_some()
        );
    }
    let after = yoink_db::query(include_str!("fixtures/count_cache_clear_rows.sql"))
        .bind(actor as i64)
        .fetch_one(disk.database())
        .await
        .unwrap();
    assert_eq!(
        after.try_get::<i64, _>("operations").unwrap(),
        operations_before
    );
    assert_eq!(
        after.try_get::<i64, _>("deliveries").unwrap(),
        deliveries_before
    );

    let repopulated_key = yoink_storage::media_cache_key(&repopulated_request).unwrap();
    yoink_db::query(include_str!("fixtures/insert_media_cache.sql"))
        .bind(actor as i64)
        .bind(bot)
        .bind(repopulated_key.as_str())
        .bind(Uuid::new_v4())
        .bind("[{\"kind\":\"video\",\"file_id\":\"repopulated\",\"size_bytes\":1}]")
        .execute(disk.database())
        .await
        .unwrap();
    let path = disk.path();
    disk.store.close().await.unwrap();
    let reopened = Store::open(&path, false, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        reopened
            .clear_media_cache(actor, bot, 499, url, true)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        reopened
            .clear_media_cache(actor, bot, 500, "https://example.com/media?part=2", true)
            .await
            .unwrap(),
        None
    );
    assert!(
        reopened
            .lookup_media_cache(&repopulated_request)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        reopened
            .lookup_media_cache(&other_url)
            .await
            .unwrap()
            .is_some()
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn cache_clear_rechecks_global_ban_before_advancing_watermark() {
    let disk = common::store().await;
    let actor = 73_302_u64;
    let bot = 842_i64;
    yoink_db::query(include_str!("fixtures/seed_cache_clear_owner.sql"))
        .bind(actor as i64)
        .execute(disk.database())
        .await
        .unwrap();
    yoink_db::query(include_str!("fixtures/ban_cache_clear_owner.sql"))
        .bind(actor as i64)
        .execute(disk.database())
        .await
        .unwrap();
    assert_eq!(
        disk.store
            .clear_media_cache(actor, bot, 600, "https://example.com/valid", true)
            .await,
        Err(StoreError::Unauthorized)
    );
    yoink_db::query(include_str!("fixtures/restore_cache_clear_owner.sql"))
        .bind(actor as i64)
        .execute(disk.database())
        .await
        .unwrap();
    assert_eq!(
        disk.store
            .clear_media_cache(actor, bot, 600, "https://example.com/valid", true)
            .await
            .unwrap(),
        Some(0)
    );
    assert_eq!(
        disk.store
            .clear_media_cache(actor, bot, 601, "https://bad@/not-a-valid-url", true)
            .await,
        Err(StoreError::InvalidInput)
    );
    assert_eq!(
        disk.store
            .clear_media_cache(actor, bot, 601, "https://example.com/valid", true)
            .await
            .unwrap(),
        Some(0)
    );
    disk.store.close().await.unwrap();
}

#[tokio::test]
async fn nsfw_cached_metadata_survives_disk_reopen() {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = dir.path().join("nsfw.db");
    let store = Store::open(&path, true, Duration::from_secs(2))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let request = request(73_020, 821, "https://example.com/metadata");
    let claim = delivered_claim(&store, &request).await;
    let metadata = yoink_domain::ContentMetadata {
        title: "metadata-only-sensitive".into(),
        description: "private prose".into(),
        tags: vec!["tag".into()],
        categories: vec!["category".into()],
    };
    let media = CachedMedia::new(CachedMediaKind::Video, "metadata-id".into(), 1024)
        .unwrap()
        .with_metadata(metadata.clone())
        .unwrap();
    assert!(
        store
            .publish_media_cache(&claim, &[media], Duration::from_secs(120))
            .await
            .unwrap()
    );
    drop(store);
    let reopened = Store::open(&path, false, Duration::from_secs(2))
        .await
        .unwrap();
    let batch = reopened
        .lookup_media_cache(&request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(batch.media[0].metadata(), Some(&metadata));
    assert!(!format!("{batch:?}").contains("private prose"));
}
