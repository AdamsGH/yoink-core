use std::time::Duration;

mod common;
use common::unmigrated_store as store;
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_domain::{DownloadParams, DownloadRequest, MediaMode};
use yoink_storage::{Store, StoreError};

async fn batch_units(
    store: &Store,
    operation_id: uuid::Uuid,
    attempt: i32,
) -> Vec<(i32, String, i32, Option<Vec<i32>>)> {
    yoink_db::query(include_str!("fixtures/delivery_units.sql"))
        .bind(operation_id)
        .bind(attempt)
        .fetch_all(store.database())
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            (
                row.try_get("ordinal").unwrap(),
                row.try_get("status").unwrap(),
                row.try_get("artifact_count").unwrap(),
                row.try_get("message_ids").unwrap(),
            )
        })
        .collect()
}

fn request(actor: u64, url: &str) -> DownloadRequest {
    DownloadRequest::new(
        actor,
        url,
        MediaMode::Video,
        None,
        DownloadParams::default(),
    )
    .unwrap()
}

#[tokio::test]
async fn queue_persistence_contract() {
    for timeout in [Duration::ZERO, Duration::from_secs(301)] {
        assert_eq!(
            Store::open(std::path::Path::new("/not-opened.db"), true, timeout)
                .await
                .err()
                .unwrap(),
            StoreError::InvalidInput
        );
    }
    let store = store().await;
    assert_eq!(store.verify_schema().await.unwrap_err(), StoreError::Schema);
    store.migrate().await.unwrap();
    store.migrate().await.unwrap();
    store.verify_schema().await.unwrap();
    let history = yoink_db::query(include_str!("fixtures/migration_history.sql"))
        .fetch_all(store.database())
        .await
        .unwrap();
    let (version, checksum): (i64, String) = (
        history[0].try_get("version").unwrap(),
        history[0].try_get("checksum").unwrap(),
    );
    let unknown_version: i64 = history
        .last()
        .unwrap()
        .try_get::<i64, _>("version")
        .unwrap()
        + 1;
    yoink_db::query(include_str!("fixtures/migration_corrupt_checksum.sql"))
        .bind(version)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(store.verify_schema().await.unwrap_err(), StoreError::Schema);
    yoink_db::query(include_str!("fixtures/migration_restore_checksum.sql"))
        .bind(version)
        .bind(&checksum)
        .execute(store.database())
        .await
        .unwrap();
    yoink_db::query(include_str!("fixtures/migration_mark_failed.sql"))
        .bind(version)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(store.verify_schema().await.unwrap_err(), StoreError::Schema);
    yoink_db::query(include_str!("fixtures/migration_mark_success.sql"))
        .bind(version)
        .execute(store.database())
        .await
        .unwrap();
    yoink_db::query(include_str!("fixtures/migration_insert_unknown.sql"))
        .bind(unknown_version)
        .bind(&checksum)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(store.verify_schema().await.unwrap_err(), StoreError::Schema);
    yoink_db::query(include_str!("fixtures/migration_delete_unknown.sql"))
        .bind(unknown_version)
        .execute(store.database())
        .await
        .unwrap();
    let history_after = yoink_db::query(include_str!("fixtures/migration_history.sql"))
        .fetch_all(store.database())
        .await
        .unwrap();
    assert_eq!(history_after.len(), history.len());
    for (actual, expected) in history_after.iter().zip(&history) {
        assert_eq!(
            actual.try_get::<i64, _>("version").unwrap(),
            expected.try_get::<i64, _>("version").unwrap()
        );
        assert_eq!(
            actual.try_get::<String, _>("checksum").unwrap(),
            expected.try_get::<String, _>("checksum").unwrap()
        );
        assert_eq!(
            actual.try_get::<bool, _>("success").unwrap(),
            expected.try_get::<bool, _>("success").unwrap()
        );
    }
    let public_migration_table: i64 =
        yoink_db::query(include_str!("fixtures/count_public_migrations.sql"))
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    assert_eq!(public_migration_table, 0);

    let first = request(10001, "https://example.com/a?secret=redacted");
    let (a, b) = tokio::join!(
        store.admit(&first, "stable-key", 2),
        store.admit(&first, "stable-key", 2)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.operation_id, b.operation_id);
    assert_ne!(a.created, b.created);
    assert_eq!(
        store
            .admit(
                &request(10001, "https://example.com/other"),
                "stable-key",
                2
            )
            .await
            .unwrap_err(),
        StoreError::Conflict
    );
    let actor_separate = store
        .admit(
            &request(10002, "https://example.com/a?secret=redacted"),
            "stable-key",
            2,
        )
        .await
        .unwrap();
    assert_ne!(a.operation_id, actor_separate.operation_id);
    assert!(store.get(99999, a.operation_id).await.unwrap().is_none());
    assert!(!store.request_cancel(99999, a.operation_id).await.unwrap());

    let fail_rollback = store.admit(&first, "rollback-key", 0).await;
    assert_eq!(fail_rollback.unwrap_err(), StoreError::InvalidInput);
    let valid_after_rollback = store.admit(&first, "rollback-key", 1).await.unwrap();
    assert!(valid_after_rollback.created);
    let mut rollback_guard = store.database().connection().await.unwrap();
    let rollback_tx = Transaction::new(&mut rollback_guard, TransactionBehavior::Immediate)
        .await
        .unwrap();
    yoink_db::query(include_str!("../sql/admit_insert.sql"))
        .bind(10009_i64)
        .bind("rolled-back")
        .bind(serde_json::json!({"probe": true}))
        .bind(1_i32)
        .bind(0_i32)
        .bind(None::<&str>)
        .bind(uuid::Uuid::new_v4())
        .fetch_one(&*rollback_tx)
        .await
        .unwrap();
    rollback_tx.rollback().await.unwrap();
    drop(rollback_guard);
    let rolled_back: i64 =
        yoink_db::query(include_str!("fixtures/count_rolled_back_operation.sql"))
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    assert_eq!(rolled_back, 0);
    assert!(store.request_cancel(10001, a.operation_id).await.unwrap());
    assert!(
        store
            .request_cancel(10002, actor_separate.operation_id)
            .await
            .unwrap()
    );
    assert!(
        store
            .request_cancel(10001, valid_after_rollback.operation_id)
            .await
            .unwrap()
    );

    let competition = store
        .admit(&request(10003, "https://example.com/compete"), "compete", 2)
        .await
        .unwrap();
    let (worker_a, worker_b) = tokio::join!(
        store.claim("worker-a", Duration::from_secs(5)),
        store.claim("worker-b", Duration::from_secs(5))
    );
    let claims = [worker_a.unwrap(), worker_b.unwrap()];
    let matching: Vec<_> = claims
        .into_iter()
        .flatten()
        .filter(|c| c.operation_id == competition.operation_id)
        .collect();
    assert_eq!(matching.len(), 1);
    let claim = matching.into_iter().next().unwrap();
    assert!(!format!("{claim:?}").contains(&claim.lease_token.to_string()));
    let mut forged = claim.clone();
    forged.worker_id = "forged-worker".to_owned();
    assert_eq!(
        store
            .heartbeat(&forged, Duration::from_secs(5))
            .await
            .unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store.complete(&forged).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store.fail(&forged, true).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store.acknowledge_cancel(&forged).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    forged = claim.clone();
    forged.lease_token = uuid::Uuid::new_v4();
    assert_eq!(
        store.complete(&forged).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    forged = claim.clone();
    forged.attempt += 1;
    assert_eq!(
        store.complete(&forged).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    store
        .heartbeat(&claim, Duration::from_secs(5))
        .await
        .unwrap();
    store.complete(&claim).await.unwrap();
    assert_eq!(
        store.complete(&claim).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );

    let retry = store
        .admit(&request(10004, "https://example.com/retry"), "retry", 2)
        .await
        .unwrap();
    let first_attempt = store
        .claim("retry-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_attempt.operation_id, retry.operation_id);
    assert_eq!(first_attempt.attempt, 1);
    assert_eq!(
        store
            .fail_with_code(&first_attempt, true, Some("raw secret URL"))
            .await,
        Err(StoreError::InvalidInput)
    );
    store
        .fail_with_code(&first_attempt, true, Some("media_timeout"))
        .await
        .unwrap();
    assert_eq!(
        store
            .get(10004, retry.operation_id)
            .await
            .unwrap()
            .unwrap()
            .failure_code
            .as_deref(),
        Some("media_timeout")
    );
    let second_attempt = store
        .claim("retry-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_attempt.operation_id, retry.operation_id);
    assert_eq!(second_attempt.attempt, 2);
    assert!(
        store
            .get(10004, retry.operation_id)
            .await
            .unwrap()
            .unwrap()
            .failure_code
            .is_none()
    );
    store
        .fail_with_code(&second_attempt, true, Some("geo_blocked"))
        .await
        .unwrap();
    assert_eq!(
        store
            .get(10004, retry.operation_id)
            .await
            .unwrap()
            .unwrap()
            .failure_code
            .as_deref(),
        Some("geo_blocked")
    );
    assert_eq!(
        store
            .get(10004, retry.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        yoink_storage::OperationStatus::Failed
    );

    let queued_cancel = store
        .admit(
            &request(10005, "https://example.com/cancel-queued"),
            "cancel-queued",
            1,
        )
        .await
        .unwrap();
    assert!(
        store
            .request_cancel(10005, queued_cancel.operation_id)
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .get(10005, queued_cancel.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        yoink_storage::OperationStatus::Cancelled
    );

    let cancellable = store
        .admit(
            &request(10006, "https://example.com/cancel-running"),
            "cancel-running",
            2,
        )
        .await
        .unwrap();
    let cancellation_claim = loop {
        let candidate = store
            .claim("cancel-worker", Duration::from_secs(30))
            .await
            .unwrap()
            .unwrap();
        if candidate.operation_id == cancellable.operation_id {
            break candidate;
        }
        store.fail(&candidate, true).await.unwrap();
    };
    assert!(
        store
            .request_cancel(10006, cancellable.operation_id)
            .await
            .unwrap()
    );
    store
        .heartbeat(&cancellation_claim, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(
        store
            .get(10006, cancellable.operation_id)
            .await
            .unwrap()
            .unwrap()
            .cancel_requested
    );
    assert!(
        store
            .claim("other-worker", Duration::from_secs(5))
            .await
            .unwrap()
            .is_none()
    );
    store.acknowledge_cancel(&cancellation_claim).await.unwrap();
    assert_eq!(
        store
            .get(10006, cancellable.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        yoink_storage::OperationStatus::Cancelled
    );

    let expiring = store
        .admit(&request(10007, "https://example.com/expiry"), "expiry", 2)
        .await
        .unwrap();
    let stale = loop {
        let candidate = store
            .claim("expiring-worker", Duration::from_secs(30))
            .await
            .unwrap()
            .unwrap();
        if candidate.operation_id == expiring.operation_id {
            break candidate;
        }
        store.fail(&candidate, true).await.unwrap();
    };
    yoink_db::query(include_str!("fixtures/expire_operation.sql"))
        .bind(expiring.operation_id)
        .execute(store.database())
        .await
        .unwrap();
    let recovered = store
        .claim("recovery-worker", Duration::from_secs(3))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.operation_id, expiring.operation_id);
    assert_eq!(recovered.attempt, 2);
    assert_eq!(
        store
            .heartbeat(&stale, Duration::from_secs(3))
            .await
            .unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store.complete(&stale).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store.fail(&stale, true).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store.acknowledge_cancel(&stale).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    store.complete(&recovered).await.unwrap();
    assert_eq!(
        store
            .get(10007, expiring.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        yoink_storage::OperationStatus::Succeeded
    );

    let final_expiry = store
        .admit(
            &request(10008, "https://example.com/final-expiry"),
            "final-expiry",
            1,
        )
        .await
        .unwrap();
    let final_claim = store
        .claim("final-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(final_claim.operation_id, final_expiry.operation_id);
    yoink_db::query(include_str!("fixtures/expire_final_operation.sql"))
        .bind(final_expiry.operation_id)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        store
            .claim("final-recovery-worker", Duration::from_secs(3))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .get(10008, final_expiry.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        yoink_storage::OperationStatus::Failed
    );

    let cancelled_expiry = store
        .admit(
            &request(10010, "https://example.com/cancelled-expiry"),
            "cancelled-expiry",
            1,
        )
        .await
        .unwrap();
    let cancelled_claim = loop {
        let candidate = store
            .claim("cancelled-expiry-worker", Duration::from_secs(30))
            .await
            .unwrap()
            .unwrap();
        if candidate.operation_id == cancelled_expiry.operation_id {
            break candidate;
        }
        store.fail(&candidate, true).await.unwrap();
    };
    assert!(
        store
            .request_cancel(10010, cancelled_expiry.operation_id)
            .await
            .unwrap()
    );
    yoink_db::query(include_str!("fixtures/expire_operation.sql"))
        .bind(cancelled_expiry.operation_id)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        store
            .claim("cancelled-recovery-worker", Duration::from_secs(3))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .get(10010, cancelled_expiry.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        yoink_storage::OperationStatus::Cancelled
    );
    assert_eq!(
        store
            .heartbeat(&cancelled_claim, Duration::from_secs(3))
            .await
            .unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store.complete(&cancelled_claim).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store.fail(&cancelled_claim, true).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store
            .acknowledge_cancel(&cancelled_claim)
            .await
            .unwrap_err(),
        StoreError::NotFoundOrStale
    );

    let contention = store
        .admit(
            &request(10012, "https://example.com/contention"),
            "contention",
            1,
        )
        .await
        .unwrap();
    let mut lock_guard = store.database().connection().await.unwrap();
    let lock_tx = Transaction::new(&mut lock_guard, TransactionBehavior::Immediate)
        .await
        .unwrap();
    let locked_id: uuid::Uuid = yoink_db::query(include_str!("fixtures/lock_operation.sql"))
        .bind(contention.operation_id)
        .fetch_one(&*lock_tx)
        .await
        .unwrap()
        .try_get("id")
        .unwrap();
    assert_eq!(locked_id, contention.operation_id);
    let blocked = tokio::time::timeout(
        Duration::from_millis(100),
        store.claim("blocked-worker", Duration::from_secs(5)),
    )
    .await;
    assert!(
        blocked.is_err(),
        "a concurrent claim must wait for the active transaction"
    );
    lock_tx.rollback().await.unwrap();
    drop(lock_guard);
    let unlocked = store
        .claim("unlocked-worker", Duration::from_secs(5))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unlocked.operation_id, contention.operation_id);
    store.complete(&unlocked).await.unwrap();

    let persisted = store
        .admit(
            &request(10013, "https://example.com/persisted"),
            "persisted",
            1,
        )
        .await
        .unwrap();
    let original_record = store
        .get(10013, persisted.operation_id)
        .await
        .unwrap()
        .unwrap();
    let path = store.path().to_path_buf();
    store.close().await.unwrap();
    let replacement = Store::open(&path, false, Duration::from_secs(5))
        .await
        .unwrap();
    replacement.migrate().await.unwrap();
    replacement.verify_schema().await.unwrap();
    assert_eq!(
        replacement
            .get(10004, retry.operation_id)
            .await
            .unwrap()
            .unwrap()
            .failure_code
            .as_deref(),
        Some("geo_blocked")
    );
    let restarted_record = replacement
        .get(10013, persisted.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restarted_record.operation_id, original_record.operation_id);
    assert_eq!(restarted_record.actor_id, original_record.actor_id);
    assert_eq!(restarted_record.status, original_record.status);
    assert_eq!(
        restarted_record.status,
        yoink_storage::OperationStatus::Queued
    );
    let replacement_claim = replacement
        .claim("replacement-worker", Duration::from_secs(5))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replacement_claim.operation_id, persisted.operation_id);
    replacement.complete(&replacement_claim).await.unwrap();
    assert_eq!(
        replacement
            .get(10013, persisted.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        yoink_storage::OperationStatus::Succeeded
    );

    let malicious = replacement
        .admit(
            &request(10011, "https://example.com/malicious"),
            "malicious",
            1,
        )
        .await
        .unwrap();
    yoink_db::query(include_str!("fixtures/poison_operation_request.sql"))
        .bind(malicious.operation_id)
        .execute(replacement.database())
        .await
        .unwrap();
    assert_eq!(
        replacement
            .claim("malicious-worker", Duration::from_secs(5))
            .await
            .unwrap_err(),
        StoreError::InvalidStoredData
    );

    replacement.close().await.unwrap();
}

#[tokio::test]
async fn bot_scope_progress_and_legacy_replay_contract() {
    use yoink_domain::DeliveryTarget;
    use yoink_storage::ProgressStage;

    let store = store().await;
    store.migrate().await.unwrap();
    yoink_db::query(include_str!("fixtures/reset_operations.sql"))
        .execute(store.database())
        .await
        .unwrap();

    let base = request(22901, "https://example.com/legacy-replay");
    let admitted = store.admit(&base, "legacy-replay", 2).await.unwrap();
    let legacy = serde_json::json!({
        "actor_id": 22901, "source_url": "https://example.com/legacy-replay",
        "media_mode": "video", "clip_range": null, "max_height": null
    });
    yoink_db::query(include_str!("fixtures/set_legacy_request.sql"))
        .bind(admitted.operation_id)
        .bind(legacy)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        !store
            .admit(&base, "legacy-replay", 2)
            .await
            .unwrap()
            .created
    );
    let legacy_claim = store
        .claim("legacy-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(legacy_claim.operation_id, admitted.operation_id);
    store.complete(&legacy_claim).await.unwrap();

    let target_request = |actor, bot| {
        request(actor, "https://example.com/scoped")
            .with_target(DeliveryTarget::new(bot, actor as i64, None, Some(9)).unwrap())
            .unwrap()
    };
    let bot_a = store
        .admit(&target_request(22902, 71), "tg:71:22902:9", 3)
        .await
        .unwrap();
    let bot_b = store
        .admit(&target_request(22903, 72), "tg:72:22903:9", 2)
        .await
        .unwrap();
    let unscoped = store
        .admit(
            &request(22904, "https://example.com/unscoped"),
            "other-key",
            2,
        )
        .await
        .unwrap();

    let claim_a = store
        .claim_for_bot("bot-a", Duration::from_secs(30), 71)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim_a.operation_id, bot_a.operation_id);
    store
        .update_progress(&claim_a, 73, ProgressStage::Processing)
        .await
        .unwrap();
    let mut wrong_lease = claim_a.clone();
    wrong_lease.worker_id = "not-bot-a".to_owned();
    assert_eq!(
        store
            .update_progress(&wrong_lease, 50, ProgressStage::Fetching)
            .await
            .unwrap_err(),
        StoreError::NotFoundOrStale
    );
    let rec = store.get(22902, bot_a.operation_id).await.unwrap().unwrap();
    assert_eq!(rec.progress_percent, Some(73));
    assert_eq!(rec.progress_stage, Some(ProgressStage::Processing));
    assert_eq!(
        store
            .update_progress(&claim_a, 101, ProgressStage::Fetching)
            .await
            .unwrap_err(),
        StoreError::InvalidInput
    );
    store.fail(&claim_a, true).await.unwrap();
    let retry_a = store
        .claim_for_bot("bot-a", Duration::from_secs(30), 71)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry_a.operation_id, bot_a.operation_id);
    assert_eq!(retry_a.attempt, 2);
    assert_eq!(
        store
            .get(22902, bot_a.operation_id)
            .await
            .unwrap()
            .unwrap()
            .progress_percent,
        None
    );
    store
        .update_progress(&retry_a, 40, ProgressStage::Fetching)
        .await
        .unwrap();
    yoink_db::query(include_str!("fixtures/expire_operation.sql"))
        .bind(bot_a.operation_id)
        .execute(store.database())
        .await
        .unwrap();
    let recovered_a = store
        .claim_for_bot("bot-a-recovery", Duration::from_secs(30), 71)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered_a.attempt, 3);
    assert_eq!(
        store
            .get(22902, bot_a.operation_id)
            .await
            .unwrap()
            .unwrap()
            .progress_percent,
        None
    );

    let claim_b = store
        .claim_for_bot("bot-b", Duration::from_secs(30), 72)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim_b.operation_id, bot_b.operation_id);
    assert!(
        store
            .claim_for_bot("bot-a", Duration::from_secs(30), 71)
            .await
            .unwrap()
            .is_none()
    );
    let generic = store
        .claim("generic", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(generic.operation_id, unscoped.operation_id);
    store.close().await.unwrap();
}

#[tokio::test]
async fn native_legacy_target_backfill_regression() {
    let store = store().await;
    store.migrate().await.unwrap();

    let mut guard = store.database().connection().await.unwrap();
    let tx = Transaction::new(&mut guard, TransactionBehavior::Immediate)
        .await
        .unwrap();
    yoink_db::query(include_str!("fixtures/legacy_backfill_cases.sql"))
        .execute(&*tx)
        .await
        .unwrap();
    for _ in 0..2 {
        yoink_db::raw_sql(include_str!("../sql/backfill_legacy_targets.sql"))
            .execute(&*tx)
            .await
            .unwrap();
    }

    let valid: serde_json::Value =
        yoink_db::query(include_str!("fixtures/read_backfill_target.sql"))
            .bind("tg:71:23101:9")
            .fetch_one(&*tx)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    assert_eq!(valid["bot_id"], 71);
    assert_eq!(valid["chat_id"], 23101);
    assert_eq!(valid["reply_to_message_id"], 9);
    for invalid_key in [
        "tg:071:23102:9",
        "tg:9223372036854775808:23103:9",
        "tg:71:9223372036854775808:9",
        "tg:71:23105:2147483648",
        "tg:71:99999:9",
    ] {
        let target: Option<serde_json::Value> =
            yoink_db::query(include_str!("fixtures/read_backfill_target.sql"))
                .bind(invalid_key)
                .fetch_one(&*tx)
                .await
                .unwrap()
                .try_get(0)
                .unwrap();
        assert!(
            target.is_none() || target == Some(serde_json::Value::Null),
            "unexpected target for {invalid_key}"
        );
    }
    let preserved: serde_json::Value =
        yoink_db::query(include_str!("fixtures/read_backfill_target.sql"))
            .bind("tg:77:23107:9")
            .fetch_one(&*tx)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    assert_eq!(preserved["bot_id"], 88);
    assert_eq!(preserved["chat_id"], 23107);
    assert_eq!(preserved["reply_to_message_id"], 17);
    tx.rollback().await.unwrap();
    drop(guard);
    store.close().await.unwrap();
}

#[tokio::test]
async fn durable_batch_delivery_contract() {
    use yoink_storage::OperationStatus;
    let store = store().await;
    store.migrate().await.unwrap();
    yoink_db::query(include_str!("fixtures/reset_operations.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let admitted = store
        .admit(&request(22801, "https://example.com/album"), "album", 2)
        .await
        .unwrap();
    let claim = store
        .claim("batch-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.operation_id, admitted.operation_id);
    assert!(store.begin_delivery_batch(&claim, 3).await.unwrap());
    assert!(!store.begin_delivery_batch(&claim, 3).await.unwrap());
    assert!(store.begin_delivery_unit(&claim, 0, 2).await.unwrap());
    assert!(!store.begin_delivery_unit(&claim, 0, 1).await.unwrap());
    assert!(
        store
            .complete_delivery_unit(&claim, 0, &[81001, 81002])
            .await
            .unwrap()
    );
    assert!(
        !store
            .complete_delivery_unit(&claim, 0, &[81001, 81002])
            .await
            .unwrap()
    );
    assert!(store.begin_delivery_unit(&claim, 1, 1).await.unwrap());
    assert!(
        store
            .request_cancel(22801, admitted.operation_id)
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_delivery_unit(&claim, 1, &[81003])
            .await
            .unwrap()
    );
    let record = store
        .get(22801, admitted.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::Succeeded);
    assert_eq!(record.planned_artifact_count, Some(3));
    assert_eq!(record.delivered_artifact_count, 3);
    assert_eq!(record.delivered_message_id, Some(81003));
    store.close().await.unwrap();
}

#[tokio::test]
async fn media_delivery_and_bounded_admission_contract() {
    use yoink_storage::{AdmissionPolicy, OperationStatus};
    let store = store().await;
    store.migrate().await.unwrap();
    yoink_db::query(include_str!("fixtures/reset_operations.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let policy = AdmissionPolicy {
        max_attempts: 2,
        retry_delay: Duration::from_secs(2),
        max_pending_global: 1,
        max_pending_actor: 1,
    };
    let first = request(22001, "https://example.com/media-first");
    let admitted = store
        .admit_bounded(&first, "media-first", policy)
        .await
        .unwrap();
    let replay = store
        .admit_bounded(&first, "media-first", policy)
        .await
        .unwrap();
    assert_eq!(replay.operation_id, admitted.operation_id);
    assert!(!replay.created);
    assert_eq!(
        store
            .admit_bounded(
                &request(22001, "https://example.com/conflict"),
                "media-first",
                policy
            )
            .await
            .unwrap_err(),
        StoreError::Conflict
    );
    assert_eq!(
        store
            .admit_bounded(
                &request(22002, "https://example.com/full"),
                "full-global",
                policy
            )
            .await
            .unwrap_err(),
        StoreError::Capacity
    );
    let race_one = request(22002, "https://example.com/race-one");
    let race_two = request(22003, "https://example.com/race-two");
    let (one, two) = tokio::join!(
        store.admit_bounded(&race_one, "race-one", policy),
        store.admit_bounded(&race_two, "race-two", policy)
    );
    assert_eq!(one.is_ok() as usize + two.is_ok() as usize, 0);

    let claim = store
        .claim("media-worker", Duration::from_secs(10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.operation_id, admitted.operation_id);
    store.begin_delivery(&claim).await.unwrap();
    assert!(
        store
            .request_cancel(22001, admitted.operation_id)
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .get(22001, admitted.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::Delivering
    );
    store
        .heartbeat(&claim, Duration::from_secs(10))
        .await
        .unwrap();
    store.complete_delivery(&claim, 987654).await.unwrap();
    let record = store
        .get(22001, admitted.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::Succeeded);
    assert_eq!(record.delivered_message_id, Some(987654));
    assert!(record.delivery_started_at.is_some());
    assert_eq!(
        store.complete_delivery(&claim, 987655).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store.complete_delivery(&claim, 0).await.unwrap_err(),
        StoreError::InvalidInput
    );

    let concurrent_policy = AdmissionPolicy {
        max_attempts: 1,
        retry_delay: Duration::ZERO,
        max_pending_global: 1,
        max_pending_actor: 1,
    };
    let concurrent_request_a = request(22009, "https://example.com/concurrent-a");
    let concurrent_request_b = request(22010, "https://example.com/concurrent-b");
    let (concurrent_a, concurrent_b) = tokio::join!(
        store.admit_bounded(&concurrent_request_a, "concurrent-a", concurrent_policy),
        store.admit_bounded(&concurrent_request_b, "concurrent-b", concurrent_policy)
    );
    assert_eq!(
        concurrent_a.is_ok() as usize + concurrent_b.is_ok() as usize,
        1
    );
    assert_eq!(
        (concurrent_a.as_ref().err() == Some(&StoreError::Capacity)) as usize
            + (concurrent_b.as_ref().err() == Some(&StoreError::Capacity)) as usize,
        1
    );
    let queued_id = concurrent_a
        .ok()
        .or_else(|| concurrent_b.ok())
        .unwrap()
        .operation_id;
    let queued_claim = store
        .claim("concurrent-worker", Duration::from_secs(5))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(queued_claim.operation_id, queued_id);
    store.fail(&queued_claim, false).await.unwrap();

    let unbounded = AdmissionPolicy {
        max_attempts: 1,
        retry_delay: Duration::ZERO,
        max_pending_global: 10,
        max_pending_actor: 10,
    };
    let unknown = store
        .admit_bounded(
            &request(22004, "https://example.com/unknown"),
            "unknown",
            unbounded,
        )
        .await
        .unwrap();
    let unknown_claim = store
        .claim("unknown-worker", Duration::from_secs(10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unknown_claim.operation_id, unknown.operation_id);
    store.begin_delivery(&unknown_claim).await.unwrap();
    let mut stale = unknown_claim.clone();
    stale.lease_token = uuid::Uuid::new_v4();
    assert_eq!(
        store.delivery_unknown(&stale).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    store.delivery_unknown(&unknown_claim).await.unwrap();
    assert_eq!(
        store
            .get(22004, unknown.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::DeliveryUnknown
    );
    assert!(
        store
            .claim("no-replay-worker", Duration::from_secs(5))
            .await
            .unwrap()
            .is_none()
    );

    let rejected = store
        .admit_bounded(
            &request(22011, "https://example.com/rejected"),
            "rejected",
            unbounded,
        )
        .await
        .unwrap();
    let rejected_claim = store
        .claim("rejected-worker", Duration::from_secs(10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rejected_claim.operation_id, rejected.operation_id);
    store.begin_delivery(&rejected_claim).await.unwrap();
    store.reject_delivery(&rejected_claim).await.unwrap();
    assert_eq!(
        store
            .get(22011, rejected.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::Failed
    );
    assert_eq!(
        store.reject_delivery(&rejected_claim).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert!(
        store
            .claim("rejected-recovery-worker", Duration::from_secs(5))
            .await
            .unwrap()
            .is_none()
    );

    let interrupted = store
        .admit_bounded(
            &request(22012, "https://example.com/cancel-failure"),
            "cancel-failure",
            unbounded,
        )
        .await
        .unwrap();
    let interrupted_claim = store
        .claim("cancel-failure-worker", Duration::from_secs(10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(interrupted_claim.operation_id, interrupted.operation_id);
    assert!(
        !store
            .heartbeat_state(&interrupted_claim, Duration::from_secs(10))
            .await
            .unwrap()
            .cancel_requested
    );
    assert!(
        store
            .request_cancel(22012, interrupted.operation_id)
            .await
            .unwrap()
    );
    assert!(
        store
            .heartbeat_state(&interrupted_claim, Duration::from_secs(10))
            .await
            .unwrap()
            .cancel_requested
    );
    store.fail(&interrupted_claim, true).await.unwrap();
    assert_eq!(
        store
            .get(22012, interrupted.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::Cancelled
    );

    let expiry = store
        .admit_bounded(
            &request(22005, "https://example.com/expiry-unknown"),
            "expiry-unknown",
            unbounded,
        )
        .await
        .unwrap();
    let expiry_claim = store
        .claim("expiry-worker", Duration::from_secs(10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expiry_claim.operation_id, expiry.operation_id);
    store.begin_delivery(&expiry_claim).await.unwrap();
    assert!(
        store
            .request_cancel(22005, expiry.operation_id)
            .await
            .unwrap()
    );
    yoink_db::query(include_str!("fixtures/expire_operation.sql"))
        .bind(expiry.operation_id)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        store
            .claim("expiry-recovery-worker", Duration::from_secs(5))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .get(22005, expiry.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::DeliveryUnknown
    );

    let retry_policy = AdmissionPolicy {
        max_attempts: 2,
        retry_delay: Duration::from_secs(2),
        max_pending_global: 10,
        max_pending_actor: 10,
    };
    let retry = store
        .admit_bounded(
            &request(22006, "https://example.com/backoff"),
            "backoff",
            retry_policy,
        )
        .await
        .unwrap();
    let retry_claim = store
        .claim("backoff-worker", Duration::from_secs(10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry_claim.operation_id, retry.operation_id);
    store.fail(&retry_claim, true).await.unwrap();
    assert!(
        store
            .claim("backoff-worker", Duration::from_secs(5))
            .await
            .unwrap()
            .is_none()
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    let second = store
        .claim("backoff-worker", Duration::from_secs(5))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.operation_id, retry.operation_id);
    store.fail(&second, false).await.unwrap();

    let expiry_retry_policy = AdmissionPolicy {
        max_attempts: 2,
        retry_delay: Duration::from_secs(1),
        max_pending_global: 10,
        max_pending_actor: 10,
    };
    let expiry_retry = store
        .admit_bounded(
            &request(22012, "https://example.com/expiry-retry"),
            "expiry-retry",
            expiry_retry_policy,
        )
        .await
        .unwrap();
    let first_expiring_claim = store
        .claim("expiry-retry-worker", Duration::from_secs(10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_expiring_claim.operation_id, expiry_retry.operation_id);
    yoink_db::query(include_str!("fixtures/expire_operation.sql"))
        .bind(expiry_retry.operation_id)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        store
            .claim("expiry-retry-worker", Duration::from_secs(5))
            .await
            .unwrap()
            .is_none()
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    let second_expiring_claim = store
        .claim("expiry-retry-worker", Duration::from_secs(5))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        second_expiring_claim.operation_id,
        expiry_retry.operation_id
    );
    assert_eq!(second_expiring_claim.attempt, 2);
    store.fail(&second_expiring_claim, false).await.unwrap();

    let actor_policy = AdmissionPolicy {
        max_attempts: 1,
        retry_delay: Duration::ZERO,
        max_pending_global: 10,
        max_pending_actor: 1,
    };
    store
        .admit_bounded(
            &request(22007, "https://example.com/actor-one"),
            "actor-one",
            actor_policy,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .admit_bounded(
                &request(22007, "https://example.com/actor-two"),
                "actor-two",
                actor_policy
            )
            .await
            .unwrap_err(),
        StoreError::Capacity
    );
    assert!(
        store
            .admit_bounded(
                &request(22008, "https://example.com/actor-isolated"),
                "actor-isolated",
                actor_policy
            )
            .await
            .unwrap()
            .created
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn durable_batch_cancel_and_rejection_prefix_contract() {
    use yoink_storage::OperationStatus;
    let store = store().await;
    store.migrate().await.unwrap();
    yoink_db::query(include_str!("fixtures/reset_operations.sql"))
        .execute(store.database())
        .await
        .unwrap();

    let partial = store
        .admit(
            &request(22701, "https://example.com/cancel-prefix"),
            "cancel-prefix",
            2,
        )
        .await
        .unwrap();
    let claim = store
        .claim("cancel-prefix-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.operation_id, partial.operation_id);
    assert!(store.begin_delivery_batch(&claim, 2).await.unwrap());
    assert!(store.begin_delivery_unit(&claim, 0, 1).await.unwrap());
    assert!(
        store
            .complete_delivery_unit(&claim, 0, &[91001])
            .await
            .unwrap()
    );
    assert!(
        store
            .request_cancel(22701, partial.operation_id)
            .await
            .unwrap()
    );
    assert!(
        store
            .stop_delivery_batch(&claim, yoink_storage::StopDeliveryReason::Cancel)
            .await
            .unwrap()
    );
    let record = store
        .get(22701, partial.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::DeliveryPartial);
    assert_eq!(record.delivered_artifact_count, 1);
    assert_eq!(record.planned_artifact_count, Some(2));
    assert_eq!(record.delivered_message_id, None);
    assert_eq!(
        batch_units(&store, partial.operation_id, 1).await,
        vec![(0, "sent".to_owned(), 1, Some(vec![91001]))]
    );

    let empty = store
        .admit(
            &request(22702, "https://example.com/cancel-empty"),
            "cancel-empty",
            1,
        )
        .await
        .unwrap();
    let empty_claim = store
        .claim("cancel-empty-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(empty_claim.operation_id, empty.operation_id);
    assert!(store.begin_delivery_batch(&empty_claim, 1).await.unwrap());
    assert!(
        store
            .request_cancel(22702, empty.operation_id)
            .await
            .unwrap()
    );
    assert!(
        store
            .stop_delivery_batch(&empty_claim, yoink_storage::StopDeliveryReason::Cancel)
            .await
            .unwrap()
    );
    let empty_record = store.get(22702, empty.operation_id).await.unwrap().unwrap();
    assert_eq!(empty_record.status, OperationStatus::Cancelled);
    assert_eq!(empty_record.delivered_artifact_count, 0);
    assert!(batch_units(&store, empty.operation_id, 1).await.is_empty());

    let rejected = store
        .admit(
            &request(22703, "https://example.com/reject-prefix"),
            "reject-prefix",
            2,
        )
        .await
        .unwrap();
    let rejected_claim = store
        .claim("reject-prefix-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rejected_claim.operation_id, rejected.operation_id);
    assert!(
        store
            .begin_delivery_batch(&rejected_claim, 2)
            .await
            .unwrap()
    );
    assert!(
        store
            .begin_delivery_unit(&rejected_claim, 0, 1)
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_delivery_unit(&rejected_claim, 0, &[91002])
            .await
            .unwrap()
    );
    assert!(
        store
            .begin_delivery_unit(&rejected_claim, 1, 1)
            .await
            .unwrap()
    );
    assert!(
        store
            .reject_delivery_unit(&rejected_claim, 1, Some(Duration::from_secs(1)))
            .await
            .unwrap()
    );
    let rejected_record = store
        .get(22703, rejected.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rejected_record.status, OperationStatus::DeliveryPartial);
    assert_eq!(rejected_record.delivered_artifact_count, 1);
    assert_eq!(rejected_record.attempt, 1);
    assert_eq!(rejected_record.delivered_message_id, None);
    assert_eq!(
        batch_units(&store, rejected.operation_id, 1).await,
        vec![
            (0, "sent".to_owned(), 1, Some(vec![91002])),
            (1, "rejected".to_owned(), 1, None),
        ]
    );
    assert!(
        store
            .claim("must-not-requeue-prefix", Duration::from_secs(5))
            .await
            .unwrap()
            .is_none()
    );
    let rate_limited = store
        .admit_bounded(
            &request(22704, "https://example.com/retry-after"),
            "retry-after",
            yoink_storage::AdmissionPolicy {
                max_attempts: 2,
                retry_delay: Duration::from_secs(4),
                max_pending_global: 100,
                max_pending_actor: 10,
            },
        )
        .await
        .unwrap();
    let rate_claim = store
        .claim("retry-after-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rate_claim.operation_id, rate_limited.operation_id);
    assert!(store.begin_delivery_batch(&rate_claim, 1).await.unwrap());
    assert!(store.begin_delivery_unit(&rate_claim, 0, 1).await.unwrap());
    let mut stale_rate_claim = rate_claim.clone();
    stale_rate_claim.worker_id = "stale-rate-worker".to_owned();
    assert!(
        !store
            .reject_delivery_unit(&stale_rate_claim, 0, Some(Duration::from_secs(8)))
            .await
            .unwrap()
    );
    let before_retry: chrono::DateTime<chrono::Utc> =
        yoink_db::query(include_str!("fixtures/database_clock.sql"))
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    assert!(
        store
            .reject_delivery_unit(&rate_claim, 0, Some(Duration::from_secs(6)))
            .await
            .unwrap()
    );
    let rate_record = store
        .get(22704, rate_limited.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rate_record.status, OperationStatus::Queued);
    assert!(rate_record.available_at >= before_retry + chrono::Duration::seconds(6));
    let after_retry: chrono::DateTime<chrono::Utc> =
        yoink_db::query(include_str!("fixtures/database_clock.sql"))
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    // PostgreSQL may evaluate updated_at before available_at. Bound the retry
    // against database clocks outside the statement, not assignment order.
    assert!(rate_record.available_at <= after_retry + chrono::Duration::seconds(6));
    assert!(
        store
            .claim("retry-before-delay", Duration::from_secs(5))
            .await
            .unwrap()
            .is_none()
    );
    // Do not leave delayed work able to preempt later fixtures on a slow runner.
    assert!(
        store
            .request_cancel(22704, rate_limited.operation_id)
            .await
            .unwrap()
    );

    let cancelled_rate = store
        .admit(
            &request(22705, "https://example.com/retry-after-cancel"),
            "retry-after-cancel",
            2,
        )
        .await
        .unwrap();
    let cancel_rate_claim = store
        .claim("retry-after-cancel-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cancel_rate_claim.operation_id, cancelled_rate.operation_id);
    assert!(
        store
            .begin_delivery_batch(&cancel_rate_claim, 1)
            .await
            .unwrap()
    );
    assert!(
        store
            .begin_delivery_unit(&cancel_rate_claim, 0, 1)
            .await
            .unwrap()
    );
    assert!(
        store
            .request_cancel(22705, cancelled_rate.operation_id)
            .await
            .unwrap()
    );
    assert!(
        store
            .reject_delivery_unit(&cancel_rate_claim, 0, Some(Duration::from_secs(2)))
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .get(22705, cancelled_rate.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::Cancelled
    );

    let denied = store
        .admit(
            &request(22706, "https://example.com/denied-before-send"),
            "denied-before-send",
            2,
        )
        .await
        .unwrap();
    let denied_claim = store
        .claim("denied-before-send-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(denied_claim.operation_id, denied.operation_id);
    assert!(store.begin_delivery_batch(&denied_claim, 1).await.unwrap());
    assert!(
        store
            .stop_delivery_batch(&denied_claim, yoink_storage::StopDeliveryReason::Denied)
            .await
            .unwrap()
    );
    let denied_record = store
        .get(22706, denied.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(denied_record.status, OperationStatus::Failed);
    assert!(!denied_record.cancel_requested);

    let denied_prefix = store
        .admit(
            &request(22708, "https://example.com/denied-prefix"),
            "denied-prefix",
            2,
        )
        .await
        .unwrap();
    let denied_prefix_claim = store
        .claim("denied-prefix-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(denied_prefix_claim.operation_id, denied_prefix.operation_id);
    assert!(
        store
            .begin_delivery_batch(&denied_prefix_claim, 2)
            .await
            .unwrap()
    );
    assert!(
        store
            .begin_delivery_unit(&denied_prefix_claim, 0, 1)
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_delivery_unit(&denied_prefix_claim, 0, &[91003])
            .await
            .unwrap()
    );
    assert!(
        store
            .stop_delivery_batch(
                &denied_prefix_claim,
                yoink_storage::StopDeliveryReason::Denied
            )
            .await
            .unwrap()
    );
    let denied_prefix_record = store
        .get(22708, denied_prefix.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        denied_prefix_record.status,
        OperationStatus::DeliveryPartial
    );
    assert!(!denied_prefix_record.cancel_requested);

    let exhausted = store
        .admit(
            &request(22710, "https://example.com/retry-after-exhausted"),
            "retry-after-exhausted",
            1,
        )
        .await
        .unwrap();
    let exhausted_claim = store
        .claim("retry-after-exhausted-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exhausted_claim.operation_id, exhausted.operation_id);
    assert!(
        store
            .begin_delivery_batch(&exhausted_claim, 1)
            .await
            .unwrap()
    );
    assert!(
        store
            .begin_delivery_unit(&exhausted_claim, 0, 1)
            .await
            .unwrap()
    );
    assert!(
        store
            .reject_delivery_unit(&exhausted_claim, 0, Some(Duration::from_secs(6)))
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .get(22710, exhausted.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::Failed
    );

    let shutdown_prefix = store
        .admit(
            &request(22709, "https://example.com/shutdown-prefix"),
            "shutdown-prefix",
            2,
        )
        .await
        .unwrap();
    let shutdown_prefix_claim = store
        .claim("shutdown-prefix-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        shutdown_prefix_claim.operation_id,
        shutdown_prefix.operation_id
    );
    assert!(
        store
            .begin_delivery_batch(&shutdown_prefix_claim, 2)
            .await
            .unwrap()
    );
    assert!(
        store
            .begin_delivery_unit(&shutdown_prefix_claim, 0, 1)
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_delivery_unit(&shutdown_prefix_claim, 0, &[91004])
            .await
            .unwrap()
    );
    assert!(
        store
            .stop_delivery_batch(
                &shutdown_prefix_claim,
                yoink_storage::StopDeliveryReason::Shutdown
            )
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .get(22709, shutdown_prefix.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::DeliveryPartial
    );

    let shutdown = store
        .admit(
            &request(22707, "https://example.com/shutdown-before-send"),
            "shutdown-before-send",
            2,
        )
        .await
        .unwrap();
    let shutdown_claim = store
        .claim("shutdown-before-send-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shutdown_claim.operation_id, shutdown.operation_id);
    assert!(
        store
            .begin_delivery_batch(&shutdown_claim, 1)
            .await
            .unwrap()
    );
    assert!(
        store
            .stop_delivery_batch(&shutdown_claim, yoink_storage::StopDeliveryReason::Shutdown)
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .get(22707, shutdown.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::Queued
    );

    store.close().await.unwrap();
}

#[tokio::test]
async fn durable_batch_unknown_persistence_and_expiry_contract() {
    use yoink_storage::OperationStatus;
    let store = store().await;
    store.migrate().await.unwrap();
    yoink_db::query(include_str!("fixtures/reset_operations.sql"))
        .execute(store.database())
        .await
        .unwrap();

    let explicit = store
        .admit(
            &request(22601, "https://example.com/explicit-unknown"),
            "explicit-unknown",
            2,
        )
        .await
        .unwrap();
    let claim = store
        .claim("unknown-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.operation_id, explicit.operation_id);
    assert!(store.begin_delivery_batch(&claim, 2).await.unwrap());
    assert!(store.begin_delivery_unit(&claim, 0, 1).await.unwrap());
    assert!(store.unknown_delivery_unit(&claim, 0).await.unwrap());
    let path = store.path().to_path_buf();
    store.close().await.unwrap();
    let reopened = Store::open(&path, false, Duration::from_secs(5))
        .await
        .unwrap();
    let record = reopened
        .get(22601, explicit.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::DeliveryUnknown);
    assert_eq!(record.delivered_artifact_count, 0);
    assert_eq!(record.planned_artifact_count, Some(2));
    assert_eq!(
        batch_units(&reopened, explicit.operation_id, 1).await,
        vec![(0, "unknown".to_owned(), 1, None)]
    );
    assert!(
        reopened
            .claim("unknown-no-replay", Duration::from_secs(5))
            .await
            .unwrap()
            .is_none()
    );
    reopened.close().await.unwrap();
    let store = Store::open(&path, false, Duration::from_secs(5))
        .await
        .unwrap();

    let expired = store
        .admit(
            &request(22602, "https://example.com/expired-sending"),
            "expired-sending",
            2,
        )
        .await
        .unwrap();
    let expired_claim = store
        .claim("expired-sending-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expired_claim.operation_id, expired.operation_id);
    assert!(store.begin_delivery_batch(&expired_claim, 2).await.unwrap());
    assert!(
        store
            .begin_delivery_unit(&expired_claim, 0, 1)
            .await
            .unwrap()
    );
    yoink_db::query(include_str!("fixtures/expire_operation.sql"))
        .bind(expired.operation_id)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        store
            .claim("lease-reaper", Duration::from_secs(3))
            .await
            .unwrap()
            .is_none()
    );
    let expired_record = store
        .get(22602, expired.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expired_record.status, OperationStatus::DeliveryUnknown);
    assert_eq!(expired_record.delivered_artifact_count, 0);
    assert_eq!(
        batch_units(&store, expired.operation_id, 1).await,
        vec![(0, "unknown".to_owned(), 1, None)]
    );
    assert!(
        !store
            .begin_delivery_unit(&expired_claim, 1, 1)
            .await
            .unwrap()
    );
    assert!(
        !store
            .complete_delivery_unit(&expired_claim, 0, &[92001])
            .await
            .unwrap()
    );
    assert!(
        !store
            .reject_delivery_unit(&expired_claim, 0, Some(Duration::ZERO))
            .await
            .unwrap()
    );
    assert!(
        !store
            .unknown_delivery_unit(&expired_claim, 0)
            .await
            .unwrap()
    );
    assert!(
        !store
            .stop_delivery_batch(&expired_claim, yoink_storage::StopDeliveryReason::Cancel)
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .heartbeat_state(&expired_claim, Duration::from_secs(3))
            .await
            .unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store.complete(&expired_claim).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store.fail(&expired_claim, true).await.unwrap_err(),
        StoreError::NotFoundOrStale
    );
    assert_eq!(
        store
            .get(22602, expired.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::DeliveryUnknown
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn durable_batch_expiry_prefix_and_retry_fencing_contract() {
    use yoink_storage::OperationStatus;
    let store = store().await;
    store.migrate().await.unwrap();
    yoink_db::query(include_str!("fixtures/reset_operations.sql"))
        .execute(store.database())
        .await
        .unwrap();

    let partial = store
        .admit(
            &request(22501, "https://example.com/expiry-prefix"),
            "expiry-prefix",
            2,
        )
        .await
        .unwrap();
    let claim = store
        .claim("expiry-prefix-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.operation_id, partial.operation_id);
    assert!(store.begin_delivery_batch(&claim, 2).await.unwrap());
    assert!(store.begin_delivery_unit(&claim, 0, 1).await.unwrap());
    assert!(
        store
            .complete_delivery_unit(&claim, 0, &[93001])
            .await
            .unwrap()
    );
    yoink_db::query(include_str!("fixtures/expire_operation.sql"))
        .bind(partial.operation_id)
        .execute(store.database())
        .await
        .unwrap();
    assert!(
        store
            .claim("prefix-reaper", Duration::from_secs(3))
            .await
            .unwrap()
            .is_none()
    );
    let partial_record = store
        .get(22501, partial.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(partial_record.status, OperationStatus::DeliveryPartial);
    assert_eq!(partial_record.delivered_artifact_count, 1);
    assert_eq!(partial_record.planned_artifact_count, Some(2));
    assert_eq!(
        batch_units(&store, partial.operation_id, 1).await,
        vec![(0, "sent".to_owned(), 1, Some(vec![93001]))]
    );
    assert!(
        !store
            .complete_delivery_unit(&claim, 0, &[93001])
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .get(22501, partial.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::DeliveryPartial
    );

    let retry = store
        .admit(
            &request(22502, "https://example.com/retry-zero-effect"),
            "retry-zero-effect",
            2,
        )
        .await
        .unwrap();
    let first = store
        .claim("retry-zero-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.operation_id, retry.operation_id);
    assert!(store.begin_delivery_batch(&first, 2).await.unwrap());
    assert!(store.begin_delivery_unit(&first, 0, 2).await.unwrap());
    assert!(
        store
            .reject_delivery_unit(&first, 0, Some(Duration::ZERO))
            .await
            .unwrap()
    );
    let queued = store.get(22502, retry.operation_id).await.unwrap().unwrap();
    assert_eq!(queued.status, OperationStatus::Queued);
    assert_eq!(queued.delivered_artifact_count, 0);
    assert_eq!(
        batch_units(&store, retry.operation_id, 1).await,
        vec![(0, "rejected".to_owned(), 2, None)]
    );
    let second = store
        .claim("retry-zero-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.operation_id, retry.operation_id);
    assert_eq!(second.attempt, 2);
    assert!(store.begin_delivery_batch(&second, 1).await.unwrap());
    assert!(store.begin_delivery_unit(&second, 0, 1).await.unwrap());
    assert!(
        !store
            .complete_delivery_unit(&first, 0, &[93002, 93003])
            .await
            .unwrap()
    );
    assert!(
        !store
            .reject_delivery_unit(&first, 0, Some(Duration::ZERO))
            .await
            .unwrap()
    );
    assert!(!store.unknown_delivery_unit(&first, 0).await.unwrap());
    assert!(
        !store
            .stop_delivery_batch(&first, yoink_storage::StopDeliveryReason::Cancel)
            .await
            .unwrap()
    );
    let before = store.get(22502, retry.operation_id).await.unwrap().unwrap();
    assert_eq!(before.status, OperationStatus::Delivering);
    assert_eq!(before.attempt, 2);
    assert_eq!(before.delivered_artifact_count, 0);
    assert!(
        store
            .complete_delivery_unit(&second, 0, &[93004])
            .await
            .unwrap()
    );
    let final_record = store.get(22502, retry.operation_id).await.unwrap().unwrap();
    assert_eq!(final_record.status, OperationStatus::Succeeded);
    assert_eq!(final_record.delivered_artifact_count, 1);
    assert_eq!(final_record.delivered_message_id, Some(93004));
    assert_eq!(
        batch_units(&store, retry.operation_id, 2).await,
        vec![(0, "sent".to_owned(), 1, Some(vec![93004]))]
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn durable_batch_receipt_validation_contract() {
    use yoink_storage::OperationStatus;
    let store = store().await;
    store.migrate().await.unwrap();
    yoink_db::query(include_str!("fixtures/reset_operations.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let admitted = store
        .admit(
            &request(22401, "https://example.com/receipt-validation"),
            "receipt-validation",
            1,
        )
        .await
        .unwrap();
    let claim = store
        .claim("receipt-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.operation_id, admitted.operation_id);
    assert!(store.begin_delivery_batch(&claim, 3).await.unwrap());
    assert!(!store.begin_delivery_batch(&claim, 3).await.unwrap());
    assert!(!store.begin_delivery_unit(&claim, 1, 1).await.unwrap());
    assert!(store.begin_delivery_unit(&claim, 0, 2).await.unwrap());
    assert!(!store.begin_delivery_unit(&claim, 0, 2).await.unwrap());
    assert!(
        !store
            .complete_delivery_unit(&claim, 0, &[94001])
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .complete_delivery_unit(&claim, 0, &[94001, 94001])
            .await
            .unwrap_err(),
        StoreError::InvalidInput
    );
    assert_eq!(
        store
            .complete_delivery_unit(
                &claim,
                0,
                &[
                    94001, 94002, 94003, 94004, 94005, 94006, 94007, 94008, 94009, 94010, 94011
                ]
            )
            .await
            .unwrap_err(),
        StoreError::InvalidInput
    );
    assert!(!store.begin_delivery_unit(&claim, 2, 1).await.unwrap());
    assert_eq!(
        store
            .get(22401, admitted.operation_id)
            .await
            .unwrap()
            .unwrap()
            .delivered_artifact_count,
        0
    );
    assert_eq!(
        batch_units(&store, admitted.operation_id, 1).await,
        vec![(0, "sending".to_owned(), 2, None),]
    );
    assert!(
        store
            .complete_delivery_unit(&claim, 0, &[94001, 94002])
            .await
            .unwrap()
    );
    assert!(store.begin_delivery_unit(&claim, 1, 1).await.unwrap());
    assert!(
        store
            .complete_delivery_unit(&claim, 1, &[94003])
            .await
            .unwrap()
    );
    let record = store
        .get(22401, admitted.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, OperationStatus::Succeeded);
    assert_eq!(record.delivered_artifact_count, 3);
    assert_eq!(record.delivered_message_id, Some(94003));
    assert_eq!(
        batch_units(&store, admitted.operation_id, 1).await,
        vec![
            (0, "sent".to_owned(), 2, Some(vec![94001, 94002])),
            (1, "sent".to_owned(), 1, Some(vec![94003])),
        ]
    );
    store.close().await.unwrap();
}
