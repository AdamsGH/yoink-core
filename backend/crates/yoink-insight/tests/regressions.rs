use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use yoink_db::{Database, query, raw_sql};
use yoink_insight::{
    content::{Citation, Content, ContentSource, normalize, video_id},
    prompts,
    provider::{Completion, Route, Shape, contract, decode},
    *,
};
struct Source;
impl ContentSource for Source {
    fn fetch<'a>(
        &'a self,
        _: &'a str,
        options: content::FetchOptions<'a>,
        _: CancellationToken,
    ) -> FutureResult<'a, Content> {
        Box::pin(async move {
            Ok(Content {
                text: "Specific factual source with 42 examples".into(),
                title: None,
                video_id: if options.transcript_only {
                    Some("abcdefghijk".into())
                } else {
                    None
                },
                video_seconds: if options.transcript_only {
                    Some(120)
                } else {
                    None
                },
                via: "confined_worker".into(),
            })
        })
    }
    fn research<'a>(&'a self, _: &'a str, _: CancellationToken) -> FutureResult<'a, Vec<Citation>> {
        Box::pin(async {
            Ok(vec![Citation {
                url: "https://independent.test/docs".into(),
                title: "Primary docs".into(),
                text: "Verified evidence".into(),
            }])
        })
    }
}
struct Llm(AtomicUsize);
impl Completion for Llm {
    fn complete<'a>(
        &'a self,
        _: &'a [Route],
        prompt: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(format!("Result: {}", &prompt[..prompt.len().min(100)]))
        })
    }
    fn models<'a>(
        &'a self,
        _: &'a Route,
        _: CancellationToken,
    ) -> FutureResult<'a, Vec<provider::Model>> {
        Box::pin(async {
            Ok(vec![provider::Model {
                id: "own-model".into(),
                supports_websearch: false,
            }])
        })
    }
}
async fn fixture() -> (tempfile::TempDir, Insight, Arc<Llm>) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let db = Database::open(&dir.path().join("test.db"), true, Duration::from_secs(5))
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
    let llm = Arc::new(Llm(AtomicUsize::new(0)));
    let service = Insight::new(
        db,
        Config {
            owner_id: Some(1),
            model: "test-model".into(),
            gemini_key: "fixture-not-live".into(),
            summary_limit: 1,
            tldr_limit: 1,
            ..Default::default()
        },
        Arc::new(Source),
        llm.clone(),
    )
    .unwrap();
    service.grant(1, 1, "en").await.err();
    query("INSERT INTO users(id,role,created_at,updated_at) VALUES(1,'owner',0,0),(2,'user',0,0),(3,'banned',0,0)").execute(&service.db).await.unwrap();
    (dir, service, llm)
}
fn request(command: &str, url: &str) -> Request {
    Request {
        command: command.into(),
        url: url.into(),
        alias: None,
        focus: None,
    }
}
#[test]
fn youtube_exact_hosts() {
    assert_eq!(
        video_id(&normalize("youtu.be/abcdefghijk").unwrap()),
        Some("abcdefghijk".into())
    );
    assert!(video_id(&normalize("evil-youtube.com/watch?v=abcdefghijk").unwrap()).is_none());
    assert!(video_id(&normalize("youtube.com/watch?v=bad").unwrap()).is_none());
}
#[test]
fn provider_shapes() {
    for shape in [Shape::OpenAi, Shape::Anthropic, Shape::Gemini] {
        let r = Route {
            shape,
            base: "https://provider.test/v1".into(),
            key: "SECRET".into(),
            model: "model-1".into(),
            via: "byok",
        };
        let (url, h, b) = contract(&r, "prompt").unwrap();
        assert!(url.starts_with("https://provider.test/v1/"));
        assert!(!format!("{r:?}").contains("SECRET"));
        assert!(h.iter().any(|(_, v)| v.contains("SECRET")));
        assert!(!b.to_string().contains("SECRET"));
    }
}
#[test]
fn response_contracts() {
    assert_eq!(
        decode(
            Shape::OpenAi,
            &json!({"choices":[{"message":{"content":"hello"}}]})
        )
        .unwrap(),
        "hello"
    );
    assert_eq!(decode(Shape::Anthropic,&json!({"content":[{"type":"text","text":"A"},{"type":"tool_use","text":"never"},{"type":"text","text":"B"}]})).unwrap(),"AB");
    assert_eq!(
        decode(
            Shape::Gemini,
            &json!({"candidates":[{"content":{"parts":[{"text":"A"},{"text":"B"}]}}]})
        )
        .unwrap(),
        "AB"
    );
    assert!(decode(Shape::OpenAi, &json!({})).is_err());
}
#[test]
fn aliases_focus_and_language() {
    let r = telegram::parse("/tldr@bot example.test nobullshit focus on benchmarks", &[]).unwrap();
    assert_eq!(r.alias.as_deref(), Some("nobullshit"));
    assert_eq!(r.focus.as_deref(), Some("focus on benchmarks"));
    let p = prompts::build(
        "tldr",
        "ru",
        "source",
        None,
        prompts::builtin("nobullshit"),
        r.focus.as_deref(),
        &[],
    );
    assert!(p.contains("Reply in ru"));
    assert!(p.contains("Not supplied. Do not claim external verification"));
    assert!(p.contains("focus on benchmarks"));
}
#[test]
fn domain_globs() {
    assert!(prompts::glob("*.lwn.net", "www.lwn.net"));
    assert!(!prompts::glob("*.lwn.net", "evil-lwn.net"));
    assert!(prompts::glob("example.com/*", "example.com/a/b"));
    assert!(!prompts::glob("example.com/*", "example.com.evil/a"));
}
#[test]
fn credentials_validation() {
    assert!(
        provider::byok(
            "custom_openai",
            Some("http://127.0.0.1"),
            "key".into(),
            "model".into()
        )
        .is_err()
    );
    assert!(
        provider::byok(
            "custom_openai",
            Some("https://localhost"),
            "key".into(),
            "model".into()
        )
        .is_err()
    );
    assert!(provider::byok("openai", None, "key".into(), "model".into()).is_ok());
}
#[tokio::test]
async fn default_cache_and_daily_quota() {
    let (_dir, s, llm) = fixture().await;
    let r = request("summary", "https://youtu.be/abcdefghijk");
    let a = s
        .summarize(1, r.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert!(!a.cached);
    assert!(
        s.summarize(1, r, CancellationToken::new())
            .await
            .unwrap()
            .cached
    );
    assert_eq!(llm.0.load(Ordering::SeqCst), 1);
    assert!(matches!(
        s.summarize(
            1,
            request("about", "https://youtu.be/abcdefghijk"),
            CancellationToken::new()
        )
        .await,
        Err(Error::Code("quota_exceeded"))
    ));
    let stats = s.stats(1).await.unwrap();
    assert_eq!(stats["total_summaries"], 1);
}
#[tokio::test]
async fn imported_cache_is_preserved() {
    let (_dir, s, llm) = fixture().await;
    query("INSERT INTO insight_summary_cache(content_key,lang,command,result,created_at,expires_at) VALUES('abcdefghijk','en','about','Imported',0,?)").bind(chrono::Utc::now()+chrono::Duration::hours(1)).execute(&s.db).await.unwrap();
    let result = s
        .summarize(
            1,
            request("about", "https://youtu.be/abcdefghijk"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.result, "Imported");
    assert!(result.cached);
    assert_eq!(llm.0.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn current_bans_override_owner_and_admin() {
    let (_dir, s, _) = fixture().await;
    assert!(s.grant(2, 3, "en").await.is_err());
    query("UPDATE users SET role='banned' WHERE id=1")
        .execute(&s.db)
        .await
        .unwrap();
    assert!(s.byok_toggle(1, Some(true)).await.is_err());
    assert!(
        s.summarize(
            1,
            request("summary", "https://youtu.be/abcdefghijk"),
            CancellationToken::new()
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn access_grants_write_unified_and_legacy() {
    let (_dir, s, _) = fixture().await;
    let v = s.grant(1, 2, "ru").await.unwrap();
    assert_eq!(v["user_id"], 2);
    assert!(
        s.settings(2).await.unwrap()["has_gemini_access"]
            .as_bool()
            .unwrap()
    );
    s.revoke(1, 2).await.unwrap();
    assert!(
        !s.settings(2).await.unwrap()["has_gemini_access"]
            .as_bool()
            .unwrap()
    );
}
#[tokio::test]
async fn byok_probe_and_global_toggle() {
    let (_dir, s, _) = fixture().await;
    s.byok_toggle(1, Some(true)).await.unwrap();
    s.save_byok(
        2,
        ByokUpdate {
            provider: "openai".into(),
            base_url: None,
            api_key: Some("fixture-secret-key".into()),
            model: "own-model".into(),
        },
    )
    .await
    .unwrap();
    assert!(
        s.settings(2).await.unwrap()["has_tldr_access"]
            .as_bool()
            .unwrap()
    );
    let view = s.byok_view(2).await.unwrap();
    assert!(!view.to_string().contains("fixture-secret"));
    s.probe_byok(2, CancellationToken::new()).await.unwrap();
    assert!(
        s.settings(2).await.unwrap()["has_tldr_access"]
            .as_bool()
            .unwrap()
    );
    s.byok_toggle(1, Some(false)).await.unwrap();
    assert!(
        !s.settings(2).await.unwrap()["has_tldr_access"]
            .as_bool()
            .unwrap()
    );
}
#[tokio::test]
async fn alias_ownership_and_conflicts() {
    let (_dir, s, _) = fixture().await;
    let input = prompts::AliasInput {
        aliases: Some("custom".into()),
        prompt: Some("Specific prompt".into()),
        ..Default::default()
    };
    let a = s.save_alias(1, None, input.clone()).await.unwrap();
    assert!(s.save_alias(1, None, input).await.is_err());
    assert!(s.delete_alias(2, a.id).await.is_err());
    assert_eq!(s.aliases(1).await.unwrap().len(), 1);
    s.delete_alias(1, a.id).await.unwrap();
    assert!(s.aliases(1).await.unwrap().is_empty());
}
#[tokio::test]
async fn custom_prompt_does_not_reuse_imported_cache() {
    let (_dir, s, llm) = fixture().await;
    query("INSERT INTO insight_summary_cache(content_key,lang,command,result,created_at,expires_at) VALUES('abcdefghijk','en','about','Imported',0,?)").bind(chrono::Utc::now()+chrono::Duration::hours(1)).execute(&s.db).await.unwrap();
    s.update_settings(
        1,
        SettingsUpdate {
            lang: "en".into(),
            tldr_model: None,
            github_token: None,
            use_search: None,
            prompts: Some([("about".into(), Some("Custom: reply in {lang}".into()))].into()),
        },
    )
    .await
    .unwrap();
    let result = s
        .summarize(
            1,
            request("about", "https://youtu.be/abcdefghijk"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!result.cached);
    assert!(result.result.contains("Custom"));
    assert_eq!(llm.0.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn http_requires_shared_authenticated_extension() {
    use tower::ServiceExt;
    let (_dir, s, _) = fixture().await;
    let shutdown = CancellationToken::new();
    let oauth = oauth::DeviceOAuth::new(s.clone(), "fixture-client".into(), None, shutdown.clone())
        .unwrap();
    let app = http::router(http::HttpState {
        insight: s,
        oauth,
        shutdown,
    });
    let result = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/settings/me")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(result.status(), axum::http::StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn cancellation_before_io() {
    let (_dir, s, llm) = fixture().await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        s.summarize(
            1,
            request("summary", "https://youtu.be/abcdefghijk"),
            cancel
        )
        .await,
        Err(Error::Code("cancelled"))
    ));
    assert_eq!(llm.0.load(Ordering::SeqCst), 0);
}
struct Gate {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Notify,
}
impl Completion for Gate {
    fn complete<'a>(
        &'a self,
        _: &'a [Route],
        _: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async move {
            self.entered.add_permits(1);
            self.release.notified().await;
            Ok("gated result".into())
        })
    }
}
#[tokio::test]
async fn parallel_admission_cannot_exceed_quota() {
    let (_dir, mut s, _) = fixture().await;
    let gate = Arc::new(Gate {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Notify::new(),
    });
    s.completion = gate.clone();
    let first = s.clone();
    let task = tokio::spawn(async move {
        first
            .summarize(
                1,
                request("tldr", "https://example.test/one"),
                CancellationToken::new(),
            )
            .await
    });
    gate.entered.acquire().await.unwrap().forget();
    let second = s
        .summarize(
            1,
            request("tldr", "https://example.test/two"),
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(second, Err(Error::Code("quota_exceeded"))));
    gate.release.notify_one();
    assert!(task.await.unwrap().is_ok());
}
#[tokio::test]
async fn credentials_rechecked_after_provider_io() {
    let (_dir, mut s, _) = fixture().await;
    let gate = Arc::new(Gate {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Notify::new(),
    });
    s.completion = gate.clone();
    let first = s.clone();
    let task = tokio::spawn(async move {
        first
            .summarize(
                1,
                request("tldr", "https://example.test/one"),
                CancellationToken::new(),
            )
            .await
    });
    gate.entered.acquire().await.unwrap().forget();
    query("INSERT INTO bot_settings(key,value,updated_at) VALUES('insight_tldr_gateway_key','replacement',0)").execute(&s.db).await.unwrap();
    gate.release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(Error::Code("route_changed"))
    ));
    let n: i64 = query("SELECT count(*) FROM insight_native_cache")
        .fetch_one(&s.db)
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(n, 0);
}
#[tokio::test]
async fn user_revocation_after_provider_io_denies_result() {
    let (_dir, mut s, _) = fixture().await;
    s.grant(1, 2, "en").await.unwrap();
    let gate = Arc::new(Gate {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Notify::new(),
    });
    s.completion = gate.clone();
    let first = s.clone();
    let task = tokio::spawn(async move {
        first
            .summarize(
                2,
                request("summary", "https://youtu.be/abcdefghijk"),
                CancellationToken::new(),
            )
            .await
    });
    gate.entered.acquire().await.unwrap().forget();
    s.revoke(1, 2).await.unwrap();
    gate.release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(Error::Code("access_denied"))
    ));
}
#[tokio::test]
async fn imported_cache_does_not_require_provider_credentials() {
    let (_dir, mut s, llm) = fixture().await;
    s.config.gemini_key.clear();
    query("INSERT INTO insight_summary_cache(content_key,lang,command,result,created_at,expires_at) VALUES('abcdefghijk','en','about','Imported',0,?)").bind(chrono::Utc::now()+chrono::Duration::hours(1)).execute(&s.db).await.unwrap();
    let result = s
        .summarize(
            1,
            request("about", "https://youtu.be/abcdefghijk"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.result, "Imported");
    assert_eq!(llm.0.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn successful_cache_and_usage_are_atomic() {
    let (_dir, s, llm) = fixture().await;
    raw_sql("CREATE TRIGGER fail_insight_usage BEFORE INSERT ON insight_usage_log BEGIN SELECT RAISE(ABORT,'fixture only'); END;").execute(&s.db).await.unwrap();
    let result = s
        .summarize(
            1,
            request("summary", "https://youtu.be/abcdefghijk"),
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, Err(Error::Database)));
    let n: i64 = query("SELECT count(*) FROM insight_native_cache")
        .fetch_one(&s.db)
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(n, 0);
    raw_sql("DROP TRIGGER fail_insight_usage")
        .execute(&s.db)
        .await
        .unwrap();
    assert!(
        !s.summarize(
            1,
            request("summary", "https://youtu.be/abcdefghijk"),
            CancellationToken::new()
        )
        .await
        .unwrap()
        .cached
    );
    assert_eq!(llm.0.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn frontend_explicit_null_resets_model_but_omission_keeps_it() {
    let (_dir, s, _) = fixture().await;
    query("INSERT INTO bot_settings(key,value,updated_at) VALUES('insight_tldr_allowed_models','[\"test-model\"]',0)").execute(&s.db).await.unwrap();
    let selected: SettingsUpdate =
        serde_json::from_value(json!({"lang":"en","tldr_model":"test-model"})).unwrap();
    assert_eq!(
        s.update_settings(1, selected).await.unwrap()["tldr_model"],
        "test-model"
    );
    let omitted: SettingsUpdate = serde_json::from_value(json!({"lang":"ru"})).unwrap();
    assert_eq!(
        s.update_settings(1, omitted).await.unwrap()["tldr_model"],
        "test-model"
    );
    let reset: SettingsUpdate =
        serde_json::from_value(json!({"lang":"en","tldr_model":null})).unwrap();
    assert!(s.update_settings(1, reset).await.unwrap()["tldr_model"].is_null());
}

struct StreamingLlm;
impl Completion for StreamingLlm {
    fn complete<'a>(
        &'a self,
        _: &'a [Route],
        _: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async { panic!("streaming must not use buffered completion") })
    }
    fn stream<'a>(
        &'a self,
        _: &'a [Route],
        _: &'a str,
        sink: &'a dyn provider::DeltaSink,
        _: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async move {
            sink.delta("first").await?;
            sink.delta(" second").await?;
            Ok("first second".into())
        })
    }
}
struct DbSink {
    db: Database,
    revoke: bool,
    calls: AtomicUsize,
}
impl provider::DeltaSink for DbSink {
    fn delta<'a>(&'a self, _: &'a str) -> FutureResult<'a, ()> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.revoke {
                query("UPDATE users SET role='banned' WHERE id=1")
                    .execute(&self.db)
                    .await?;
            }
            Ok(())
        })
    }
}
#[tokio::test]
async fn telegram_stream_commits_once_and_cache_emits_no_fake_deltas() {
    let (_dir, mut s, _) = fixture().await;
    s.completion = Arc::new(StreamingLlm);
    let sink = DbSink {
        db: s.db.clone(),
        revoke: false,
        calls: AtomicUsize::new(0),
    };
    let first = telegram::execute_stream(
        &s,
        1,
        "/tldr https://example.test/stream",
        &sink,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(first.result, "first second");
    assert!(!first.cached);
    assert_eq!(sink.calls.load(Ordering::SeqCst), 2);
    let cached = telegram::execute_stream(
        &s,
        1,
        "/tldr https://example.test/stream",
        &sink,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(cached.cached);
    assert_eq!(sink.calls.load(Ordering::SeqCst), 2);
    let n: i64 = query("SELECT count(*) FROM insight_usage_log WHERE status='ok'")
        .fetch_one(&s.db)
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(n, 1);
}
#[tokio::test]
async fn revocation_between_deltas_denies_remaining_delivery_and_commit() {
    let (_dir, mut s, _) = fixture().await;
    s.completion = Arc::new(StreamingLlm);
    let sink = DbSink {
        db: s.db.clone(),
        revoke: true,
        calls: AtomicUsize::new(0),
    };
    assert!(
        telegram::execute_stream(
            &s,
            1,
            "/tldr https://example.test/stream",
            &sink,
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
    let n: i64 = query("SELECT count(*) FROM insight_native_cache")
        .fetch_one(&s.db)
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(n, 0);
    let n: i64 = query("SELECT count(*) FROM insight_usage_log WHERE status='ok'")
        .fetch_one(&s.db)
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(n, 0);
}
