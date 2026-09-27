use std::time::Duration;

use sqlx::Row;
use yoink_domain::{DownloadParams, DownloadRequest, MediaMode};
use yoink_storage::{Store, StoreError};

fn database_url() -> String {
    std::env::var("YOINK_TEST_DATABASE_URL")
        .expect("YOINK_TEST_DATABASE_URL must point to a disposable PostgreSQL database")
}

async fn store() -> Store {
    Store::connect(&database_url(), 8, Duration::from_secs(5))
        .await
        .expect("connect to disposable test database")
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
#[ignore = "requires a fresh disposable PostgreSQL database"]
async fn queue_persistence_contract() {
    assert_eq!(
        Store::connect("invalid", 0, Duration::from_secs(1))
            .await
            .err()
            .unwrap(),
        StoreError::InvalidInput
    );
    assert_eq!(
        Store::connect("invalid", 33, Duration::from_secs(1))
            .await
            .err()
            .unwrap(),
        StoreError::InvalidInput
    );
    assert_eq!(
        Store::connect("invalid", 1, Duration::from_secs(301))
            .await
            .err()
            .unwrap(),
        StoreError::InvalidInput
    );
    let store = store().await;
    assert_eq!(store.verify_schema().await.unwrap_err(), StoreError::Schema);
    store.migrate().await.unwrap();
    store.migrate().await.unwrap();
    store.verify_schema().await.unwrap();
    let history = sqlx::query(include_str!("fixtures/migration_history.sql"))
        .fetch_all(store.pool())
        .await
        .unwrap();
    let (version, checksum): (i64, Vec<u8>) = (
        history[0].try_get("version").unwrap(),
        history[0].try_get("checksum").unwrap(),
    );
    sqlx::query(include_str!("fixtures/migration_corrupt_checksum.sql"))
        .bind(version)
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(store.verify_schema().await.unwrap_err(), StoreError::Schema);
    sqlx::query(include_str!("fixtures/migration_restore_checksum.sql"))
        .bind(version)
        .bind(&checksum)
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query(include_str!("fixtures/migration_mark_failed.sql"))
        .bind(version)
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(store.verify_schema().await.unwrap_err(), StoreError::Schema);
    sqlx::query(include_str!("fixtures/migration_mark_success.sql"))
        .bind(version)
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query(include_str!("fixtures/migration_insert_unknown.sql"))
        .bind(version + 1)
        .bind(&checksum)
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(store.verify_schema().await.unwrap_err(), StoreError::Schema);
    sqlx::query(include_str!("fixtures/migration_delete_unknown.sql"))
        .bind(version + 1)
        .execute(store.pool())
        .await
        .unwrap();
    let history_after = sqlx::query(include_str!("fixtures/migration_history.sql"))
        .fetch_all(store.pool())
        .await
        .unwrap();
    assert_eq!(history_after.len(), history.len());
    for (actual, expected) in history_after.iter().zip(&history) {
        assert_eq!(
            actual.try_get::<i64, _>("version").unwrap(),
            expected.try_get::<i64, _>("version").unwrap()
        );
        assert_eq!(
            actual.try_get::<Vec<u8>, _>("checksum").unwrap(),
            expected.try_get::<Vec<u8>, _>("checksum").unwrap()
        );
        assert_eq!(
            actual.try_get::<bool, _>("success").unwrap(),
            expected.try_get::<bool, _>("success").unwrap()
        );
    }
    let public_migration_table: i64 =
        sqlx::query(include_str!("fixtures/count_public_migrations.sql"))
            .fetch_one(store.pool())
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
    let mut rollback_tx = store.pool().begin().await.unwrap();
    sqlx::query(include_str!("../sql/admit_insert.sql"))
        .bind(10009_i64)
        .bind("rolled-back")
        .bind(serde_json::json!({"probe": true}))
        .bind(1_i32)
        .execute(&mut *rollback_tx)
        .await
        .unwrap();
    rollback_tx.rollback().await.unwrap();
    let rolled_back: i64 =
        sqlx::query_scalar(include_str!("fixtures/count_rolled_back_operation.sql"))
            .fetch_one(store.pool())
            .await
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
    store.fail(&first_attempt, true).await.unwrap();
    let second_attempt = store
        .claim("retry-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_attempt.operation_id, retry.operation_id);
    assert_eq!(second_attempt.attempt, 2);
    store.fail(&second_attempt, true).await.unwrap();
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
    sqlx::query(include_str!("fixtures/expire_operation.sql"))
        .bind(expiring.operation_id)
        .execute(store.pool())
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
    sqlx::query(include_str!("fixtures/expire_final_operation.sql"))
        .bind(final_expiry.operation_id)
        .execute(store.pool())
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
    sqlx::query(include_str!("fixtures/expire_operation.sql"))
        .bind(cancelled_expiry.operation_id)
        .execute(store.pool())
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
    let mut lock_tx = store.pool().begin().await.unwrap();
    let locked_id: uuid::Uuid = sqlx::query(include_str!("fixtures/lock_operation.sql"))
        .bind(contention.operation_id)
        .fetch_one(&mut *lock_tx)
        .await
        .unwrap()
        .try_get("id")
        .unwrap();
    assert_eq!(locked_id, contention.operation_id);
    let skipped = tokio::time::timeout(
        Duration::from_secs(1),
        store.claim("skip-locked-worker", Duration::from_secs(5)),
    )
    .await
    .expect("claim must not wait on a locked queue row")
    .unwrap();
    assert!(skipped.is_none());
    lock_tx.rollback().await.unwrap();
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
    let replacement = Store::connect(&database_url(), 2, Duration::from_secs(5))
        .await
        .unwrap();
    let original_record = store
        .get(10013, persisted.operation_id)
        .await
        .unwrap()
        .unwrap();
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
    replacement.close().await;

    let malicious = store
        .admit(
            &request(10011, "https://example.com/malicious"),
            "malicious",
            1,
        )
        .await
        .unwrap();
    sqlx::query(include_str!("fixtures/poison_operation_request.sql"))
        .bind(malicious.operation_id)
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(
        store
            .claim("malicious-worker", Duration::from_secs(5))
            .await
            .unwrap_err(),
        StoreError::InvalidStoredData
    );

    store.close().await;
}
