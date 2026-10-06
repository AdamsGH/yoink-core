//! Saved-preference and interactive clip admission through the public download service.
use std::{path::PathBuf, sync::Arc, time::Duration};

use yoink_domain::{
    AudioCodec, DeliveryTarget, DownloadParams, DownloadPreferenceOverrides, DownloadRequest,
    MediaMode, VideoCodec,
};
use yoink_downloads::{
    Delivery, DeliveryArtifact, DeliveryFuture, DownloadConfig, DownloadError, DownloadService,
};
use yoink_executor::{Executor, ExecutorConfig};
use yoink_identity::IdentityStore;
use yoink_storage::Store;

const ACTOR: u64 = 42;
const BOT: i64 = 123456;
const URL: &str = "http://127.0.0.1:12345/fixture.mp4?source-secret-marker-fake";

struct PanicDelivery;
impl Delivery for PanicDelivery {
    fn bot_id(&self) -> i64 {
        BOT
    }
    fn deliver(&self, _: DeliveryTarget, _: bool, _: Vec<DeliveryArtifact>) -> DeliveryFuture<'_> {
        panic!("delivery must not run during admission-only regression")
    }
}

async fn setup() -> (tempfile::TempDir, Arc<Store>, DownloadService) {
    setup_with_owner(None).await
}

async fn setup_with_owner(
    owner_id: Option<i64>,
) -> (tempfile::TempDir, Arc<Store>, DownloadService) {
    setup_with_quotas(owner_id, yoink_storage::QuotaLimits::default()).await
}

async fn setup_with_quotas(
    owner_id: Option<i64>,
    quotas: yoink_storage::QuotaLimits,
) -> (tempfile::TempDir, Arc<Store>, DownloadService) {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let store = Store::open(
        &directory.path().join("preferences.db"),
        true,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    let database = store.database().clone();
    yoink_db::raw_sql(include_str!("../../yoink-identity/tests/fixtures.sql"))
        .execute(&database)
        .await
        .unwrap();
    yoink_db::raw_sql(include_str!("admission_users.sql"))
        .execute(&database)
        .await
        .unwrap();
    yoink_db::raw_sql(include_str!("saved_preference_settings.sql"))
        .execute(&database)
        .await
        .unwrap();
    let identities = Arc::new(IdentityStore::new(database, owner_id, false));
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
    let store = Arc::new(store);
    let service = DownloadService::new(
        Arc::clone(&store),
        identities,
        executor,
        Arc::new(PanicDelivery),
        DownloadConfig {
            quotas,
            admission: yoink_storage::AdmissionPolicy {
                max_pending_actor: 10,
                ..DownloadConfig::default().admission
            },
            ..DownloadConfig::default()
        },
    )
    .unwrap();
    (directory, store, service)
}

#[tokio::test]
async fn cut_confirmation_replays_frozen_preferences_and_checks_authorization() {
    use yoink_downloads::CutStage;
    let (_directory, store, service) = setup_with_owner(Some(ACTOR as i64)).await;
    let target = DeliveryTarget::new(BOT, ACTOR as i64, None, Some(2100)).unwrap();
    let callback_target = DeliveryTarget::new(BOT, ACTOR as i64, None, Some(9999)).unwrap();
    let draft = service
        .cut_begin(ACTOR, target, 2100, URL, None)
        .await
        .unwrap();
    assert_eq!(draft.stage, CutStage::Start);
    let end = service
        .cut_input(ACTOR, target, 2101, 0.2, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(end.stage, CutStage::End);
    let ready = service
        .cut_input(ACTOR, target, 2102, 0.7, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.stage, CutStage::Ready);
    let admitted = service
        .cut_confirm(ACTOR, callback_target, ready.id, ready.revision)
        .await
        .unwrap();
    assert!(admitted.created);
    let frozen = persisted_request(&store, &admitted.operation_id).await;
    assert_eq!(
        frozen["clip_range"],
        serde_json::json!({"start_seconds": 0.2, "end_seconds": 0.7})
    );
    assert_eq!(frozen["target"]["reply_to_message_id"], 2100);
    assert_eq!(frozen["params"]["send_as_file"], true);
    // A valid new preference must not replace the already-admitted snapshot.
    let settings = service.settings(ACTOR, target).await.unwrap();
    let change =
        yoink_downloads::SettingChange::new(yoink_downloads::SettingField::SendAsFile, "false")
            .unwrap();
    service
        .change_setting(ACTOR, target, change, Some(settings.revision))
        .await
        .unwrap();
    let replay = service
        .cut_confirm(ACTOR, callback_target, ready.id, ready.revision)
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.operation_id, admitted.operation_id);
    assert_eq!(
        persisted_request(&store, &admitted.operation_id).await,
        frozen
    );
    yoink_db::query(include_str!("sql/set_banned.sql"))
        .bind(ACTOR as i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service
            .cut_confirm(ACTOR, callback_target, ready.id, ready.revision)
            .await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        service.cut_current(ACTOR, target).await.unwrap_err(),
        DownloadError::Denied
    );
}

#[tokio::test]
async fn formats_submission_ignores_saved_quality_and_enforces_private_owner_target() {
    use yoink_downloads::TelegramSubmission;

    let (_directory, store, service) = setup_with_owner(Some(ACTOR as i64)).await;
    yoink_db::raw_sql(include_str!("sql/set_quality_ask.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let target = DeliveryTarget::new(BOT, ACTOR as i64, None, Some(2600)).unwrap();
    let request = DownloadRequest::new(
        ACTOR,
        URL,
        MediaMode::Formats,
        None,
        DownloadParams::default().with_send_as_file(true),
    )
    .unwrap()
    .with_target(target)
    .unwrap();
    let admitted = service
        .submit_telegram_request(&request, 2600, DownloadPreferenceOverrides::default())
        .await
        .unwrap();
    let TelegramSubmission::Admitted(admission) = admitted else {
        panic!("formats request must be admitted, not sent to quality selection");
    };
    assert!(admission.created);
    let frozen = persisted_request(&store, &admission.operation_id).await;
    assert_eq!(frozen["params"]["send_as_file"], true);
    assert_eq!(frozen["media_mode"], "formats");
    yoink_db::raw_sql(include_str!("sql/formats_invalid_preferences.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let preferred = service
        .submit_preferred_request(
            &request,
            "formats-preferred-admission",
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap();
    assert!(preferred.created);
    let preferred_frozen = persisted_request(&store, &preferred.operation_id).await;
    assert_eq!(preferred_frozen["params"]["send_as_file"], true);
    let direct = service
        .submit_request(&request, "formats-direct-admission")
        .await
        .unwrap();
    assert!(direct.created);
    assert_eq!(
        persisted_request(&store, &direct.operation_id).await["media_mode"],
        "formats"
    );
    let network = persisted_request(&store, &direct.operation_id).await;
    assert_eq!(network["network"]["cookie"], serde_json::Value::Null);
    assert_eq!(network["network"]["proxy"], serde_json::Value::Null);
    assert_eq!(
        service
            .submit_telegram_request(&request, 2600, DownloadPreferenceOverrides::default(),)
            .await
            .unwrap(),
        TelegramSubmission::Admitted(yoink_storage::Admission {
            operation_id: admission.operation_id,
            created: false,
        })
    );

    let group_target = DeliveryTarget::new(BOT, -100, None, Some(2601)).unwrap();
    let group_request = DownloadRequest::new(
        ACTOR,
        URL,
        MediaMode::Formats,
        None,
        DownloadParams::default().with_send_as_file(true),
    )
    .unwrap()
    .with_target(group_target)
    .unwrap();
    assert_eq!(
        service
            .submit_telegram_request(&group_request, 2601, DownloadPreferenceOverrides::default(),)
            .await
            .unwrap_err(),
        DownloadError::Denied
    );
    assert_eq!(
        service
            .submit_request(&group_request, "formats-direct-group")
            .await
            .unwrap_err(),
        DownloadError::Denied
    );
    assert_eq!(
        service
            .submit_preferred_request(
                &group_request,
                "formats-preferred-group",
                DownloadPreferenceOverrides::default()
            )
            .await
            .unwrap_err(),
        DownloadError::Denied
    );
    // A stored owner role does not open the experimental command without configured ownership.
    let (_unconfigured_directory, _unconfigured_store, unconfigured_service) = setup().await;
    assert_eq!(
        unconfigured_service
            .submit_request(&request, "formats-unconfigured-owner")
            .await
            .unwrap_err(),
        DownloadError::Denied
    );
    let wrong_bot_request = DownloadRequest::new(
        ACTOR,
        URL,
        MediaMode::Formats,
        None,
        DownloadParams::default().with_send_as_file(true),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT + 1, ACTOR as i64, None, Some(2603)).unwrap())
    .unwrap();
    assert_eq!(
        service
            .submit_telegram_request(
                &wrong_bot_request,
                2603,
                DownloadPreferenceOverrides::default(),
            )
            .await
            .unwrap_err(),
        DownloadError::InvalidInput
    );

    yoink_db::query(include_str!("sql/set_banned.sql"))
        .bind(ACTOR as i64)
        .execute(store.database())
        .await
        .unwrap();
    let banned_request = DownloadRequest::new(
        ACTOR,
        URL,
        MediaMode::Formats,
        None,
        DownloadParams::default().with_send_as_file(true),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(2602)).unwrap())
    .unwrap();
    assert_eq!(
        service
            .submit_telegram_request(
                &banned_request,
                2602,
                DownloadPreferenceOverrides::default(),
            )
            .await
            .unwrap_err(),
        DownloadError::Denied
    );
}

#[tokio::test]
async fn cut_recovers_admission_before_session_completion_without_a_second_job() {
    use yoink_domain::ClipRange;
    use yoink_downloads::CutStage;
    let (_directory, store, service) = setup_with_owner(Some(ACTOR as i64)).await;
    let target = DeliveryTarget::new(BOT, ACTOR as i64, None, Some(2200)).unwrap();
    service
        .cut_begin(ACTOR, target, 2200, URL, Some(0.2))
        .await
        .unwrap();
    let ready = service
        .cut_input(ACTOR, target, 2201, 0.7, true)
        .await
        .unwrap()
        .unwrap();
    let request = DownloadRequest::new(
        ACTOR,
        URL,
        MediaMode::Video,
        Some(ClipRange::new(0.2, 0.7).unwrap()),
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(target)
    .unwrap();
    let key = format!("cut:{}:{}:{}", BOT, ACTOR, ready.id.simple());
    let admitted = service
        .submit_preferred_request(&request, &key, DownloadPreferenceOverrides::default())
        .await
        .unwrap();
    yoink_db::raw_sql(include_str!("sql/cut_recovery_confirming.sql"))
        .execute(store.database())
        .await
        .unwrap();
    // Fault boundary: SQL restores the durable Confirming checkpoint after admission.
    assert_eq!(
        service
            .cut_current(ACTOR, target)
            .await
            .unwrap()
            .unwrap()
            .stage,
        CutStage::Confirming
    );
    assert!(
        service
            .cut_cancel(ACTOR, target, ready.id, ready.revision, None)
            .await
            .is_err()
    );
    assert!(
        service
            .cut_edit(ACTOR, target, ready.id, ready.revision)
            .await
            .is_err()
    );
    assert!(
        service
            .cut_begin(ACTOR, target, 2202, URL, None)
            .await
            .is_err()
    );
    let recovered = service
        .cut_confirm(ACTOR, target, ready.id, ready.revision)
        .await
        .unwrap();
    assert!(!recovered.created);
    assert_eq!(recovered.operation_id, admitted.operation_id);
    let session = service.cut_current(ACTOR, target).await.unwrap().unwrap();
    assert_eq!(session.stage, CutStage::Submitted);
    assert_eq!(session.operation_id, Some(admitted.operation_id));
}

#[tokio::test]
async fn cut_state_is_denied_outside_the_actors_own_private_delivery_target() {
    let (_directory, _store, service) = setup_with_owner(Some(ACTOR as i64)).await;
    for target in [
        DeliveryTarget::new(BOT, 99, None, None).unwrap(),
        DeliveryTarget::new(BOT, -100, None, None).unwrap(),
        DeliveryTarget::new(BOT + 1, ACTOR as i64, None, None).unwrap(),
    ] {
        assert_eq!(
            service
                .cut_begin(ACTOR, target, 2300, URL, None)
                .await
                .unwrap_err(),
            DownloadError::Denied
        );
        assert_eq!(
            service.cut_current(ACTOR, target).await.unwrap_err(),
            DownloadError::Denied
        );
        assert_eq!(
            service
                .cut_input(ACTOR, target, 2301, 0.2, true)
                .await
                .unwrap_err(),
            DownloadError::Denied
        );
    }
}

#[tokio::test]
async fn cut_ask_choice_admits_only_after_selection_with_frozen_explicit_quality() {
    use yoink_downloads::{CutStage, CutSubmission, QualityChoice};
    let (_directory, store, service) = setup_with_owner(Some(ACTOR as i64)).await;
    let target = DeliveryTarget::new(BOT, ACTOR as i64, None, Some(2400)).unwrap();
    yoink_db::raw_sql(include_str!("sql/set_quality_ask.sql"))
        .execute(store.database())
        .await
        .unwrap();
    service
        .cut_begin(ACTOR, target, 2400, URL, Some(2.0))
        .await
        .unwrap();
    let ready = service
        .cut_input(ACTOR, target, 2401, 4.0, false)
        .await
        .unwrap()
        .unwrap();
    let CutSubmission::Quality(chooser) = service
        .cut_submit(ACTOR, target, ready.id, ready.revision)
        .await
        .unwrap()
    else {
        panic!("ask preference must show chooser")
    };
    assert_eq!(chooser.stage, CutStage::Quality);
    assert_eq!(chooser.operation_id, None);
    let selected = service
        .cut_choose(
            ACTOR,
            target,
            chooser.id,
            chooser.revision,
            QualityChoice::Height(720),
        )
        .await
        .unwrap();
    let admitted = service
        .cut_confirm(ACTOR, target, selected.id, selected.revision)
        .await
        .unwrap();
    let frozen = persisted_request(&store, &admitted.operation_id).await;
    assert_eq!(
        frozen["clip_range"],
        serde_json::json!({"start_seconds": 2.0, "end_seconds": 4.0})
    );
    assert_eq!(frozen["target"]["reply_to_message_id"], 2400);
    assert_eq!(frozen["params"]["max_height"], 720);
    assert_eq!(frozen["params"]["send_as_file"], true);
    let replay = service
        .cut_confirm(ACTOR, target, selected.id, selected.revision)
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.operation_id, admitted.operation_id);
    service
        .cut_begin(ACTOR, target, 2410, URL, Some(5.0))
        .await
        .unwrap();
    let ready = service
        .cut_input(ACTOR, target, 2411, 7.0, false)
        .await
        .unwrap()
        .unwrap();
    let CutSubmission::Quality(chooser) = service
        .cut_submit(ACTOR, target, ready.id, ready.revision)
        .await
        .unwrap()
    else {
        panic!("ask preference must show chooser")
    };
    let selected = service
        .cut_choose(
            ACTOR,
            target,
            chooser.id,
            chooser.revision,
            QualityChoice::Best,
        )
        .await
        .unwrap();
    let best = service
        .cut_confirm(ACTOR, target, selected.id, selected.revision)
        .await
        .unwrap();
    assert_eq!(
        persisted_request(&store, &best.operation_id).await["params"]["max_height"],
        serde_json::Value::Null
    );
    yoink_db::query(include_str!("sql/set_banned.sql"))
        .bind(ACTOR as i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service
            .cut_confirm(ACTOR, target, selected.id, selected.revision)
            .await,
        Err(DownloadError::Denied)
    );
}

fn request() -> DownloadRequest {
    DownloadRequest::new(
        ACTOR,
        URL,
        MediaMode::Video,
        None,
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(2001)).unwrap())
    .unwrap()
}

async fn persisted_request(store: &Store, operation_id: &uuid::Uuid) -> serde_json::Value {
    yoink_db::query(include_str!("sql/preference_request_by_id.sql"))
        .bind(*operation_id)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get::<serde_json::Value, _>("request")
        .unwrap()
}

#[tokio::test]
async fn subtitle_preferences_are_frozen_and_replay_does_not_reresolve_them() {
    let (_directory, store, service) = setup().await;
    let request = DownloadRequest::new(
        ACTOR,
        URL,
        MediaMode::Subtitles,
        None,
        DownloadParams::default(),
    )
    .unwrap()
    .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(2001)).unwrap())
    .unwrap();
    let update = |language: &'static str, automatic: bool| {
        yoink_db::query(include_str!("sql/update_subtitle_preferences.sql"))
            .bind(ACTOR as i64)
            .bind(language)
            .bind(automatic)
            .bind(false)
    };
    update("ru", true).execute(store.database()).await.unwrap();
    let first = service
        .submit_preferred_request(
            &request,
            "subtitle-frozen",
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap();
    let frozen = persisted_request(&store, &first.operation_id).await;
    assert_eq!(
        frozen["params"]["subtitles"],
        serde_json::json!({"language": "ru", "automatic": true})
    );
    assert_eq!(frozen["params"]["max_height"], serde_json::Value::Null);
    assert_eq!(frozen["params"]["audio_codec"], "best");
    // subs_enabled is false: an explicit /subs request is not an embedding preference.
    update("en.*", false)
        .execute(store.database())
        .await
        .unwrap();
    let replay = service
        .submit_preferred_request(
            &request,
            "subtitle-frozen",
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap();
    assert_eq!(replay.operation_id, first.operation_id);
    assert!(!replay.created);
    assert_eq!(persisted_request(&store, &first.operation_id).await, frozen);
    assert_eq!(
        service
            .submit_preferred_request(
                &request,
                "subtitle-invalid",
                DownloadPreferenceOverrides::default()
            )
            .await,
        Err(DownloadError::InvalidPreferences)
    );
    let override_options = DownloadPreferenceOverrides {
        subtitle_language: Some("en".into()),
        subtitle_automatic: Some(false),
        ..Default::default()
    };
    let overridden = service
        .submit_preferred_request(&request, "subtitle-overridden", override_options.clone())
        .await
        .unwrap();
    assert_eq!(
        persisted_request(&store, &overridden.operation_id).await["params"]["subtitles"],
        serde_json::json!({"language": "en", "automatic": false})
    );
    assert!(
        service
            .submit_preferred_request(&request, "subtitle-frozen", override_options)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn admitted_preferences_are_frozen_and_replayed_before_settings_are_read() {
    // This preference contract admits more than five NEW operations in one minute.
    // Its explicit fixture limit isolates preference resolution from quota exhaustion.
    let (_directory, store, service) = setup_with_quotas(
        None,
        yoink_storage::QuotaLimits {
            minute: 20,
            ..yoink_storage::QuotaLimits::default()
        },
    )
    .await;
    let request = request();
    let first = service
        .submit_preferred_request(
            &request,
            "preferred-freeze",
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap();
    assert!(first.created);
    let frozen_request = persisted_request(&store, &first.operation_id).await;
    let params = &frozen_request["params"];
    assert_eq!(params["max_height"], 720);
    assert_eq!(params["audio_codec"], "best");
    assert_eq!(params["video_codec"], "h264");
    assert_eq!(params["send_as_file"], true);
    assert_eq!(frozen_request["source_url"], URL);
    assert!(params.get("args").is_none());
    assert!(params.get("subtitle").is_none());
    assert!(
        frozen_request
            .to_string()
            .find("raw-args-secret-marker")
            .is_none()
    );
    assert!(frozen_request.to_string().find("submark").is_none());

    let explicit_overrides = DownloadPreferenceOverrides {
        max_height: Some(None),
        video_codec: Some(VideoCodec::Any),
        audio_codec: Some(AudioCodec::Best),
        send_as_file: Some(false),
        ..DownloadPreferenceOverrides::default()
    };
    let rescued = service
        .submit_preferred_request(&request, "preferred-overrides", explicit_overrides.clone())
        .await
        .unwrap();
    assert!(rescued.created);
    let rescued_request = persisted_request(&store, &rescued.operation_id).await;
    assert_eq!(
        rescued_request["params"]["max_height"],
        serde_json::Value::Null
    );
    assert_eq!(rescued_request["params"]["video_codec"], "any");
    assert_eq!(rescued_request["params"]["send_as_file"], false);

    yoink_db::query(include_str!("sql/corrupt_preferences.sql"))
        .bind(ACTOR as i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service
            .submit_preferred_request(
                &request,
                "preferred-invalid-key",
                DownloadPreferenceOverrides::default()
            )
            .await,
        Err(DownloadError::InvalidPreferences)
    );

    let rescued_while_corrupt = service
        .submit_preferred_request(
            &request,
            "preferred-overrides-after-corruption",
            explicit_overrides.clone(),
        )
        .await
        .unwrap();
    assert!(rescued_while_corrupt.created);
    assert_eq!(
        persisted_request(&store, &rescued_while_corrupt.operation_id).await,
        rescued_request
    );

    let rescued_replay = service
        .submit_preferred_request(&request, "preferred-overrides", explicit_overrides)
        .await
        .unwrap();
    assert_eq!(rescued_replay.operation_id, rescued.operation_id);
    assert!(!rescued_replay.created);
    assert_eq!(
        persisted_request(&store, &rescued.operation_id).await,
        rescued_request
    );

    for (mode, key) in [
        (MediaMode::Audio, "irrelevant-audio"),
        (MediaMode::Image, "irrelevant-image"),
    ] {
        let irrelevant_format_request =
            DownloadRequest::new(ACTOR, URL, mode, None, DownloadParams::default())
                .unwrap()
                .with_target(DeliveryTarget::new(BOT, ACTOR as i64, None, Some(2002)).unwrap())
                .unwrap();
        assert!(
            service
                .submit_preferred_request(
                    &irrelevant_format_request,
                    key,
                    DownloadPreferenceOverrides::default()
                )
                .await
                .is_ok()
        );
    }

    yoink_db::query(include_str!("sql/delete_preferences.sql"))
        .bind(ACTOR as i64)
        .execute(store.database())
        .await
        .unwrap();
    let defaults = service
        .submit_preferred_request(
            &request,
            "preferred-new-key",
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap();
    assert!(defaults.created);
    let default_request = persisted_request(&store, &defaults.operation_id).await;
    assert_eq!(
        default_request["params"]["max_height"],
        serde_json::Value::Null
    );
    let replay = service
        .submit_preferred_request(
            &request,
            "preferred-freeze",
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap();
    assert_eq!(replay.operation_id, first.operation_id);
    assert!(!replay.created);
    assert_eq!(
        persisted_request(&store, &first.operation_id).await,
        frozen_request
    );

    yoink_db::query(include_str!("sql/set_banned.sql"))
        .bind(ACTOR as i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service
            .submit_preferred_request(
                &request,
                "preferred-overrides",
                explicit_overrides_for_ban(),
            )
            .await,
        Err(DownloadError::Denied)
    );
}

fn explicit_overrides_for_ban() -> DownloadPreferenceOverrides {
    DownloadPreferenceOverrides {
        max_height: Some(None),
        video_codec: Some(VideoCodec::Any),
        audio_codec: Some(AudioCodec::Best),
        send_as_file: Some(false),
        ..DownloadPreferenceOverrides::default()
    }
}

#[tokio::test]
async fn settings_writes_preserve_secrets_and_admitted_jobs_and_survive_reopen() {
    use yoink_downloads::{SettingChange, SettingField, profile::ProfileStore};
    let (directory, store, service) = setup().await;
    let target = *request().target().unwrap();
    let profiles = ProfileStore::new(store.database().clone());
    let before = profiles.preferences(ACTOR).await.unwrap().unwrap();
    let admission = service
        .submit_preferred_request(
            &request(),
            "before-settings",
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap();
    let frozen = persisted_request(&store, &admission.operation_id).await;
    for (field, value) in [
        (SettingField::Quality, "360"),
        (SettingField::SendAsFile, "false"),
        (SettingField::SubtitleLanguage, "zh-Hans"),
    ] {
        let change = SettingChange::new(field, value).unwrap();
        service
            .change_setting(ACTOR, target, change.clone(), None)
            .await
            .unwrap();
        service
            .change_setting(ACTOR, target, change, None)
            .await
            .unwrap();
    }
    let after = profiles.preferences(ACTOR).await.unwrap().unwrap();
    assert_eq!(after.quality, "360");
    assert!(!after.send_as_file);
    assert_eq!(after.subs_lang, "zh-Hans");
    assert_eq!(after.args_json, before.args_json);
    assert_eq!(after.proxy_url, before.proxy_url);
    assert_eq!(after.audio_codec, before.audio_codec);
    assert_eq!(after.use_pool_cookies, before.use_pool_cookies);
    assert_eq!(
        persisted_request(&store, &admission.operation_id).await,
        frozen
    );
    let next = service
        .submit_preferred_request(
            &request(),
            "after-settings",
            DownloadPreferenceOverrides::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        persisted_request(&store, &next.operation_id).await["params"]["max_height"],
        360
    );
    assert!(
        !format!("{:?}", service.settings(ACTOR, target).await.unwrap())
            .contains("raw-args-secret-marker")
    );
    drop(profiles);
    drop(service);
    store.close().await.unwrap();
    drop(store);
    let reopened = Store::open(
        &directory.path().join("preferences.db"),
        false,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert_eq!(
        ProfileStore::new(reopened.database().clone())
            .preferences(ACTOR)
            .await
            .unwrap()
            .unwrap()
            .quality,
        "360"
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn stale_settings_buttons_cannot_replace_a_later_choice() {
    use yoink_downloads::{SettingChange, SettingField};
    let (_directory, _store, service) = setup().await;
    let target = *request().target().unwrap();
    let revision = service.settings(ACTOR, target).await.unwrap().revision;
    let first = SettingChange::new(SettingField::Quality, "360").unwrap();
    service
        .change_setting(ACTOR, target, first.clone(), Some(revision))
        .await
        .unwrap();
    let newer = service.settings(ACTOR, target).await.unwrap().revision;
    assert!(newer > revision);
    service
        .change_setting(
            ACTOR,
            target,
            SettingChange::new(SettingField::Quality, "720").unwrap(),
            Some(newer),
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .change_setting(ACTOR, target, first, Some(revision))
            .await,
        Err(DownloadError::Conflict)
    );
    assert!(
        service
            .settings(ACTOR, target)
            .await
            .unwrap()
            .values
            .contains(&(SettingField::Quality, "720".into()))
    );
}

#[tokio::test]
async fn settings_create_only_for_authorized_private_actor_and_never_change_other_actor() {
    use yoink_downloads::{SettingChange, SettingField};
    let (_directory, _store, service) = setup().await;
    let change = || SettingChange::new(SettingField::AudioCodec, "mp3").unwrap();
    let user = 8000000000011;
    let target = DeliveryTarget::new(BOT, user as i64, None, None).unwrap();
    service
        .change_setting(user, target, change(), None)
        .await
        .unwrap();
    assert!(
        service
            .settings(user, target)
            .await
            .unwrap()
            .values
            .contains(&(SettingField::AudioCodec, "mp3".into()))
    );
    for actor in [8000000000012, 8000000000013, 8000000000014, 999999] {
        let target = DeliveryTarget::new(BOT, actor as i64, None, None).unwrap();
        assert_eq!(
            service.change_setting(actor, target, change(), None).await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service.settings(actor, target).await,
            Err(DownloadError::Denied)
        );
    }
    for target in [
        DeliveryTarget::new(BOT, 43, None, None).unwrap(),
        DeliveryTarget::new(BOT, -43, None, None).unwrap(),
        DeliveryTarget::new(BOT + 1, 42, None, None).unwrap(),
    ] {
        assert_eq!(
            service.change_setting(ACTOR, target, change(), None).await,
            Err(DownloadError::Denied)
        );
    }
}

#[tokio::test]
async fn explicit_submit_still_rejects_a_forged_preference_marker() {
    let (_directory, _store, service) = setup().await;
    let forged = request().with_preference_overrides(DownloadPreferenceOverrides::default());
    assert_eq!(
        service.submit_request(&forged, "forged-marker").await,
        Err(DownloadError::InvalidInput)
    );
}

#[tokio::test]
async fn time_quota_uses_configured_id_not_owner_role_and_all_public_admission_paths() {
    use yoink_db::query;
    // This fixture user has an owner ROLE but no configured owner ID.
    let (_dir, store, service) = setup().await;
    let req = request();
    let mut admissions = Vec::new();
    for i in 0..5 {
        let key = format!("quota-{i}");
        let admission = if i % 2 == 0 {
            service.submit_request(&req, &key).await.unwrap()
        } else {
            service
                .submit_preferred_request(&req, &key, DownloadPreferenceOverrides::default())
                .await
                .unwrap()
        };
        admissions.push(admission);
    }
    assert!(matches!(
        service.submit_request(&req, "quota-over").await,
        Err(DownloadError::QuotaExceeded {
            retry_after_secs: 1..=60
        })
    ));
    assert!(
        !service
            .submit_request(&req, "quota-0")
            .await
            .unwrap()
            .created
    );
    assert!(
        !service
            .submit_preferred_request(&req, "quota-1", DownloadPreferenceOverrides::default())
            .await
            .unwrap()
            .created
    );
    // Cancellation remains charged, and its replay is free even at full quota.
    service
        .cancel(ACTOR, admissions[0].operation_id)
        .await
        .unwrap();
    assert!(
        !service
            .submit_request(&req, "quota-0")
            .await
            .unwrap()
            .created
    );
    let counts = query("SELECT count FROM rate_limits WHERE user_id=?1 ORDER BY \"window\"")
        .bind(ACTOR as i64)
        .fetch_all(store.database())
        .await
        .unwrap();
    assert_eq!(counts.len(), 3);
    assert!(counts.iter().all(|r| r.try_get::<i64, _>(0).unwrap() == 5));
    query("UPDATE users SET role='restricted' WHERE id=?1")
        .bind(ACTOR as i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service.submit_request(&req, "denied").await,
        Err(DownloadError::Denied)
    );
    assert!(
        query("SELECT count FROM rate_limits WHERE user_id=?1")
            .bind(ACTOR as i64)
            .fetch_all(store.database())
            .await
            .unwrap()
            .iter()
            .all(|r| r.try_get::<i64, _>(0).unwrap() == 5)
    );

    let (_dir, store, service) = setup_with_owner(Some(ACTOR as i64)).await;
    for i in 0..6 {
        service
            .submit_request(&req, &format!("exempt-{i}"))
            .await
            .unwrap();
    }
    let n: i64 = query("SELECT count(*) FROM rate_limits WHERE user_id=?1")
        .bind(ACTOR as i64)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(n, 0);
}

#[path = "cases/deferred_media.rs"]
mod deferred_media_cases;
