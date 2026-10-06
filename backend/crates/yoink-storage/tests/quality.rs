use std::time::Duration;

use uuid::Uuid;
use yoink_db::raw_sql;
use yoink_domain::{ClipRange, Container, DownloadPreferenceOverrides, Engine, PlaylistRange};
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
async fn cut_handoff_is_atomic_linked_and_replayed_after_reopen() {
    let fixture = common::store().await;
    let path = fixture.path();
    let params = DownloadParams::default().with_engine(Engine::Gallery);
    let seeded = DownloadRequest::new(
        ACTOR,
        "https://example.com/clip",
        MediaMode::Video,
        Some(ClipRange::new(4.0, 12.0).unwrap()),
        params,
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(14)).unwrap())
    .unwrap()
    .with_preference_overrides(DownloadPreferenceOverrides {
        container: Some(Container::Mkv),
        send_as_file: Some(true),
        ..Default::default()
    });
    let quality = fixture
        .quality_begin(ACTOR, target(), 14, &seeded)
        .await
        .unwrap();
    let cut = fixture
        .quality_cut(ACTOR, target(), quality.id, quality.revision)
        .await
        .unwrap();
    assert_eq!(cut.stage, yoink_storage::CutStage::Ready);
    assert_eq!((cut.start, cut.end), (Some(4.0), Some(12.0)));
    assert_eq!(cut.original_request(), Some(&seeded));
    assert_eq!(cut.linked_quality_id(), Some(quality.id));
    assert_eq!(
        fixture
            .quality_current(ACTOR, target())
            .await
            .unwrap()
            .unwrap()
            .stage,
        QualityStage::Cutting
    );
    assert!(
        fixture
            .quality_claim(
                ACTOR,
                target(),
                quality.id,
                quality.revision,
                QualityChoice::Audio
            )
            .await
            .is_err()
    );
    fixture.close().await.unwrap();
    let reopened = Store::open(&path, false, Duration::from_secs(2))
        .await
        .unwrap();
    let replay = reopened
        .quality_cut(ACTOR, target(), quality.id, quality.revision)
        .await
        .unwrap();
    assert_eq!(replay.id, cut.id);
    assert_eq!(replay.original_request(), Some(&seeded));
    let cancelled = reopened
        .cut_cancel(ACTOR, target(), cut.id, cut.revision, Some(15))
        .await
        .unwrap();
    assert_eq!(cancelled.stage, yoink_storage::CutStage::Cancelled);
    let replay_cancelled = reopened
        .quality_cut(ACTOR, target(), quality.id, quality.revision)
        .await
        .unwrap();
    assert_eq!(replay_cancelled.id, cut.id);
    assert_eq!(replay_cancelled.stage, yoink_storage::CutStage::Cancelled);
    let unrelated = reopened
        .cut_begin(ACTOR, target(), 16, "https://example.com/new", None)
        .await
        .unwrap();
    assert_eq!(unrelated.source_url(), Some("https://example.com/new"));
    assert!(unrelated.original_request().is_none());
    assert_eq!(
        reopened
            .quality_cut(ACTOR, target(), quality.id, quality.revision)
            .await,
        Err(StoreError::Conflict)
    );
}

#[tokio::test]
async fn cut_handoff_rejects_playlist_without_mutating_either_draft() {
    let store = common::store().await;
    let params =
        DownloadParams::default().with_playlist(Some(PlaylistRange::new(1, Some(2)).unwrap()));
    let request = DownloadRequest::new(
        ACTOR,
        "https://example.com/list",
        MediaMode::Video,
        None,
        params,
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(20)).unwrap())
    .unwrap();
    let quality = store
        .quality_begin(ACTOR, target(), 20, &request)
        .await
        .unwrap();
    assert_eq!(
        store
            .quality_cut(ACTOR, target(), quality.id, quality.revision)
            .await,
        Err(StoreError::InvalidInput)
    );
    assert_eq!(
        store
            .quality_current(ACTOR, target())
            .await
            .unwrap()
            .unwrap(),
        quality
    );
    assert!(store.cut_current(ACTOR, target()).await.unwrap().is_none());
}

#[tokio::test]
async fn cut_handoff_preserves_unrelated_active_cut_and_rejects_old_origin() {
    let store = common::store().await;
    let active = store
        .cut_begin(ACTOR, target(), 29, "https://example.com/existing", None)
        .await
        .unwrap();
    let quality = store
        .quality_begin(
            ACTOR,
            target(),
            30,
            &request(30, "https://example.com/owner"),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .quality_cut(ACTOR, target(), quality.id, quality.revision)
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        store
            .cut_current(ACTOR, target())
            .await
            .unwrap()
            .unwrap()
            .id,
        active.id
    );
    assert_eq!(
        store
            .quality_current(ACTOR, target())
            .await
            .unwrap()
            .unwrap(),
        quality
    );

    let older = common::store().await;
    let cut = older
        .cut_begin(ACTOR, target(), 32, "https://example.com/newer", None)
        .await
        .unwrap();
    let quality = older
        .quality_begin(
            ACTOR,
            target(),
            31,
            &request(31, "https://example.com/older"),
        )
        .await
        .unwrap();
    assert_eq!(
        older
            .quality_cut(ACTOR, target(), quality.id, quality.revision)
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        older
            .cut_current(ACTOR, target())
            .await
            .unwrap()
            .unwrap()
            .id,
        cut.id
    );

    let choosing = common::store().await;
    let _initial = choosing
        .cut_begin(ACTOR, target(), 34, "https://example.com/full", Some(1.0))
        .await
        .unwrap();
    let ready = choosing
        .cut_input(ACTOR, target(), 35, 4.0, false)
        .await
        .unwrap()
        .unwrap();
    let old_choice = choosing
        .cut_quality(ACTOR, target(), ready.id, ready.revision)
        .await
        .unwrap();
    let quality = choosing
        .quality_begin(
            ACTOR,
            target(),
            36,
            &request(36, "https://example.com/new-source"),
        )
        .await
        .unwrap();
    assert_eq!(
        choosing
            .quality_cut(ACTOR, target(), quality.id, quality.revision)
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        choosing
            .cut_current(ACTOR, target())
            .await
            .unwrap()
            .unwrap(),
        old_choice
    );
}

#[tokio::test]
async fn cut_handoff_checks_cut_watermark_and_submitted_replay_is_stable() {
    let store = common::store().await;
    let target = target();
    let cancelled = store
        .cut_begin(ACTOR, target, 40, "https://example.com/c", None)
        .await
        .unwrap();
    let cancelled = store
        .cut_cancel(ACTOR, target, cancelled.id, cancelled.revision, Some(45))
        .await
        .unwrap();
    let quality = store
        .quality_begin(ACTOR, target, 44, &request(44, "https://example.com/q"))
        .await
        .unwrap();
    assert_eq!(
        store
            .quality_cut(ACTOR, target, quality.id, quality.revision)
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        store.cut_current(ACTOR, target).await.unwrap().unwrap(),
        cancelled
    );

    let equal = common::store().await;
    let cut = equal
        .cut_begin(ACTOR, target, 47, "https://example.com/equal", None)
        .await
        .unwrap();
    let quality = equal
        .quality_begin(
            ACTOR,
            target,
            47,
            &request(47, "https://example.com/equal-quality"),
        )
        .await
        .unwrap();
    assert_eq!(
        equal
            .quality_cut(ACTOR, target, quality.id, quality.revision)
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        equal.cut_current(ACTOR, target).await.unwrap().unwrap().id,
        cut.id
    );

    let fresh = common::store().await;
    let ready_request = DownloadRequest::new(
        ACTOR,
        "https://example.com/submitted",
        MediaMode::Video,
        Some(ClipRange::new(2.0, 8.0).unwrap()),
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(50)).unwrap())
    .unwrap();
    let quality = fresh
        .quality_begin(ACTOR, target, 50, &ready_request)
        .await
        .unwrap();
    let cut = fresh
        .quality_cut(ACTOR, target, quality.id, quality.revision)
        .await
        .unwrap();
    let confirming = fresh
        .cut_claim(ACTOR, target, cut.id, cut.revision)
        .await
        .unwrap();
    let operation = Uuid::new_v4();
    let submitted = fresh
        .cut_submitted(ACTOR, target, cut.id, confirming.revision, operation)
        .await
        .unwrap();
    assert_eq!(
        fresh
            .cut_submitted(ACTOR, target, cut.id, confirming.revision, operation)
            .await
            .unwrap(),
        submitted
    );
    assert_eq!(
        fresh
            .quality_cut(ACTOR, target, quality.id, quality.revision)
            .await
            .unwrap(),
        submitted
    );
}

#[tokio::test]
async fn linked_original_must_match_row_and_unsupported_multiclip_is_rejected() {
    let store = common::store().await;
    let target = target();
    let params = DownloadParams::default()
        .with_clips(vec![ClipRange::new(1.0, 2.0).unwrap()])
        .unwrap();
    let multi_request = DownloadRequest::new(
        ACTOR,
        "https://example.com/multi",
        MediaMode::Video,
        None,
        params,
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(61)).unwrap())
    .unwrap();
    let quality = store
        .quality_begin(ACTOR, target, 61, &multi_request)
        .await
        .unwrap();
    assert_eq!(
        store
            .quality_cut(ACTOR, target, quality.id, quality.revision)
            .await,
        Err(StoreError::InvalidInput)
    );
    assert_eq!(
        store.quality_current(ACTOR, target).await.unwrap().unwrap(),
        quality
    );
    assert!(store.cut_current(ACTOR, target).await.unwrap().is_none());

    let request = request(62, "https://example.com/valid");
    let quality = store
        .quality_begin(ACTOR, target, 62, &request)
        .await
        .unwrap();
    let cut = store
        .quality_cut(ACTOR, target, quality.id, quality.revision)
        .await
        .unwrap();
    let database = store.database().clone();
    yoink_db::query(include_str!("../sql/corrupt_cut_original.sql"))
        .bind(ACTOR as i64)
        .execute(&database)
        .await
        .unwrap();
    assert_eq!(
        store.cut_current(ACTOR, target).await,
        Err(StoreError::InvalidStoredData)
    );
    assert!(!format!("{cut:?}").contains("example.com"));
}

#[tokio::test]
async fn cut_handoff_rolls_back_quality_transition_when_cut_insert_fails() {
    let store = common::store().await;
    let target = target();
    let quality = store
        .quality_begin(
            ACTOR,
            target,
            80,
            &request(80, "https://example.com/rollback"),
        )
        .await
        .unwrap();
    yoink_db::raw_sql(include_str!("../sql/test_fail_cut_handoff.sql"))
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        store
            .quality_cut(ACTOR, target, quality.id, quality.revision)
            .await,
        Err(StoreError::Database)
    );
    yoink_db::raw_sql(include_str!("../sql/test_restore_cut_handoff.sql"))
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        store.quality_current(ACTOR, target).await.unwrap().unwrap(),
        quality
    );
    assert!(store.cut_current(ACTOR, target).await.unwrap().is_none());
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
async fn audio_choice_is_durable_exclusive_and_replay_safe() {
    let fixture = common::store().await;
    let draft = fixture
        .quality_begin(
            ACTOR,
            target(),
            31,
            &request(31, "https://example.com/audio"),
        )
        .await
        .unwrap();
    let selected = fixture
        .quality_claim(
            ACTOR,
            target(),
            draft.id,
            draft.revision,
            QualityChoice::Audio,
        )
        .await
        .unwrap();
    assert_eq!(selected.selected, Some(QualityChoice::Audio));
    assert_eq!(selected.request().media_mode(), MediaMode::Video);
    assert_eq!(
        fixture
            .quality_claim(
                ACTOR,
                target(),
                draft.id,
                draft.revision,
                QualityChoice::Audio
            )
            .await
            .unwrap(),
        selected
    );
    assert_eq!(
        fixture
            .quality_claim(
                ACTOR,
                target(),
                draft.id,
                draft.revision,
                QualityChoice::Height(720)
            )
            .await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        fixture
            .quality_cancel(ACTOR, target(), draft.id, draft.revision)
            .await,
        Err(StoreError::Conflict)
    );
    let path = fixture.path();
    fixture.close().await.unwrap();
    let reopened = Store::open(&path, false, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        reopened.quality_current(ACTOR, target()).await.unwrap(),
        Some(selected)
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

#[tokio::test]
async fn quality_original_supersession_is_scoped_and_preserves_current_id_revisions() {
    let store = common::store().await;
    let first_request = request(10, "https://example.com/first-original");
    let first = store
        .quality_begin_media(ACTOR, target(), 10, &first_request, Some(&first_request))
        .await
        .unwrap();
    let other_actor = 43;
    let other_bot = BOT + 1;
    let scopes = [(other_actor, BOT), (ACTOR, other_bot)];
    let mut others = Vec::new();
    for (actor, bot) in scopes {
        let scope = DeliveryTarget::new(bot, actor as i64, None, Some(10)).unwrap();
        let original = DownloadRequest::new(
            actor,
            "https://example.com/other-original",
            MediaMode::Video,
            None,
            DownloadParams::default(),
        )
        .unwrap()
        .with_target(scope)
        .unwrap();
        let session = store
            .quality_begin_media(actor, scope, 10, &original, Some(&original))
            .await
            .unwrap();
        others.push((session, original));
    }
    // An idempotent begin and a same-ID revision transition keep the exact original.
    assert_eq!(
        store
            .quality_begin_media(ACTOR, target(), 10, &first_request, Some(&first_request))
            .await
            .unwrap()
            .id,
        first.id
    );
    let cancelled = store
        .quality_cancel(ACTOR, target(), first.id, first.revision)
        .await
        .unwrap();
    assert_eq!(cancelled.id, first.id);
    assert!(cancelled.revision > first.revision);
    assert_eq!(
        store.quality_media_intent(first.id).await.unwrap(),
        Some(first_request)
    );
    let replacement_request = request(11, "https://example.com/replacement-original");
    let replacement = store
        .quality_begin_media(
            ACTOR,
            target(),
            11,
            &replacement_request,
            Some(&replacement_request),
        )
        .await
        .unwrap();
    assert_ne!(replacement.id, first.id);
    assert!(
        store
            .quality_media_intent(first.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.quality_media_intent(replacement.id).await.unwrap(),
        Some(replacement_request)
    );
    for (session, original) in others {
        assert_eq!(
            store.quality_media_intent(session.id).await.unwrap(),
            Some(original)
        );
    }
    // A new non-deferred quality draft must clean its predecessor as well.
    store
        .quality_begin(
            ACTOR,
            target(),
            12,
            &request(12, "https://example.com/plain"),
        )
        .await
        .unwrap();
    assert!(
        store
            .quality_media_intent(replacement.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn superseding_quality_keeps_original_needed_by_surviving_cut_handoff() {
    let store = common::store().await;
    let original = DownloadRequest::new(
        ACTOR,
        "https://example.com/cut-original",
        MediaMode::Video,
        Some(ClipRange::new(1.0, 2.0).unwrap()),
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(10)).unwrap())
    .unwrap();
    let first = store
        .quality_begin_media(ACTOR, target(), 10, &original, Some(&original))
        .await
        .unwrap();
    let cut = store
        .quality_cut(ACTOR, target(), first.id, first.revision)
        .await
        .unwrap();
    assert_eq!(
        store.quality_media_intent(first.id).await.unwrap(),
        Some(original.clone())
    );
    let replacement_request = request(11, "https://example.com/replacement");
    let replacement = store
        .quality_begin_media(
            ACTOR,
            target(),
            11,
            &replacement_request,
            Some(&replacement_request),
        )
        .await
        .unwrap();
    assert_ne!(replacement.id, first.id);
    // The existing cut is independently current and claimable after quality replacement.
    let claimed = store
        .cut_claim(ACTOR, target(), cut.id, cut.revision)
        .await
        .unwrap();
    assert_eq!(claimed.linked_quality_id(), Some(first.id));
    assert_eq!(claimed.original_request(), Some(&original));
    assert_eq!(
        store.quality_media_intent(first.id).await.unwrap(),
        Some(original)
    );
    assert_eq!(
        store.quality_media_intent(replacement.id).await.unwrap(),
        Some(replacement_request)
    );
}

fn linked_original(actor: u64, bot: i64, message: i32) -> DownloadRequest {
    DownloadRequest::new(
        actor,
        &format!("https://example.com/lifecycle/{actor}/{bot}/{message}"),
        MediaMode::Video,
        Some(ClipRange::new(1.0, 2.0).unwrap()),
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(bot, actor as i64, None, Some(message)).unwrap())
    .unwrap()
}

#[tokio::test]
async fn replacing_last_cut_reference_reclaims_only_its_obsolete_quality_original() {
    let store = common::store().await;
    let mut other_scopes = Vec::new();
    for (actor, bot) in [(ACTOR + 1, BOT), (ACTOR, BOT + 1)] {
        let original = linked_original(actor, bot, 10);
        let scope = *original.target().unwrap();
        let quality = store
            .quality_begin_media(actor, scope, 10, &original, Some(&original))
            .await
            .unwrap();
        store
            .quality_cut(actor, scope, quality.id, quality.revision)
            .await
            .unwrap();
        other_scopes.push((quality.id, original));
    }
    let original = linked_original(ACTOR, BOT, 10);
    let mut quality = store
        .quality_begin_media(ACTOR, target(), 10, &original, Some(&original))
        .await
        .unwrap();
    let mut cut = store
        .quality_cut(ACTOR, target(), quality.id, quality.revision)
        .await
        .unwrap();
    // Repeated qualityA -> cutA -> qualityB -> cutB workflows must not accumulate A.
    for message in 11..15 {
        let claimed = store
            .cut_claim(ACTOR, target(), cut.id, cut.revision)
            .await
            .unwrap();
        let submitted = store
            .cut_submitted(ACTOR, target(), cut.id, claimed.revision, Uuid::new_v4())
            .await
            .unwrap();
        assert!(
            store
                .quality_media_intent(quality.id)
                .await
                .unwrap()
                .is_some(),
            "same-ID cut revisions must retain the original"
        );
        let old_quality = quality.id;
        let old_cut = cut.id;
        let original = linked_original(ACTOR, BOT, message);
        quality = store
            .quality_begin_media(ACTOR, target(), message, &original, Some(&original))
            .await
            .unwrap();
        assert!(
            store
                .quality_media_intent(old_quality)
                .await
                .unwrap()
                .is_some(),
            "submitted cut remains replayable until replaced"
        );
        assert_eq!(
            store
                .cut_claim(ACTOR, target(), old_cut, submitted.revision)
                .await
                .unwrap()
                .id,
            old_cut
        );
        cut = store
            .quality_cut(ACTOR, target(), quality.id, quality.revision)
            .await
            .unwrap();
        assert!(
            store
                .quality_media_intent(old_quality)
                .await
                .unwrap()
                .is_none(),
            "no consumer remains after cut supersession"
        );
        assert_eq!(
            store.quality_media_intent(quality.id).await.unwrap(),
            Some(original)
        );
        assert_eq!(
            store
                .cut_claim(ACTOR, target(), old_cut, submitted.revision)
                .await,
            Err(StoreError::Conflict)
        );
        for (id, original) in &other_scopes {
            assert_eq!(
                store.quality_media_intent(*id).await.unwrap(),
                Some(original.clone())
            );
        }
    }
}

#[tokio::test]
async fn cancelling_last_cut_reference_reclaims_original_unless_quality_still_needs_it() {
    for supersede_quality in [false, true] {
        let store = common::store().await;
        let original = linked_original(ACTOR, BOT, 10);
        let quality = store
            .quality_begin_media(ACTOR, target(), 10, &original, Some(&original))
            .await
            .unwrap();
        let cut = store
            .quality_cut(ACTOR, target(), quality.id, quality.revision)
            .await
            .unwrap();
        let replacement = if supersede_quality {
            let request = linked_original(ACTOR, BOT, 11);
            Some(
                store
                    .quality_begin_media(ACTOR, target(), 11, &request, Some(&request))
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        store
            .cut_cancel(ACTOR, target(), cut.id, cut.revision, None)
            .await
            .unwrap();
        assert_eq!(
            store
                .quality_media_intent(quality.id)
                .await
                .unwrap()
                .is_none(),
            supersede_quality
        );
        if let Some(replacement) = replacement {
            assert!(
                store
                    .quality_media_intent(replacement.id)
                    .await
                    .unwrap()
                    .is_some()
            );
        } else {
            assert_eq!(
                store
                    .quality_current(ACTOR, target())
                    .await
                    .unwrap()
                    .unwrap()
                    .id,
                quality.id
            );
        }
    }
}
