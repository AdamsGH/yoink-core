use std::{path::PathBuf, sync::Arc, time::Duration};

use yoink_domain::{
    AudioCodec, ClipRange, DeliveryTarget, DownloadParams, DownloadPreferenceOverrides,
    DownloadRequest, MediaMode, PlaylistRange,
};
use yoink_downloads::{
    Delivery, DeliveryArtifact, DeliveryFuture, DownloadConfig, DownloadError, DownloadService,
    TelegramSubmission,
};
use yoink_executor::{Executor, ExecutorConfig};
use yoink_identity::IdentityStore;
use yoink_storage::{QualityChoice, QualityStage, Store};

const ACTOR: u64 = 42;
const BOT: i64 = 123456;
const URL: &str = "https://example.invalid/video";

struct PanicDelivery;
impl Delivery for PanicDelivery {
    fn bot_id(&self) -> i64 {
        BOT
    }
    fn deliver(&self, _: DeliveryTarget, _: bool, _: Vec<DeliveryArtifact>) -> DeliveryFuture<'_> {
        panic!("quality admission must not deliver")
    }
}

async fn setup() -> (tempfile::TempDir, Arc<Store>, DownloadService) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let workspace = dir.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let store = Store::open(&dir.path().join("quality.db"), true, Duration::from_secs(2))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let db = store.database().clone();
    yoink_db::raw_sql(include_str!("../../yoink-identity/tests/fixtures.sql"))
        .execute(&db)
        .await
        .unwrap();
    yoink_db::raw_sql(include_str!("admission_users.sql"))
        .execute(&db)
        .await
        .unwrap();
    yoink_db::raw_sql(include_str!("saved_preference_settings.sql"))
        .execute(&db)
        .await
        .unwrap();
    let store = Arc::new(store);
    let service = service_for(Arc::clone(&store), workspace, Some(ACTOR as i64));
    (dir, store, service)
}

fn service_for(store: Arc<Store>, workspace: PathBuf, owner: Option<i64>) -> DownloadService {
    let identities = Arc::new(IdentityStore::new(store.database().clone(), owner, false));
    let executor = Executor::new(ExecutorConfig {
        program: PathBuf::from("/bin/false"),
        args: Vec::new(),
        workspace_root: workspace,
        timeout: Duration::from_secs(30),
        processing_timeout: Duration::from_secs(20),
        socket_timeout: Duration::from_secs(5),
        max_artifact_bytes: 1024 * 1024,
        max_workspace_bytes: 2 * 1024 * 1024,
        max_output_bytes: 1024 * 1024,
        max_artifacts: 1,
        allowed_proxy_origins: Vec::new(),
        allowed_private_origins: Vec::new(),
    })
    .unwrap();
    DownloadService::new(
        Arc::clone(&store),
        identities,
        executor,
        Arc::new(PanicDelivery),
        DownloadConfig::default(),
    )
    .unwrap()
}

async fn persisted_request(store: &Store, operation: uuid::Uuid) -> serde_json::Value {
    let row = yoink_db::query(include_str!("sql/preference_request_by_id.sql"))
        .bind(operation.to_string())
        .fetch_one(store.database())
        .await
        .unwrap();
    serde_json::from_str(&row.try_get::<String, _>("request").unwrap()).unwrap()
}

fn request(url: &str, message: i32) -> DownloadRequest {
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

async fn ask(store: &Store) {
    yoink_db::query(include_str!("sql/quality_ask.sql"))
        .bind(ACTOR as i64)
        .execute(store.database())
        .await
        .unwrap();
}

#[tokio::test]
async fn ask_prompt_is_durable_without_admission_and_replay_checks_actual_intent() {
    let (_dir, store, service) = setup().await;
    ask(&store).await;
    let draft = service
        .submit_telegram_request(
            &request(URL, 2100),
            2100,
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap();
    let TelegramSubmission::Quality(draft) = draft else {
        panic!("ask preference should prompt")
    };
    assert_eq!(draft.stage, QualityStage::Choosing);
    assert_eq!(draft.origin_message_id, 2100);
    let changed = request("https://example.invalid/other", 2100);
    assert_eq!(
        service
            .submit_telegram_request(&changed, 2100, DownloadPreferenceOverrides::default())
            .await,
        Err(DownloadError::Conflict)
    );
    let changed_settings =
        yoink_downloads::SettingChange::new(yoink_downloads::SettingField::Quality, "best")
            .unwrap();
    let settings = service
        .settings(
            ACTOR,
            DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap(),
        )
        .await
        .unwrap();
    service
        .change_setting(
            ACTOR,
            DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap(),
            changed_settings,
            Some(settings.revision),
        )
        .await
        .unwrap();
    let replay = service
        .submit_telegram_request(
            &request(URL, 2100),
            2100,
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap();
    assert!(matches!(replay, TelegramSubmission::Quality(s) if s.id == draft.id));
    // A callback prompt only stores a session; it never creates a download admission.
    assert_eq!(
        store
            .quality_current(
                ACTOR,
                DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap()
            )
            .await
            .unwrap()
            .unwrap()
            .id,
        draft.id
    );
}

#[tokio::test]
async fn superseded_origin_is_fenced_before_preferences_or_direct_admission() {
    let (_dir, store, service) = setup().await;
    ask(&store).await;
    let first = service
        .submit_telegram_request(
            &request(URL, 2200),
            2200,
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap();
    let TelegramSubmission::Quality(first) = first else {
        panic!("expected prompt")
    };
    let next = service
        .submit_telegram_request(
            &request(URL, 2201),
            2201,
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap();
    let TelegramSubmission::Quality(next) = next else {
        panic!("expected replacement prompt")
    };
    assert_ne!(first.id, next.id);
    assert_eq!(
        service
            .submit_telegram_request(
                &request(URL, 2200),
                2200,
                DownloadPreferenceOverrides::default()
            )
            .await,
        Err(DownloadError::Conflict)
    );
    let explicit = DownloadPreferenceOverrides {
        max_height: Some(Some(480)),
        ..DownloadPreferenceOverrides::default()
    };
    assert_eq!(
        service
            .submit_telegram_request(&request(URL, 2200), 2200, explicit)
            .await,
        Err(DownloadError::Conflict)
    );
    assert_eq!(
        store
            .quality_current(
                ACTOR,
                DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap()
            )
            .await
            .unwrap()
            .unwrap()
            .id,
        next.id
    );
}

#[tokio::test]
async fn confirmation_admits_once_and_replays_the_original_callback_revision() {
    let (_dir, store, service) = setup().await;
    ask(&store).await;
    let target = DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap();
    let TelegramSubmission::Quality(draft) = service
        .submit_telegram_request(
            &request(URL, 2300),
            2300,
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap()
    else {
        panic!("expected prompt")
    };
    let admitted = service
        .quality_confirm(
            ACTOR,
            target,
            draft.id,
            draft.revision,
            QualityChoice::Height(720),
        )
        .await
        .unwrap();
    assert!(admitted.created);
    let recovered = service
        .quality_confirm(
            ACTOR,
            target,
            draft.id,
            draft.revision,
            QualityChoice::Height(720),
        )
        .await
        .unwrap();
    assert_eq!(recovered.operation_id, admitted.operation_id);
    assert!(!recovered.created);
    let confirming_revision = service
        .quality_confirm(
            ACTOR,
            target,
            draft.id,
            draft.revision + 1,
            QualityChoice::Height(720),
        )
        .await
        .unwrap();
    assert_eq!(confirming_revision.operation_id, admitted.operation_id);
    assert_eq!(
        service
            .quality_confirm(ACTOR, target, draft.id, 99, QualityChoice::Height(720))
            .await,
        Err(DownloadError::Conflict)
    );
    assert_eq!(
        service
            .quality_confirm(
                ACTOR,
                target,
                draft.id,
                draft.revision,
                QualityChoice::Height(480)
            )
            .await,
        Err(DownloadError::Conflict)
    );
    assert_eq!(
        service
            .quality_cancel(ACTOR, target, draft.id, draft.revision)
            .await,
        Err(DownloadError::Conflict)
    );
    let state = store.quality_current(ACTOR, target).await.unwrap().unwrap();
    assert_eq!(state.stage, QualityStage::Submitted);
    assert_eq!(state.operation_id, Some(admitted.operation_id));
    assert_eq!(state.request().source_url().as_str(), URL);
}

#[tokio::test]
async fn audio_choice_keeps_video_draft_and_selects_saved_or_explicit_audio_preferences() {
    let (_dir, store, service) = setup().await;
    ask(&store).await;
    let original_params = DownloadParams::default()
        .with_engine(yoink_domain::Engine::YtDlp)
        .with_playlist(Some(PlaylistRange::new(2, Some(4)).unwrap()));
    let original = DownloadRequest::new(ACTOR, URL, MediaMode::Video, None, original_params)
        .unwrap()
        .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(2400)).unwrap())
        .unwrap();
    let TelegramSubmission::Quality(draft) = service
        .submit_telegram_request(&original, 2400, DownloadPreferenceOverrides::default())
        .await
        .unwrap()
    else {
        panic!("expected quality prompt")
    };
    assert_eq!(
        draft.request(),
        &original
            .clone()
            .with_preference_overrides(DownloadPreferenceOverrides::default())
    );
    // Invalid video-only settings after the menu is durable must not block audio selection.
    yoink_db::raw_sql(include_str!("sql/invalid_video_preferences.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let target = DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap();
    let selected = service
        .quality_confirm(
            ACTOR,
            target,
            draft.id,
            draft.revision,
            QualityChoice::Audio,
        )
        .await
        .unwrap();
    assert!(selected.created);
    let submitted = store.quality_current(ACTOR, target).await.unwrap().unwrap();
    assert_eq!(submitted.stage, QualityStage::Submitted);
    assert_eq!(submitted.selected, Some(QualityChoice::Audio));
    assert_eq!(
        submitted.request(),
        &original
            .clone()
            .with_preference_overrides(DownloadPreferenceOverrides::default())
    );
    assert_eq!(
        service
            .quality_confirm(
                ACTOR,
                target,
                draft.id,
                draft.revision,
                QualityChoice::Height(720)
            )
            .await,
        Err(DownloadError::Conflict)
    );
    assert_eq!(
        service
            .quality_cancel(ACTOR, target, draft.id, draft.revision)
            .await,
        Err(DownloadError::Conflict)
    );
    let saved = yoink_db::query(include_str!("sql/saved_audio_preferences.sql"))
        .bind(ACTOR as i64)
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(saved.try_get::<String, _>("audio_codec").unwrap(), "opus");
    assert_eq!(saved.try_get::<i64, _>("send_as_file").unwrap(), 1);
    let frozen = persisted_request(&store, selected.operation_id).await;
    assert_eq!(frozen["media_mode"], "audio");
    assert_eq!(frozen["params"]["audio_codec"], "opus");
    assert_eq!(frozen["params"]["send_as_file"], true);
    assert_eq!(
        frozen["params"]["playlist"],
        serde_json::json!({"start": 2, "end": 4})
    );
    assert_eq!(frozen["params"]["engine"], "yt_dlp");
    yoink_db::query(include_str!("sql/change_saved_audio_preferences.sql"))
        .bind(ACTOR as i64)
        .execute(store.database())
        .await
        .unwrap();
    let replay = service
        .quality_confirm(
            ACTOR,
            target,
            draft.id,
            draft.revision,
            QualityChoice::Audio,
        )
        .await
        .unwrap();
    assert_eq!(replay.operation_id, selected.operation_id);
    assert!(!replay.created);
    assert_eq!(
        persisted_request(&store, selected.operation_id).await,
        frozen
    );

    let (_dir, store, service) = setup().await;
    ask(&store).await;
    let TelegramSubmission::Quality(draft) = service
        .submit_telegram_request(
            &request(URL, 2401),
            2401,
            DownloadPreferenceOverrides {
                audio_codec: Some(AudioCodec::Aac),
                send_as_file: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap()
    else {
        panic!("expected quality prompt")
    };
    let target = DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap();
    let explicit = service
        .quality_confirm(
            ACTOR,
            target,
            draft.id,
            draft.revision,
            QualityChoice::Audio,
        )
        .await
        .unwrap();
    let effective = persisted_request(&store, explicit.operation_id).await;
    assert_eq!(effective["media_mode"], "audio");
    assert_eq!(effective["params"]["audio_codec"], "aac");
    assert_eq!(effective["params"]["send_as_file"], false);
    let submitted = store.quality_current(ACTOR, target).await.unwrap().unwrap();
    assert_eq!(submitted.request().media_mode(), MediaMode::Video);
    assert_eq!(submitted.selected, Some(QualityChoice::Audio));
    assert_eq!(
        yoink_db::query(include_str!("sql/saved_audio_preferences.sql"))
            .bind(ACTOR as i64)
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get::<String, _>("audio_codec")
            .unwrap(),
        "opus"
    );
}

#[tokio::test]
async fn audio_choice_preserves_clip_structures_and_cancel_remains_exclusive() {
    let (_dir, store, service) = setup().await;
    ask(&store).await;
    let clip = ClipRange::new(3.0, 8.0).unwrap();
    let params = DownloadParams::default()
        .with_clips(vec![ClipRange::new(1.0, 2.0).unwrap(), clip])
        .unwrap();
    let original = DownloadRequest::new(ACTOR, URL, MediaMode::Video, None, params)
        .unwrap()
        .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(2500)).unwrap())
        .unwrap();
    let TelegramSubmission::Quality(draft) = service
        .submit_telegram_request(&original, 2500, DownloadPreferenceOverrides::default())
        .await
        .unwrap()
    else {
        panic!("expected prompt")
    };
    assert_eq!(draft.request().params().clips(), original.params().clips());
    let target = DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap();
    let admission = service
        .quality_confirm(
            ACTOR,
            target,
            draft.id,
            draft.revision,
            QualityChoice::Audio,
        )
        .await
        .unwrap();
    let submitted = store.quality_current(ACTOR, target).await.unwrap().unwrap();
    assert_eq!(submitted.stage, QualityStage::Submitted);
    assert_eq!(submitted.selected, Some(QualityChoice::Audio));
    assert_eq!(
        submitted.request(),
        &original
            .clone()
            .with_preference_overrides(DownloadPreferenceOverrides::default())
    );
    assert_eq!(
        service
            .quality_confirm(
                ACTOR,
                target,
                draft.id,
                draft.revision,
                QualityChoice::Height(720)
            )
            .await,
        Err(DownloadError::Conflict)
    );
    let effective = persisted_request(&store, admission.operation_id).await;
    assert_eq!(effective["media_mode"], "audio");
    assert_eq!(
        effective["params"]["clips"],
        serde_json::json!([
            {"start_seconds": 1.0, "end_seconds": 2.0},
            {"start_seconds": 3.0, "end_seconds": 8.0},
        ])
    );
    let TelegramSubmission::Quality(next) = service
        .submit_telegram_request(
            &request(URL, 2501),
            2501,
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap()
    else {
        panic!("expected replacement prompt")
    };
    let cancelled = service
        .quality_cancel(ACTOR, target, next.id, next.revision)
        .await
        .unwrap();
    assert_eq!(cancelled.stage, QualityStage::Cancelled);
    assert_eq!(cancelled.request().media_mode(), MediaMode::Video);
}

#[tokio::test]
async fn audio_quality_replay_and_actions_recheck_ban_private_chat_and_current_bot() {
    let (_dir, store, service) = setup().await;
    ask(&store).await;
    let TelegramSubmission::Quality(draft) = service
        .submit_telegram_request(
            &request(URL, 2600),
            2600,
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap()
    else {
        panic!("expected prompt")
    };
    assert_eq!(
        service
            .quality_current(
                ACTOR,
                DeliveryTarget::new(BOT + 1, ACTOR as i64, None, None).unwrap()
            )
            .await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        service
            .quality_current(ACTOR, DeliveryTarget::new(BOT, 43, None, None).unwrap())
            .await,
        Err(DownloadError::Denied)
    );
    yoink_db::query(include_str!("sql/set_banned.sql"))
        .bind(ACTOR as i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service
            .submit_telegram_request(
                &request(URL, 2600),
                2600,
                DownloadPreferenceOverrides::default()
            )
            .await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        service
            .quality_confirm(
                ACTOR,
                DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap(),
                draft.id,
                draft.revision,
                QualityChoice::Audio,
            )
            .await,
        Err(DownloadError::Denied)
    );
}

#[tokio::test]
async fn audio_crash_recovery_reopens_frozen_request_and_rechecks_configured_owner() {
    let (dir, store, service) = setup().await;
    ask(&store).await;
    let target = DeliveryTarget::new(BOT, ACTOR as i64, None, None).unwrap();
    let original = DownloadRequest::new(
        ACTOR,
        URL,
        MediaMode::Video,
        Some(ClipRange::new(2.0, 4.0).unwrap()),
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(2700)).unwrap())
    .unwrap();
    let TelegramSubmission::Quality(draft) = service
        .submit_telegram_request(&original, 2700, DownloadPreferenceOverrides::default())
        .await
        .unwrap()
    else {
        panic!("expected quality draft")
    };
    let admitted = service
        .quality_confirm(
            ACTOR,
            target,
            draft.id,
            draft.revision,
            QualityChoice::Audio,
        )
        .await
        .unwrap();
    let frozen = persisted_request(&store, admitted.operation_id).await;
    assert_eq!(frozen["media_mode"], "audio");
    assert_eq!(
        frozen["clip_range"],
        serde_json::json!({"start_seconds": 2.0, "end_seconds": 4.0})
    );
    yoink_db::query(include_str!("sql/quality_audio_recovery.sql"))
        .bind(ACTOR as i64)
        .execute(store.database())
        .await
        .unwrap();
    yoink_db::query(include_str!("sql/change_saved_audio_preferences.sql"))
        .bind(ACTOR as i64)
        .execute(store.database())
        .await
        .unwrap();
    drop(service);
    drop(store);
    let reopened = Arc::new(
        Store::open(
            &dir.path().join("quality.db"),
            false,
            Duration::from_secs(2),
        )
        .await
        .unwrap(),
    );
    reopened.migrate().await.unwrap();
    let foreign_owner = service_for(
        Arc::clone(&reopened),
        dir.path().join("workspace"),
        Some(43),
    );
    assert_eq!(
        foreign_owner
            .quality_confirm(
                ACTOR,
                target,
                draft.id,
                draft.revision,
                QualityChoice::Audio
            )
            .await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        reopened
            .quality_current(ACTOR, target)
            .await
            .unwrap()
            .unwrap()
            .stage,
        QualityStage::Confirming
    );
    let service = service_for(
        Arc::clone(&reopened),
        dir.path().join("workspace"),
        Some(ACTOR as i64),
    );
    let recovered = service
        .quality_confirm(
            ACTOR,
            target,
            draft.id,
            draft.revision,
            QualityChoice::Audio,
        )
        .await
        .unwrap();
    assert_eq!(recovered.operation_id, admitted.operation_id);
    assert!(!recovered.created);
    assert_eq!(
        persisted_request(&reopened, recovered.operation_id).await,
        frozen
    );
    let session = reopened
        .quality_current(ACTOR, target)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.stage, QualityStage::Submitted);
    assert_eq!(
        session.request(),
        &original
            .clone()
            .with_preference_overrides(DownloadPreferenceOverrides::default())
    );
    for target in [
        DeliveryTarget::new(BOT + 1, ACTOR as i64, None, None).unwrap(),
        DeliveryTarget::new(BOT, 43, None, None).unwrap(),
        DeliveryTarget::new(BOT, ACTOR as i64, Some(7), None).unwrap(),
    ] {
        assert_eq!(
            service
                .quality_confirm(
                    ACTOR,
                    target,
                    draft.id,
                    draft.revision,
                    QualityChoice::Audio
                )
                .await,
            Err(DownloadError::Denied)
        );
    }
    yoink_db::query(include_str!("sql/set_banned.sql"))
        .bind(ACTOR as i64)
        .execute(reopened.database())
        .await
        .unwrap();
    assert_eq!(
        service
            .quality_confirm(
                ACTOR,
                target,
                draft.id,
                draft.revision,
                QualityChoice::Audio
            )
            .await,
        Err(DownloadError::Denied)
    );
}
