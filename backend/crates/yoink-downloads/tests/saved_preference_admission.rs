//! Saved-preference admission regressions through the public download service.
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
    let identities = Arc::new(IdentityStore::new(database, None, false));
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
    let (_directory, store, service) = setup().await;
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
async fn explicit_submit_still_rejects_a_forged_preference_marker() {
    let (_directory, _store, service) = setup().await;
    let forged = request().with_preference_overrides(DownloadPreferenceOverrides::default());
    assert_eq!(
        service.submit_request(&forged, "forged-marker").await,
        Err(DownloadError::InvalidInput)
    );
}
