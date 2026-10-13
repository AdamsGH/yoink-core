//! Macro compiled against the actual native Inbox crate.
mod support;
use yoink_inbox::{Error, contracts};
yoink_insight::impl_inbox_adapter!(pub ActualInboxInsight);
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use yoink_db::{Database, query, raw_sql};
use yoink_insight::{
    Config, FutureResult, Insight,
    provider::{Completion, Route},
};
struct Fixtures;
impl Completion for Fixtures {
    fn complete<'a>(
        &'a self,
        routes: &'a [Route],
        prompt: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async move {
            assert_eq!(routes[0].model, "fixture-model");
            assert_eq!(prompt, "categorize item");
            Ok("{\"categories\":[],\"summary\":\"fixture\"}".into())
        })
    }
}
async fn fixture() -> (
    tempfile::TempDir,
    Insight,
    ActualInboxInsight,
    CancellationToken,
) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let db = Database::open(&dir.path().join("inbox.db"), true, Duration::from_secs(3))
        .await
        .unwrap();
    raw_sql(include_str!(
        "../../yoink-storage/migrations-turso/0002_application.sql"
    ))
    .execute(&db)
    .await
    .unwrap();
    query("INSERT INTO users(id,role,language,created_at,updated_at) VALUES(1,'owner','en',0,0),(2,'user','ru',0,0)").execute(&db).await.unwrap();
    query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(2,'inbox','gh_sync',1),(2,'inbox','classify',1)").execute(&db).await.unwrap();
    let s = Insight::completion_only(
        db,
        Config {
            model: "fixture-model".into(),
            owner_id: Some(1),
            ..Default::default()
        },
        Arc::new(Fixtures),
    )
    .unwrap();
    let shutdown = CancellationToken::new();
    let adapter = ActualInboxInsight::new(s.clone(), shutdown.clone());
    (dir, s, adapter, shutdown)
}
#[tokio::test]
async fn language_and_completion_use_concrete_inbox_trait() {
    let (_dir, s, a, _) = fixture().await;
    let adapter: &dyn contracts::Insight = &a;
    assert_eq!(adapter.language(2).await.unwrap(), "en");
    assert_eq!(
        adapter.complete(2, "categorize item").await.unwrap(),
        "{\"categories\":[],\"summary\":\"fixture\"}"
    );
    query("INSERT INTO insight_user_settings(user_id,lang) VALUES(2,'en')")
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(adapter.language(2).await.unwrap(), "en");
}
#[tokio::test]
async fn separate_tokens_current_grants_and_bans() {
    let (_dir, s, a, _) = fixture().await;
    let adapter: &dyn contracts::Insight = &a;
    assert!(adapter.github_token(2, false).await.unwrap().is_none());
    query("INSERT INTO insight_user_settings(user_id,github_token,github_token_public_repo) VALUES(2,'read-fixture-secret','write-fixture-secret')").execute(&s.db).await.unwrap();
    assert_eq!(
        adapter.github_token(2, false).await.unwrap().as_deref(),
        Some("read-fixture-secret")
    );
    assert!(matches!(
        adapter.github_token(2, true).await,
        Err(Error::Forbidden)
    ));
    query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(2,'inbox','gh_write',1)").execute(&s.db).await.unwrap();
    assert_eq!(
        adapter.github_token(2, true).await.unwrap().as_deref(),
        Some("write-fixture-secret")
    );
    assert!(!format!("{a:?}").contains("fixture-secret"));
    query("UPDATE users SET role='banned' WHERE id=2")
        .execute(&s.db)
        .await
        .unwrap();
    assert!(matches!(
        adapter.github_token(2, false).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(adapter.language(2).await, Err(Error::Forbidden)));
}
#[tokio::test]
async fn no_read_token_promotion_and_expired_grants() {
    let (_dir, s, a, _) = fixture().await;
    query(
        "INSERT INTO insight_user_settings(user_id,github_token) VALUES(2,'read-fixture-secret')",
    )
    .execute(&s.db)
    .await
    .unwrap();
    query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(2,'inbox','gh_write',1)").execute(&s.db).await.unwrap();
    let adapter: &dyn contracts::Insight = &a;
    assert!(adapter.github_token(2, true).await.unwrap().is_none());
    query("UPDATE users SET role='restricted' WHERE id=2")
        .execute(&s.db)
        .await
        .unwrap();
    query("UPDATE user_permissions SET expires_at=0 WHERE user_id=2")
        .execute(&s.db)
        .await
        .unwrap();
    assert!(matches!(
        adapter.github_token(2, false).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        adapter.complete(2, "categorize item").await,
        Err(Error::Forbidden)
    ));
}
#[tokio::test]
async fn adapter_shutdown_denies_credentials_and_completion() {
    let (_dir, _s, a, shutdown) = fixture().await;
    shutdown.cancel();
    let adapter: &dyn contracts::Insight = &a;
    assert!(matches!(
        adapter.github_token(2, false).await,
        Err(Error::Cancelled)
    ));
    assert!(matches!(adapter.language(2).await, Err(Error::Cancelled)));
    assert!(matches!(
        adapter.complete(2, "categorize item").await,
        Err(Error::Cancelled)
    ));
}
#[tokio::test]
async fn imported_ai_language_and_organise_grant_are_supported() {
    let (_dir, s, a, _) = fixture().await;
    query("UPDATE users SET language='en' WHERE id=2")
        .execute(&s.db)
        .await
        .unwrap();
    query("INSERT INTO insight_access(user_id,lang,granted_by,granted_at) VALUES(2,'ru',1,0)")
        .execute(&s.db)
        .await
        .unwrap();
    query("DELETE FROM user_permissions WHERE user_id=2 AND feature='classify'")
        .execute(&s.db)
        .await
        .unwrap();
    query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(2,'inbox','gh_organise',1)").execute(&s.db).await.unwrap();
    let adapter: &dyn contracts::Insight = &a;
    assert_eq!(adapter.language(2).await.unwrap(), "ru");
    assert!(adapter.complete(2, "categorize item").await.is_ok());
}

#[tokio::test]
async fn ingest_only_and_language_conflict() {
    let (_dir, s, a, _) = fixture().await;
    query("DELETE FROM user_permissions WHERE user_id=2")
        .execute(&s.db)
        .await
        .unwrap();
    query("INSERT INTO insight_user_settings(user_id,lang,github_token) VALUES(2,'de','read')")
        .execute(&s.db)
        .await
        .unwrap();
    let adapter: &dyn contracts::Insight = &a;
    assert_eq!(
        adapter.github_token(2, false).await.unwrap().as_deref(),
        Some("read")
    );
    assert!(matches!(
        adapter.github_token(2, true).await,
        Err(Error::Forbidden)
    ));
    assert_eq!(adapter.language(2).await.unwrap(), "en");
    query("INSERT INTO insight_access(user_id,lang,granted_by,granted_at) VALUES(2,'ru',1,0)")
        .execute(&s.db)
        .await
        .unwrap();
    assert_eq!(adapter.language(2).await.unwrap(), "ru");
    query("UPDATE users SET role='restricted' WHERE id=2")
        .execute(&s.db)
        .await
        .unwrap();
    assert!(matches!(
        adapter.github_token(2, false).await,
        Err(Error::Forbidden)
    ));
}

#[test]
fn sanitized_failure_mapping_against_real_inbox() {
    for code in [
        "auth_failed",
        "quota_exceeded",
        "insufficient_credits",
        "model_not_found",
        "no_route",
        "unsafe_endpoint",
        "parse_failed",
        "llm_error",
        "response_too_large",
    ] {
        assert!(matches!(
            ActualInboxInsight::map_error(yoink_insight::Error::Code(code)),
            Error::Completion(_)
        ));
    }
    for code in ["provider_unavailable", "timeout"] {
        assert!(matches!(
            ActualInboxInsight::map_error(yoink_insight::Error::Code(code)),
            Error::External
        ));
    }
    assert!(matches!(
        ActualInboxInsight::map_error(yoink_insight::Error::Code("provider_rate_limited")),
        Error::RateLimited
    ));
    assert!(matches!(
        ActualInboxInsight::map_error(yoink_insight::Error::Code("cancelled")),
        Error::Cancelled
    ));
    assert!(matches!(
        ActualInboxInsight::map_error(yoink_insight::Error::Code("access_denied")),
        Error::Forbidden
    ));
}

struct RouteCheck {
    byok: bool,
}
impl Completion for RouteCheck {
    fn complete<'a>(
        &'a self,
        routes: &'a [Route],
        _: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async move {
            assert_eq!(routes[0].via, if self.byok { "byok" } else { "gateway" });
            assert_eq!(
                routes[0].model,
                if self.byok {
                    "own-model"
                } else {
                    "fixture-model"
                }
            );
            assert_eq!(routes.len(), 1);
            Ok("ok".into())
        })
    }
}
#[tokio::test]
async fn completion_uses_only_tested_clean_byok_otherwise_gateway() {
    let (_dir, mut s, _, shutdown) = fixture().await;
    query("INSERT INTO bot_settings(key,value,updated_at) VALUES('insight_byok_enabled','true',0)")
        .execute(&s.db)
        .await
        .unwrap();
    query("INSERT INTO insight_user_byok(user_id,provider,api_key,model,created_at,updated_at) VALUES(2,'openai','fixture-key','own-model',0,0)").execute(&s.db).await.unwrap();
    for (tested, error, byok) in [
        (None, None, false),
        (Some(1), Some("auth_failed"), false),
        (Some(1), None, true),
    ] {
        query("UPDATE insight_user_byok SET tested_at=?,test_error=? WHERE user_id=2")
            .bind(tested)
            .bind(error)
            .execute(&s.db)
            .await
            .unwrap();
        s.completion = Arc::new(RouteCheck { byok });
        let a = ActualInboxInsight::new(s.clone(), shutdown.clone());
        assert_eq!(
            contracts::Insight::complete(&a, 2, "prompt").await.unwrap(),
            "ok"
        );
    }
}

struct Revoke(yoink_db::Database);
impl Completion for Revoke {
    fn complete<'a>(
        &'a self,
        _: &'a [Route],
        _: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async move {
            query("DELETE FROM user_permissions WHERE user_id=2 AND plugin='inbox'")
                .execute(&self.0)
                .await?;
            Ok("must not escape".into())
        })
    }
}
#[tokio::test]
async fn completion_rechecks_revoked_grant_after_io() {
    let (_dir, mut s, _, shutdown) = fixture().await;
    s.completion = Arc::new(Revoke(s.db.clone()));
    let a = ActualInboxInsight::new(s, shutdown);
    assert!(matches!(
        contracts::Insight::complete(&a, 2, "prompt").await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn real_http_completion_errors_and_inflight_cancellation() {
    use axum::{Router, http::StatusCode, routing::post};
    let (_dir, mut s, _, shutdown) = fixture().await;
    s.completion = Arc::new(support::completion(Duration::from_secs(2), 4096).await);
    for code in [401, 402, 404, 429, 500] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        s.config.gateway_base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/chat/completions",
            post(move || async move { StatusCode::from_u16(code).unwrap() }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let a = ActualInboxInsight::new(s.clone(), shutdown.clone());
        let err = contracts::Insight::complete(&a, 2, "prompt")
            .await
            .unwrap_err();
        assert!(match code {
            401 | 402 | 404 => matches!(err, Error::Completion(_)),
            429 => matches!(err, Error::RateLimited),
            500 => matches!(err, Error::External),
            _ => false,
        });
        server.abort();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    s.config.gateway_base = format!("http://{}", listener.local_addr().unwrap());
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    let app = Router::new().route(
        "/chat/completions",
        post(move || {
            let notify = notify.clone();
            async move {
                notify.notify_one();
                std::future::pending::<StatusCode>().await
            }
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let a = ActualInboxInsight::new(s.clone(), shutdown.clone());
    let call = tokio::spawn(async move { contracts::Insight::complete(&a, 2, "prompt").await });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    shutdown.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), call)
            .await
            .unwrap()
            .unwrap(),
        Err(Error::Cancelled)
    ));
    s.config.request_timeout = Duration::from_millis(20);
    let a = ActualInboxInsight::new(s, CancellationToken::new());
    assert!(matches!(
        contracts::Insight::complete(&a, 2, "prompt").await,
        Err(Error::External)
    ));
    server.abort();
}
