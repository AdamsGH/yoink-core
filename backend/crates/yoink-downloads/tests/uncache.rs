use std::{path::PathBuf, sync::Arc, time::Duration};

use yoink_domain::DeliveryTarget;
use yoink_downloads::{
    Delivery, DeliveryArtifact, DeliveryFuture, DownloadConfig, DownloadError, DownloadService,
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
        panic!("cache-clear authorization must not deliver")
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
    let store = Arc::new(
        Store::open(
            &directory.path().join("uncache.db"),
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
    yoink_db::raw_sql(include_str!("saved_preference_settings.sql"))
        .execute(store.database())
        .await
        .unwrap();
    yoink_db::query(include_str!("sql/corrupt_preferences.sql"))
        .bind(OWNER as i64)
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
    let service = DownloadService::new(
        store.clone(),
        identities,
        executor,
        Arc::new(NoDelivery),
        DownloadConfig::default(),
    )
    .unwrap();
    (directory, store, service)
}

#[tokio::test]
async fn cache_clear_requires_configured_owner_private_current_bot_and_valid_url() {
    let (_directory, store, service) = setup().await;
    let target = DeliveryTarget::new(BOT, OWNER as i64, None, None).unwrap();
    assert_eq!(
        service
            .clear_media_cache(OWNER, target, 200, "HTTPS://EXAMPLE.com:443/no-cache")
            .await,
        Ok(Some(0))
    );
    for source in ["https://example.com/\na", "https://example.com/a b", ""] {
        assert_eq!(
            service.clear_media_cache(OWNER, target, 202, source).await,
            Err(DownloadError::InvalidInput)
        );
    }
    assert_eq!(
        service
            .clear_media_cache(
                OWNER,
                target,
                202,
                &format!("https://example.com/{}", "a".repeat(4096))
            )
            .await,
        Err(DownloadError::InvalidInput)
    );
    assert_eq!(
        service
            .clear_media_cache(OWNER, target, 201, "https://user:secret@example.com/a")
            .await,
        Err(DownloadError::InvalidInput)
    );
    assert_eq!(
        service
            .clear_media_cache(OWNER, target, 202, "file:///private")
            .await,
        Err(DownloadError::InvalidInput)
    );
    assert_eq!(
        service
            .clear_media_cache(
                OWNER,
                target,
                201,
                "https://example.com/valid-after-invalid"
            )
            .await,
        Ok(Some(0))
    );
    assert_eq!(
        service
            .clear_media_cache(
                43,
                DeliveryTarget::new(BOT, 43, None, None).unwrap(),
                203,
                "https://example.com/a"
            )
            .await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        service
            .clear_media_cache(
                OWNER,
                DeliveryTarget::new(BOT, -100, None, None).unwrap(),
                204,
                "https://example.com/a"
            )
            .await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        service
            .clear_media_cache(
                OWNER,
                DeliveryTarget::new(BOT, OWNER as i64, Some(7), None).unwrap(),
                205,
                "https://example.com/a"
            )
            .await,
        Err(DownloadError::Denied)
    );
    assert_eq!(
        service
            .clear_media_cache(
                OWNER,
                DeliveryTarget::new(BOT + 1, OWNER as i64, None, None).unwrap(),
                206,
                "https://example.com/a"
            )
            .await,
        Err(DownloadError::Denied)
    );
    yoink_db::query(include_str!("sql/set_banned.sql"))
        .bind(OWNER as i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        service
            .clear_media_cache(OWNER, target, 207, "https://example.com/a")
            .await,
        Err(DownloadError::Denied)
    );
    store.close().await.unwrap();
}
