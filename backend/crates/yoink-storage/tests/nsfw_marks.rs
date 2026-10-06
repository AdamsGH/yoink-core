use std::time::Duration;
use yoink_db::{query, raw_sql};
use yoink_domain::{DeliveryTarget, DownloadParams, DownloadRequest, MediaMode};
use yoink_storage::{AdmissionPolicy, Store, StoreError, media_cache_key};

fn request() -> DownloadRequest {
    DownloadRequest::new(
        44,
        "https://source.example/clean",
        MediaMode::Video,
        None,
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(123456, 44, None, None).unwrap())
    .unwrap()
}
fn policy() -> AdmissionPolicy {
    AdmissionPolicy {
        max_attempts: 3,
        retry_delay: Duration::ZERO,
        max_pending_global: 100,
        max_pending_actor: 100,
    }
}
async fn mark(store: &Store) {
    query(include_str!("../sql/test_nsfw_mark_put.sql"))
        .bind(44_i64)
        .execute(store.database())
        .await
        .unwrap();
}
async fn snapshot(store: &Store, admission: &yoink_storage::Admission) -> serde_json::Value {
    query(include_str!("../sql/test_nsfw_mark_snapshot.sql"))
        .bind(admission.operation_id)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap()
}
async fn fixture() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Store::open(&dir.path().join("marks.db"), true, Duration::from_secs(5))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    raw_sql(include_str!("../../yoink-identity/tests/nsfw_fixture.sql"))
        .execute(store.database())
        .await
        .unwrap();
    (dir, store)
}

#[tokio::test]
async fn nsfw_mark_restart_atomic_snapshot_replay_conflict_capacity_invalid_and_concurrency() {
    let (dir, store) = fixture().await;
    let req = request();
    let original = store
        .admit_network_snapshot(&req, "original", policy())
        .await
        .unwrap();
    mark(&store).await;
    assert_eq!(
        store.admit_network_snapshot(&req, "", policy()).await,
        Err(StoreError::InvalidInput)
    );
    let changed = req
        .clone()
        .with_params(DownloadParams::new(Some(720)).unwrap())
        .unwrap();
    assert_eq!(
        store
            .admit_network_snapshot(&changed, "original", policy())
            .await,
        Err(StoreError::Conflict)
    );
    let replay = store
        .admit_network_snapshot(&req, "original", policy())
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(snapshot(&store, &original).await.get("force_nsfw"), None);
    let full = AdmissionPolicy {
        max_pending_global: 1,
        ..policy()
    };
    assert_eq!(
        store.admit_network_snapshot(&req, "capacity", full).await,
        Err(StoreError::Capacity)
    );
    raw_sql(include_str!("../sql/test_nsfw_mark_fail_insert.sql"))
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        store
            .admit_network_snapshot(&req, "insert-fails", policy())
            .await,
        Err(StoreError::Database)
    );
    raw_sql(include_str!("../sql/test_nsfw_mark_drop_trigger.sql"))
        .execute(store.database())
        .await
        .unwrap();
    drop(store);
    let store = Store::open(&dir.path().join("marks.db"), false, Duration::from_secs(5))
        .await
        .unwrap();
    store.verify_schema().await.unwrap();
    let (a, b) = tokio::join!(
        store.admit_network_snapshot(&req, "a", policy()),
        store.admit_network_snapshot(&req, "b", policy())
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    let snapshots = [snapshot(&store, &a).await, snapshot(&store, &b).await];
    assert_eq!(
        snapshots.iter().filter(|s| s["force_nsfw"] == true).count(),
        1
    );
    let forced = if snapshots[0]["force_nsfw"] == true {
        &a
    } else {
        &b
    };
    let before = snapshot(&store, forced).await;
    // Replaying a forced request is compared as caller intent, not its admitted mark.
    let key = if forced == &a { "a" } else { "b" };
    assert!(
        !store
            .admit_network_snapshot(&req, key, policy())
            .await
            .unwrap()
            .created
    );
    drop(store);
    let store = Store::open(&dir.path().join("marks.db"), false, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        snapshot(&store, forced).await,
        before,
        "job snapshot survives process restart"
    );
    let mut restored_forced = false;
    for worker in ["restart-a", "restart-b", "restart-original"] {
        let claim = store
            .claim_for_bot(worker, Duration::from_secs(30), 123456)
            .await
            .unwrap()
            .unwrap();
        if claim.operation_id == forced.operation_id {
            assert!(claim.request.force_nsfw());
            restored_forced = true;
        }
    }
    assert!(restored_forced);
    let caller_override = store
        .admit_network_snapshot(&req.clone().with_force_nsfw(true), "untrusted", policy())
        .await
        .unwrap();
    assert_eq!(
        snapshot(&store, &caller_override).await.get("force_nsfw"),
        None
    );
    assert_eq!(
        media_cache_key(&req).unwrap(),
        media_cache_key(&req.with_force_nsfw(true)).unwrap()
    );
}

#[tokio::test]
async fn nsfw_mark_quota_rejection_leaves_arm_and_queued_claim_restores_forced_intent() {
    let (_dir, store) = fixture().await;
    let store = store
        .with_quotas(
            yoink_storage::QuotaLimits {
                minute: 1,
                hour: 1,
                day: 1,
            },
            None,
        )
        .unwrap();
    let req = request();
    store
        .admit_network_snapshot(&req, "charge", policy())
        .await
        .unwrap();
    mark(&store).await;
    assert!(matches!(
        store
            .admit_network_snapshot(&req, "quota-denied", policy())
            .await,
        Err(StoreError::QuotaExceeded { .. })
    ));
    // Configured trusted owner exemption lets admission proceed; rejection did not consume.
    let store = store
        .with_quotas(yoink_storage::QuotaLimits::default(), Some(44))
        .unwrap();
    let forced = store
        .admit_network_snapshot(&req, "forced", policy())
        .await
        .unwrap();
    assert_eq!(snapshot(&store, &forced).await["force_nsfw"], true);
    let mut saw_forced = false;
    for worker in ["first", "second"] {
        let claim = store
            .claim_for_bot(worker, Duration::from_secs(30), 123456)
            .await
            .unwrap()
            .unwrap();
        if claim.operation_id == forced.operation_id {
            assert!(claim.request.force_nsfw());
            saw_forced = true;
        }
    }
    assert!(saw_forced);
}
