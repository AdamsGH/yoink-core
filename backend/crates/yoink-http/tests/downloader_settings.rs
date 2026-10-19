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

#[tokio::test]
async fn history_lists_imported_and_native_downloads_with_actor_filters() {
    let (_d, app, store) = history_fixture().await;
    let owner = login(&app, 42).await;
    let other = login(&app, 43).await;
    yoink_db::raw_sql(include_str!("sql/history_seed.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let path = "/api/v1/dl/downloads";
    assert_eq!(
        request(&app, "GET", path, None, Value::Null).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = request(&app, "GET", path, Some(&owner), Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 2);
    assert_eq!(body["items"][0]["url"], "https://x.com/example/status/123");
    assert_eq!(
        body["items"][0]["id"],
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(body["items"][0]["status"], "error");
    assert_eq!(body["items"][0]["created_at"], "2026-10-19T00:00:00Z");
    let (_, other_body) = request(&app, "GET", path, Some(&other), Value::Null).await;
    assert_eq!(other_body["total"], 0);
    for query in [
        "?domain=x.com",
        "?status=error",
        "?search=EXAMPLE%2FSTATUS",
        "?date_from=2026-10-19",
        "?limit=1&offset=1",
    ] {
        let (status, body) = request(
            &app,
            "GET",
            &format!("{path}{query}"),
            Some(&owner),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{query}: {body}");
        assert_eq!(
            body["items"].as_array().unwrap().len(),
            1,
            "{query}: {body}"
        );
    }
    for query in ["?limit=0", "?offset=-1", "?date_from=bad", "?status=bad"] {
        assert_eq!(
            request(
                &app,
                "GET",
                &format!("{path}{query}"),
                Some(&owner),
                Value::Null
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let (status, body) = request(
        &app,
        "GET",
        "/api/v1/dl/downloads/domains",
        Some(&owner),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["domains"], json!(["example.com", "x.com"]));
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/v1/dl/downloads/1/retry",
            Some(&other),
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
async fn fixture() -> (tempfile::TempDir, Router) {
    let (d, app, _) = history_fixture().await;
    (d, app)
}
async fn history_fixture() -> (tempfile::TempDir, Router, Arc<Store>) {
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
            .install_downloader_settings(yoink_downloads::UserSettingsService::new(
                store.database().clone(),
                Arc::new(IdentityStore::new(
                    store.database().clone(),
                    Some(42),
                    false
                ))
            ))
            .is_ok()
    );
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
    (d, router_with_auth(system, Some(state)), store)
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
async fn downloader_frontend_settings_get_patch_credentials_null_pool_and_closed_fields() {
    let (_d, app) = fixture().await;
    let owner = login(&app, 42).await;
    let user = login(&app, 43).await;
    let path = "/api/v1/dl/settings";
    assert_eq!(
        request(&app, "GET", path, None, Value::Null).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, original) = request(&app, "GET", path, Some(&user), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(original["split_size"], 2043000000_i64);
    assert_eq!(original["keyboard"], "2x3");
    assert_eq!(original["has_pool_access"], false);
    let (status, saved) = request(&app,"PATCH",path,Some(&user),json!({"proxy_url":"https://fixture:secret@proxy.example:3128", "proxy_enabled":true,"use_pool_cookies":true,"mediainfo":true,"subs_enabled":true,"subs_always_ask":true,"keyboard":"OFF","split_size":1610612736_i64})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["has_pool_access"], false);
    assert_eq!(saved["mediainfo"], true);
    assert_eq!(
        saved["proxy_url"],
        "https://fixture:secret@proxy.example:3128"
    );
    let (status, saved) =
        request(&app, "PATCH", path, Some(&user), json!({"proxy_url":null})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        saved["proxy_url"],
        "https://fixture:secret@proxy.example:3128"
    );
    let (_, own) = request(&app, "GET", path, Some(&owner), Value::Null).await;
    assert_eq!(own["proxy_url"], Value::Null);
    assert_eq!(own["has_pool_access"], true);
    for bad in [
        json!({"args_json":{"foo":"bar"}}),
        json!({"user_id":42}),
        json!({"has_pool_access":true}),
        json!({"split_size":1}),
        json!({"proxy_url":"file:///etc/passwd"}),
    ] {
        assert_eq!(
            request(&app, "PATCH", path, Some(&user), bad).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}

#[tokio::test]
async fn downloader_admin_limits_require_fresh_authority_and_patch_atomically() {
    let (_dir, app) = fixture().await;
    let owner = login(&app, 42).await;
    let user = login(&app, 43).await;
    let path = "/api/v1/dl/admin/settings";
    assert_eq!(
        request(&app, "GET", path, None, Value::Null).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "GET", path, Some(&user), Value::Null).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, initial) = request(&app, "GET", path, Some(&owner), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(initial["max_playlist_count"], 50);
    let patch = json!({"download_retries":2,"download_timeout":120,"max_file_size_mb":512,"rate_limit_per_minute":2,"rate_limit_per_hour":20,"rate_limit_per_day":200,"max_playlist_count":10});
    let (status, saved) = request(&app, "PATCH", path, Some(&owner), patch.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved, patch);
    for bad in [
        json!({"download_timeout":0,"max_file_size_mb":100}),
        json!({"download_retries":-1}),
        json!({"max_playlist_count":501}),
        json!({"unexpected":true}),
    ] {
        assert_eq!(
            request(&app, "PATCH", path, Some(&owner), bad).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            request(&app, "GET", path, Some(&owner), Value::Null)
                .await
                .1,
            saved
        );
    }
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
    assert_eq!(
        request(&app, "GET", path, Some(&user), Value::Null).await.0,
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
    assert_eq!(
        request(
            &app,
            "PATCH",
            path,
            Some(&user),
            json!({"max_file_size_mb":256})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "GET", path, Some(&owner), Value::Null)
            .await
            .1,
        saved
    );
}

#[tokio::test]
async fn settings_http_etag_no_store_and_atomic_conflict() {
    let (_d, app) = fixture().await;
    let token = login(&app, 43).await;
    let get = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/dl/settings")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(get.headers()["cache-control"], "no-store");
    let etag = get.headers()["etag"].to_str().unwrap().to_owned();
    let make = || {
        Request::builder()
            .method("PATCH")
            .uri("/api/v1/dl/settings")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("if-match", &etag)
            .body(Body::from(r#"{"quality":"720","mediainfo":true}"#))
            .unwrap()
    };
    let first = app.clone().oneshot(make()).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_ne!(first.headers()["etag"], etag);
    let replay = app.clone().oneshot(make()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::CONFLICT);
    assert_eq!(replay.headers()["cache-control"], "no-store");
    let (_, current) = request(
        &app,
        "GET",
        "/api/v1/dl/settings",
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(current["quality"], "720");
    assert_eq!(current["mediainfo"], true);
}

#[tokio::test]
async fn proxy_patch_and_get_share_the_maximum_persisted_bound() {
    let (_dir, app) = fixture().await;
    let token = login(&app, 43).await;
    let prefix = "https://";
    let suffix = "@proxy.example:3128";
    let proxy = format!(
        "{prefix}{}{suffix}",
        "u".repeat(yoink_downloads::profile::MAX_PROXY_URL_BYTES - prefix.len() - suffix.len())
    );
    assert_eq!(proxy.len(), yoink_downloads::profile::MAX_PROXY_URL_BYTES);
    let (status, patched) = request(
        &app,
        "PATCH",
        "/api/v1/dl/settings",
        Some(&token),
        json!({"proxy_url":proxy}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["proxy_url"], proxy);
    let (status, fetched) = request(
        &app,
        "GET",
        "/api/v1/dl/settings",
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["proxy_url"], proxy);
    let too_long = format!(
        "{prefix}u{}{suffix}",
        "u".repeat(yoink_downloads::profile::MAX_PROXY_URL_BYTES - prefix.len() - suffix.len())
    );
    assert_eq!(
        request(
            &app,
            "PATCH",
            "/api/v1/dl/settings",
            Some(&token),
            json!({"proxy_url":too_long})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (status, unchanged) = request(
        &app,
        "GET",
        "/api/v1/dl/settings",
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unchanged["proxy_url"], proxy);
}
