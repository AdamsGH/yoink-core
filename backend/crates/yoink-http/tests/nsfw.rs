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
async fn nsfw_frontend_crud_check_import_export_and_current_db_authorization() {
    let (_dir, app) = fixture().await;
    let owner = login(&app, 42).await;
    let user = login(&app, 43).await;
    let base = "/api/v1/dl/nsfw";
    assert_eq!(
        request(&app, "GET", &format!("{base}/domains"), None, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("{base}/domains"),
            Some(&user),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, row) = request(
        &app,
        "POST",
        &format!("{base}/domains"),
        Some(&owner),
        json!({"domain":"www.Adult.example","note":"kept"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(row["domain"], "adult.example");
    assert!(row["created_at"].is_string());
    let id = row["id"].as_i64().unwrap();
    assert_eq!(
        request(
            &app,
            "PATCH",
            &format!("{base}/domains/{id}"),
            Some(&owner),
            json!({"domain":"adult.example","note":""})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, row) = request(
        &app,
        "POST",
        &format!("{base}/check"),
        Some(&owner),
        json!({"url":"https://sub.adult.example/"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(row["matched_domain"], "adult.example");
    assert_eq!(row["is_nsfw"], true);
    let (_, imported) = request(
        &app,
        "POST",
        &format!("{base}/import"),
        Some(&owner),
        json!({"domains":[{"domain":"adult.example"}],"keywords":[{"keyword":"adult content"}]}),
    )
    .await;
    assert_eq!(imported, json!({"domains_added":0,"keywords_added":1}));
    let (_, checked) = request(
        &app,
        "POST",
        &format!("{base}/check"),
        Some(&owner),
        json!({"url":"https://example.com/adult-content"}),
    )
    .await;
    assert_eq!(checked["matched_keyword"], "adult content");
    assert_eq!(checked["matched_keywords"], json!(["adult content"]));
    let (_, exported) = request(
        &app,
        "GET",
        &format!("{base}/export"),
        Some(&owner),
        Value::Null,
    )
    .await;
    assert_eq!(exported["domains"].as_array().unwrap().len(), 1);
    assert_eq!(exported["keywords"].as_array().unwrap().len(), 1);
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("{base}/domains/{id}"),
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
            "PATCH",
            "/api/v1/users/43",
            Some(&owner),
            json!({"role":"moderator"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let moderator = login(&app, 43).await;
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("{base}/domains"),
            Some(&moderator),
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    // A still-valid moderator JWT cannot overrule a committed DB demotion.
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
            "GET",
            &format!("{base}/domains"),
            Some(&moderator),
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}
