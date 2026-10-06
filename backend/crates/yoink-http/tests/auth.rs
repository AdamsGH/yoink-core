use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
    if uri.starts_with("/api/v1/") && status != StatusCode::NOT_FOUND {
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    }
    if status == StatusCode::UNAUTHORIZED {
        assert_eq!(
            response.headers().get("www-authenticate").unwrap(),
            "Bearer"
        );
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
    for uri in [
        "/api/v1/users",
        "/api/v1/users/42",
        "/api/v1/features",
        "/api/v1/feature-access/me",
        "/api/v1/permissions/all",
        "/api/v1/bot-settings",
    ] {
        assert_eq!(
            request(&app, "GET", uri, None, Value::Null).await.0,
            StatusCode::NOT_FOUND
        );
    }
    let app = router_with_auth(SystemService::new("test"), Some(AuthState::new(false)));
    assert_eq!(login(&app, 43).await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        request(&app, "GET", "/api/v1/users", None, Value::Null)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

async fn access_token(app: &Router, id: i64) -> String {
    let (status, value) = login(app, id).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value["access_token"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn admin_routes_use_current_actor_and_protect_hierarchy() {
    let (_directory, store, app) = fixture(false).await;
    let owner = access_token(&app, 42).await;
    let regular = access_token(&app, 43).await;
    access_token(&app, 44).await;
    for uri in [
        "/api/v1/users",
        "/api/v1/users/44",
        "/api/v1/features",
        "/api/v1/users/44/feature-access",
        "/api/v1/users/44/permissions",
        "/api/v1/permissions/all",
        "/api/v1/bot-settings",
    ] {
        assert_eq!(
            request(&app, "GET", uri, None, Value::Null).await.0,
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
        assert_eq!(
            request(&app, "GET", uri, Some("invalid"), Value::Null)
                .await
                .0,
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
        assert_eq!(
            request(&app, "GET", uri, Some(&regular), Value::Null)
                .await
                .0,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
        assert_eq!(
            request(&app, "GET", uri, Some(&owner), Value::Null).await.0,
            StatusCode::OK,
            "{uri}"
        );
    }
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/feature-access/me",
            Some(&regular),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/users/43",
            Some(&owner),
            json!({"role":"admin"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let admin = access_token(&app, 43).await;
    for body in [
        json!({"role":"user"}),
        json!({"language":"ru"}),
        json!({"ban_until":null}),
    ] {
        assert_eq!(
            request(
                &app,
                "PATCH",
                "/api/v1/users/42",
                Some(&admin),
                body.clone()
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(&app, "PATCH", "/api/v1/users/43", Some(&admin), body)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/users/44",
            Some(&admin),
            json!({"role":"admin"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/users/44",
            Some(&owner),
            json!({"role":"owner"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/users/42",
            Some(&owner),
            json!({"role":"user"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/bot-settings",
            Some(&admin),
            json!({"bot_access_mode":"approved_only"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/users/44",
            Some(&admin),
            json!({"language":"ru"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/users/43",
            Some(&owner),
            json!({"role":"user"})
        )
        .await
        .0,
        StatusCode::OK
    );
    for uri in ["/api/v1/users", "/api/v1/features", "/api/v1/bot-settings"] {
        assert_eq!(
            request(&app, "GET", uri, Some(&admin), Value::Null).await.0,
            StatusCode::FORBIDDEN,
            "stale admin JWT {uri}"
        );
    }
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/users/44",
            Some(&admin),
            json!({"role":"banned"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn admin_dtos_pagination_validation_and_explicit_null_bans() {
    let (_directory, store, app) = fixture(false).await;
    let owner = access_token(&app, 42).await;
    let target = access_token(&app, 44).await;
    let ban = (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    let (status, user) = request(
        &app,
        "PATCH",
        "/api/v1/users/44",
        Some(&owner),
        json!({"ban_until":ban}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{user}");
    assert!(user["ban_until"].is_string());
    for key in [
        "id",
        "username",
        "first_name",
        "photo_url",
        "role",
        "language",
        "theme",
        "created_at",
        "updated_at",
        "dl_count",
        "dl_last_at",
    ] {
        assert!(user.get(key).is_some(), "missing {key}");
    }
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/users/44",
            Some(&owner),
            json!({"language":"ru"})
        )
        .await
        .1["ban_until"],
        user["ban_until"]
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/feature-access/me",
            Some(&target),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, unbanned) = request(
        &app,
        "PATCH",
        "/api/v1/users/44",
        Some(&owner),
        json!({"ban_until":null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{unbanned}");
    assert!(unbanned["ban_until"].is_null());
    assert_eq!(unbanned["language"], "ru");
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/feature-access/me",
            Some(&target),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    for body in [
        json!({"role":"superadmin"}),
        json!({"language":"xx","role":"restricted"}),
        json!({"language":"en","ban_until":"invalid"}),
        json!({"id":55}),
        Value::Null,
    ] {
        assert_eq!(
            request(&app, "PATCH", "/api/v1/users/44", Some(&owner), body)
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let unchanged = request(&app, "GET", "/api/v1/users/44", Some(&owner), Value::Null)
        .await
        .1;
    assert_eq!(unchanged["role"], "user");
    assert_eq!(unchanged["language"], "ru");
    for uri in [
        "/api/v1/users/bad",
        "/api/v1/users/0",
        "/api/v1/users/-1",
        "/api/v1/users?limit=0",
        "/api/v1/users?limit=201",
        "/api/v1/users?offset=-1",
        "/api/v1/users?page=0",
        "/api/v1/users?page=2&offset=0",
        "/api/v1/users?role=superadmin",
        "/api/v1/users?status=unknown",
        "/api/v1/users?sort=invalid",
        "/api/v1/users?direction=invalid",
        "/api/v1/users?period=8",
    ] {
        assert_eq!(
            request(&app, "GET", uri, Some(&owner), Value::Null).await.0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{uri}"
        );
    }
    for uri in [
        "/api/v1/users/999",
        "/api/v1/users/999/feature-access",
        "/api/v1/users/999/permissions",
    ] {
        assert_eq!(
            request(&app, "GET", uri, Some(&owner), Value::Null).await.0,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
    let (status, first) = request(
        &app,
        "GET",
        "/api/v1/users?limit=1&sort=name&direction=asc",
        Some(&owner),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["total"], 2);
    assert_eq!(first["limit"], 1);
    assert_eq!(first["offset"], 0);
    assert_eq!(first["has_more"], true);
    let second = request(
        &app,
        "GET",
        "/api/v1/users?limit=1&page=2&sort=name&direction=asc",
        Some(&owner),
        Value::Null,
    )
    .await
    .1;
    assert_eq!(second["offset"], 1);
    assert_eq!(second["has_more"], false);
    assert_ne!(first["items"][0]["id"], second["items"][0]["id"]);
    store.close().await.unwrap();
}

#[tokio::test]
async fn permission_dtos_and_effective_access_follow_shared_policy() {
    let (_directory, store, app) = fixture(false).await;
    let owner = access_token(&app, 42).await;
    let target = access_token(&app, 44).await;
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/users/44",
            Some(&owner),
            json!({"role":"restricted"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let features = request(&app, "GET", "/api/v1/features", Some(&owner), Value::Null)
        .await
        .1;
    assert!(features.as_array().unwrap().len() >= 14);
    let feature = features
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["plugin"] == "dl" && item["feature"] == "inline")
        .unwrap();
    for key in [
        "plugin",
        "feature",
        "label",
        "description",
        "default_min_role",
    ] {
        assert!(feature.get(key).is_some(), "{key}");
    }
    let access = request(
        &app,
        "GET",
        "/api/v1/feature-access/me",
        Some(&target),
        Value::Null,
    )
    .await
    .1;
    let download = |value: &Value| {
        value
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["plugin"] == "dl" && item["feature"] == "inline")
            .unwrap()
            .clone()
    };
    assert_eq!(download(&access)["effective"], false);
    let grant = json!({"plugin":"dl","feature":"inline"});
    let (status, permission) = request(
        &app,
        "POST",
        "/api/v1/users/44/permissions",
        Some(&owner),
        grant.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{permission}");
    for key in [
        "id",
        "user_id",
        "plugin",
        "feature",
        "granted_by",
        "granted_at",
        "expires_at",
        "grant_source",
    ] {
        assert!(permission.get(key).is_some(), "{key}");
    }
    assert_eq!(permission["user_id"], 44);
    assert_eq!(permission["granted_by"], 42);
    assert_eq!(permission["grant_source"], "manual");
    let second = request(
        &app,
        "POST",
        "/api/v1/users/44/permissions",
        Some(&owner),
        grant,
    )
    .await
    .1;
    assert_eq!(permission["id"], second["id"]);
    let all = request(
        &app,
        "GET",
        "/api/v1/permissions/all",
        Some(&owner),
        Value::Null,
    )
    .await
    .1;
    assert_eq!(all.as_array().unwrap().len(), 1);
    let access = request(
        &app,
        "GET",
        "/api/v1/feature-access/me",
        Some(&target),
        Value::Null,
    )
    .await
    .1;
    let item = download(&access);
    assert_eq!(item["access_via_role"], false);
    assert_eq!(item["access_via_grant"], true);
    assert_eq!(item["effective"], true);
    assert_eq!(item["grant_source"], "manual");
    assert!(item["grant_expires_at"].is_null());
    let (status, revoked) = request(
        &app,
        "DELETE",
        "/api/v1/users/44/permissions/dl/inline",
        Some(&owner),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revoked, json!({"ok":true}));
    assert_eq!(
        request(
            &app,
            "DELETE",
            "/api/v1/users/44/permissions/dl/inline",
            Some(&owner),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    let access = request(
        &app,
        "GET",
        "/api/v1/feature-access/me",
        Some(&target),
        Value::Null,
    )
    .await
    .1;
    assert_eq!(download(&access)["effective"], false);
    for body in [
        json!({"plugin":"unknown","feature":"download"}),
        json!({"plugin":"yoink-dl","feature":"unknown"}),
        json!({"plugin":"dl","feature":"inline","expires_at":"bad"}),
        json!({"plugin":"dl","feature":"inline","granted_by":42}),
    ] {
        assert_eq!(
            request(
                &app,
                "POST",
                "/api/v1/users/44/permissions",
                Some(&owner),
                body
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    store.close().await.unwrap();
}

#[tokio::test]
async fn registration_policy_changes_are_validated_atomic_and_applied_to_signed_login() {
    let (_directory, store, app) = fixture(false).await;
    let owner = access_token(&app, 42).await;
    let settings = request(
        &app,
        "GET",
        "/api/v1/bot-settings",
        Some(&owner),
        Value::Null,
    )
    .await
    .1;
    assert_eq!(settings["bot_access_mode"], "open");
    assert_eq!(settings["browser_cookies_min_role"], "owner");
    for body in [
        json!({"bot_access_mode":"broken"}),
        json!({"bot_access_mode":"approved_only","browser_cookies_min_role":"broken"}),
        json!({"inline_storage_thread_id":-1}),
        json!({"secret":"ignored"}),
    ] {
        assert_eq!(
            request(&app, "PATCH", "/api/v1/bot-settings", Some(&owner), body)
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            request(
                &app,
                "GET",
                "/api/v1/bot-settings",
                Some(&owner),
                Value::Null
            )
            .await
            .1["bot_access_mode"],
            "open"
        );
    }
    let (status, result) = request(
        &app,
        "PATCH",
        "/api/v1/bot-settings",
        Some(&owner),
        json!({"bot_access_mode":"approved_only"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result, json!({"ok":true}));
    let (status, pending) = login(&app, 44).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pending["role"], "restricted");
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/users/44",
            Some(&owner),
            json!({"role":"user"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(login(&app, 44).await.1["role"], "user");
    yoink_db::query(include_str!("sql/set_policy.sql"))
        .bind("corrupt")
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(login(&app, 45).await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/bot-settings",
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
            "PATCH",
            "/api/v1/bot-settings",
            Some(&owner),
            json!({"bot_access_mode":"open"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(login(&app, 45).await.1["role"], "user");
    store.close().await.unwrap();
}

#[tokio::test]
async fn tag_map_routes_match_legacy_contract_and_validate_atomically() {
    let (_directory, store, app) = fixture(false).await;
    let owner = access_token(&app, 42).await;
    let regular = access_token(&app, 43).await;
    let path = "/api/v1/bot-settings/tag-map";
    assert_eq!(
        request(&app, "GET", path, Some(&regular), Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/bot-settings/available-features",
            Some(&regular),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "GET", path, Some(&owner), Value::Null)
            .await
            .1,
        json!([])
    );
    let features = request(
        &app,
        "GET",
        "/api/v1/bot-settings/available-features",
        Some(&owner),
        Value::Null,
    )
    .await
    .1;
    let feature_keys: Vec<_> = features
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["key"].as_str().unwrap())
        .collect();
    let mut sorted_feature_keys = feature_keys.clone();
    sorted_feature_keys.sort();
    assert_eq!(
        feature_keys, sorted_feature_keys,
        "available feature order is deterministic"
    );
    assert!(features.as_array().unwrap().iter().any(|item| item == &json!({"key":"dl:inline","plugin":"dl","feature":"inline","label":"Inline search","description":"YouTube search and URL lookup via @bot inline mode"})));
    let mapping = json!([
        {"tag":"Бета", "features":["stats:stats"]},
        {"tag":"Alpha", "features":["dl:inline"]}
    ]);
    let (status, written) = request(&app, "PUT", path, Some(&owner), mapping.clone()).await;
    assert_eq!(status, StatusCode::OK, "{written}");
    assert_eq!(written[0]["tag"], "Alpha");
    assert_eq!(written[1]["tag"], "Бета");
    assert_eq!(
        request(&app, "GET", path, Some(&owner), Value::Null)
            .await
            .1,
        written
    );
    assert_eq!(
        request(&app, "PUT", path, Some(&regular), json!([]))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for invalid in [
        json!({"tag":"single object","features":[]}),
        json!([{"tag":"missing features"}]),
        json!([{"tag":"extra","features":[],"ignored":true}]),
        json!([{"tag":" ","features":[]}]),
        json!([{"tag":"duplicate","features":[]},{"tag":"duplicate","features":[]}]),
        json!([{"tag":"unknown","features":["missing:feature"]}]),
        json!([{"tag":"bad feature","features":["dl:inline:extra"]}]),
        json!([{"tag":"bad type","features":[1]}]),
    ] {
        assert_eq!(
            request(&app, "PUT", path, Some(&owner), invalid.clone())
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{invalid}"
        );
        assert_eq!(
            request(&app, "GET", path, Some(&owner), Value::Null)
                .await
                .1,
            written
        );
    }
    assert_eq!(
        request(&app, "PUT", path, Some(&owner), json!([])).await.0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "GET", path, Some(&owner), Value::Null)
            .await
            .1,
        json!([])
    );
    yoink_db::query(include_str!("sql/set_role.sql"))
        .bind("banned")
        .bind(42_i64)
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        request(&app, "PUT", path, Some(&owner), mapping).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "GET", path, Some(&owner), Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn api_key_management_matches_legacy_contract_and_rechecks_admin_authority() {
    let (_directory, store, app) = fixture(false).await;
    let owner = access_token(&app, 42).await;
    let regular = access_token(&app, 43).await;
    assert_eq!(
        request(&app, "GET", "/api/v1/api-keys", None, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    for (method, uri, body) in [
        ("GET", "/api/v1/api-keys", Value::Null),
        ("GET", "/api/v1/api-keys/scopes", Value::Null),
        ("POST", "/api/v1/api-keys", json!({"name":"denied"})),
        ("DELETE", "/api/v1/api-keys/1", Value::Null),
    ] {
        assert_eq!(
            request(&app, method, uri, Some(&regular), body).await.0,
            StatusCode::FORBIDDEN,
            "{method} {uri}"
        );
    }
    let (status, scope_result) = request(
        &app,
        "GET",
        "/api/v1/api-keys/scopes",
        Some(&owner),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(scope_result["scopes"][0], "users:read");
    let expiry = "2024-01-02T03:04:05.123456Z";
    let (status, created) = request(&app, "POST", "/api/v1/api-keys", Some(&owner), json!({"name":"\u{9375} \u{00e9}quipe","scopes":["users:read","groups:*"],"expires_at":expiry})).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let raw = created["key"].as_str().unwrap();
    assert!(raw.starts_with("yoink_"));
    assert_eq!(created["prefix"], raw.chars().take(8).collect::<String>());
    assert_eq!(created["scopes"], json!(["users:read", "groups:*"]));
    assert_eq!(created["expires_at"], expiry);
    let id = created["id"].as_i64().unwrap();
    let stored = yoink_db::query(include_str!("sql/api_key_hash.sql"))
        .bind(id)
        .fetch_one(store.database())
        .await
        .unwrap();
    let hash: String = stored.try_get(0).unwrap();
    let prefix: String = stored.try_get(1).unwrap();
    let scopes: String = stored.try_get(2).unwrap();
    assert_eq!(hash, format!("{:x}", sha2::Sha256::digest(raw.as_bytes())));
    assert_eq!(prefix, created["prefix"]);
    assert_eq!(scopes, r#"["users:read","groups:*"]"#);
    let (status, listed) =
        request(&app, "GET", "/api/v1/api-keys", Some(&owner), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let list_json = listed.to_string();
    assert!(!list_json.contains(raw));
    assert!(!list_json.contains(&hash));
    assert_eq!(listed[0]["name"], "\u{9375} \u{00e9}quipe");
    assert_eq!(listed[0]["created_by"], 42);
    let (status, defaults) = request(
        &app,
        "POST",
        "/api/v1/api-keys",
        Some(&owner),
        json!({"name":"default"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(defaults["scopes"], json!(["*"]));
    assert!(defaults["expires_at"].is_null());
    for body in [json!({"name":"bad","scopes":["unknown:read"]})] {
        assert_eq!(
            request(&app, "POST", "/api/v1/api-keys", Some(&owner), body)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    for body in [
        json!({"name":"bad","scopes":[1]}),
        json!({"name":"bad","expires_at":"not-a-date"}),
        json!({"name":"bad","extra":true}),
    ] {
        assert_eq!(
            request(&app, "POST", "/api/v1/api-keys", Some(&owner), body)
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/api/v1/api-keys/{id}"),
            Some(&owner),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/api/v1/api-keys/{id}"),
            Some(&owner),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            "/api/v1/api-keys/999999",
            Some(&owner),
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/users/43",
            Some(&owner),
            json!({"role":"admin"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let stale_admin = access_token(&app, 43).await;
    assert_eq!(
        request(
            &app,
            "GET",
            "/api/v1/api-keys",
            Some(&stale_admin),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/users/43",
            Some(&owner),
            json!({"role":"user"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let key_uri = format!("/api/v1/api-keys/{id}");
    for (method, uri, body) in [
        ("GET", "/api/v1/api-keys", Value::Null),
        ("GET", "/api/v1/api-keys/scopes", Value::Null),
        ("POST", "/api/v1/api-keys", json!({"name":"stale"})),
        ("DELETE", key_uri.as_str(), Value::Null),
    ] {
        assert_eq!(
            request(&app, method, uri, Some(&stale_admin), body).await.0,
            StatusCode::FORBIDDEN,
            "stale admin {method} {uri}"
        );
    }
    yoink_db::query(include_str!("sql/set_role.sql"))
        .bind("banned")
        .bind(42_i64)
        .execute(store.database())
        .await
        .unwrap();
    for (method, uri, body) in [
        ("GET", "/api/v1/api-keys", Value::Null),
        ("GET", "/api/v1/api-keys/scopes", Value::Null),
        ("POST", "/api/v1/api-keys", json!({"name":"banned-owner"})),
        ("DELETE", key_uri.as_str(), Value::Null),
    ] {
        assert_eq!(
            request(&app, method, uri, Some(&owner), body).await.0,
            StatusCode::FORBIDDEN,
            "banned configured owner {method} {uri}"
        );
    }
    store.close().await.unwrap();
}

#[tokio::test]
async fn rejection_and_body_limit_responses_keep_security_headers() {
    let (_directory, store, app) = fixture(false).await;
    let owner = access_token(&app, 42).await;
    for (uri, body) in [
        ("/api/v1/users/43", "{".to_owned()),
        ("/api/v1/users/43", "x".repeat(21000)),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(uri)
                    .header("authorization", format!("Bearer {owner}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/users")
                .header("authorization", format!("Bearer {owner}"))
                .header("authorization", format!("Bearer {owner}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    assert_eq!(
        response.headers().get("www-authenticate").unwrap(),
        "Bearer"
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn core_configuration_frontend_strings_defaults_atomic_validation_and_stale_roles() {
    let (_directory, store, app) = fixture(false).await;
    let owner = access_token(&app, 42).await;
    let user = access_token(&app, 43).await;
    let uri = "/api/v1/bot-settings";
    let (status, defaults) = request(&app, "GET", uri, Some(&owner), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(defaults["inline_storage_chat_id"].is_null());
    let body = json!({"inline_storage_chat_id":"-100123", "inline_storage_thread_id":"42"});
    assert_eq!(
        request(&app, "PATCH", uri, Some(&user), body.clone())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "PATCH", uri, Some(&owner), body).await,
        (StatusCode::OK, json!({"ok":true}))
    );
    let saved = request(&app, "GET", uri, Some(&owner), Value::Null).await.1;
    assert_eq!(saved["inline_storage_chat_id"], "-100123");
    for body in [
        json!({"inline_storage_chat_id":"-100456", "inline_storage_thread_id":true}),
        json!({"unknown":null}),
    ] {
        let (status, error) = request(&app, "PATCH", uri, Some(&owner), body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_ne!(error["detail"], "Invalid request");
        assert_eq!(
            request(&app, "GET", uri, Some(&owner), Value::Null).await.1,
            saved
        );
    }
    assert_eq!(
        request(
            &app,
            "PATCH",
            uri,
            Some(&owner),
            json!({"inline_storage_chat_id":null})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "GET", uri, Some(&owner), Value::Null).await.1,
        saved
    );
    // A previously minted owner token cannot bypass a database ban.
    let connection = store.database().connection().await.unwrap();
    yoink_db::query(include_str!("sql/configuration_ban_owner.sql"))
        .execute(&*connection)
        .await
        .unwrap();
    drop(connection);
    assert_eq!(
        request(
            &app,
            "PATCH",
            uri,
            Some(&owner),
            json!({"bot_access_mode":"approved_only"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn configuration_metadata_requires_current_admin_role() {
    let (_directory, store, app) = fixture(false).await;
    let owner = access_token(&app, 42).await;
    let user = access_token(&app, 43).await;
    let uri = "/api/v1/_meta/openapi.json";
    assert_eq!(
        request(&app, "GET", uri, None, Value::Null).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "GET", uri, Some(&user), Value::Null).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, schema) = request(&app, "GET", uri, Some(&owner), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(schema["openapi"], "3.1.0");
    assert!(schema["paths"]["/api/v1/bot-settings"]["patch"].is_object());
    store.close().await.unwrap();
}

#[tokio::test]
async fn thread_session_status_is_read_only_and_scan_is_explicitly_unavailable() {
    let (_directory, store, app) = fixture(false).await;
    let owner = access_token(&app, 42).await;
    let user = access_token(&app, 43).await;
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
            "/api/v1/threads/status",
            Some(&user),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/threads/scan/-100123",
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
            "POST",
            "/api/v1/threads/scan/-100123",
            Some(&user),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/threads/scan/bad",
            Some(&owner),
            Value::Null
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn legacy_health_is_public_http_200_and_probes_actual_database() {
    let (directory, store, app) = fixture(false).await;
    assert!(!directory.path().starts_with("/app/data"));
    let (status, health) = request(&app, "GET", "/health", None, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(matches!(health["status"].as_str(), Some("ok" | "degraded")));
    assert_eq!(health["checks"]["db"]["status"], "ok");
    assert!(health["checks"]["db"]["latency_ms"].is_number());
    assert_eq!(health["checks"]["bot"]["message"], "standalone API mode");
    assert_eq!(health["checks"]["disk"]["status"], "ok");
    assert!(health["checks"]["disk"]["free_gb"].is_number());
    assert!(health["checks"]["disk"]["total_gb"].as_f64().unwrap() > 0.0);
    assert!(health["checks"]["disk"]["used_pct"].is_number());
    assert!(
        !health
            .to_string()
            .contains(directory.path().to_str().unwrap())
    );
    assert_eq!(
        request(&app, "POST", "/health", None, Value::Null).await.0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    store.close().await.unwrap();
}
