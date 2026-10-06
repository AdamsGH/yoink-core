use serde_json::json;
use std::time::Duration;
use yoink_identity::{
    IdentityStore,
    admin::{AdminError, configuration::BotSettingsPatch},
};
use yoink_storage::Store;

#[tokio::test]
async fn typed_configuration_survives_reopen_and_preserves_imported_rows() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = directory.path().join("configuration.db");
    let store = Store::open(&path, true, Duration::from_secs(2))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    yoink_db::raw_sql(include_str!("sql/admin_policy.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let identities = IdentityStore::new(store.database().clone(), Some(3001), false);
    assert_eq!(
        identities.configuration_health_directory().as_deref(),
        Some(directory.path())
    );
    let patch = BotSettingsPatch::parse(
        &json!({"inline_storage_chat_id":"-100123", "bot_access_mode":"approved_only"}),
    )
    .unwrap();
    assert_eq!(
        identities
            .admin_patch_configuration(3002, &patch)
            .await
            .unwrap_err(),
        AdminError::Forbidden
    );
    identities
        .admin_patch_configuration(3001, &patch)
        .await
        .unwrap();
    let before = identities.admin_bot_settings(3001).await.unwrap();
    assert_eq!(before["unrelated"], "keep");
    drop(identities);
    store.close().await.unwrap();
    drop(store);
    let reopened = Store::open(&path, false, Duration::from_secs(2))
        .await
        .unwrap();
    let identities = IdentityStore::new(reopened.database().clone(), Some(3001), false);
    assert_eq!(
        identities.configuration_health_directory().as_deref(),
        Some(directory.path())
    );
    assert_eq!(identities.admin_bot_settings(3001).await.unwrap(), before);
    drop(identities);
    reopened.close().await.unwrap();
}
