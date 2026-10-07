use std::{path::PathBuf, sync::Arc, time::Duration};

use yoink_domain::DeliveryTarget;
use yoink_downloads::{
    Delivery, DeliveryArtifact, DeliveryFuture, DownloadConfig, DownloadNetworkConfig,
    DownloadService,
};
use yoink_executor::{Executor, ExecutorConfig};
use yoink_identity::IdentityStore;
use yoink_storage::Store;

const OWNER: u64 = 42;
const FOREIGN: u64 = 43;
const BOT: i64 = 123456;

struct NoDelivery;
impl Delivery for NoDelivery {
    fn bot_id(&self) -> i64 {
        BOT
    }
    fn deliver(&self, _: DeliveryTarget, _: bool, _: Vec<DeliveryArtifact>) -> DeliveryFuture<'_> {
        panic!("personal cookie service must not deliver")
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
            &directory.path().join("cookies.db"),
            true,
            Duration::from_secs(2),
        )
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
    let service = DownloadService::new_with_network_config(
        store.clone(),
        identities,
        executor,
        Arc::new(NoDelivery),
        DownloadConfig::default(),
        DownloadNetworkConfig::new(None, vec![]).unwrap(),
    )
    .unwrap();
    (directory, store, service)
}
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use yoink_auth::{AuthConfig, JwtProfile};
use yoink_http::{AuthService, AuthState, router_with_auth};
async fn fixture() -> (tempfile::TempDir, Arc<Store>, Router, String, String) {
    let (dir, store, svc) = setup().await;
    svc.install_cookie_web_schema().await.unwrap();
    let svc = svc.with_cookie_oauth_provider(Arc::new(HttpOAuth));
    let crypto = AuthConfig::new(
        "fixture-bot".into(),
        "fixture-cookie-jwt-secret-at-least-32".into(),
        86400,
        30,
        86400,
    )
    .unwrap();
    let jwt = |id| {
        crypto
            .issue_token(
                JwtProfile {
                    user_id: id,
                    role: "owner",
                    username: None,
                    first_name: None,
                },
                chrono::Utc::now().timestamp(),
            )
            .unwrap()
    };
    let owner = jwt(OWNER as i64);
    let foreign = jwt(FOREIGN as i64);
    let state = AuthState::new(false);
    assert!(state.install_cookies(svc).is_ok());
    assert!(
        state
            .install(AuthService {
                identities: Arc::new(IdentityStore::new(
                    store.database().clone(),
                    Some(OWNER as i64),
                    false
                )),
                crypto
            })
            .is_ok()
    );
    let system = yoink_application::SystemService::new("test");
    system.set_ready(true);
    (
        dir,
        store,
        router_with_auth(system, Some(state)),
        owner,
        foreign,
    )
}
async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 3 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[tokio::test]
async fn cookie_web_http_frontend_token_extension_upload_ownership_and_admin() {
    let (_dir, store, app, owner, foreign) = fixture().await;
    assert_eq!(
        request(&app, "GET", "/api/v1/dl/cookies", None, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, token) = request(
        &app,
        "POST",
        "/api/v1/dl/cookies/token",
        Some(&owner),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(token["expires_in"], 600);
    assert_eq!(token["submit_url"], "/api/v1/dl/cookies/submit");
    let body = json!({"token":token["token"],"cookies":{"example.com":[{"domain":"example.com","path":"/","name":"sid","value":"private-secret","httpOnly":true}]}});
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/dl/cookies/submit",
            None,
            body.clone()
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&app, "POST", "/api/v1/dl/cookies/submit", None, body)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, rows) = request(&app, "GET", "/api/v1/dl/cookies", Some(&owner), Value::Null).await;
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert!(rows[0].get("content").is_none());
    assert!(!rows.to_string().contains("private-secret"));
    let id = rows[0]["id"].as_i64().unwrap();
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/api/v1/dl/cookies/by-id/{id}"),
            Some(&foreign),
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // JWT owner role cannot grant current DB administrator privileges.
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/dl/cookies/all",
            Some(&foreign),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let input = json!({"domain":"example.com","content":"# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tFALSE\t0\tsid\tsecret\n"});
    let (status, row) = request(
        &app,
        "POST",
        "/api/v1/dl/cookies/upload",
        Some(&owner),
        input.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(row["id"], id);
    let (status, pool) =
        request(&app, "POST", "/api/v1/dl/cookies/pool", Some(&owner), input).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, filtered) = request(
        &app,
        "GET",
        "/api/v1/dl/cookies/pool?domain=other.com",
        Some(&owner),
        Value::Null,
    )
    .await;
    assert!(filtered.as_array().unwrap().is_empty());
    let (_, filtered) = request(
        &app,
        "GET",
        "/api/v1/dl/cookies/pool?domain=example.com",
        Some(&owner),
        Value::Null,
    )
    .await;
    assert_eq!(filtered.as_array().unwrap().len(), 1);
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/api/v1/dl/cookies/pool/{}", pool["id"]),
            Some(&owner),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    // Fresh ban overrides valid, previously issued JWT.
    yoink_db::query("UPDATE users SET ban_until=?1 WHERE id=?2")
        .bind(chrono::Utc::now() + chrono::Duration::hours(1))
        .bind(OWNER as i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/api/v1/dl/cookies/by-id/{id}"),
            Some(&owner),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}
#[tokio::test]
async fn cookie_web_http_large_netscape_upload_and_safe_validation_errors() {
    let (_dir, _store, app, owner, _foreign) = fixture().await;
    let content = format!(
        "# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tFALSE\t0\tsid\t{}\n",
        "x".repeat(30000)
    );
    let (status, row) = request(
        &app,
        "POST",
        "/api/v1/dl/cookies/upload",
        Some(&owner),
        json!({"user_id":999,"domain":"example.com","content":content}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(row["user_id"], OWNER);
    let (status, error) = request(
        &app,
        "POST",
        "/api/v1/dl/cookies/upload",
        Some(&owner),
        json!({"domain":"example.com","content":"private-invalid-secret"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!error.to_string().contains("private-invalid-secret"));
}
#[tokio::test]
async fn cookie_web_http_extension_submit_without_jwt_auth_enabled() {
    let (_dir, _store, svc) = setup().await;
    svc.install_cookie_web_schema().await.unwrap();
    let svc = svc.with_cookie_oauth_provider(Arc::new(HttpOAuth));
    let token = svc
        .issue_cookie_token(OWNER as i64, chrono::Utc::now().timestamp())
        .await
        .unwrap();
    let state = AuthState::new(false);
    assert!(state.install_cookies(svc).is_ok());
    let system = yoink_application::SystemService::new("test");
    system.set_ready(true);
    let app = yoink_http::router_with_identity(system, None, state);
    let body = json!({"token":token.token,"cookies":{"example.com":[{"domain":"example.com","path":"/","name":"sid","value":"private-secret"}]}});
    assert_eq!(
        request(&app, "POST", "/api/v1/dl/cookies/submit", None, body)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
}
use yoink_downloads::cookie_oauth::{
    OAuthCredential, OAuthProvider, ProviderDevice, ProviderFuture, ProviderPoll,
};
struct HttpOAuth;
impl OAuthProvider for HttpOAuth {
    fn start(&self) -> ProviderFuture<'_, ProviderDevice> {
        Box::pin(async {
            Ok(ProviderDevice {
                device_code: "private-device".into(),
                verification_url: "https://www.google.com/device".into(),
                user_code: "PUBLIC-CODE".into(),
                expires_in: 1800,
                interval: 5,
            })
        })
    }
    fn poll<'a>(&'a self, _: &'a str, now: i64) -> ProviderFuture<'a, ProviderPoll> {
        Box::pin(async move {
            Ok(ProviderPoll::Ready(OAuthCredential {
                access_token: "private-access".into(),
                refresh_token: "private-refresh".into(),
                expires_at: (chrono::DateTime::from_timestamp(now, 0).unwrap()
                    + chrono::Duration::hours(1))
                .to_rfc3339(),
            }))
        })
    }
    fn refresh<'a>(
        &'a self,
        c: &'a OAuthCredential,
        _: i64,
    ) -> ProviderFuture<'a, OAuthCredential> {
        Box::pin(async move { Ok(c.clone()) })
    }
}
#[tokio::test]
async fn cookie_web_http_yttv_start_poll_ownership_and_typed_metadata() {
    let (_dir, _store, app, owner, foreign) = fixture().await;
    let (status, start) = request(
        &app,
        "POST",
        "/api/v1/dl/cookies/yttv/start",
        Some(&owner),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(start["verification_url"], "https://www.google.com/device");
    assert_eq!(start["user_code"], "PUBLIC-CODE");
    assert_eq!(start["expires_in"], 1800);
    assert_eq!(start["interval"], 5);
    let path = format!(
        "/api/v1/dl/cookies/yttv/poll/{}",
        start["session_id"].as_str().unwrap()
    );
    assert_eq!(
        request(&app, "GET", &path, Some(&foreign), Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, poll) = request(&app, "GET", &path, Some(&owner), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(poll["status"], "ok");
    assert!(!poll.to_string().contains("private-"));
    let (_, rows) = request(&app, "GET", "/api/v1/dl/cookies", Some(&owner), Value::Null).await;
    assert_eq!(rows[0]["domain"], "youtube.com");
    assert_eq!(rows[0]["is_oauth"], true);
    assert!(rows[0].get("content").is_none());
    let (_, poll) = request(&app, "GET", &path, Some(&owner), Value::Null).await;
    assert_eq!(poll["status"], "expired");
}

#[tokio::test]
async fn cookie_web_http_oauth_validation_is_actionable_and_nonmutating() {
    let (_dir, store, app, owner, _) = fixture().await;
    let content = "__oauth2__{\"access_token\":\"private-access\",\"refresh_token\":\"private-refresh\",\"expires_at\":\"2099-01-01T00:00:00Z\"}";
    yoink_db::query(include_str!(
        "../../yoink-downloads/src/cookie_web_insert.sql"
    ))
    .bind(OWNER as i64)
    .bind("youtube.com")
    .bind(content)
    .bind(chrono::Utc::now())
    .bind(true)
    .execute(store.database())
    .await
    .unwrap();
    let id = yoink_db::query("SELECT id FROM cookies WHERE user_id=?1 AND domain='youtube.com'")
        .bind(OWNER as i64)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get::<i64, _>("id")
        .unwrap();
    let (status, body) = request(
        &app,
        "POST",
        &format!("/api/v1/dl/cookies/{id}/validate"),
        Some(&owner),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let message = body.to_string();
    assert!(message.contains("OAuth"));
    assert!(!message.contains("private-access"));
    assert!(!message.contains("private-refresh"));
    let row = yoink_db::query("SELECT content,is_valid FROM cookies WHERE id=?1")
        .bind(id)
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(row.try_get::<String, _>("content").unwrap(), content);
    assert!(row.try_get::<bool, _>("is_valid").unwrap());
}
