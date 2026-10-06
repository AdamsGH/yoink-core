use std::{path::PathBuf, sync::Arc, time::Duration};

use yoink_domain::DeliveryTarget;
use yoink_downloads::{
    Delivery, DeliveryArtifact, DeliveryFuture, DownloadConfig, DownloadError,
    DownloadNetworkConfig, DownloadService, SettingChange, SettingField,
};
use yoink_executor::{Executor, ExecutorConfig};
use yoink_identity::IdentityStore;
use yoink_storage::Store;

const OWNER: u64 = 42;
const BOT: i64 = 123456;

struct NoDelivery;
impl Delivery for NoDelivery {
    fn bot_id(&self) -> i64 {
        BOT
    }
    fn deliver(&self, _: DeliveryTarget, _: bool, _: Vec<DeliveryArtifact>) -> DeliveryFuture<'_> {
        panic!("proxy settings must not deliver")
    }
}

fn target(chat: i64, thread: Option<i64>, bot: i64) -> DeliveryTarget {
    DeliveryTarget::new(bot, chat, thread, None).unwrap()
}

async fn setup(system_proxy: Option<&str>) -> (tempfile::TempDir, Arc<Store>, DownloadService) {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let store = Arc::new(
        Store::open(
            &directory.path().join("proxy.db"),
            true,
            Duration::from_secs(2),
        )
        .await
        .unwrap(),
    );
    store.migrate().await.unwrap();
    yoink_db::raw_sql(include_str!("admission_users.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let identities = Arc::new(IdentityStore::new(
        store.database().clone(),
        Some(OWNER as i64),
        false,
    ));
    let executor = Executor::new(ExecutorConfig {
        program: PathBuf::from("/bin/false"),
        args: vec![],
        workspace_root: workspace,
        timeout: Duration::from_secs(30),
        processing_timeout: Duration::from_secs(20),
        socket_timeout: Duration::from_secs(5),
        max_artifact_bytes: 1024 * 1024,
        max_workspace_bytes: 2 * 1024 * 1024,
        max_output_bytes: 1024 * 1024,
        max_artifacts: 1,
        allowed_proxy_origins: vec![],
        allowed_private_origins: vec![],
    })
    .unwrap();
    let network = DownloadNetworkConfig::new(system_proxy.map(str::to_owned), vec![]).unwrap();
    let service = DownloadService::new_with_network_config(
        store.clone(),
        identities,
        executor,
        Arc::new(NoDelivery),
        DownloadConfig::default(),
        network,
    )
    .unwrap();
    (directory, store, service)
}

async fn seed(store: &Store, proxy: Option<&str>, enabled: bool, revision: i64) {
    yoink_db::query(include_str!("sql/proxy_seed.sql"))
        .bind(OWNER as i64)
        .bind(enabled)
        .bind(proxy)
        .bind(revision)
        .execute(store.database())
        .await
        .unwrap();
}

#[tokio::test]
async fn admission_no_row_defaults_and_enable_creates_settings_only_with_proxy() {
    let (_dir, store, service) = setup(None).await;
    let own = target(OWNER as i64, None, BOT);
    let initial = service.proxy_settings(OWNER, own).await.unwrap();
    assert!(!initial.configured && !initial.enabled);
    assert_eq!(initial.revision, 0);
    assert_eq!(
        service.change_proxy_setting(OWNER, own, true, 0).await,
        Err(DownloadError::ProxyUnavailable)
    );
    let missing: Option<i64> = yoink_db::query(include_str!("sql/proxy_exists.sql"))
        .bind(OWNER as i64)
        .fetch_optional(store.database())
        .await
        .unwrap()
        .map(|r| r.try_get(0).unwrap());
    assert_eq!(missing, None);
    assert_eq!(
        service.proxy_settings(OWNER, target(43, None, BOT)).await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        service
            .proxy_settings(OWNER, target(OWNER as i64, Some(2), BOT))
            .await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        service
            .proxy_settings(OWNER, target(OWNER as i64, None, BOT + 1))
            .await,
        Err(DownloadError::Denied)
    );
    assert!(
        service
            .proxy_settings(43, target(43, None, BOT))
            .await
            .is_ok()
    );

    for (actor, destination) in [
        (OWNER, target(43, None, BOT)),
        (OWNER, target(-10042, None, BOT)),
        (OWNER, target(OWNER as i64, Some(2), BOT)),
        (OWNER, target(OWNER as i64, None, BOT + 1)),
    ] {
        assert_eq!(
            service.proxy_settings(actor, destination).await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service
                .change_proxy_setting(actor, destination, false, 0)
                .await,
            Err(DownloadError::Denied)
        );
    }
    let (_dir, store, service) = setup(Some("http://system.example:8080")).await;
    assert_eq!(
        service.change_proxy_setting(OWNER, own, true, 0).await,
        Ok(())
    );
    let settings = service.proxy_settings(OWNER, own).await.unwrap();
    assert!(settings.enabled && settings.configured);
    let values = yoink_db::query(include_str!("sql/proxy_values.sql"))
        .bind(OWNER as i64)
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(values.try_get::<String, _>("quality").unwrap(), "best");
}

#[tokio::test]
async fn language_settings_are_actor_private_revisioned_and_freshly_authorized() {
    let (_dir, store, service) = setup(None).await;
    let own = target(OWNER as i64, None, BOT);
    assert_eq!(
        service.preferred_language(OWNER).await.unwrap().as_deref(),
        Some("en")
    );
    assert_eq!(
        service.preferred_language(43).await.unwrap().as_deref(),
        Some("en")
    );
    assert_eq!(service.preferred_language(999).await.unwrap(), None);
    assert_eq!(
        service.preferred_language(0).await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        service.preferred_language(u64::MAX).await,
        Err(DownloadError::Denied)
    );
    let initial = service.language_settings(OWNER, own).await.unwrap();
    let selected = service
        .change_language(OWNER, own, "ru", initial.revision)
        .await
        .unwrap();
    assert_eq!(selected.code, "ru");
    assert!(selected.revision > initial.revision);
    assert_eq!(
        service
            .change_language(OWNER, own, "en", initial.revision)
            .await,
        Err(DownloadError::Conflict)
    );
    assert_eq!(
        service
            .change_language(OWNER, own, "de", selected.revision)
            .await,
        Err(DownloadError::InvalidPreferences)
    );
    let ordinary_target = target(43, None, BOT);
    let ordinary = service
        .language_settings(43, ordinary_target)
        .await
        .unwrap();
    let changed = service
        .change_language(43, ordinary_target, "ru", ordinary.revision)
        .await
        .unwrap();
    assert_eq!(changed.code, "ru");
    assert!(changed.revision > ordinary.revision);
    assert_eq!(
        service
            .change_language(43, ordinary_target, "en", ordinary.revision)
            .await,
        Err(DownloadError::Conflict)
    );
    for (actor, destination) in [
        (43, target(OWNER as i64, None, BOT)),
        (OWNER, target(43, None, BOT)),
        (OWNER, target(OWNER as i64, None, BOT + 1)),
        (OWNER, target(OWNER as i64, Some(7), BOT)),
    ] {
        assert_eq!(
            service.language_settings(actor, destination).await,
            Err(DownloadError::Denied)
        );
        assert_eq!(
            service
                .change_language(actor, destination, "en", selected.revision)
                .await,
            Err(DownloadError::Denied)
        );
    }
    yoink_db::query(include_str!("sql/language_ban.sql"))
        .bind(OWNER as i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service.language_settings(OWNER, own).await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        service
            .change_language(OWNER, own, "en", selected.revision)
            .await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        service.preferred_language(OWNER).await.unwrap().as_deref(),
        Some("ru")
    );
}

#[tokio::test]
async fn personal_proxy_precedence_validation_disable_and_secret_redaction() {
    let (_dir, store, service) = setup(Some("http://system.example:8080")).await;
    let own = target(OWNER as i64, None, BOT);
    seed(&store, Some("not a proxy"), false, 10).await;
    assert!(!service.proxy_settings(OWNER, own).await.unwrap().configured);
    assert_eq!(
        service.change_proxy_setting(OWNER, own, true, 10).await,
        Err(DownloadError::ProxyUnavailable)
    );
    assert_eq!(
        service.change_proxy_setting(OWNER, own, false, 10).await,
        Ok(())
    );
    seed(
        &store,
        Some("http://private:password@example.com:8080"),
        false,
        20,
    )
    .await;
    let debug = format!("{:?}", service.proxy_settings(OWNER, own).await.unwrap());
    assert!(!debug.contains("password"));
    assert!(service.proxy_settings(OWNER, own).await.unwrap().configured);
    seed(&store, Some(""), false, 30).await;
    assert!(service.proxy_settings(OWNER, own).await.unwrap().configured);
    // The imported schema bounds proxy text before the service reads it.
    assert!(
        yoink_db::query(include_str!("sql/proxy_seed.sql"))
            .bind(OWNER as i64)
            .bind(false)
            .bind("x".repeat(yoink_worker_protocol::MAX_PROXY_URL_BYTES + 1))
            .bind(35_i64)
            .execute(store.database())
            .await
            .is_err()
    );
    assert_eq!(
        service.proxy_settings(OWNER, own).await.unwrap().revision,
        30
    );
    seed(&store, Some("not a proxy"), true, 40).await;
    yoink_db::query(include_str!("sql/proxy_corrupt_unrelated.sql"))
        .bind(OWNER as i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service.preferred_language(OWNER).await.unwrap().as_deref(),
        Some("en")
    );
    let language = service.language_settings(OWNER, own).await.unwrap();
    assert_eq!(language.code, "en");
    let changed_language = service
        .change_language(OWNER, own, "ru", language.revision)
        .await
        .unwrap();
    assert_eq!(changed_language.code, "ru");
    assert_eq!(
        service.change_proxy_setting(OWNER, own, false, 40).await,
        Ok(())
    );
}

#[tokio::test]
async fn ban_and_shared_revision_guard_proxy_changes() {
    let (_dir, store, service) = setup(Some("http://system.example:8080")).await;
    let own = target(OWNER as i64, None, BOT);
    let view = service.proxy_settings(OWNER, own).await.unwrap();
    assert_eq!(view.revision, 0);
    assert_eq!(
        service
            .change_proxy_setting(OWNER, own, true, view.revision)
            .await,
        Ok(())
    );
    let updated = service.proxy_settings(OWNER, own).await.unwrap();
    assert!(updated.revision > view.revision);
    assert_eq!(
        service
            .change_proxy_setting(OWNER, own, false, view.revision)
            .await,
        Err(DownloadError::Conflict)
    );
    let settings = service.settings(OWNER, own).await.unwrap();
    assert_eq!(settings.revision, updated.revision);
    service
        .change_setting(
            OWNER,
            own,
            SettingChange::new(SettingField::Quality, "720").unwrap(),
            Some(settings.revision),
        )
        .await
        .unwrap();
    assert!(service.proxy_settings(OWNER, own).await.unwrap().revision > updated.revision);
    assert_eq!(
        service
            .change_proxy_setting(OWNER, own, false, updated.revision)
            .await,
        Err(DownloadError::Conflict)
    );
    yoink_db::query(include_str!("sql/proxy_ban.sql"))
        .bind(OWNER as i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service.proxy_settings(OWNER, own).await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        service
            .change_proxy_setting(OWNER, own, false, updated.revision)
            .await,
        Err(DownloadError::Denied)
    );
}

#[tokio::test]
async fn proxy_toggle_preserves_other_settings_credentials_and_download_history() {
    let (_dir, store, service) = setup(Some("http://system.example:8080")).await;
    let own = target(OWNER as i64, None, BOT);
    seed(&store, Some("http://personal.example:8080"), false, 100).await;
    let before = yoink_db::query(include_str!("sql/proxy_state.sql"))
        .bind(OWNER as i64)
        .fetch_one(store.database())
        .await
        .unwrap();
    service
        .change_proxy_setting(OWNER, own, true, 100)
        .await
        .unwrap();
    let after = yoink_db::query(include_str!("sql/proxy_state.sql"))
        .bind(OWNER as i64)
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(
        before.try_get::<String, _>("quality").unwrap(),
        after.try_get::<String, _>("quality").unwrap()
    );
    assert_eq!(
        before.try_get::<String, _>("proxy_url").unwrap(),
        after.try_get::<String, _>("proxy_url").unwrap()
    );
    assert_eq!(
        before.try_get::<i64, _>("created_at").unwrap(),
        after.try_get::<i64, _>("created_at").unwrap()
    );
    assert_eq!(
        before.try_get::<i64, _>("cookie_count").unwrap(),
        after.try_get::<i64, _>("cookie_count").unwrap()
    );
    assert_eq!(
        before.try_get::<i64, _>("download_count").unwrap(),
        after.try_get::<i64, _>("download_count").unwrap()
    );
    assert!(
        after.try_get::<i64, _>("updated_at").unwrap()
            > before.try_get::<i64, _>("updated_at").unwrap()
    );
    assert!(after.try_get::<bool, _>("proxy_enabled").unwrap());
}

#[tokio::test]
async fn ordinary_user_own_proxy_enable_disable_cas_preserves_credentials_and_isolation() {
    async fn invariant(store: &Store, actor: u64) -> (String, i64, i64, i64) {
        let row = yoink_db::query(include_str!("sql/proxy_state.sql"))
            .bind(actor as i64)
            .fetch_one(store.database())
            .await
            .unwrap();
        (
            row.try_get("proxy_url").unwrap(),
            row.try_get("created_at").unwrap(),
            row.try_get("cookie_count").unwrap(),
            row.try_get("download_count").unwrap(),
        )
    }
    let (_dir, store, service) = setup(Some("http://system.example:8080")).await;
    let actor = 43;
    let own = target(actor as i64, None, BOT);
    let foreign = target(OWNER as i64, None, BOT);
    seed(&store, Some("http://foreign.example:8080"), true, 100).await;
    yoink_db::query(include_str!("sql/proxy_seed.sql"))
        .bind(actor as i64)
        .bind(false)
        .bind("http://ordinary:private@example.com:8080")
        .bind(200_i64)
        .execute(store.database())
        .await
        .unwrap();
    // Seed nondefault unrelated preferences through the same approved-user service.
    for (field, value) in [
        (SettingField::Codec, "vp09"),
        (SettingField::GalleryZip, "true"),
    ] {
        let revision = service.settings(actor, own).await.unwrap().revision;
        service
            .change_setting(
                actor,
                own,
                SettingChange::new(field, value).unwrap(),
                Some(revision),
            )
            .await
            .unwrap();
    }
    let preferences = service.settings(actor, own).await.unwrap();
    let credentials = invariant(&store, actor).await;
    let foreign_preferences = service.settings(OWNER, foreign).await.unwrap();
    let foreign_proxy = service.proxy_settings(OWNER, foreign).await.unwrap();
    let foreign_credentials = invariant(&store, OWNER).await;
    let before = service.proxy_settings(actor, own).await.unwrap();
    assert!(before.configured && !before.enabled);
    service
        .change_proxy_setting(actor, own, true, before.revision)
        .await
        .unwrap();
    let enabled = service.proxy_settings(actor, own).await.unwrap();
    assert!(enabled.configured && enabled.enabled);
    assert!(enabled.revision > before.revision);
    assert_eq!(
        service.settings(actor, own).await.unwrap().values,
        preferences.values
    );
    assert!(
        invariant(&store, actor).await == credentials,
        "own credentials/history unchanged by enable"
    );
    assert_eq!(
        service
            .change_proxy_setting(actor, own, false, before.revision)
            .await,
        Err(DownloadError::Conflict)
    );
    assert_eq!(service.proxy_settings(actor, own).await.unwrap(), enabled);
    // Another preference write shares the revision and invalidates an old proxy button.
    service
        .change_setting(
            actor,
            own,
            SettingChange::new(SettingField::Quality, "720").unwrap(),
            Some(enabled.revision),
        )
        .await
        .unwrap();
    let changed_preferences = service.settings(actor, own).await.unwrap();
    assert!(changed_preferences.revision > enabled.revision);
    for (field, value) in &preferences.values {
        let expected = if *field == SettingField::Quality {
            "720"
        } else {
            value.as_str()
        };
        assert_eq!(
            changed_preferences
                .values
                .iter()
                .find(|(candidate, _)| candidate == field)
                .unwrap()
                .1,
            expected
        );
    }
    assert_eq!(
        service
            .change_proxy_setting(actor, own, false, enabled.revision)
            .await,
        Err(DownloadError::Conflict)
    );
    let after_preference = service.proxy_settings(actor, own).await.unwrap();
    assert!(after_preference.enabled);
    assert_eq!(after_preference.revision, changed_preferences.revision);
    service
        .change_proxy_setting(actor, own, false, after_preference.revision)
        .await
        .unwrap();
    let disabled = service.proxy_settings(actor, own).await.unwrap();
    assert!(disabled.configured && !disabled.enabled);
    assert!(disabled.revision > after_preference.revision);
    assert_eq!(
        service
            .change_proxy_setting(actor, own, true, after_preference.revision)
            .await,
        Err(DownloadError::Conflict)
    );
    assert_eq!(service.proxy_settings(actor, own).await.unwrap(), disabled);
    assert_eq!(
        service.settings(actor, own).await.unwrap().values,
        changed_preferences.values
    );
    assert!(
        invariant(&store, actor).await == credentials,
        "own URL/history unchanged by toggles/preferences"
    );
    assert_eq!(
        service.settings(OWNER, foreign).await.unwrap(),
        foreign_preferences
    );
    assert_eq!(
        service.proxy_settings(OWNER, foreign).await.unwrap(),
        foreign_proxy
    );
    assert!(
        invariant(&store, OWNER).await == foreign_credentials,
        "foreign actor state unchanged"
    );
}
