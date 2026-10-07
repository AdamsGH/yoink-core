use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{sync::Arc, time::Duration};
use tower::ServiceExt;
use yoink_application::SystemService;
use yoink_auth::AuthConfig;
use yoink_http::{AuthService, AuthState, router_with_identity_and_user_session};
use yoink_identity::IdentityStore;
use yoink_storage::Store;
const BOT: &str = "fixture-bot-secret";
const SECRET: &str = "fixture-jwt-secret-at-least-32bytes";
async fn fixture(
    client: Option<Arc<yoink_user_session::Client>>,
) -> (tempfile::TempDir, Router, Arc<IdentityStore>) {
    fixture_with_readiness(client, true).await
}

async fn fixture_with_readiness(
    client: Option<Arc<yoink_user_session::Client>>,
    ready: bool,
) -> (tempfile::TempDir, Router, Arc<IdentityStore>) {
    let d = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Arc::new(
        Store::open(
            &d.path().join("groups-http.db"),
            true,
            Duration::from_secs(2),
        )
        .await
        .unwrap(),
    );
    store.migrate().await.unwrap();
    let state = AuthState::new(false);
    let identities = Arc::new(IdentityStore::new(
        store.database().clone(),
        Some(42),
        false,
    ));
    assert!(
        state
            .install(AuthService {
                identities: identities.clone(),
                crypto: AuthConfig::new(BOT.into(), SECRET.into(), 86400, 30, 86400).unwrap(),
            })
            .is_ok()
    );
    let system = SystemService::new("test");
    system.set_ready(ready);
    (
        d,
        router_with_identity_and_user_session(system, Some(state.clone()), state, client),
        identities,
    )
}
fn init_data(id: i64) -> String {
    let user = json!({"id":id,"first_name":"Test","username":"test"}).to_string();
    let timestamp = Utc::now().timestamp();
    let data = format!("auth_date={timestamp}\nquery_id=fixture\nuser={user}");
    let mut key = Hmac::<Sha256>::new_from_slice(b"WebAppData").unwrap();
    key.update(BOT.as_bytes());
    let mut mac = Hmac::<Sha256>::new_from_slice(&key.finalize().into_bytes()).unwrap();
    mac.update(data.as_bytes());
    let hash: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect();
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("user", &user)
        .append_pair("hash", &hash)
        .append_pair("query_id", "fixture")
        .append_pair("auth_date", &timestamp.to_string())
        .finish()
}
async fn request(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let r = app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = r.status();
    let bytes = to_bytes(r.into_body(), 100000).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn login(app: &Router, id: i64) -> String {
    let (s, b) = request(
        app,
        "POST",
        "/api/v1/auth/token",
        None,
        json!({"init_data":init_data(id)}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    b["access_token"].as_str().unwrap().to_owned()
}
#[tokio::test]
async fn session_routes_are_real_and_failed_scan_does_not_write() {
    use std::os::unix::fs::PermissionsExt;
    use yoink_user_session::{Client, Config};
    let d = tempfile::tempdir().unwrap();
    let token_file = d.path().join("session");
    std::fs::write(&token_file, "fake-session").unwrap();
    std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", l.local_addr().unwrap())
        .parse()
        .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let upstream=Router::new().fallback(move |uri:axum::http::Uri| {let count=count.clone();async move{
        let i=count.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        if uri.path().ends_with("getForumTopics") && i>0 {return axum::Json(json!({"ok":false,"description":"fake-session"}));}
        axum::Json(json!({"ok":true,"result":{"topics":[{"message_thread_id":7,"name":"Real fixture"}],"total_count":1}}))
    }});
    let task = tokio::spawn(async move { axum::serve(l, upstream).await.unwrap() });
    let client = Arc::new(
        Client::new(Config {
            origin,
            token_file,
            timeout: Duration::from_secs(2),
            token_timeout: Duration::from_secs(1),
            max_response_bytes: 8192,
            max_token_bytes: 128,
            max_scan_pages: 2,
            max_scan_topics: 200,
        })
        .unwrap(),
    );
    let (_d, app, ids) = fixture(Some(client)).await;
    let owner = login(&app, 42).await;
    let user = login(&app, 43).await;
    ids.group_create(
        42,
        &yoink_identity::admin::groups::GroupCreate {
            id: -10,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/threads/status",
            Some(&owner),
            Value::Null
        )
        .await,
        (StatusCode::OK, json!({"available":true}))
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/forum/history/-10",
            Some(&user),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/threads/scan/-10",
            Some(&owner),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    let rows = ids.threads_list(42, -10).await.unwrap();
    assert_eq!(rows.len(), 1);
    let (status, body) = request(
        &app,
        "POST",
        "/api/v1/threads/scan/-10",
        Some(&owner),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(!body.to_string().contains("fake-session"));
    assert_eq!(ids.threads_list(42, -10).await.unwrap(), rows);
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/forum/topics/-10?limit=101",
            Some(&owner),
            Value::Null
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    task.abort();
}
#[tokio::test]
async fn absent_session_is_only_unavailable_after_authorization() {
    let (_d, app, _) = fixture(None).await;
    let owner = login(&app, 42).await;
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/threads/status",
            Some(&owner),
            Value::Null
        )
        .await,
        (StatusCode::OK, json!({"available":false}))
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/messages/-10/7/viewers",
            Some(&owner),
            Value::Null
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/messages/-10/7/viewers",
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn composed_session_routes_wait_for_runtime_readiness() {
    let (_directory, app, _identities) = fixture_with_readiness(None, false).await;
    for uri in ["/api/v1/threads/status", "/api/v1/forum/history/-10"] {
        assert_eq!(
            request(&app, "GET", uri, None, Value::Null).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

#[tokio::test]
async fn current_admin_can_check_status_but_cannot_proxy_owner_session() {
    let (_directory, app, identities) = fixture(None).await;
    login(&app, 42).await;
    let token = login(&app, 43).await;
    identities
        .admin_update_user(
            42,
            43,
            &yoink_identity::admin::AdminUserPatch {
                role: Some(yoink_access::Role::Admin),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/threads/status",
            Some(&token),
            Value::Null
        )
        .await,
        (StatusCode::OK, json!({"available": false}))
    );
    for uri in [
        "/api/v1/forum/topics/-10",
        "/api/v1/forum/search/-10",
        "/api/v1/forum/history/-10",
        "/api/v1/messages/43/7/viewers",
        "/api/v1/messages/43/7/link",
        "/api/v1/messages/43/7/read-date",
        "/api/v1/messages/43/7/thread",
        "/api/v1/messages/43/by-date?date=0",
    ] {
        assert_eq!(
            request(&app, "GET", uri, Some(&token), Value::Null).await.0,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
    }
}
