// Included as a private oauth module test so fixture endpoints cannot be set by
// application callers. No external OAuth or provider calls are made.
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
    let mut oauth = DeviceOAuth::new(
        service,
        "fixture-client".into(),
        Some("write-fixture".into()),
        CancellationToken::new(),
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
    oauth.base = format!("http://{}", listener.local_addr().unwrap());
    oauth.user_url = format!("{}/user", oauth.base);
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
