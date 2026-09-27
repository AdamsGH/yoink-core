use std::time::Duration;

use yoink_domain::{
    CookieReference, CredentialVersion, DeliveryTarget, DownloadParams, DownloadRequest, MediaMode,
    NetworkSelection,
};
use yoink_storage::{AdmissionPolicy, Store, StoreError};

const ACTOR: u64 = 8_000_000_000_501;
const BOT: i64 = 8_000_000_000_502;

fn request(url: &str, params: DownloadParams) -> DownloadRequest {
    DownloadRequest::new(ACTOR, url, MediaMode::Video, None, params)
        .unwrap()
        .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap())
        .unwrap()
}

fn prepared(raw: &DownloadRequest, version: &str) -> DownloadRequest {
    raw.clone()
        .with_network(NetworkSelection::new(
            Some(
                CookieReference::new(
                    501,
                    ACTOR,
                    "example.com",
                    false,
                    CredentialVersion::new(version.repeat(64)).unwrap(),
                )
                .unwrap(),
            ),
            None,
        ))
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL migrated through Alembic and runtime bootstrap"]
async fn first_network_snapshot_survives_replay_rotation_and_admission_races() {
    let url =
        std::env::var("YOINK_TEST_DATABASE_URL").expect("requires disposable database fixture");
    let store = Store::connect(&url, 4, Duration::from_secs(5))
        .await
        .unwrap();
    store.verify_schema().await.unwrap();
    let raw = request("https://example.com/media", DownloadParams::default());
    let first = prepared(&raw, "a");
    let rotated = prepared(&raw, "b");
    let policy = AdmissionPolicy {
        max_attempts: 2,
        retry_delay: Duration::ZERO,
        max_pending_global: 1000,
        max_pending_actor: 1,
    };
    let key = "network-snapshot-replay";
    assert_eq!(
        store.replay_network_admission(&raw, key).await.unwrap(),
        None
    );
    let admission = store
        .admit_network_snapshot(&first, key, policy)
        .await
        .unwrap();
    assert!(admission.created);
    let replay = store
        .replay_network_admission(&raw, key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.operation_id, admission.operation_id);
    assert!(!replay.created);
    assert_eq!(
        store.replay_network_admission(&first, key).await,
        Err(StoreError::InvalidInput)
    );
    let rotated_replay = store
        .admit_network_snapshot(&rotated, key, policy)
        .await
        .unwrap();
    assert_eq!(rotated_replay, replay);
    assert_eq!(
        store.admit_bounded(&rotated, key, policy).await,
        Err(StoreError::Conflict)
    );

    for changed in [
        request("https://example.com/different", DownloadParams::default()),
        request(
            "https://example.com/media",
            DownloadParams::new(Some(720)).unwrap(),
        ),
        raw.clone()
            .with_target(DeliveryTarget::new(BOT + 1, ACTOR as i64, None, None).unwrap())
            .unwrap(),
        raw.clone()
            .with_target(DeliveryTarget::new(BOT, ACTOR as i64, Some(17), None).unwrap())
            .unwrap(),
    ] {
        assert_eq!(
            store.replay_network_admission(&changed, key).await,
            Err(StoreError::Conflict)
        );
        assert_eq!(
            store.admit_network_snapshot(&changed, key, policy).await,
            Err(StoreError::Conflict)
        );
    }
    let claim = store
        .claim_for_bot("snapshot-fixture", Duration::from_secs(30), BOT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.operation_id, admission.operation_id);
    assert_eq!(
        claim.request, first,
        "rotation/replay must not replace frozen credentials"
    );
    store.fail(&claim, false).await.unwrap();

    let (a, b) = tokio::join!(
        store.admit_network_snapshot(&first, "network-snapshot-race", policy),
        store.admit_network_snapshot(&rotated, "network-snapshot-race", policy),
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.operation_id, b.operation_id);
    assert_ne!(
        a.created, b.created,
        "exactly one racing caller creates the operation"
    );
    let claim = store
        .claim_for_bot("snapshot-fixture", Duration::from_secs(30), BOT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.request, if a.created { first } else { rotated });
    store.fail(&claim, false).await.unwrap();
}
