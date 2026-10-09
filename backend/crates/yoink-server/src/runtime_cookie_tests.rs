use super::*;
use yoink_domain::DeliveryTarget;
use yoink_downloads::cookie_oauth::{
    OAuthCredential, OAuthProvider, ProviderDevice, ProviderFuture, ProviderPoll,
};
use yoink_downloads::{
    Delivery, DeliveryArtifact, DeliveryFuture, DownloadConfig, DownloadService,
};

struct NoDelivery;
impl Delivery for NoDelivery {
    fn bot_id(&self) -> i64 {
        123456
    }
    fn deliver(&self, _: DeliveryTarget, _: bool, _: Vec<DeliveryArtifact>) -> DeliveryFuture<'_> {
        panic!("OAuth lifecycle must not deliver media")
    }
}

#[derive(Default)]
struct Provider {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl OAuthProvider for Provider {
    fn start(&self) -> ProviderFuture<'_, ProviderDevice> {
        Box::pin(async {
            Ok(ProviderDevice {
                device_code: "fixture-device".into(),
                verification_url: "https://www.google.com/device".into(),
                user_code: "FIXTURE".into(),
                expires_in: 1800,
                interval: 5,
            })
        })
    }
    fn poll<'a>(&'a self, _: &'a str, _: i64) -> ProviderFuture<'a, ProviderPoll> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(ProviderPoll::Ready(OAuthCredential {
                access_token: "fixture-access".into(),
                refresh_token: "fixture-refresh".into(),
                expires_at: "2099-01-01T00:00:00Z".into(),
            }))
        })
    }
    fn refresh<'a>(
        &'a self,
        credential: &'a OAuthCredential,
        _: i64,
    ) -> ProviderFuture<'a, OAuthCredential> {
        Box::pin(async move { Ok(credential.clone()) })
    }
}

#[tokio::test]
async fn cookie_oauth_lifecycle_cleans_expiry_and_drains_cancelled_request_before_store_close() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = directory.path().join("lifecycle.db");
    let store = Arc::new(
        yoink_storage::Store::open(&path, true, Duration::from_secs(2))
            .await
            .unwrap(),
    );
    store.migrate().await.unwrap();
    yoink_db::raw_sql(include_str!(
        "../../yoink-downloads/tests/admission_users.sql"
    ))
    .execute(store.database())
    .await
    .unwrap();
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let executor = yoink_executor::Executor::new(yoink_executor::ExecutorConfig {
        program: "/bin/false".into(),
        args: vec![],
        workspace_root: workspace,
        timeout: Duration::from_secs(30),
        processing_timeout: Duration::from_secs(20),
        socket_timeout: Duration::from_secs(5),
        max_artifact_bytes: 1024,
        max_workspace_bytes: 2048,
        max_output_bytes: 1024,
        max_artifacts: 1,
        allowed_proxy_origins: vec![],
        allowed_private_origins: vec![],
    })
    .unwrap();
    let provider = Arc::new(Provider::default());
    let service = DownloadService::new(
        store.clone(),
        Arc::new(yoink_identity::IdentityStore::new(
            store.database().clone(),
            Some(42),
            false,
        )),
        executor,
        Arc::new(NoDelivery),
        DownloadConfig::default(),
    )
    .unwrap()
    .with_cookie_oauth_provider(provider.clone());
    let now = epoch_seconds();
    service.oauth_start(42, now - 2000).await.unwrap();
    let session = service.oauth_start(42, now).await.unwrap();
    let probe = session.session_id.clone();
    let cancel = CancellationToken::new();
    let mut tasks = JoinSet::new();
    tasks.spawn(run_cookie_oauth_lifecycle(
        service.clone(),
        cancel.clone(),
        Duration::from_millis(5),
    ));
    timeout(Duration::from_secs(5), async {
        loop {
            let row = yoink_db::query("SELECT COUNT(*) AS n FROM cookie_oauth_sessions")
                .fetch_one(store.database())
                .await
                .unwrap();
            if row.try_get::<i64, _>("n").unwrap() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let polling = service.clone();
    let request =
        tokio::spawn(async move { polling.oauth_poll(42, &session.session_id, now).await });
    timeout(Duration::from_secs(5), provider.entered.notified())
        .await
        .unwrap();
    request.abort();
    assert!(matches!(request.await, Err(error) if error.is_cancelled()));
    let database = store.clone();
    let shutdown = tokio::spawn(async move {
        let result = supervise(
            tasks,
            SystemService::new("fixture"),
            cancel,
            Duration::from_secs(5),
            async { Ok(()) },
            &yoink_http::LateMusicStats::default(),
            None,
        )
        .await;
        finish_database(result, Some(database), Duration::from_secs(5)).await
    });
    timeout(Duration::from_secs(5), async {
        loop {
            match service.oauth_poll(42, &probe, now).await {
                Err(yoink_downloads::DownloadError::Capacity) => break,
                Err(yoink_downloads::DownloadError::ChangedCredentials) => {
                    tokio::task::yield_now().await
                }
                _ => panic!("accepted poll must remain owned until shutdown closes admission"),
            }
        }
    })
    .await
    .unwrap();
    assert!(!shutdown.is_finished());
    provider.release.notify_one();
    timeout(Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let reopened = yoink_storage::Store::open(&path, false, Duration::from_secs(2))
        .await
        .unwrap();
    let row = yoink_db::query(
        "SELECT content,is_valid FROM cookies WHERE user_id=42 AND domain='youtube.com'",
    )
    .fetch_one(reopened.database())
    .await
    .unwrap();
    assert!(row.try_get::<bool, _>("is_valid").unwrap());
    let credential =
        OAuthCredential::decode_content(&row.try_get::<String, _>("content").unwrap()).unwrap();
    assert_eq!(credential.access_token, "fixture-access");
    reopened.close().await.unwrap();
}
