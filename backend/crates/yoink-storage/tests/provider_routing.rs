use std::time::Duration;
use yoink_domain::{DeliveryTarget, DownloadParams, DownloadRequest, Engine, MediaMode};
use yoink_storage::{AdmissionPolicy, Store, StoreError, media_cache_key};

#[tokio::test]
async fn provider_routing_persisted_snapshot_replay_restart_and_cache_partition() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = directory.path().join("routing.db");
    let store = Store::open(&path, true, Duration::from_secs(5))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let raw = DownloadRequest::new(
        42,
        "https://instagram.com/p/fixture",
        MediaMode::Video,
        None,
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(700, 42, None, None).unwrap())
    .unwrap();
    let frozen = raw.clone().freeze_provider_routing();
    assert_eq!(frozen.params().engine(), Engine::AutoFallbackV1);
    let policy = AdmissionPolicy {
        max_attempts: 1,
        retry_delay: Duration::ZERO,
        max_pending_global: 2,
        max_pending_actor: 1,
    };
    let first = store
        .admit_network_snapshot(&frozen, "routing-replay", policy)
        .await
        .unwrap();
    let replay = store
        .replay_network_admission(&raw, "routing-replay")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.operation_id, replay.operation_id);
    assert!(!replay.created);
    let explicit = raw
        .clone()
        .with_params(raw.params().clone().with_engine(Engine::YtDlp))
        .unwrap();
    assert_eq!(
        store
            .replay_network_admission(&explicit, "routing-replay")
            .await,
        Err(StoreError::Conflict)
    );
    assert_ne!(
        media_cache_key(&raw).unwrap(),
        media_cache_key(&frozen).unwrap()
    );
    assert_ne!(
        media_cache_key(&explicit).unwrap(),
        media_cache_key(&frozen).unwrap()
    );
    drop(store);
    let reopened = Store::open(&path, false, Duration::from_secs(5))
        .await
        .unwrap();
    let replay = reopened
        .replay_network_admission(&raw, "routing-replay")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.operation_id, replay.operation_id);
    let claim = reopened
        .claim_for_bot("routing-fixture", Duration::from_secs(30), 700)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.request, frozen);
    assert_eq!(claim.operation_id, first.operation_id);
    reopened.fail(&claim, false).await.unwrap();
}
