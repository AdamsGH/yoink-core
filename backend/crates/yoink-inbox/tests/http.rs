use axum::{
    Extension,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use std::sync::Arc;
use tower::ServiceExt;
use yoink_db::{Database, Executor, query};
use yoink_inbox::{
    Error,
    contracts::{Authorization, Call, Insight},
    github::GithubClient,
    http::{Actor, HttpState, router},
    store::Store,
};
struct Auth;
impl Authorization for Auth {
    fn require<'a>(&'a self, user: i64, _feature: &'a str) -> Call<'a, ()> {
        Box::pin(async move {
            if user == 3 {
                Err(Error::Forbidden)
            } else {
                Ok(())
            }
        })
    }
    fn require_admin(&self, user: i64) -> Call<'_, ()> {
        Box::pin(async move {
            if user == 2 {
                Ok(())
            } else {
                Err(Error::Forbidden)
            }
        })
    }
}
struct FakeInsight;
impl Insight for FakeInsight {
    fn github_token(&self, _user: i64, _write: bool) -> Call<'_, Option<String>> {
        Box::pin(async { Ok(None) })
    }
    fn complete<'a>(&'a self, _user: i64, _prompt: &'a str) -> Call<'a, String> {
        Box::pin(async { Ok("{}".into()) })
    }
    fn language(&self, _user: i64) -> Call<'_, String> {
        Box::pin(async { Ok("en".into()) })
    }
}
async fn state() -> (tempfile::TempDir, HttpState) {
    let d = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let db = Database::open(
        &d.path().join("test.db"),
        true,
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap();
    db.batch(include_str!(
        "../../yoink-storage/migrations-turso/0002_application.sql"
    ))
    .await
    .unwrap();
    db.batch(include_str!("../sql/durable_jobs.sql"))
        .await
        .unwrap();
    db.batch(include_str!("../sql/github_mutations.sql"))
        .await
        .unwrap();
    db.batch(include_str!("../sql/notification_outbox.sql"))
        .await
        .unwrap();
    for id in 1..=3 {
        query("INSERT INTO users(id,created_at,updated_at) VALUES(?,1,1)")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
    }
    (
        d,
        HttpState {
            store: Store::new(db),
            auth: Arc::new(Auth),
            insight: Arc::new(FakeInsight),
            github: Arc::new(GithubClient::new(std::time::Duration::from_secs(2), None).unwrap()),
        },
    )
}
#[tokio::test]
async fn transport_identity_and_admin_authorization() {
    let (_d, s) = state().await;
    let app = router(s.clone());
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/items")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let r = app
        .clone()
        .layer(Extension(Actor(3)))
        .oneshot(
            Request::builder()
                .uri("/items")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = app
        .clone()
        .layer(Extension(Actor(1)))
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/admin/classify-prompt")
                .header("Content-Type", "application/json")
                .body(Body::from("{\"classify_system_prompt\":\"not allowed\"}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = app
        .layer(Extension(Actor(2)))
        .oneshot(
            Request::builder()
                .uri("/admin/classify-prompt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}
#[tokio::test]
async fn frontend_item_contract_and_durable_post() {
    let (_d, s) = state().await;
    let app = router(s.clone()).layer(Extension(Actor(1)));
    let req = || {
        Request::builder()
            .method("POST")
            .uri("/items")
            .header("Content-Type", "application/json")
            .body(Body::from(
                "{\"url\":\"https://example.com/a?utm_source=x\",\"source\":\"web\"}",
            ))
            .unwrap()
    };
    let r = app.clone().oneshot(req()).await.unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let body = to_bytes(r.into_body(), 65536).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["normalized_url"], "https://example.com/a");
    assert!(v["categories"].is_array());
    assert!(v["created_at"].is_string());
    assert!(v.get("user_id").is_none());
    assert!(v.get("content_text").is_none());
    let r = app.oneshot(req()).await.unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let n: i64 = query("SELECT count(*) AS n FROM inbox_jobs")
        .fetch_one(&s.store.db)
        .await
        .unwrap()
        .try_get("n")
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn omitted_is_local_is_source_default_without_github_credentials() {
    let (_d, s) = state().await;
    let app = router(s.clone()).layer(Extension(Actor(1)));
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/folders")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"name":"Local by default"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let v: serde_json::Value =
        serde_json::from_slice(&to_bytes(r.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(v["name"], "Local by default");
    assert!(v["gh_list_id"].is_null());
    assert!(s.store.active_mutations(1).await.unwrap().is_empty());
    let r = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/folders")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"name":"Invalid","is_local":"false"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

async fn request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let r = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = r.status();
    let bytes = to_bytes(r.into_body(), 65536).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}
#[tokio::test]
async fn source_folder_local_patch_put_delete_and_full_rule_replacement() {
    use serde_json::json;
    use yoink_inbox::catalog::Catalog;
    let (_d, s) = state().await;
    let folder = s
        .store
        .save_catalogue(
            1,
            Catalog::Folder,
            None,
            &json!({"name":"Old","icon":"old","description":"old","is_pinned":true,"sort_order":7}),
            1,
        )
        .await
        .unwrap();
    let id = folder["id"].as_i64().unwrap();
    query("UPDATE inbox_gh_folders SET gh_list_id='L1' WHERE id=?")
        .bind(id)
        .execute(&s.store.db)
        .await
        .unwrap();
    let app = router(s.clone()).layer(Extension(Actor(1)));
    let (status,v)=request(app.clone(),"PATCH",&format!("/folders/{id}"),json!({"name":"Renamed locally","description":null,"icon":"ignored","slug":"ignored","sort_order":"8.0"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["description"], "old");
    assert_eq!(v["icon"], "old");
    assert_eq!(v["slug"], "old");
    assert_eq!(v["sort_order"], 8);
    assert_eq!(v["gh_list_id"], "L1");
    let (status, v) = request(
        app.clone(),
        "PUT",
        &format!("/folders/{id}"),
        json!({"name":"New metadata"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["slug"], "new-metadata");
    assert!(v["icon"].is_null());
    assert!(v["description"].is_null());
    assert_eq!(v["sort_order"], 8);
    assert_eq!(v["is_pinned"], true);
    assert_eq!(v["gh_list_id"], "L1");
    assert!(s.store.active_mutations(1).await.unwrap().is_empty());
    let (status, _) = request(
        app.clone(),
        "DELETE",
        &format!("/folders/{id}"),
        json!(null),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_,rule)=request(app.clone(),"POST","/rules",json!({"name":"R","trigger":"item_ingested","enabled":false,"priority":7,"conditions":[{}]})).await;
    let rid = rule["id"].as_i64().unwrap();
    let (status, v) = request(
        app.clone(),
        "PATCH",
        &format!("/rules/{rid}"),
        json!({"name":"Patched"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["priority"], 7);
    assert_eq!(v["enabled"], false);
    let (status, v) = request(
        app,
        "PUT",
        &format!("/rules/{rid}"),
        json!({"name":"Replaced","trigger":"item_classified"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["priority"], 100);
    assert_eq!(v["enabled"], true);
    assert!(v["conditions"].is_null());
}
#[tokio::test]
async fn remote_create_requires_key_before_admission_and_prompt_limits_are_unicode() {
    use serde_json::json;
    let (_d, s) = state().await;
    let app = router(s.clone()).layer(Extension(Actor(1)));
    let (status, _) = request(
        app.clone(),
        "POST",
        "/folders",
        json!({"name":"Remote","is_local":false}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(s.store.active_mutations(1).await.unwrap().is_empty());
    let (status, _) = request(
        app.clone(),
        "PUT",
        "/settings/classify-hint",
        json!({"classify_user_hint":"🙂".repeat(4001)}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = request(
        app,
        "POST",
        "/items",
        json!({"url":"example.com/no-scheme"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let admin = router(s).layer(Extension(Actor(2)));
    let (status, _) = request(
        admin,
        "PUT",
        "/admin/classify-prompt",
        json!({"classify_system_prompt":"🙂".repeat(8001)}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
