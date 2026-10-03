use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use tower::ServiceExt;
use yoink_application::SystemService;
use yoink_auth::AuthConfig;
use yoink_http::{AuthService, AuthState, router_with_auth};
use yoink_identity::IdentityStore;
use yoink_storage::Store;

const BOT: &str = "fixture-bot-secret";
const SECRET: &str = "fixture-jwt-secret-at-least-32bytes";

async fn fixture(dev: bool) -> (tempfile::TempDir, Arc<Store>, Router) {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Arc::new(
        Store::open(
            &directory.path().join("http.db"),
            true,
            Duration::from_secs(2),
        )
        .await
        .unwrap(),
    );
    store.migrate().await.unwrap();
    let state = AuthState::new(dev);
    let service = AuthService {
        identities: Arc::new(IdentityStore::new(
            store.database().clone(),
            Some(42),
            false,
        )),
        crypto: AuthConfig::new(BOT.into(), SECRET.into(), 86400, 30, 86400).unwrap(),
    };
    assert!(state.install(service).is_ok());
    let system = SystemService::new("test");
    system.set_ready(true);
    (directory, store, router_with_auth(system, Some(state)))
}

fn init_data(id: i64, username: &str, timestamp: i64) -> String {
    let user = json!({"id": id, "first_name":"Test + %2F", "username":username}).to_string();
    let data = format!("auth_date={timestamp}\nquery_id=fixture\nuser={user}");
    let mut key = Hmac::<Sha256>::new_from_slice(b"WebAppData").unwrap();
    key.update(BOT.as_bytes());
    let mut mac = Hmac::<Sha256>::new_from_slice(&key.finalize().into_bytes()).unwrap();
    mac.update(data.as_bytes());
    let hash: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
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
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    if status.is_success() && uri.starts_with("/api/v1/") {
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    }
    let bytes = to_bytes(response.into_body(), 100_000).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn login(app: &Router, id: i64) -> (StatusCode, Value) {
    request(
        app,
        "POST",
        "/api/v1/auth/token",
        None,
        json!({"init_data":init_data(id, "test_user", Utc::now().timestamp())}),
    )
    .await
}

#[tokio::test]
async fn installed_auth_is_gated_by_startup_and_shutdown_readiness() {
    let (_directory, store, _) = fixture(false).await;
    let identities = Arc::new(IdentityStore::new(
        store.database().clone(),
        Some(42),
        false,
    ));
    let state = AuthState::new(true);
    assert!(
        state
            .install(AuthService {
                identities: identities.clone(),
                crypto: AuthConfig::new(BOT.into(), SECRET.into(), 86400, 30, 86400).unwrap(),
            })
            .is_ok()
    );
    let system = SystemService::new("lifecycle");
    let app = router_with_auth(system.clone(), Some(state));
    assert_eq!(login(&app, 43).await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(identities.current_profile(43).await.is_err());
    system.set_ready(true);
    let (status, body) = login(&app, 43).await;
    assert_eq!(status, StatusCode::OK);
    let token = body["access_token"].as_str().unwrap();
    let before = identities.current_profile(43).await.unwrap();
    system.set_ready(false);
    for (method, uri, body) in [
        ("GET", "/api/v1/users/me", Value::Null),
        ("GET", "/api/v1/settings", Value::Null),
        ("PATCH", "/api/v1/settings", json!({"language":"ru"})),
        ("POST", "/api/v1/auth/dev?user_id=43", Value::Null),
    ] {
        assert_eq!(
            request(&app, method, uri, Some(token), body).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    assert_eq!(login(&app, 44).await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(identities.current_profile(44).await.is_err());
    let after = identities.current_profile(43).await.unwrap();
    assert_eq!(before.language, after.language);
    assert_eq!(before.updated_at, after.updated_at);
}

#[tokio::test]
async fn native_http_auth_and_settings_contract() {
    let (_directory, store, app) = fixture(false).await;
    let (status, body) = login(&app, 43).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["token_type"], "bearer");
    assert_eq!(body["role"], "user");
    let token = body["access_token"].as_str().unwrap();
    let (status, user) = request(&app, "GET", "/api/v1/users/me", Some(token), Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{user}");
    assert_eq!(user["id"], 43);
    assert_eq!(user["first_name"], "Test + %2F");
    for key in [
        "username",
        "photo_url",
        "role",
        "language",
        "theme",
        "ban_until",
        "created_at",
        "updated_at",
        "dl_count",
        "dl_last_at",
    ] {
        assert!(user.get(key).is_some(), "missing {key}");
    }
    let (status, settings) = request(
        &app,
        "PATCH",
        "/api/v1/settings",
        Some(token),
        json!({"language":"ru","theme":"latte"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(
        settings,
        json!({"user_id":43,"language":"ru","theme":"latte"})
    );
    let (status, untouched) = request(
        &app,
        "PATCH",
        "/api/v1/settings",
        Some(token),
        json!({"language":null,"theme":null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(untouched, settings);
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/settings",
            Some(token),
            json!({"language":"en","theme":"invalid"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request(&app, "GET", "/api/v1/settings", Some(token), Value::Null)
            .await
            .1,
        settings
    );
    assert_eq!(
        request(&app, "GET", "/api/v1/users/me", None, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/settings",
            Some("malformed"),
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/auth/dev?user_id=43&role=owner",
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (status, owner) = login(&app, 42).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(owner["role"], "owner");
    yoink_db::query(include_str!("sql/set_role.sql"))
        .bind("restricted")
        .bind(43_i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        request(&app, "GET", "/api/v1/users/me", Some(token), Value::Null)
            .await
            .1["role"],
        "restricted"
    );
    yoink_db::query(include_str!("sql/set_role.sql"))
        .bind("banned")
        .bind(43_i64)
        .execute(store.database())
        .await
        .unwrap();
    for (method, path) in [
        ("GET", "/api/v1/users/me"),
        ("GET", "/api/v1/settings"),
        ("PATCH", "/api/v1/settings"),
    ] {
        assert_eq!(
            request(&app, method, path, Some(token), json!({"language":"en"}))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(login(&app, 43).await.0, StatusCode::FORBIDDEN);
    yoink_db::query(include_str!("sql/set_role.sql"))
        .bind("banned")
        .bind(42_i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(login(&app, 42).await.0, StatusCode::FORBIDDEN);
    let owner_token = owner["access_token"].as_str().unwrap();
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/users/me",
            Some(owner_token),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn signed_auth_policy_and_dev_cannot_escalate_role() {
    let (_directory, store, app) = fixture(true).await;
    yoink_db::query(include_str!("sql/set_policy.sql"))
        .bind("approved_only")
        .execute(store.database())
        .await
        .unwrap();
    let (status, user) = login(&app, 44).await;
    assert_eq!(status, StatusCode::OK, "{user}");
    assert_eq!(user["role"], "restricted");
    let (status, dev) = request(
        &app,
        "POST",
        "/api/v1/auth/dev?user_id=44&role=owner",
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(dev["role"], "restricted");
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/auth/dev?user_id=900",
            None,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    for init in [
        "bad".into(),
        init_data(44, "test", Utc::now().timestamp() - 86401),
        init_data(44, "test", Utc::now().timestamp() + 301),
        format!(
            "{}&auth_date=1",
            init_data(44, "test", Utc::now().timestamp())
        ),
    ] {
        assert_eq!(
            request(
                &app,
                "POST",
                "/api/v1/auth/token",
                None,
                json!({"init_data":init})
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    yoink_db::query(include_str!("sql/set_policy.sql"))
        .bind("invalid")
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(login(&app, 45).await.0, StatusCode::SERVICE_UNAVAILABLE);
    store.close().await.unwrap();
}

#[tokio::test]
async fn routes_disabled_by_default_and_fail_closed_before_install() {
    let app = router_with_auth(SystemService::new("test"), None);
    assert_eq!(login(&app, 43).await.0, StatusCode::NOT_FOUND);
    let app = router_with_auth(SystemService::new("test"), Some(AuthState::new(false)));
    assert_eq!(login(&app, 43).await.0, StatusCode::SERVICE_UNAVAILABLE);
}
