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
use yoink_http::{AuthService, AuthState, router_with_auth};
use yoink_identity::IdentityStore;
use yoink_storage::Store;
const BOT: &str = "fixture-bot-secret";
const SECRET: &str = "fixture-jwt-secret-at-least-32bytes";
async fn fixture() -> (tempfile::TempDir, Router) {
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
    assert!(
        state
            .install(AuthService {
                identities: Arc::new(IdentityStore::new(
                    store.database().clone(),
                    Some(42),
                    false,
                )),
                crypto: AuthConfig::new(BOT.into(), SECRET.into(), 86400, 30, 86400).unwrap(),
            })
            .is_ok()
    );
    let system = SystemService::new("test");
    system.set_ready(true);
    (d, router_with_auth(system, Some(state)))
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
async fn group_endpoints_preserve_contracts_and_validate_ids() {
    let (_d, app) = fixture().await;
    let owner = login(&app, 42).await;
    let member = login(&app, 43).await;
    let gid = -9001;
    let (s, g) = request(
        &app,
        "POST",
        "/api/v1/groups",
        Some(&owner),
        json!({"id":gid,"title":"Group"}),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(g["id"], gid);
    assert_eq!(g["enabled"], false);
    assert_eq!(g["thread_policies"], json!([]));
    let (s, _) = request(
        &app,
        "POST",
        &format!("/api/v1/groups/{gid}/threads"),
        Some(&owner),
        json!({"thread_id":null,"name":"Main","enabled":false}),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, list) = request(&app, "GET", "/api/v1/groups", Some(&owner), Value::Null).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list[0]["thread_policies"][0]["thread_id"], Value::Null);
    let (s, m) = request(
        &app,
        "PUT",
        &format!("/api/v1/groups/{gid}/members/43"),
        Some(&owner),
        json!({"role_override":"moderator","allow_pm_override":false}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(m["group_id"], gid);
    let (s, _) = request(
        &app,
        "GET",
        &format!("/api/v1/groups/{gid}/members"),
        Some(&member),
        Value::Null,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/groups",
            Some(&owner),
            json!({"id":12})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/api/v1/groups/{gid}"),
            Some(&owner),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn frontend_storage_patch_and_unicode_character_limits() {
    let (_d, app) = fixture().await;
    let owner = login(&app, 42).await;
    let title = "я".repeat(256);
    let (status, created) = request(
        &app,
        "POST",
        "/api/v1/groups",
        Some(&owner),
        json!({"id":-9002,"title":title}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["title"], title);
    let path = "/api/v1/groups/-9002";
    let (status, updated) = request(
        &app,
        "PATCH",
        path,
        Some(&owner),
        json!({"title":title,"enabled":true,"auto_grant_role":"user","allow_pm":true,
            "nsfw_allowed":false,"storage_chat_id":42,"storage_thread_id":null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["storage_chat_id"], 42);
    assert_eq!(updated["title"], title);
    let (status, topic) = request(
        &app,
        "POST",
        &format!("{path}/threads"),
        Some(&owner),
        json!({"thread_id":5,"name":title}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(topic["name"], title);
    let too_long = "я".repeat(257);
    for (method, uri, body) in [
        (
            "POST",
            "/api/v1/groups",
            json!({"id":-9003,"title":too_long}),
        ),
        ("PATCH", path, json!({"title":too_long})),
        (
            "POST",
            "/api/v1/groups/-9002/threads",
            json!({"thread_id":5,"name":too_long}),
        ),
        ("PATCH", path, json!({"storage_chat_id":0})),
        ("PATCH", path, json!({"storage_thread_id":-1})),
        ("PATCH", path, json!({"storage_thread_id":0})),
    ] {
        assert_eq!(
            request(&app, method, uri, Some(&owner), body).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let (_, unchanged) = request(&app, "GET", path, Some(&owner), Value::Null).await;
    assert_eq!(unchanged["title"], title);
    assert_eq!(unchanged["storage_chat_id"], 42);
    let (status, cleared) = request(
        &app,
        "PATCH",
        path,
        Some(&owner),
        json!({"storage_chat_id":null,"storage_thread_id":null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared["storage_chat_id"], Value::Null);
}

#[tokio::test]
async fn issued_token_cannot_bypass_demotions_or_timed_bans() {
    let (_d, app) = fixture().await;
    let owner = login(&app, 42).await;
    let admin = login(&app, 43).await;
    let (_, _) = request(
        &app,
        "POST",
        "/api/v1/groups",
        Some(&owner),
        json!({"id":-9004}),
    )
    .await;
    let mutations = [
        ("PATCH", "/api/v1/groups/-9004", json!({"enabled":true})),
        ("POST", "/api/v1/groups", json!({"id":-9005})),
        ("DELETE", "/api/v1/groups/-9004", Value::Null),
        (
            "POST",
            "/api/v1/groups/-9004/threads",
            json!({"thread_id":1}),
        ),
        ("DELETE", "/api/v1/groups/-9004/threads/1", Value::Null),
        (
            "PUT",
            "/api/v1/groups/-9004/members/43",
            json!({"role_override":"user"}),
        ),
        ("DELETE", "/api/v1/groups/-9004/members/43", Value::Null),
    ];
    let promote = json!({"role":"admin"});
    assert_eq!(
        request(&app, "PATCH", "/api/v1/users/43", Some(&owner), promote)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "GET", "/api/v1/groups", Some(&admin), Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    for patch in [
        json!({"role":"user"}),
        json!({"role":"admin","ban_until":(Utc::now()+chrono::Duration::minutes(10)).to_rfc3339()}),
    ] {
        assert_eq!(
            request(&app, "PATCH", "/api/v1/users/43", Some(&owner), patch)
                .await
                .0,
            StatusCode::OK
        );
        for (method, uri, body) in &mutations {
            assert_eq!(
                request(&app, method, uri, Some(&admin), body.clone())
                    .await
                    .0,
                StatusCode::FORBIDDEN
            );
        }
        for uri in [
            "/api/v1/groups",
            "/api/v1/groups/-9004",
            "/api/v1/groups/-9004/threads",
            "/api/v1/groups/-9004/members",
        ] {
            assert_eq!(
                request(&app, "GET", uri, Some(&admin), Value::Null).await.0,
                StatusCode::FORBIDDEN
            );
        }
    }
    let (_, group) = request(
        &app,
        "GET",
        "/api/v1/groups/-9004",
        Some(&owner),
        Value::Null,
    )
    .await;
    assert_eq!(group["enabled"], false);
    assert_eq!(group["thread_policies"], json!([]));
}
