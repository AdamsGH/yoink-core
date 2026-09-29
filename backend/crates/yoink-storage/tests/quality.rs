use std::time::Duration;

use uuid::Uuid;
use yoink_db::raw_sql;
use yoink_domain::{ClipRange, DownloadPreferenceOverrides, Engine, PlaylistRange};
use yoink_domain::{DeliveryTarget, DownloadParams, DownloadRequest, MediaMode};
use yoink_storage::{QualityChoice, QualityStage, Store, StoreError};

mod common;

const ACTOR: u64 = 42;
const BOT: i64 = 123;
fn target() -> DeliveryTarget {
    DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap()
}
fn request(message: i32, url: &str) -> DownloadRequest {
    DownloadRequest::new(
        ACTOR,
        url,
        MediaMode::Video,
        None,
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(message)).unwrap())
    .unwrap()
}

#[tokio::test]
async fn quality_migration_is_accepted_by_native_engine() {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Store::open(&dir.path().join("schema.db"), true, Duration::from_secs(2))
        .await
        .unwrap();
    let connection = store.database().connection().await.unwrap();
    connection
        .execute(
            include_str!("../migrations-turso/0009_quality_sessions.sql"),
            (),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn begin_watermarks_replace_intent_and_replay_without_mutation() {
    let store = common::store().await;
    let first = store
        .quality_begin(ACTOR, target(), 10, &request(10, "https://example.com/a"))
        .await
        .unwrap();
    let replay = store
        .quality_begin(ACTOR, target(), 10, &request(10, "https://example.com/b"))
        .await;
    assert_eq!(replay, Err(StoreError::Conflict));
    assert_eq!(
        store
            .quality_begin(ACTOR, target(), 9, &request(9, "https://example.com/old"))
            .await
            .unwrap(),
        first
    );
    let replacement = store
        .quality_begin(ACTOR, target(), 11, &request(11, "https://example.com/c"))
        .await
        .unwrap();
    assert_ne!(first.id, replacement.id);
    assert_eq!(replacement.origin_message_id, 11);
}

#[tokio::test]
async fn transitions_replay_guards_and_restart_persistence() {
    let fixture = common::store().await;
    let s = fixture
        .quality_begin(ACTOR, target(), 10, &request(10, "https://example.com/a"))
        .await
        .unwrap();
    assert_eq!(
        fixture
            .quality_claim(ACTOR, target(), s.id, 0, QualityChoice::Best)
            .await,
        Err(StoreError::InvalidInput)
    );
    assert_eq!(
        fixture
            .quality_claim(ACTOR, target(), Uuid::new_v4(), 1, QualityChoice::Best)
            .await,
        Err(StoreError::NotFoundOrStale)
    );
    let confirming = fixture
        .quality_claim(ACTOR, target(), s.id, 1, QualityChoice::Height(720))
        .await
        .unwrap();
    assert_eq!(confirming.stage, QualityStage::Confirming);
    assert_eq!(
        fixture
            .quality_claim(ACTOR, target(), s.id, 1, QualityChoice::Height(720))
            .await
            .unwrap(),
        confirming
    );
    assert_eq!(
        fixture
            .quality_claim(ACTOR, target(), s.id, 2, QualityChoice::Height(720))
            .await
            .unwrap(),
        confirming
    );
    assert_eq!(
        fixture
            .quality_claim(ACTOR, target(), s.id, 1, QualityChoice::Height(1080))
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        fixture.quality_cancel(ACTOR, target(), s.id, 1).await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        fixture
            .quality_begin(ACTOR, target(), 11, &request(11, "https://example.com/b"))
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        fixture
            .quality_submitted(ACTOR, target(), s.id, 1, Uuid::new_v4())
            .await,
        Err(StoreError::Conflict)
    );
    let operation = Uuid::new_v4();
    let submitted = fixture
        .quality_submitted(ACTOR, target(), s.id, 2, operation)
        .await
        .unwrap();
    assert_eq!(submitted.stage, QualityStage::Submitted);
    assert_eq!(
        fixture
            .quality_submitted(ACTOR, target(), s.id, 3, operation)
            .await
            .unwrap(),
        submitted
    );
    let path = fixture.path();
    fixture.close().await.unwrap();
    let reopened = Store::open(&path, false, Duration::from_secs(5))
        .await
        .unwrap();
    reopened.verify_schema().await.unwrap();
    assert_eq!(
        reopened.quality_current(ACTOR, target()).await.unwrap(),
        Some(submitted)
    );
}

#[tokio::test]
async fn cancellation_replay_is_limited_and_debug_redacts_source() {
    let store = common::store().await;
    let s = store
        .quality_begin(
            ACTOR,
            target(),
            10,
            &request(10, "https://example.com/a?secret=private"),
        )
        .await
        .unwrap();
    assert!(!format!("{s:?}").contains("secret=private"));
    assert_eq!(s.request().actor_id(), ACTOR);
    let cancelled = store
        .quality_cancel(ACTOR, target(), s.id, 1)
        .await
        .unwrap();
    assert_eq!(cancelled.stage, QualityStage::Cancelled);
    assert_eq!(
        store
            .quality_cancel(ACTOR, target(), s.id, 1)
            .await
            .unwrap(),
        cancelled
    );
    assert_eq!(
        store.quality_cancel(ACTOR, target(), s.id, 2).await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        store
            .quality_claim(ACTOR, target(), s.id, 1, QualityChoice::Best)
            .await,
        Err(StoreError::Conflict)
    );
}

#[tokio::test]
async fn rejects_invalid_actor_target_and_draft_intent_before_persisting() {
    let store = common::store().await;
    assert_eq!(
        store.quality_current(0, target()).await,
        Err(StoreError::InvalidInput)
    );
    let other_target = DeliveryTarget::new(BOT, 43, None, None).unwrap();
    assert_eq!(
        store
            .quality_begin(ACTOR, other_target, 1, &request(1, "https://example.com/a"))
            .await,
        Err(StoreError::InvalidInput)
    );
    let audio = DownloadRequest::new(
        ACTOR,
        "https://example.com/a",
        MediaMode::Audio,
        None,
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(1)).unwrap())
    .unwrap();
    assert_eq!(
        store.quality_begin(ACTOR, target(), 1, &audio).await,
        Err(StoreError::InvalidInput)
    );
    let constrained = DownloadRequest::new(
        ACTOR,
        "https://example.com/a",
        MediaMode::Video,
        None,
        DownloadParams::new(Some(720)).unwrap(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(1)).unwrap())
    .unwrap();
    assert_eq!(
        store.quality_begin(ACTOR, target(), 1, &constrained).await,
        Err(StoreError::InvalidInput)
    );
    assert_eq!(store.quality_current(ACTOR, target()).await.unwrap(), None);
}

#[tokio::test]
async fn rejects_corrupt_persisted_request_without_exposing_it() {
    let store = common::store().await;
    store
        .quality_begin(ACTOR, target(), 10, &request(10, "https://example.com/a"))
        .await
        .unwrap();
    raw_sql("UPDATE runtime_quality_sessions SET request_json='{}' WHERE actor_id=42")
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        store.quality_current(ACTOR, target()).await,
        Err(StoreError::InvalidStoredData)
    );
}

#[tokio::test]
async fn draft_roundtrip_preserves_clips_playlist_engine_and_explicit_intent() {
    let store = common::store().await;
    for (message, clip, playlist) in [
        (10, Some(ClipRange::new(0.2, 0.7).unwrap()), None),
        (11, None, Some(PlaylistRange::new(2, Some(3)).unwrap())),
    ] {
        let original = DownloadRequest::new(
            ACTOR,
            "https://example.com/private-marker",
            MediaMode::Video,
            clip,
            DownloadParams::default()
                .with_engine(Engine::YtDlp)
                .with_playlist(playlist),
        )
        .unwrap()
        .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(message)).unwrap())
        .unwrap()
        .with_preference_overrides(DownloadPreferenceOverrides {
            send_as_file: Some(true),
            ..Default::default()
        });
        let created = store
            .quality_begin(ACTOR, target(), message, &original)
            .await
            .unwrap();
        let stored = store
            .quality_current(ACTOR, target())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored, created);
        assert_eq!(stored.request(), &original);
        assert!(!format!("{stored:?}").contains("private-marker"));
        let explicit_height =
            original
                .clone()
                .with_preference_overrides(DownloadPreferenceOverrides {
                    max_height: Some(None),
                    ..Default::default()
                });
        assert_eq!(
            store
                .quality_begin(ACTOR, target(), message, &explicit_height)
                .await,
            Err(StoreError::InvalidInput)
        );
    }
}
