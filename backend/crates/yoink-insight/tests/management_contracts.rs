mod support;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use yoink_db::{Database, query, raw_sql};
use yoink_insight::{
    content::{Content, ContentSource, FetchOptions},
    provider::{Completion, Model, Route},
    *,
};

struct Unused;
impl ContentSource for Unused {
    fn fetch<'a>(
        &'a self,
        _: &'a str,
        _: FetchOptions<'a>,
        _: CancellationToken,
    ) -> FutureResult<'a, Content> {
        Box::pin(async { panic!("management cannot fetch content") })
    }
    fn research<'a>(
        &'a self,
        _: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, Vec<content::Citation>> {
        Box::pin(async { panic!("management cannot research") })
    }
}
#[derive(Default)]
struct Spy {
    calls: Mutex<Vec<Route>>,
    fail: AtomicBool,
}
impl Completion for Spy {
    fn complete<'a>(
        &'a self,
        _: &'a [Route],
        _: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async { panic!("management cannot generate buffered output") })
    }
    fn models<'a>(
        &'a self,
        route: &'a Route,
        _: CancellationToken,
    ) -> FutureResult<'a, Vec<Model>> {
        Box::pin(async move {
            self.calls.lock().await.push(route.clone());
            if self.fail.load(Ordering::SeqCst) {
                Err(Error::Code("provider-secret-fixture"))
            } else {
                Ok(vec![Model {
                    id: "fixture-model".into(),
                    supports_websearch: false,
                }])
            }
        })
    }
}
async fn fixture() -> (tempfile::TempDir, Insight, Arc<Spy>) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let db = Database::open(
        &dir.path().join("management.db"),
        true,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    raw_sql(include_str!(
        "../../yoink-storage/migrations-turso/0002_application.sql"
    ))
    .execute(&db)
    .await
    .unwrap();
    raw_sql(include_str!("../sql/native.sql"))
        .execute(&db)
        .await
        .unwrap();
    query("INSERT INTO users(id,role,created_at,updated_at) VALUES(1,'user',0,0),(2,'user',0,0),(3,'banned',0,0),(4,'owner',0,0),(5,'admin',0,0)").execute(&db).await.unwrap();
    let spy = Arc::new(Spy::default());
    let s = Insight::new(
        db,
        Config {
            owner_id: Some(1),
            model: "fallback-model".into(),
            gateway_base: "https://stored.test".into(),
            gateway_key: "stored-fixture-secret".into(),
            ..Default::default()
        },
        Arc::new(Unused),
        spy.clone(),
    )
    .unwrap();
    (dir, s, spy)
}
fn update() -> ByokUpdate {
    ByokUpdate {
        provider: "openai".into(),
        base_url: None,
        api_key: Some(" fixture-only-secret ".into()),
        model: " fixture-model ".into(),
    }
}
fn probe() -> ByokTest {
    ByokTest {
        provider: "openai".into(),
        base_url: None,
        api_key: Some("fixture-only-secret".into()),
    }
}
fn options() -> ModelProbeOptions {
    ModelProbeOptions {
        url: Some("https://unsaved.test/".into()),
        api_key: Some("unsaved-fixture-secret".into()),
    }
}
fn code<T>(result: Result<T, Error>, expected: &'static str) {
    assert!(matches!(result,Err(Error::Code(c)) if c == expected));
}

#[tokio::test]
async fn operator_probes_preserve_origins_and_do_not_forward_stored_keys_to_unsaved_urls() {
    let (_dir, mut service, spy) = fixture().await;
    service.config.mode = "openrouter".into();
    service.config.openrouter_key = "operator-fixture-key".into();
    service
        .test_gateway(1, CancellationToken::new())
        .await
        .unwrap();
    for body in [
        json!({"api_key":"unsaved-operator-key"}),
        json!({"url":"https://custom.example"}),
        json!({"url":"https://custom.example", "api_key":"custom-fixture-key"}),
    ] {
        service
            .test_gateway_with_options(
                1,
                serde_json::from_value(body).unwrap(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    let routes = spy.calls.lock().await;
    assert_eq!(routes.len(), 4);
    assert_eq!(routes[0].via, "openrouter");
    assert_eq!(routes[0].base, "https://openrouter.ai/api/v1");
    assert_eq!(routes[0].key, "operator-fixture-key");
    assert_eq!(routes[1].base, routes[0].base);
    assert_eq!(routes[1].key, "operator-fixture-key");
    assert_eq!(routes[2].via, "gateway");
    assert_eq!(routes[2].base, "https://custom.example/v1");
    assert!(routes[2].key.is_empty());
    assert_eq!(routes[3].key, "custom-fixture-key");
}

#[tokio::test]
async fn disabled_byok_owner_save_test_refresh_delete_and_nonowner_denials() {
    let (_dir, s, spy) = fixture().await;
    for uid in [1, 4] {
        let view = s.save_byok(uid, update()).await.unwrap();
        assert_eq!(view["enabled"], false);
        assert_eq!(view["model"], "fixture-model");
        assert!(!view.to_string().contains("fixture-only-secret"));
        assert_eq!(
            s.test_byok_input(uid, probe(), CancellationToken::new())
                .await
                .unwrap()["ok"],
            true
        );
        assert!(
            s.refresh_byok_models(uid, CancellationToken::new())
                .await
                .unwrap()["tested_at"]
                .is_string()
        );
        s.delete_byok(uid).await.unwrap();
        assert_eq!(s.byok_view(uid).await.unwrap()["has_config"], false);
    }
    let count = spy.calls.lock().await.len();
    for uid in [2, 5] {
        code(s.save_byok(uid, update()).await, "byok_disabled");
        code(
            s.test_byok_input(uid, probe(), CancellationToken::new())
                .await,
            "byok_disabled",
        );
        code(
            s.refresh_byok_models(uid, CancellationToken::new()).await,
            "byok_disabled",
        );
    }
    code(s.save_byok(3, update()).await, "access_denied");
    query("UPDATE users SET ban_until=? WHERE id=1")
        .bind(chrono::Utc::now() + chrono::Duration::minutes(1))
        .execute(&s.db)
        .await
        .unwrap();
    code(s.save_byok(1, update()).await, "access_denied");
    assert_eq!(spy.calls.lock().await.len(), count);
}

#[tokio::test]
async fn stored_key_reuse_and_missing_key_contracts_match_frontend() {
    let (_dir, s, spy) = fixture().await;
    let mut missing = update();
    missing.api_key = None;
    code(s.save_byok(1, missing).await, "api_key_required");
    let mut p = probe();
    p.api_key = None;
    assert_eq!(
        s.test_byok_input(1, p, CancellationToken::new())
            .await
            .unwrap()["error"],
        "api_key_required"
    );
    code(
        s.refresh_byok_models(1, CancellationToken::new()).await,
        "not_found",
    );
    s.save_byok(1, update()).await.unwrap();
    let mut changed = update();
    changed.provider = "anthropic".into();
    // Omission is the unchanged-key signal; explicit empty now clears.
    changed.api_key = None;
    changed.base_url = Some(" https://custom.test/v1 ".into());
    s.save_byok(1, changed).await.unwrap();
    let mut p = probe();
    p.provider = "custom_openai".into();
    p.base_url = Some("https://other.test/v1".into());
    p.api_key = None;
    assert_eq!(
        s.test_byok_input(1, p, CancellationToken::new())
            .await
            .unwrap()["ok"],
        true
    );
    let calls = spy.calls.lock().await;
    assert_eq!(calls.last().unwrap().key, "fixture-only-secret");
    assert_eq!(calls.last().unwrap().base, "https://other.test/v1");
    drop(calls);
    let mut short = update();
    short.api_key = Some("tiny".into());
    let view = s.save_byok(1, short).await.unwrap();
    assert_eq!(view["api_key_masked"], "***");
    assert!(!view.to_string().contains("tiny"));
}

#[tokio::test]
async fn failed_owner_save_and_refresh_are_redacted_and_do_not_grant_byok() {
    let (_dir, s, spy) = fixture().await;
    spy.fail.store(true, Ordering::SeqCst);
    let view = s.save_byok(1, update()).await.unwrap();
    assert_eq!(view["test_error"], "provider_probe_failed");
    assert!(view["tested_at"].is_null());
    assert!(!view.to_string().contains("provider-secret"));
    spy.fail.store(false, Ordering::SeqCst);
    s.refresh_byok_models(1, CancellationToken::new())
        .await
        .unwrap();
    let good = s.byok_view(1).await.unwrap();
    spy.fail.store(true, Ordering::SeqCst);
    let failed = s
        .refresh_byok_models(1, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(failed["models"], good["models"]);
    assert_eq!(failed["tested_at"], good["tested_at"]);
    assert_eq!(failed["models_fetched_at"], good["models_fetched_at"]);
    s.byok_toggle(1, Some(true)).await.unwrap();
    let failed = s.save_byok(2, update()).await.unwrap();
    assert_eq!(failed["test_error"], "provider_probe_failed");
    assert_eq!(s.settings(2).await.unwrap()["has_tldr_access"], false);
}

struct Mutation {
    db: Database,
    sql: &'static str,
}
impl Completion for Mutation {
    fn complete<'a>(
        &'a self,
        _: &'a [Route],
        _: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async { panic!("unexpected generation") })
    }
    fn models<'a>(&'a self, _: &'a Route, _: CancellationToken) -> FutureResult<'a, Vec<Model>> {
        Box::pin(async move {
            query(self.sql).execute(&self.db).await?;
            Ok(vec![Model {
                id: "stale-result".into(),
                supports_websearch: false,
            }])
        })
    }
}
#[tokio::test]
async fn byok_post_io_rechecks_owner_toggle_and_credential_generation() {
    for (uid, sql, expected) in [
        (
            4,
            "UPDATE users SET role='user' WHERE id=4",
            "byok_disabled",
        ),
        (
            1,
            "UPDATE users SET role='banned' WHERE id=1",
            "access_denied",
        ),
        (
            1,
            "DELETE FROM insight_user_byok WHERE user_id=1",
            "credential_changed",
        ),
        (
            1,
            "UPDATE insight_user_byok SET api_key='replacement-secret' WHERE user_id=1",
            "credential_changed",
        ),
    ] {
        let (_dir, mut s, _) = fixture().await;
        s.save_byok(uid, update()).await.unwrap();
        s.completion = Arc::new(Mutation {
            db: s.db.clone(),
            sql,
        });
        code(
            s.refresh_byok_models(uid, CancellationToken::new()).await,
            expected,
        );
        let row = query("SELECT models_json FROM insight_user_byok WHERE user_id=?")
            .bind(uid)
            .fetch_optional(&s.db)
            .await
            .unwrap();
        if let Some(row) = row {
            assert!(
                !row.try_get::<String, _>(0)
                    .unwrap()
                    .contains("stale-result")
            );
        }
    }
    let (_dir, mut s, _) = fixture().await;
    s.byok_toggle(1, Some(true)).await.unwrap();
    s.save_byok(2, update()).await.unwrap();
    s.completion = Arc::new(Mutation {
        db: s.db.clone(),
        sql: "UPDATE bot_settings SET value='false' WHERE key='insight_byok_enabled'",
    });
    code(
        s.refresh_byok_models(2, CancellationToken::new()).await,
        "byok_disabled",
    );
}

#[tokio::test]
async fn stored_test_key_generation_and_explicit_key_probe_are_distinct() {
    let (_dir, mut s, _) = fixture().await;
    s.save_byok(1, update()).await.unwrap();
    s.completion = Arc::new(Mutation {
        db: s.db.clone(),
        sql: "DELETE FROM insight_user_byok WHERE user_id=1",
    });
    let mut p = probe();
    p.api_key = None;
    code(
        s.test_byok_input(1, p, CancellationToken::new()).await,
        "credential_changed",
    );
    // Explicit-key tests are intentionally non-persistent and need no saved row.
    assert_eq!(
        s.test_byok_input(1, probe(), CancellationToken::new())
            .await
            .unwrap()["ok"],
        true
    );
}

#[tokio::test]
async fn model_override_roles_and_fallback_contracts() {
    let (_dir, s, spy) = fixture().await;
    assert_eq!(
        s.models_with_options(1, options(), CancellationToken::new())
            .await
            .unwrap()[0]["id"],
        "fixture-model"
    );
    let call = spy.calls.lock().await[0].clone();
    assert_eq!(call.base, "https://unsaved.test/v1");
    assert_eq!(call.key, "unsaved-fixture-secret");
    // Preserve the explicit empty-list contract; absent settings now bootstrap
    // from the configured default (covered separately below).
    query("INSERT INTO bot_settings(key,value,updated_at) VALUES('insight_tldr_allowed_models','[]',0)").execute(&s.db).await.unwrap();
    for uid in [2, 5] {
        assert_eq!(
            s.models_with_options(
                uid,
                ModelProbeOptions {
                    url: Some("not a URL".into()),
                    api_key: Some("ignore".into())
                },
                CancellationToken::new()
            )
            .await
            .unwrap(),
            json!([])
        );
    }
    code(
        s.test_gateway_with_options(2, options(), CancellationToken::new())
            .await,
        "access_denied",
    );
    let answer = s
        .test_gateway_with_options(5, options(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        answer,
        json!({"ok":true,"model_count":1,"url":"https://unsaved.test"})
    );
    assert!(!answer.to_string().contains("secret"));
    spy.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        s.models_with_options(1, options(), CancellationToken::new())
            .await
            .unwrap(),
        json!([{"id":"fallback-model"}])
    );
    let failed = s
        .test_gateway_with_options(5, options(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(failed["ok"], false);
    assert_eq!(failed["url"], "https://unsaved.test");
    assert!(!failed.to_string().contains("secret"));
    assert_eq!(s.config.gateway_base, "https://stored.test");
    let stored = s.admin_config(1).await.unwrap();
    assert_eq!(stored["gateway_base_url"], "https://stored.test");
}

#[tokio::test]
async fn model_post_io_role_ban_and_persisted_route_rechecks() {
    for (uid, sql, expected) in [
        (
            4,
            "UPDATE users SET role='user' WHERE id=4",
            "access_denied",
        ),
        (
            1,
            "UPDATE users SET role='banned' WHERE id=1",
            "access_denied",
        ),
        (
            1,
            "INSERT INTO bot_settings(key,value,updated_at) VALUES('insight_tldr_gateway_key','changed-secret',0)",
            "route_changed",
        ),
    ] {
        let (_dir, mut s, _) = fixture().await;
        s.completion = Arc::new(Mutation {
            db: s.db.clone(),
            sql,
        });
        code(s.models(uid, CancellationToken::new()).await, expected);
    }
    let (_dir, mut s, _) = fixture().await;
    s.completion = Arc::new(Mutation {
        db: s.db.clone(),
        sql: "UPDATE users SET role='user' WHERE id=5",
    });
    code(
        s.test_gateway_with_options(5, options(), CancellationToken::new())
            .await,
        "access_denied",
    );
}

async fn http(app: axum::Router, uid: i64, uri: String) -> (axum::http::StatusCode, Value) {
    use tower::ServiceExt;
    let mut req = axum::http::Request::builder()
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(yoink_insight::http::AuthenticatedUser(uid));
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn router(s: Insight) -> axum::Router {
    let shutdown = CancellationToken::new();
    let oauth = oauth::DeviceOAuth::new(s.clone(), "fixture-client".into(), None, shutdown.clone())
        .unwrap();
    yoink_insight::http::router(yoink_insight::http::HttpState {
        insight: s,
        oauth,
        shutdown,
    })
}
#[tokio::test]
async fn real_http_query_contract_sends_only_selected_credentials_and_normalizes_v1_once() {
    use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
    let seen = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    async fn models(
        State(seen): State<Arc<Mutex<Vec<Option<String>>>>>,
        headers: HeaderMap,
    ) -> Json<Value> {
        seen.lock().await.push(
            headers
                .get("authorization")
                .map(|h| h.to_str().unwrap().to_owned()),
        );
        Json(json!({"data":[{"id":"wire-model"}]}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen_server = seen.clone();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/models", get(models))
                .with_state(seen_server),
        )
        .await
        .unwrap();
    });
    let (_dir, mut s, _) = fixture().await;
    s.config.gateway_base = base.clone();
    s.completion = Arc::new(support::completion(Duration::from_secs(2), 4096).await);
    let app = router(s);
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("url", &format!("{base}/v1/"))
        .append_pair("api_key", "unsaved-fixture-key")
        .finish();
    let (status, body) = http(app.clone(), 1, format!("/models?{encoded}")).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body[0]["id"], "wire-model");
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("url", &base)
        .finish();
    let (_, body) = http(app.clone(), 5, format!("/config/test?{encoded}")).await;
    assert_eq!(body, json!({"ok":true,"model_count":1,"url":base}));
    http(app.clone(), 1, "/models?api_key=ignored-fixture-key".into()).await;
    assert_eq!(
        *seen.lock().await,
        vec![
            Some("Bearer unsaved-fixture-key".into()),
            None,
            Some("Bearer stored-fixture-secret".into())
        ]
    );
    let (status, _) = http(app, 2, "/config/test?url=http://127.0.0.1:1".into()).await;
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
    task.abort();
}

#[tokio::test]
async fn invalid_overrides_and_shutdown_do_not_echo_secrets_or_return_success() {
    let (_dir, s, spy) = fixture().await;
    for url in [
        "file:///tmp/data",
        "https://name:secret@host.test",
        "https://host.test?api_key=secret",
    ] {
        code(
            s.models_with_options(
                1,
                ModelProbeOptions {
                    url: Some(url.into()),
                    api_key: None,
                },
                CancellationToken::new(),
            )
            .await,
            "invalid_request",
        );
    }
    s.save_byok(1, update()).await.unwrap();
    let good = s.byok_view(1).await.unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    code(s.refresh_byok_models(1, cancel.clone()).await, "cancelled");
    assert_eq!(
        s.byok_view(1).await.unwrap()["tested_at"],
        good["tested_at"]
    );
    code(s.models(1, cancel.clone()).await, "cancelled");
    code(s.test_gateway(1, cancel).await, "cancelled");
    assert_eq!(spy.calls.lock().await.len(), 1);
}

#[tokio::test]
async fn save_probe_before_write_cannot_resurrect_or_overwrite_revoked_revision() {
    let (_dir, mut s, _) = fixture().await;
    s.completion = Arc::new(Mutation {
        db: s.db.clone(),
        sql: "UPDATE users SET role='banned' WHERE id=1",
    });
    code(s.save_byok(1, update()).await, "access_denied");
    assert!(
        query("SELECT 1 FROM insight_user_byok WHERE user_id=1")
            .fetch_optional(&s.db)
            .await
            .unwrap()
            .is_none()
    );
    for sql in [
        "DELETE FROM insight_user_byok WHERE user_id=1",
        "UPDATE insight_user_byok SET api_key='replacement-secret' WHERE user_id=1",
    ] {
        let (_dir, mut s, _) = fixture().await;
        s.save_byok(1, update()).await.unwrap();
        s.completion = Arc::new(Mutation {
            db: s.db.clone(),
            sql,
        });
        let mut changed = update();
        changed.api_key = Some("new-input-secret".into());
        code(s.save_byok(1, changed).await, "credential_changed");
        let row = query("SELECT api_key FROM insight_user_byok WHERE user_id=1")
            .fetch_optional(&s.db)
            .await
            .unwrap();
        if let Some(row) = row {
            assert_eq!(row.try_get::<String, _>(0).unwrap(), "replacement-secret");
        }
    }
}

#[tokio::test]
async fn failed_replacement_preserves_legacy_catalogue_but_never_validates_new_key() {
    let (_dir, s, spy) = fixture().await;
    s.byok_toggle(1, Some(true)).await.unwrap();
    let good = s.save_byok(2, update()).await.unwrap();
    spy.fail.store(true, Ordering::SeqCst);
    let mut replacement = update();
    replacement.api_key = Some("replacement-fixture-secret".into());
    let failed = s.save_byok(2, replacement).await.unwrap();
    assert_eq!(failed["models"], good["models"]);
    assert_eq!(failed["tested_at"], good["tested_at"]);
    assert_eq!(failed["models_fetched_at"], good["models_fetched_at"]);
    assert_eq!(failed["test_error"], "provider_probe_failed");
    assert!(!failed.to_string().contains("replacement-fixture"));
    assert_eq!(s.settings(2).await.unwrap()["has_tldr_access"], false);
}

struct Hanging {
    first: Arc<tokio::sync::Notify>,
}
impl Completion for Hanging {
    fn complete<'a>(
        &'a self,
        _: &'a [Route],
        _: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async { panic!("unexpected generation") })
    }
    fn models<'a>(&'a self, _: &'a Route, _: CancellationToken) -> FutureResult<'a, Vec<Model>> {
        Box::pin(async move {
            self.first.notify_one();
            std::future::pending().await
        })
    }
}
#[tokio::test]
async fn cancellation_during_save_keeps_previous_revision_and_timeout_is_bounded() {
    let (_dir, mut s, _) = fixture().await;
    let good = s.save_byok(1, update()).await.unwrap();
    let first = Arc::new(tokio::sync::Notify::new());
    s.completion = Arc::new(Hanging {
        first: first.clone(),
    });
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    let cancelling = tokio::spawn(async move {
        first.notified().await;
        child.cancel();
    });
    let mut replacement = update();
    replacement.api_key = Some("new-cancelled-secret".into());
    code(
        s.save_byok_with_cancel(1, replacement, cancel).await,
        "cancelled",
    );
    cancelling.await.unwrap();
    assert_eq!(s.byok_view(1).await.unwrap(), good);
    s.config.request_timeout = Duration::from_millis(20);
    let timed = s
        .test_gateway_with_options(5, options(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(timed["ok"], false);
    assert_eq!(timed["error"], "provider_probe_failed");
}

#[tokio::test]
async fn frontend_http_byok_put_test_refresh_disabled_owner_and_failure_shapes() {
    use tower::ServiceExt;
    let (_dir, s, spy) = fixture().await;
    let app = router(s);
    async fn send(
        app: axum::Router,
        uid: i64,
        method: &str,
        path: &str,
        body: Value,
    ) -> (axum::http::StatusCode, Value) {
        let mut req = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        req.extensions_mut()
            .insert(yoink_insight::http::AuthenticatedUser(uid));
        let response = app.oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    let body = json!({"provider":"openai","api_key":"fixture-only-secret","model":"fixture-model"});
    let (status, view) = send(app.clone(), 1, "PUT", "/byok/me", body.clone()).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(view["enabled"], false);
    assert!(!view.to_string().contains("fixture-only-secret"));
    let (status, view) = send(
        app.clone(),
        1,
        "POST",
        "/byok/me/test",
        json!({"provider":"openai"}),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(view["ok"], true);
    let (status, _) = send(
        app.clone(),
        1,
        "POST",
        "/byok/me/refresh-models",
        json!(null),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let (status, view) = send(app.clone(), 2, "PUT", "/byok/me", body).await;
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(view["detail"], "byok_disabled");
    spy.fail.store(true, Ordering::SeqCst);
    let (_, view) = send(
        app,
        1,
        "POST",
        "/byok/me/test",
        json!({"provider":"openai"}),
    )
    .await;
    assert_eq!(
        view,
        json!({"ok":false,"error":"provider_probe_failed","models":[]})
    );
}

#[tokio::test]
async fn absent_allowlist_bootstraps_default_without_provider_io() {
    let (_directory, service, provider) = fixture().await;
    let models = service.models(2, CancellationToken::new()).await.unwrap();
    assert_eq!(models, json!([{"id":"fallback-model"}]));
    assert!(provider.calls.lock().await.is_empty());
}

#[tokio::test]
async fn committed_changes_notify_all_clones_including_failed_readiness() {
    let (_directory, service, provider) = fixture().await;
    let notifications = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = notifications.clone();
    let service = service.with_access_change_callback(Arc::new(move |user| {
        captured.lock().unwrap().push(user);
    }));
    let cloned = service.clone().with_content(Arc::new(Unused));
    cloned.byok_toggle(1, Some(true)).await.unwrap();
    provider.fail.store(true, Ordering::SeqCst);
    cloned.save_byok(2, update()).await.unwrap();
    cloned
        .refresh_byok_models(2, CancellationToken::new())
        .await
        .unwrap();
    cloned.delete_byok(2).await.unwrap();
    cloned.delete_byok(2).await.unwrap(); // no row, no committed change
    assert_eq!(
        *notifications.lock().unwrap(),
        vec![None, Some(2), Some(2), Some(2)]
    );
    assert!(cloned.grant(1, 999, "en").await.is_err());
    assert_eq!(notifications.lock().unwrap().len(), 4);
    cloned.grant(1, 2, "ja").await.unwrap();
    let before = cloned.access_list(1).await.unwrap()[0].clone();
    let after = cloned.update_access(1, 2, "de").await.unwrap();
    assert_eq!(after["granted_by"], before["granted_by"]);
    assert_eq!(after["granted_at"], before["granted_at"]);
    assert_eq!(after["lang"], "de");
    cloned.revoke(1, 2).await.unwrap();
    assert!(matches!(
        cloned.revoke(1, 2).await,
        Err(Error::Code("not_found"))
    ));
    assert_eq!(
        *notifications.lock().unwrap(),
        vec![None, Some(2), Some(2), Some(2), Some(2), Some(2)]
    );
}

#[tokio::test]
async fn credential_patch_presence_and_openrouter_only_config() {
    let (_directory, service, _) = fixture().await;
    let mut patch = json!({"allowed_models":["fallback-model"],"default_model":"fallback-model","gateway_base_url":"","ai_route_mode":"openrouter"});
    let unchanged = service.update_config(1, patch.clone()).await.unwrap();
    assert_eq!(unchanged["gateway_api_key"], "");
    assert_eq!(unchanged["gateway_api_key_set"], true);
    patch["gateway_api_key"] = json!("");
    let cleared = service.update_config(1, patch).await.unwrap();
    assert_eq!(cleared["gateway_api_key_set"], false);
    assert!(!cleared.to_string().contains("stored-fixture-secret"));
}

#[tokio::test]
async fn byok_empty_key_clears_readiness_and_never_reuses_stored_key() {
    let (_directory, service, provider) = fixture().await;
    service.byok_toggle(1, Some(true)).await.unwrap();
    service.save_byok(2, update()).await.unwrap();
    let mut clear = update();
    clear.api_key = Some(String::new());
    let view = service.save_byok(2, clear).await.unwrap();
    assert_eq!(view["api_key_set"], false);
    assert_eq!(view["api_key_masked"], Value::Null);
    assert_eq!(service.settings(2).await.unwrap()["has_tldr_access"], false);
    assert_eq!(provider.calls.lock().await.len(), 1);
    let mut empty_probe = probe();
    empty_probe.api_key = Some(String::new());
    assert_eq!(
        service
            .test_byok_input(2, empty_probe, CancellationToken::new())
            .await
            .unwrap()["error"],
        "api_key_required"
    );
    assert_eq!(provider.calls.lock().await.len(), 1);
}

#[tokio::test]
async fn byok_settings_languages_prompts_masking_and_alias_partial_merge() {
    let (_directory, service, _) = fixture().await;
    service.byok_toggle(1, Some(true)).await.unwrap();
    service.save_byok(2, update()).await.unwrap();
    for language in ["en", "ru", "uk", "de", "fr", "es", "zh", "ja"] {
        let settings = service.update_settings(2, serde_json::from_value(json!({"lang":language,"github_token":"fixture-read-token","prompts":{"about":"inactive", "tldr":"active"}})).unwrap()).await.unwrap();
        assert_eq!(settings["lang"], language);
        assert_eq!(settings["github_token_set"], true);
        assert_eq!(settings["tldr_allowed_models"], json!([]));
        assert_eq!(settings["tldr_model"], Value::Null);
        assert_eq!(settings["prompts"], json!({"tldr":"active"}));
        assert!(!settings.to_string().contains("fixture-read-token"));
    }
    assert!(
        service
            .update_settings(5, serde_json::from_value(json!({"lang":"en"})).unwrap())
            .await
            .is_err()
    );
    let alias = service
        .save_alias(
            2,
            None,
            serde_json::from_value(
                json!({"aliases":"привет-мир", "prompt":"custom", "domains":"[ab].example"}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let merged = service
        .save_alias(
            2,
            Some(alias.id),
            serde_json::from_value(json!({"prompt":"updated"})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(merged.aliases, alias.aliases);
    assert_eq!(merged.domains, alias.domains);
    assert_eq!(merged.prompt.as_deref(), Some("updated"));
    service.delete_byok(2).await.unwrap();
    assert!(service.aliases(2).await.is_err());
    // Existing ownership, not a continuing TLDR grant, authorizes edit/delete.
    service
        .save_alias(
            2,
            Some(alias.id),
            serde_json::from_value(json!({"prompt":"still owned"})).unwrap(),
        )
        .await
        .unwrap();
    service.delete_alias(2, alias.id).await.unwrap();
}
