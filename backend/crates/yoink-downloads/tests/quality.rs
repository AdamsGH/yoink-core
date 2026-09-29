use std::{path::PathBuf, sync::Arc, time::Duration};

use yoink_domain::{
    DeliveryTarget, DownloadParams, DownloadPreferenceOverrides, DownloadRequest, MediaMode,
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
    let identities = Arc::new(IdentityStore::new(db, Some(ACTOR as i64), false));
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
        DownloadConfig::default(),
    )
    .unwrap();
    (dir, store, service)
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
