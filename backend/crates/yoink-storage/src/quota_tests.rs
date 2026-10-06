//! Native disk contracts with explicit admission time. No clock sleeps or production paths.
use super::*;
use yoink_domain::{DeliveryTarget, DownloadParams, MediaMode};

const NOW: i64 = 1_700_000_000_000_000;
fn policy() -> AdmissionPolicy {
    AdmissionPolicy {
        max_attempts: 3,
        retry_delay: Duration::ZERO,
        max_pending_global: 1000,
        max_pending_actor: 1000,
    }
}
fn request(actor: u64, bot: i64, chat: i64, topic: Option<i64>) -> DownloadRequest {
    DownloadRequest::new(
        actor,
        "https://example.test/media",
        MediaMode::Video,
        None,
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(bot, chat, topic, Some(10)).unwrap())
    .unwrap()
}
async fn fixture(limits: QuotaLimits, owner: Option<i64>) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Store::open(&dir.path().join("quota.db"), true, Duration::from_secs(5))
        .await
        .unwrap()
        .with_quotas(limits, owner)
        .unwrap();
    store.migrate().await.unwrap();
    (dir, store)
}
async fn admit(
    store: &Store,
    req: &DownloadRequest,
    key: &str,
    now: i64,
) -> Result<Admission, StoreError> {
    store
        .admit_inner(req, key, policy(), true, Some(now), None)
        .await
}
async fn counters(store: &Store, actor: u64) -> Vec<(String, i64, i64)> {
    query("SELECT \"window\",count,reset_at FROM rate_limits WHERE user_id=?1 ORDER BY \"window\"")
        .bind(actor as i64)
        .fetch_all(store.database())
        .await
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.try_get(0).unwrap(),
                r.try_get(1).unwrap(),
                r.try_get(2).unwrap(),
            )
        })
        .collect()
}
async fn seed(store: &Store, actor: u64, window: &str, count: i64, reset: i64) {
    query(include_str!("../sql/quota_put.sql"))
        .bind(actor as i64)
        .bind(window)
        .bind(count)
        .bind(reset)
        .execute(store.database())
        .await
        .unwrap();
}

#[tokio::test]
async fn quota_each_window_n_plus_one_and_equality() {
    for (limits, seconds) in [
        (
            QuotaLimits {
                minute: 2,
                hour: 100,
                day: 100,
            },
            60,
        ),
        (
            QuotaLimits {
                minute: 100,
                hour: 2,
                day: 100,
            },
            3600,
        ),
        (
            QuotaLimits {
                minute: 100,
                hour: 100,
                day: 2,
            },
            86400,
        ),
    ] {
        let (_dir, store) = fixture(limits, None).await;
        let req = request(7, 9, 7, None);
        for i in 0..2 {
            assert!(
                admit(&store, &req, &format!("n{i}"), NOW)
                    .await
                    .unwrap()
                    .created
            );
        }
        let before = counters(&store, 7).await;
        assert_eq!(
            admit(&store, &req, "over", NOW).await,
            Err(StoreError::QuotaExceeded {
                retry_after_secs: seconds
            })
        );
        assert_eq!(
            admit(&store, &req, "over", NOW + seconds as i64 * 1_000_000 - 1).await,
            Err(StoreError::QuotaExceeded {
                retry_after_secs: 1
            })
        );
        assert_eq!(counters(&store, 7).await, before);
        assert!(
            admit(&store, &req, "equal", NOW + seconds as i64 * 1_000_000)
                .await
                .unwrap()
                .created
        );
    }
}

#[tokio::test]
async fn quota_independent_anchors_and_rejection_does_not_reset_expired_window() {
    let (_dir, store) = fixture(
        QuotaLimits {
            minute: 2,
            hour: 2,
            day: 4,
        },
        None,
    )
    .await;
    let req = request(7, 9, 7, None);
    seed(&store, 7, "minute", 2, NOW).await;
    seed(&store, 7, "hour", 2, NOW + 7_000_000).await;
    seed(&store, 7, "day", 4, NOW + 11_000_001).await;
    let before = counters(&store, 7).await;
    assert_eq!(
        admit(&store, &req, "refused", NOW).await,
        Err(StoreError::QuotaExceeded {
            retry_after_secs: 12
        })
    );
    assert_eq!(counters(&store, 7).await, before);
    seed(&store, 7, "hour", 1, NOW + 7_000_000).await;
    seed(&store, 7, "day", 1, NOW + 11_000_001).await;
    admit(&store, &req, "new", NOW).await.unwrap();
    assert_eq!(
        counters(&store, 7).await,
        vec![
            ("day".into(), 2, NOW + 11_000_001),
            ("hour".into(), 2, NOW + 7_000_000),
            ("minute".into(), 1, NOW + 60_000_000)
        ]
    );
}

#[tokio::test]
async fn quota_full_replay_conflict_terminal_and_restart_are_noncharging() {
    let limits = QuotaLimits {
        minute: 1,
        hour: 1,
        day: 1,
    };
    let (dir, store) = fixture(limits, None).await;
    let req = request(7, 9, 7, None);
    let first = admit(&store, &req, "same", NOW).await.unwrap();
    let before = counters(&store, 7).await;
    assert!(!admit(&store, &req, "same", NOW).await.unwrap().created);
    assert_eq!(
        admit(&store, &request(7, 10, 7, None), "same", NOW).await,
        Err(StoreError::Conflict)
    );
    let claim = store
        .claim("worker", Duration::from_secs(60))
        .await
        .unwrap()
        .unwrap();
    store.complete(&claim).await.unwrap();
    store.close().await.unwrap();
    drop(store);
    let store = Store::open(&dir.path().join("quota.db"), false, Duration::from_secs(5))
        .await
        .unwrap()
        .with_quotas(limits, None)
        .unwrap();
    store.verify_schema().await.unwrap();
    let replay = admit(&store, &req, "same", NOW + 100_000_000_000)
        .await
        .unwrap();
    assert_eq!(replay.operation_id, first.operation_id);
    assert!(!replay.created);
    assert_eq!(counters(&store, 7).await, before);
}

#[tokio::test]
async fn quota_duplicate_and_distinct_key_last_slot_races() {
    let (_dir, store) = fixture(
        QuotaLimits {
            minute: 1,
            hour: 10,
            day: 10,
        },
        None,
    )
    .await;
    let req = request(7, 9, 7, None);
    let (a, b) = tokio::join!(
        admit(&store, &req, "same", NOW),
        admit(&store, &req, "same", NOW)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.operation_id, b.operation_id);
    assert_ne!(a.created, b.created);
    assert!(counters(&store, 7).await.iter().all(|(_, n, _)| *n == 1));
    let req = request(8, 9, 8, None);
    let (a, b) = tokio::join!(admit(&store, &req, "a", NOW), admit(&store, &req, "b", NOW));
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(
        a.err().or(b.err()),
        Some(StoreError::QuotaExceeded {
            retry_after_secs: 60
        })
    );
    assert!(counters(&store, 8).await.iter().all(|(_, n, _)| *n == 1));
}

#[tokio::test]
async fn quota_user_scope_owner_exact_id_not_role_and_imported_counters() {
    let (_dir, store) = fixture(
        QuotaLimits {
            minute: 2,
            hour: 10,
            day: 10,
        },
        Some(42),
    )
    .await;
    query("INSERT INTO users(id,role,created_at,updated_at) VALUES(7,'owner',1,1)")
        .execute(store.database())
        .await
        .unwrap();
    seed(&store, 7, "minute", 1, NOW + 60_000_000).await;
    seed(&store, 7, "legacy-other", 99, NOW + 90_000_000).await;
    admit(&store, &request(7, 9, -100, Some(1)), "topic-one", NOW)
        .await
        .unwrap();
    assert_eq!(
        admit(&store, &request(7, 10, -200, Some(2)), "other-bot", NOW).await,
        Err(StoreError::QuotaExceeded {
            retry_after_secs: 60
        })
    );
    for i in 0..4 {
        admit(&store, &request(42, 9, 42, None), &format!("owner{i}"), NOW)
            .await
            .unwrap();
    }
    assert!(counters(&store, 42).await.is_empty());
    admit(&store, &request(8, 9, -100, Some(1)), "other-actor", NOW)
        .await
        .unwrap();
    assert_eq!(
        counters(&store, 7)
            .await
            .iter()
            .find(|(w, _, _)| w == "legacy-other")
            .unwrap()
            .1,
        99
    );
}

#[tokio::test]
async fn quota_capacity_invalid_input_failed_insert_and_malformed_import_do_not_charge() {
    let (_dir, store) = fixture(QuotaLimits::default(), None).await;
    let req = request(7, 9, 7, None);
    assert_eq!(
        admit(&store, &req, "", NOW).await,
        Err(StoreError::InvalidInput)
    );
    let tight = AdmissionPolicy {
        max_pending_global: 1,
        ..policy()
    };
    store
        .admit_inner(&req, "one", tight, true, Some(NOW), None)
        .await
        .unwrap();
    let before = counters(&store, 7).await;
    assert_eq!(
        store
            .admit_inner(&req, "two", tight, true, Some(NOW), None)
            .await,
        Err(StoreError::Capacity)
    );
    assert_eq!(counters(&store, 7).await, before);
    yoink_db::raw_sql("CREATE TRIGGER quota_fail_insert BEFORE INSERT ON runtime_operations BEGIN SELECT RAISE(ABORT,'fixture'); END;")
        .execute(store.database()).await.unwrap();
    assert_eq!(
        admit(&store, &req, "fail", NOW).await,
        Err(StoreError::Database)
    );
    assert_eq!(counters(&store, 7).await, before);
    assert!(counters(&store, 8).await.is_empty());
    assert_eq!(
        admit(&store, &request(8, 9, 8, None), "fail", NOW).await,
        Err(StoreError::Database)
    );
    assert!(counters(&store, 8).await.is_empty());
    yoink_db::raw_sql("DROP TRIGGER quota_fail_insert")
        .execute(store.database())
        .await
        .unwrap();
    seed(&store, 7, "minute", -1, NOW - 1).await;
    let before = counters(&store, 7).await;
    assert_eq!(
        admit(&store, &req, "malformed", NOW).await,
        Err(StoreError::InvalidStoredData)
    );
    assert_eq!(counters(&store, 7).await, before);
}

#[tokio::test]
async fn active_skip_busy_actor_including_delivering_and_owner_restart_expiry_fencing() {
    let (dir, store) = fixture(QuotaLimits::default(), Some(42)).await;
    for i in 0..4 {
        admit(&store, &request(42, 9, 42, None), &format!("owner{i}"), NOW)
            .await
            .unwrap();
    }
    let other = admit(&store, &request(7, 9, 7, None), "other", NOW)
        .await
        .unwrap();
    let mut claims = Vec::new();
    for i in 0..3 {
        claims.push(
            store
                .claim_for_bot("worker", Duration::from_secs(60), 9)
                .await
                .unwrap()
                .unwrap(),
        );
        assert_eq!(claims[i].request.actor_id(), 42);
    }
    store.begin_delivery(&claims[0]).await.unwrap();
    let other_claim = store
        .claim_for_bot("worker", Duration::from_secs(60), 9)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(other_claim.operation_id, other.operation_id);
    assert!(
        store
            .claim_for_bot("worker", Duration::from_secs(60), 9)
            .await
            .unwrap()
            .is_none()
    );
    store.close().await.unwrap();
    drop(store);
    let store = Store::open(&dir.path().join("quota.db"), false, Duration::from_secs(5))
        .await
        .unwrap()
        .with_quotas(QuotaLimits::default(), Some(42))
        .unwrap();
    assert!(
        store
            .claim_for_bot("worker", Duration::from_secs(60), 9)
            .await
            .unwrap()
            .is_none()
    );
    query("UPDATE runtime_operations SET lease_expires_at=0 WHERE id=?1")
        .bind(claims[0].operation_id)
        .execute(store.database())
        .await
        .unwrap();
    let next = store
        .claim_for_bot("worker2", Duration::from_secs(60), 9)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.request.actor_id(), 42);
    assert_eq!(
        store.heartbeat(&claims[0], Duration::from_secs(60)).await,
        Err(StoreError::NotFoundOrStale)
    );
    assert_eq!(
        store
            .get(42, claims[0].operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::DeliveryUnknown
    );
    // Expired running work recovers under a fresh fence after the same restart.
    query("UPDATE runtime_operations SET lease_expires_at=0 WHERE id=?1")
        .bind(claims[1].operation_id)
        .execute(store.database())
        .await
        .unwrap();
    let recovered = store
        .claim_for_bot("worker3", Duration::from_secs(60), 9)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.operation_id, claims[1].operation_id);
    assert_eq!(recovered.attempt, claims[1].attempt + 1);
    assert_ne!(recovered.lease_token, claims[1].lease_token);
    assert_eq!(
        store.complete(&claims[1]).await,
        Err(StoreError::NotFoundOrStale)
    );
    assert!(counters(&store, 42).await.is_empty());
}

#[tokio::test]
async fn active_transactional_race_cross_bot_and_retries_release_without_charging() {
    let (_dir, store) = fixture(
        QuotaLimits {
            minute: 100,
            hour: 100,
            day: 100,
        },
        None,
    )
    .await;
    for (i, bot) in [(0, 9), (1, 10), (2, 9)] {
        admit(&store, &request(7, bot, 7, None), &format!("k{i}"), NOW)
            .await
            .unwrap();
    }
    let before = counters(&store, 7).await;
    let bounded = ClaimPolicy {
        max_active_actor: 1,
    };
    let (a, b) = tokio::join!(
        store.claim_for_bot_bounded("a", Duration::from_secs(60), 9, bounded),
        store.claim_for_bot_bounded("b", Duration::from_secs(60), 10, bounded)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(usize::from(a.is_some()) + usize::from(b.is_some()), 1);
    let claim = a.or(b).unwrap();
    store.fail(&claim, true).await.unwrap();
    let next = store
        .claim_filtered("c", Duration::from_secs(60), None, bounded)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.complete(&claim).await,
        Err(StoreError::NotFoundOrStale)
    );
    store.request_cancel(7, next.operation_id).await.unwrap();
    store.acknowledge_cancel(&next).await.unwrap();
    assert!(
        store
            .claim_filtered("d", Duration::from_secs(60), None, bounded)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(counters(&store, 7).await, before);
}

#[test]
fn quota_and_active_positive_bounded_defaults() {
    assert_eq!(
        QuotaLimits::default(),
        QuotaLimits {
            minute: 5,
            hour: 30,
            day: 100
        }
    );
    assert_eq!(ClaimPolicy::default().max_active_actor, 3);
    for n in [0, 1_000_001, u32::MAX] {
        for limits in [
            QuotaLimits {
                minute: n,
                ..QuotaLimits::default()
            },
            QuotaLimits {
                hour: n,
                ..QuotaLimits::default()
            },
            QuotaLimits {
                day: n,
                ..QuotaLimits::default()
            },
        ] {
            assert_eq!(limits.validate(), Err(StoreError::InvalidInput));
        }
    }
    for n in [0, 101, u32::MAX] {
        assert_eq!(
            ClaimPolicy {
                max_active_actor: n
            }
            .validate(),
            Err(StoreError::InvalidInput)
        );
    }
}
