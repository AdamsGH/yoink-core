use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Semaphore,
};
use yoink_http::activity::{Activity, ActivityCall, ActivityProvider};

struct DropMark(Arc<AtomicBool>);
impl Drop for DropMark {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
struct PendingActivity {
    reached: Arc<Semaphore>,
    dropped: Arc<AtomicBool>,
}
impl ActivityProvider for PendingActivity {
    fn activity(&self, subject: i64) -> ActivityCall<'_> {
        Box::pin(async move {
            assert_eq!(subject, 42);
            let _guard = DropMark(self.dropped.clone());
            self.reached.add_permits(1);
            std::future::pending().await
        })
    }
}

#[tokio::test]
async fn populated_http_holder_shutdown_cancels_provider_before_database_close() {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Arc::new(
        yoink_storage::Store::open(&dir.path().join("http.db"), true, Duration::from_secs(2))
            .await
            .unwrap(),
    );
    store.migrate().await.unwrap();
    let identities = Arc::new(yoink_identity::IdentityStore::new(
        store.database().clone(),
        Some(42),
        false,
    ));
    identities
        .register(&yoink_identity::TrustedProfile {
            user_id: 42,
            username: None,
            first_name: None,
            photo_url: None,
        })
        .await
        .unwrap();
    let crypto = yoink_auth::AuthConfig::new(
        "fixture".into(),
        "fixture-http-lifecycle-secret-32-bytes".into(),
        86400,
        30,
        86400,
    )
    .unwrap();
    let token = crypto
        .issue_token(
            yoink_auth::JwtProfile {
                user_id: 42,
                role: "owner",
                username: None,
                first_name: None,
            },
            epoch_seconds(),
        )
        .unwrap();
    let auth = yoink_http::AuthState::new(false);
    assert!(
        auth.install(yoink_http::AuthService { identities, crypto })
            .is_ok()
    );
    let reached = Arc::new(Semaphore::new(0));
    let dropped = Arc::new(AtomicBool::new(false));
    let activity = Activity::default();
    activity
        .install(
            "pending",
            Arc::new(PendingActivity {
                reached: reached.clone(),
                dropped: dropped.clone(),
            }),
        )
        .unwrap();
    let system = SystemService::new("fixture");
    system.set_ready(true);
    let products = yoink_http::LateMusicStats::default();
    products
        .install(system.clone(), auth.clone(), None, None, activity)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let cancel = CancellationToken::new();
    let server_cancel = cancel.clone();
    let server_system = system.clone();
    let server_products = products.clone();
    let mut tasks = JoinSet::new();
    tasks.spawn(async move {
        yoink_http::serve_with_late_music_stats(
            listener,
            server_system,
            Some(auth.clone()),
            auth,
            None,
            yoink_http::LateInbox::default(),
            yoink_http::LateInsight::default(),
            server_products,
            server_cancel,
        )
        .await
        .map_err(|_| ServerError::Http)
    });
    let request = tokio::spawn(async move {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(format!(
            "GET /api/v1/users/me/stats HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
        ).as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        response
    });
    timeout(Duration::from_secs(2), reached.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert!(!dropped.load(Ordering::SeqCst));
    let result = timeout(
        Duration::from_secs(2),
        supervise(
            tasks,
            system.clone(),
            cancel.clone(),
            Duration::from_millis(30),
            async { Ok(()) },
            &products,
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(result, Ok(()));
    assert!(dropped.load(Ordering::SeqCst));
    assert!(!system.snapshot().ready);
    assert!(cancel.is_cancelled());
    timeout(Duration::from_secs(1), products.drain())
        .await
        .unwrap();
    let response = timeout(Duration::from_secs(2), request)
        .await
        .unwrap()
        .unwrap();
    assert!(!response.starts_with(b"HTTP/1.1 200"));
    assert_eq!(
        finish_database(result, Some(store), Duration::from_secs(2)).await,
        Ok(())
    );
}
