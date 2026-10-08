// Fixture endpoints are installed through the trusted typed constructor.
// No external OAuth or provider calls are made.
use super::*;
use crate::{
    Config, FutureResult,
    content::{Citation, Content, ContentSource},
    provider::{Completion, Route},
};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use yoink_db::{Database, raw_sql};
struct Unused;
impl ContentSource for Unused {
    fn fetch<'a>(
        &'a self,
        _: &'a str,
        _: crate::content::FetchOptions<'a>,
        _: CancellationToken,
    ) -> FutureResult<'a, Content> {
        Box::pin(async { Err(invalid()) })
    }
    fn research<'a>(&'a self, _: &'a str, _: CancellationToken) -> FutureResult<'a, Vec<Citation>> {
        Box::pin(async { Err(invalid()) })
    }
}
impl Completion for Unused {
    fn complete<'a>(
        &'a self,
        _: &'a [Route],
        _: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async { Err(invalid()) })
    }
}
async fn fixture(response: Value) -> (tempfile::TempDir, DeviceOAuth, tokio::task::JoinHandle<()>) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let db = Database::open(&dir.path().join("oauth.db"), true, Duration::from_secs(3))
        .await
        .unwrap();
    raw_sql(include_str!(
        "../../yoink-storage/migrations-turso/0002_application.sql"
    ))
    .execute(&db)
    .await
    .unwrap();
    query("INSERT INTO users(id,role,created_at,updated_at) VALUES(1,'owner',0,0)")
        .execute(&db)
        .await
        .unwrap();
    let service = Insight::new(
        db,
        Config {
            owner_id: Some(1),
            ..Default::default()
        },
        Arc::new(Unused),
        Arc::new(Unused),
    )
    .unwrap();
    async fn start() -> Json<Value> {
        Json(
            json!({"device_code":"never-exposed-secret","user_code":"USER-CODE","interval":1,"expires_in":3}),
        )
    }
    async fn poll(State(response): State<Value>) -> Json<Value> {
        Json(response)
    }
    let app = Router::new()
        .route("/login/device/code", post(start))
        .route("/login/oauth/access_token", post(poll))
        .route(
            "/user",
            get(|| async { Json(json!({"login":"fixture-user"})) }),
        )
        .with_state(response);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let oauth = DeviceOAuth::with_options(
        service,
        "fixture-client".into(),
        Some("write-fixture".into()),
        CancellationToken::new(),
        OAuthOptions {
            device_base_url: base.clone(),
            user_url: format!("{base}/user"),
            ..Default::default()
        },
    )
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (dir, oauth, server)
}
async fn wait_terminal(oauth: &DeviceOAuth, write: bool) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let v = oauth.status(1, write).await.unwrap();
            if v["status"] != "pending" {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(20)).await
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn success_persists_redacted_status_and_shutdown_joins() {
    let (_dir, o, h) =
        fixture(json!({"access_token":"fixture-oauth-token","scope":"read:user"})).await;
    let started = o.start(1, false).await.unwrap();
    assert!(
        !serde_json::to_string(&started)
            .unwrap()
            .contains("never-exposed")
    );
    assert!(serde_json::to_value(&started).unwrap()["expires_at"].is_number());
    let terminal = wait_terminal(&o, false).await;
    assert_eq!(terminal["status"], "success");
    assert_eq!(terminal["username"], "fixture-user");
    let row = query("SELECT github_token FROM insight_user_settings WHERE user_id=1")
        .fetch_one(&o.service.db)
        .await
        .unwrap();
    assert_eq!(row.try_get::<String, _>(0).unwrap(), "fixture-oauth-token");
    o.shutdown().await;
    h.abort();
}
#[tokio::test]
async fn cancel_cannot_resurrect_deleted_token() {
    let (_dir, o, h) = fixture(json!({"access_token":"fixture-oauth-token"})).await;
    o.start(1, false).await.unwrap();
    o.cancel(1, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(o.status(1, false).await.unwrap()["status"], "none");
    assert!(
        query("SELECT github_token FROM insight_user_settings WHERE user_id=1")
            .fetch_optional(&o.service.db)
            .await
            .unwrap()
            .is_none()
    );
    o.shutdown().await;
    h.abort();
}
#[tokio::test]
async fn revoked_identity_before_token_write() {
    let (_dir, o, h) = fixture(json!({"access_token":"fixture-oauth-token"})).await;
    o.start(1, false).await.unwrap();
    query("UPDATE users SET role='banned' WHERE id=1")
        .execute(&o.service.db)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        o.flows.lock().await.get(&(1, false)).unwrap().status.status,
        "error"
    );
    assert!(
        query("SELECT github_token FROM insight_user_settings WHERE user_id=1")
            .fetch_optional(&o.service.db)
            .await
            .unwrap()
            .is_none()
    );
    o.shutdown().await;
    h.abort();
}
#[tokio::test]
async fn scope_mismatch_does_not_save_write_token() {
    let (_dir, o, h) =
        fixture(json!({"access_token":"fixture-oauth-token","scope":"read:user"})).await;
    o.start(1, true).await.unwrap();
    assert_eq!(wait_terminal(&o, true).await["status"], "error");
    assert!(
        query("SELECT github_token_public_repo FROM insight_user_settings WHERE user_id=1")
            .fetch_optional(&o.service.db)
            .await
            .unwrap()
            .is_none()
    );
    o.shutdown().await;
    h.abort();
}
#[tokio::test]
async fn pending_expires_and_slow_down_is_bounded() {
    let (_dir, o, h) = fixture(json!({"error":"authorization_pending"})).await;
    o.start(1, false).await.unwrap();
    assert_eq!(wait_terminal(&o, false).await["status"], "expired");
    o.shutdown().await;
    h.abort();
    let (_dir, o, h) = fixture(json!({"error":"slow_down"})).await;
    o.start(1, false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(o.status(1, false).await.unwrap()["interval"], 6);
    o.shutdown().await;
    h.abort();
}

#[tokio::test]
async fn username_failure_is_best_effort_and_write_flow_reports_login() {
    let (_dir, mut o, h) =
        fixture(json!({"access_token":"fixture-token","scope":"public_repo"})).await;
    o.start(1, true).await.unwrap();
    let terminal = wait_terminal(&o, true).await;
    assert_eq!(terminal["status"], "success");
    assert_eq!(terminal["username"], "fixture-user");
    // The local server's 404 is deterministic; never contact GitHub in tests.
    o.user_url = format!("{}/missing-user", o.base);
    o.start(1, false).await.unwrap();
    let terminal = wait_terminal(&o, false).await;
    assert_eq!(terminal["status"], "success");
    assert!(terminal["username"].is_null());
    o.shutdown().await;
    h.abort();
}

#[tokio::test]
async fn revocation_during_username_lookup_cannot_persist_token() {
    let (_dir, mut o, h) =
        fixture(json!({"access_token":"fixture-token","scope":"read:user"})).await;
    let service = o.service.clone();
    let app = Router::new().route(
        "/user",
        get(move || {
            let service = service.clone();
            async move {
                query("UPDATE users SET role='banned' WHERE id=1")
                    .execute(&service.db)
                    .await
                    .unwrap();
                Json(json!({"login":"fixture-user"}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    o.user_url = format!("http://{}/user", listener.local_addr().unwrap());
    let user_server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    o.start(1, false).await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if o.flows.lock().await.get(&(1, false)).unwrap().status.status != "pending" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        o.flows.lock().await.get(&(1, false)).unwrap().status.status,
        "error"
    );
    assert!(
        query("SELECT github_token FROM insight_user_settings WHERE user_id=1")
            .fetch_optional(&o.service.db)
            .await
            .unwrap()
            .is_none()
    );
    o.shutdown().await;
    h.abort();
    user_server.abort();
}

#[test]
fn trusted_oauth_options_reject_unbounded_or_credential_bearing_endpoints() {
    for endpoint in [
        "http://external.example",
        "https://user:password@example.com",
        "https://example.com?token=secret",
        "file:///tmp/oauth",
    ] {
        let options = OAuthOptions {
            device_base_url: endpoint.into(),
            ..Default::default()
        };
        assert!(options.validate().is_err());
    }
    assert!(
        OAuthOptions {
            request_timeout: Duration::ZERO,
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        OAuthOptions {
            device_base_url: "http://127.0.0.1:1234".into(),
            user_url: "http://127.0.0.1:1234/user".into(),
            ..Default::default()
        }
        .validate()
        .is_ok()
    );
}

#[tokio::test]
async fn ordinary_user_can_connect_write_token_without_write_operation_grant() {
    let (_directory, oauth, server) =
        fixture(json!({"access_token":"fixture-write-secret", "scope":"public_repo"})).await;
    query("UPDATE users SET role='user' WHERE id=1")
        .execute(&oauth.service.db)
        .await
        .unwrap();
    let mut options = oauth.service.config.clone();
    options.owner_id = None;
    let service = Insight::new(
        oauth.service.db.clone(),
        options,
        Arc::new(Unused),
        Arc::new(Unused),
    )
    .unwrap();
    let flow = DeviceOAuth::with_options(
        service,
        "fixture-read".into(),
        Some("fixture-write".into()),
        CancellationToken::new(),
        OAuthOptions {
            device_base_url: oauth.base.clone(),
            user_url: oauth.user_url.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    flow.start(1, true).await.unwrap();
    assert_eq!(wait_terminal(&flow, true).await["status"], "success");
    assert_eq!(
        flow.service.write_token_status(1).await.unwrap()["enabled"],
        true
    );
    assert!(
        flow.service
            .authorize(
                &*flow.service.db.connection().await.unwrap(),
                1,
                Some("inbox:gh_write"),
                false
            )
            .await
            .is_err()
    );
    flow.cancel(1, true).await.unwrap();
    assert_eq!(
        flow.service.write_token_status(1).await.unwrap()["enabled"],
        false
    );
    flow.shutdown().await;
    oauth.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn held_database_fences_shutdown_cancel_and_expiry_for_both_token_types() {
    for write in [false, true] {
        for action in ["shutdown", "cancel", "expiry"] {
            let (_directory, mut oauth, server) =
                fixture(json!({"access_token":"must-not-persist", "scope":"repo,read:user"})).await;
            let username_entered = Arc::new(tokio::sync::Notify::new());
            let username_release = Arc::new(tokio::sync::Notify::new());
            let entered = username_entered.clone();
            let release = username_release.clone();
            let app = Router::new().route(
                "/user",
                get(move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        entered.notify_one();
                        release.notified().await;
                        Json(json!({"login":"fixture-user"}))
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            oauth.user_url = format!("http://{}/user", listener.local_addr().unwrap());
            let user_server =
                tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let started = oauth.start(1, write).await.unwrap();
            tokio::time::timeout(Duration::from_secs(2), username_entered.notified())
                .await
                .unwrap();
            let flow_cancel = oauth
                .flows
                .lock()
                .await
                .get(&(1, write))
                .unwrap()
                .cancel
                .clone();
            let database = oauth.service.db.connection().await.unwrap();
            username_release.notify_one();
            // Test-only notification is emitted at the exact final DB-acquire
            // boundary. The held guard makes this an actual contended acquire,
            // not a timing guess or cancellation before the username request.
            tokio::time::timeout(Duration::from_secs(1), oauth.persistence_waiting.notified())
                .await
                .unwrap();
            match action {
                "shutdown" => {
                    // Joining must complete even with the DB still held.
                    tokio::time::timeout(Duration::from_secs(1), oauth.shutdown())
                        .await
                        .unwrap();
                    assert_eq!(
                        oauth
                            .flows
                            .lock()
                            .await
                            .get(&(1, write))
                            .unwrap()
                            .status
                            .status,
                        "cancelled"
                    );
                    drop(database);
                }
                "cancel" => {
                    let copy = oauth.clone();
                    let disconnect = tokio::spawn(async move { copy.cancel(1, write).await });
                    tokio::time::timeout(Duration::from_secs(1), flow_cancel.cancelled())
                        .await
                        .unwrap();
                    assert!(!oauth.flows.lock().await.contains_key(&(1, write)));
                    drop(database);
                    disconnect.await.unwrap().unwrap();
                    oauth.shutdown().await;
                }
                "expiry" => {
                    let until_expiry = (started.expires_at - Utc::now())
                        .to_std()
                        .unwrap_or_default();
                    tokio::time::sleep(until_expiry + Duration::from_millis(10)).await;
                    oauth.tracker.close();
                    tokio::time::timeout(Duration::from_secs(1), oauth.tracker.wait())
                        .await
                        .unwrap();
                    assert_eq!(
                        oauth
                            .flows
                            .lock()
                            .await
                            .get(&(1, write))
                            .unwrap()
                            .status
                            .status,
                        "expired"
                    );
                    drop(database);
                    oauth.shutdown().await;
                }
                _ => unreachable!(),
            }
            let row = query("SELECT github_token,github_token_public_repo FROM insight_user_settings WHERE user_id=1")
                .fetch_optional(&oauth.service.db).await.unwrap();
            if let Some(row) = row {
                assert!(row.try_get::<Option<String>, _>(0).unwrap().is_none());
                assert!(row.try_get::<Option<String>, _>(1).unwrap().is_none());
            }
            assert!(
                oauth
                    .flows
                    .lock()
                    .await
                    .values()
                    .all(|flow| flow.status.status != "success")
            );
            server.abort();
            user_server.abort();
        }
    }
}
