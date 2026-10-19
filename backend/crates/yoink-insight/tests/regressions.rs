use serde_json::json;
use std::{
    sync::{
        Arc, Mutex,
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
    assert!(p.contains(&format!(
        "Current UTC date: {}",
        chrono::Utc::now().format("%Y-%m-%d")
    )));
    assert!(p.contains("training memory is not independent verification"));
    for command in ["tldr", "summary", "about"] {
        let p = prompts::build(
            command,
            "ru",
            "The author reports GPT-5.6 and Astra in 2026.",
            Some("CUSTOM TASK AND STYLE"),
            None,
            None,
            &[],
        );
        assert!(p.contains("CUSTOM TASK AND STYLE"));
        assert!(p.contains("GPT-5.6 and Astra in 2026"));
        assert!(p.contains("cannot establish that a recent product or model does not exist"));
        assert!(p.ends_with("Follow the selected task and style without changing their intent."));
    }
}
#[test]
fn reviewer_question_is_last_task_without_repeating_the_role() {
    let question = "на девопсячьем разложи в двух словах, что это и зачем?";
    let request = telegram::parse(
        &format!("/tldr https://deno.com/ nobullshit {question}"),
        &[],
    )
    .unwrap();
    assert_eq!(request.alias.as_deref(), Some("nobullshit"));
    assert_eq!(request.focus.as_deref(), Some(question));
    let role = prompts::builtin("nobullshit")
        .unwrap()
        .replace("{lang}", "ru");
    let prompt = prompts::build(
        "tldr",
        "ru",
        "Deno is a Node-compatible runtime. CODE EXAMPLE IS NOT THE PRODUCT.",
        Some("USER CUSTOM BULLET LIST"),
        prompts::builtin("nobullshit"),
        request.focus.as_deref(),
        &[],
    );
    assert_eq!(prompt.matches(&role).count(), 1);
    assert!(!prompt.contains("USER CUSTOM BULLET LIST"));
    assert!(
        prompt.find(question).unwrap() > prompt.find("CODE EXAMPLE IS NOT THE PRODUCT").unwrap()
    );
    assert!(prompt.contains("requested length, audience and format take priority"));
    assert!(prompt.contains("ordinary technical knowledge"));
    assert!(!prompt.contains("If evidence is absent or insufficient, say the claim is unverified"));
    assert!(prompt.contains("one or two useful sentences with a position"));
    assert!(prompt.contains("First-person judgments"));
    assert!(
        prompt.find("Reviewer voice reminder:").unwrap()
            > prompt.find("Current UTC date:").unwrap()
    );
    assert!(prompt.ends_with("Do not fabricate experience, measurements or defects."));
    assert!(
        !prompts::build(
            "tldr",
            "ru",
            "source",
            Some("MY CUSTOM ROLE"),
            None,
            None,
            &[]
        )
        .contains("Reviewer voice reminder:")
    );
}

#[tokio::test]
async fn telegram_reviewer_keeps_alias_and_focus_at_completion_boundary() {
    struct QuestionCompletion;
    impl Completion for QuestionCompletion {
        fn complete<'a>(
            &'a self,
            _: &'a [Route],
            prompt: &'a str,
            _: CancellationToken,
        ) -> FutureResult<'a, String> {
            Box::pin(async move {
                assert!(prompt.contains("Specific factual source with 42 examples"));
                assert!(prompt.contains("на девопсячьем разложи в двух словах"));
                assert!(prompt.contains("one or two useful sentences"));
                assert!(prompt.contains("No bullet lists, numbered lists"));
                assert!(!prompt.contains("Output only the bullets"));
                Ok("A direct answer.".into())
            })
        }
    }
    let (_directory, mut service, _) = fixture().await;
    service.completion = Arc::new(QuestionCompletion);
    let result = telegram::execute(
        &service,
        1,
        "/tldr https://deno.com/ nobullshit на девопсячьем разложи в двух словах, что это и зачем?",
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(result.result, "A direct answer.");
    assert_eq!(result.alias.as_deref(), Some("nobullshit"));
    assert!(!result.cached);
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
async fn imported_cache_is_preserved_but_quarantined() {
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
    assert_ne!(result.result, "Imported");
    assert!(!result.cached);
    assert_eq!(llm.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        query("SELECT result FROM insight_summary_cache")
            .fetch_one(&s.db)
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "Imported"
    );
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
async fn summary_uses_the_selected_openai_model_without_gemini_credentials() {
    struct ExactModel;
    impl Completion for ExactModel {
        fn complete<'a>(
            &'a self,
            routes: &'a [Route],
            _: &'a str,
            _: CancellationToken,
        ) -> FutureResult<'a, String> {
            Box::pin(async move {
                assert_eq!(routes.len(), 1);
                assert_eq!(routes[0].shape, Shape::OpenAi);
                assert_eq!(routes[0].via, "openrouter");
                assert_eq!(routes[0].model, "openai/gpt-6-luna");
                Ok("Configured provider summary".into())
            })
        }
        fn models<'a>(
            &'a self,
            _: &'a Route,
            _: CancellationToken,
        ) -> FutureResult<'a, Vec<provider::Model>> {
            Box::pin(async { Ok(vec![]) })
        }
    }
    let (_dir, mut service, _) = fixture().await;
    service.config.gemini_key.clear();
    service.config.mode = "openrouter".into();
    service.config.openrouter_key = "fixture-key".into();
    service.config.model = "openai/gpt-6-luna".into();
    service.completion = Arc::new(ExactModel);
    let result = service
        .summarize(
            1,
            request("summary", "https://youtu.be/abcdefghijk"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.result, "Configured provider summary");
}

#[tokio::test]
async fn unverifiable_imported_cache_cannot_bypass_missing_provider_credentials() {
    let (_dir, mut s, llm) = fixture().await;
    s.config.mode = "openrouter".into();
    s.config.openrouter_key.clear();
    query("INSERT INTO insight_summary_cache(content_key,lang,command,result,created_at,expires_at) VALUES('abcdefghijk','en','about','Imported',0,?)").bind(chrono::Utc::now()+chrono::Duration::hours(1)).execute(&s.db).await.unwrap();
    assert!(
        s.summarize(
            1,
            request("about", "https://youtu.be/abcdefghijk"),
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert_eq!(llm.0.load(Ordering::SeqCst), 0);
    assert_eq!(
        query("SELECT result FROM insight_summary_cache")
            .fetch_one(&s.db)
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "Imported"
    );
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

#[derive(Default)]
struct StreamingLlm(AtomicUsize);
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
            self.0.fetch_add(1, Ordering::SeqCst);
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
    phases: Mutex<Vec<provider::Phase>>,
    revoke_phase: Option<provider::Phase>,
}
impl provider::DeltaSink for DbSink {
    fn phase<'a>(
        &'a self,
        phase: provider::Phase,
        source: &'a str,
        _: Option<&'a str>,
    ) -> FutureResult<'a, ()> {
        Box::pin(async move {
            assert_eq!(source, "https://example.test/stream");
            // A sink can refresh authority on the same database, including cache hits.
            query("SELECT role FROM users WHERE id=1")
                .fetch_one(&self.db)
                .await?;
            self.phases.lock().unwrap().push(phase);
            if self.revoke_phase == Some(phase) {
                query("UPDATE users SET role='banned' WHERE id=1")
                    .execute(&self.db)
                    .await?;
            }
            Ok(())
        })
    }
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
    // TLDR admission precedes cache lookup: leave room for the repeat.
    s.config.tldr_limit = 3;
    s.completion = Arc::new(StreamingLlm::default());
    let sink = DbSink {
        db: s.db.clone(),
        revoke: false,
        calls: AtomicUsize::new(0),
        phases: Mutex::new(vec![]),
        revoke_phase: None,
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
    use provider::Phase;
    assert_eq!(
        *sink.phases.lock().unwrap(),
        vec![Phase::Fetching, Phase::Generating, Phase::Cached]
    );
    s.update_settings(
        1,
        serde_json::from_value(json!({"lang":"en","use_search":true})).unwrap(),
    )
    .await
    .unwrap();
    telegram::execute_stream(
        &s,
        1,
        "/tldr https://example.test/stream",
        &sink,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        &sink.phases.lock().unwrap()[3..],
        &[Phase::Fetching, Phase::Researching, Phase::Generating]
    );
}
#[tokio::test]
async fn revocation_between_deltas_denies_remaining_delivery_and_commit() {
    let (_dir, mut s, _) = fixture().await;
    s.completion = Arc::new(StreamingLlm::default());
    let sink = DbSink {
        db: s.db.clone(),
        revoke: true,
        calls: AtomicUsize::new(0),
        phases: Mutex::new(vec![]),
        revoke_phase: None,
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
#[tokio::test]
async fn revocation_during_status_delivery_prevents_the_provider_request() {
    let (_dir, mut s, _) = fixture().await;
    let llm = Arc::new(StreamingLlm::default());
    s.completion = llm.clone();
    let sink = DbSink {
        db: s.db.clone(),
        revoke: false,
        calls: AtomicUsize::new(0),
        phases: Mutex::new(vec![]),
        revoke_phase: Some(provider::Phase::Generating),
    };
    assert!(matches!(
        telegram::execute_stream(
            &s,
            1,
            "/tldr https://example.test/stream",
            &sink,
            CancellationToken::new()
        )
        .await,
        Err(Error::Code("access_denied"))
    ));
    assert_eq!(llm.0.load(Ordering::SeqCst), 0);
    assert_eq!(sink.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        query("SELECT count(*) FROM insight_native_reservations")
            .fetch_one(&s.db)
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        0
    );
}

#[test]
fn unicode_chunks_brackets_and_youtube_anywhere() {
    let text = "😀".repeat(4001);
    let messages = telegram::messages(&text);
    assert_eq!(messages.concat(), text);
    assert!(
        messages
            .iter()
            .all(|message| message.encode_utf16().count() <= 4000)
    );
    assert!(prompts::glob("[ab].example", "a.example"));
    assert!(!prompts::glob("[!a-c].example", "b.example"));
    assert!(prompts::glob("[!a-c].example", "z.example"));
    assert!(prompts::glob("?.example", "я.example"));
    let request = telegram::parse(
        "/summary Please summarize:https://youtu.be/abcdefghijk thanks",
        &[],
    )
    .unwrap();
    assert_eq!(request.url, "https://youtu.be/abcdefghijk");
    assert!(request.focus.is_none());
}

#[tokio::test]
async fn tldr_quota_precedes_native_cache_but_summary_native_cache_precedes_quota() {
    let (_directory, mut service, _) = fixture().await;
    service.config.tldr_limit = 2;
    query("INSERT INTO insight_summary_cache(content_key,lang,command,result,created_at,expires_at) VALUES('https://example.test/imported','en','tldr:v3','Imported TLDR',0,?)").bind(chrono::Utc::now()+chrono::Duration::hours(1)).execute(&service.db).await.unwrap();
    let imported = service
        .summarize(
            1,
            request("tldr", "https://example.test/imported"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!imported.cached);
    assert_ne!(imported.result, "Imported TLDR");
    service
        .summarize(
            1,
            request("tldr", "https://example.test/new"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(
        service
            .summarize(
                1,
                request("tldr", "https://example.test/imported"),
                CancellationToken::new()
            )
            .await,
        Err(Error::Code("quota_exceeded"))
    ));
    // Summary's own private native cache still precedes quota at the daily limit.
    let (_summary_directory, summary_service, _) = fixture().await;
    summary_service
        .summarize(
            1,
            request("summary", "https://youtu.be/abcdefghijk"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        summary_service
            .summarize(
                1,
                request("summary", "https://youtu.be/abcdefghijk"),
                CancellationToken::new()
            )
            .await
            .unwrap()
            .cached
    );
}

#[tokio::test]
async fn focus_never_cached_and_native_repeat_does_not_fetch_offline() {
    let (_directory, mut service, provider) = fixture().await;
    service.config.tldr_limit = 10;
    let mut focused = request("tldr", "https://example.test/focus");
    focused.focus = Some("What changed?".into());
    for _ in 0..2 {
        assert!(
            !service
                .summarize(1, focused.clone(), CancellationToken::new())
                .await
                .unwrap()
                .cached
        );
    }
    assert_eq!(provider.0.load(Ordering::SeqCst), 2);
    service
        .summarize(
            1,
            request("tldr", "https://example.test/offline"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    service.content = None;
    assert!(
        service
            .summarize(
                1,
                request("tldr", "https://example.test/offline"),
                CancellationToken::new()
            )
            .await
            .unwrap()
            .cached
    );
}

#[tokio::test]
async fn executable_summary_grant_not_docstring_tldr_grant_controls_route_precedence() {
    let (_directory, service, _) = fixture().await;
    service.byok_toggle(1, Some(true)).await.unwrap();
    service
        .save_byok(
            2,
            ByokUpdate {
                provider: "openai".into(),
                base_url: None,
                api_key: Some("fixture-key".into()),
                model: "own-model".into(),
            },
        )
        .await
        .unwrap();
    query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(2,'insight','tldr',1)").execute(&service.db).await.unwrap();
    let result = service
        .summarize(
            2,
            request("tldr", "https://example.test/byok"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.route, "byok");
    service.grant(1, 2, "en").await.unwrap(); // executable legacy summary gate
    // Clear usage to isolate routing from this fixture's one-request limit.
    query("DELETE FROM insight_usage_log WHERE user_id=2")
        .execute(&service.db)
        .await
        .unwrap();
    let result = service
        .summarize(
            2,
            request("tldr", "https://example.test/gateway"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.route, "gateway");
}

#[tokio::test]
async fn stats_preserves_imported_alias_order_cutoff_and_zero_duration_fallback() {
    let (_directory, service, _) = fixture().await;
    let now = chrono::Utc::now();
    for (alias, days, seconds, chars) in [
        ("zeta", 29, Some(0), 825),
        ("zeta", 29, Some(60), 0),
        ("alpha", 31, None, 1000),
    ] {
        query("INSERT INTO insight_usage_log(user_id,command,lang,status,created_at,content_chars,video_seconds,alias_key,route) VALUES(1,'tldr','en','ok',?,?,?,?,'gateway')").bind(now-chrono::Duration::days(days)).bind(chars).bind(seconds).bind(alias).execute(&service.db).await.unwrap();
    }
    let stats = service.stats(1).await.unwrap();
    assert_eq!(stats["tldr"]["total"], 3);
    assert_eq!(
        stats["tldr"]["by_alias"][0],
        serde_json::json!({"alias":"zeta","count":2})
    );
    assert_eq!(stats["tldr"]["video_minutes_saved"], 2.0);
    assert_eq!(stats["tldr"]["reading_minutes_saved"], 1.0);
    assert_eq!(
        stats["tldr"]["by_day"]
            .as_array()
            .unwrap()
            .iter()
            .map(|day| day["count"].as_i64().unwrap())
            .sum::<i64>(),
        2
    );
}

#[tokio::test]
async fn dropping_inflight_generation_releases_quota_without_partial_commit() {
    let (_directory, mut service, _) = fixture().await;
    let gate = Arc::new(Gate {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Notify::new(),
    });
    service.completion = gate.clone();
    let running = service.clone();
    let task = tokio::spawn(async move {
        running
            .summarize(
                1,
                request("tldr", "https://example.test/drop"),
                CancellationToken::new(),
            )
            .await
    });
    gate.entered.acquire().await.unwrap().forget();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let reservations: i64 = query("SELECT count(*) FROM insight_native_reservations")
                .fetch_one(&service.db)
                .await
                .unwrap()
                .try_get(0)
                .unwrap();
            if reservations == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for table in ["insight_native_cache", "insight_usage_log"] {
        let rows: i64 = query(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&service.db)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        assert_eq!(rows, 0);
    }
}

struct UnavailableResearch;
impl yoink_insight::research::Research for UnavailableResearch {
    fn search<'a>(
        &'a self,
        _: &'a Route,
        _: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, Vec<Citation>> {
        Box::pin(async { Err(Error::Code("provider_unavailable")) })
    }
}
struct NoFabricatedCitations;
impl Completion for NoFabricatedCitations {
    fn complete<'a>(
        &'a self,
        _: &'a [Route],
        prompt: &'a str,
        _: CancellationToken,
    ) -> FutureResult<'a, String> {
        Box::pin(async move {
            assert!(prompt.contains("Independent web research: Not supplied"));
            assert!(!prompt.contains("https://independent.test/docs"));
            Ok("degraded but honest".into())
        })
    }
}
#[tokio::test]
async fn research_failure_degrades_without_fabricated_citations() {
    let (_directory, mut service, _) = fixture().await;
    service = service.with_research(Arc::new(UnavailableResearch));
    service.completion = Arc::new(NoFabricatedCitations);
    service
        .update_settings(
            1,
            serde_json::from_value(serde_json::json!({"lang":"en", "use_search":true})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .summarize(
                1,
                request("tldr", "https://example.test/research"),
                CancellationToken::new()
            )
            .await
            .unwrap()
            .result,
        "degraded but honest"
    );
}

#[tokio::test]
async fn telegram_summary_only_user_does_not_need_alias_list_permission() {
    let (_directory, service, _) = fixture().await;
    service.grant(1, 2, "ja").await.unwrap();
    assert!(service.aliases(2).await.is_err());
    assert!(
        !telegram::execute(
            &service,
            2,
            "/summary please summarize https://youtu.be/abcdefghijk",
            CancellationToken::new()
        )
        .await
        .unwrap()
        .cached
    );
}

#[tokio::test]
async fn cross_user_imports_cannot_disclose_private_github_byok_or_custom_prompts() {
    let (_directory, mut service, provider) = fixture().await;
    service.config.tldr_limit = 10;
    service.config.summary_limit = 10;
    service.grant(1, 2, "en").await.unwrap();
    query("INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(2,'insight','tldr',1)").execute(&service.db).await.unwrap();
    // These represent legacy producer-user-1 results. The shared table contains
    // no producer identity, credential, prompt, or route columns to verify them.
    let imports = [
        (
            "https://github.com/acme/private",
            "tldr:v3",
            "PRIVATE-GITHUB-USER-1",
        ),
        (
            "https://example.test/byok",
            "tldr:v3",
            "BYOK-PRIVATE-USER-1",
        ),
        (
            "https://example.test/custom-alias",
            "tldr:max:v3",
            "CUSTOM-ALIAS-USER-1",
        ),
        ("abcdefghijk", "about", "ABOUT-PROMPT-OVERRIDE-USER-1"),
        ("abcdefghijk", "summary", "SUMMARY-PROMPT-OVERRIDE-USER-1"),
    ];
    for (key, command, secret) in imports {
        query("INSERT INTO insight_summary_cache(content_key,lang,command,result,created_at,expires_at) VALUES(?,'en',?,?,0,?)")
            .bind(key).bind(command).bind(secret)
            .bind(chrono::Utc::now() + chrono::Duration::hours(1))
            .execute(&service.db).await.unwrap();
    }
    // A reader with no source access must fail rather than receive the imported
    // private GitHub result. No fetch adapter/token is installed for this step.
    service.content = None;
    assert!(matches!(
        service
            .summarize(2, request("tldr", imports[0].0), CancellationToken::new())
            .await,
        Err(Error::Code("content_unavailable"))
    ));
    service.content = Some(Arc::new(Source));
    for (key, command, secret) in imports.into_iter().skip(1) {
        let mut req = request(
            if command.starts_with("tldr") {
                "tldr"
            } else {
                command
            },
            if key == "abcdefghijk" {
                "https://youtu.be/abcdefghijk"
            } else {
                key
            },
        );
        if command == "tldr:max:v3" {
            req.alias = Some("max".into());
        }
        let fresh = service
            .summarize(2, req.clone(), CancellationToken::new())
            .await
            .unwrap();
        assert!(!fresh.cached);
        assert!(!fresh.result.contains(secret));
        // Verified native replay remains offline and bound to this reader.
        service.content = None;
        let replay = service
            .summarize(2, req, CancellationToken::new())
            .await
            .unwrap();
        assert!(replay.cached);
        assert_eq!(replay.result, fresh.result);
        service.content = Some(Arc::new(Source));
    }
    assert_eq!(provider.0.load(Ordering::SeqCst), 4);
    for (key, command, secret) in imports {
        let row =
            query("SELECT result FROM insight_summary_cache WHERE content_key=? AND command=?")
                .bind(key)
                .bind(command)
                .fetch_one(&service.db)
                .await
                .unwrap();
        assert_eq!(row.try_get::<String, _>(0).unwrap(), secret);
    }
}

// Poll exactly once, without sleeps or scheduling assumptions. Polling the fence
// also closes admission before any subsequent assertion/operation.
async fn assert_fence_pending(
    mut fence: std::pin::Pin<&mut impl std::future::Future<Output = ()>>,
) {
    std::future::poll_fn(|cx| {
        assert!(fence.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn quota_fence_tracks_live_owner_then_drop_cleanup_behind_held_db() {
    let (_dir, mut service, _) = fixture().await;
    let gate = Arc::new(Gate {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Notify::new(),
    });
    service.completion = gate.clone();
    let clone = service.clone();
    let task = tokio::spawn(async move {
        clone
            .summarize(
                1,
                request("tldr", "https://example.test/fence"),
                CancellationToken::new(),
            )
            .await
    });
    gate.entered.acquire().await.unwrap().forget();
    let held = service.db.connection().await.unwrap();
    let clone = service.clone();
    let mut fence = Box::pin(service.shutdown_quota_cleanup());
    let mut other_fence = Box::pin(clone.shutdown_quota_cleanup());
    assert_fence_pending(fence.as_mut()).await;
    assert_fence_pending(other_fence.as_mut()).await;
    // Admission must fail without even waiting for the held DB, across clones.
    assert!(matches!(
        clone
            .summarize(
                1,
                request("tldr", "https://example.test/late"),
                CancellationToken::new()
            )
            .await,
        Err(Error::Code("shutting_down"))
    ));
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    // The owner is gone, but its token moved to DB cleanup: neither fence can
    // observe an empty tracker while the DB guard blocks that cleanup.
    assert_fence_pending(fence.as_mut()).await;
    assert_fence_pending(other_fence.as_mut()).await;
    // Cancelling a waiter must neither reopen admission nor detach cleanup.
    drop(fence);
    let mut fence = Box::pin(service.shutdown_quota_cleanup());
    assert_fence_pending(fence.as_mut()).await;
    let count: i64 = query("SELECT count(*) FROM insight_native_reservations")
        .fetch_one(&*held)
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(count, 1);
    drop(held);
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(fence, other_fence);
    })
    .await
    .unwrap();
    let count: i64 = query("SELECT count(*) FROM insight_native_reservations")
        .fetch_one(&service.db)
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(count, 0);
    for table in ["insight_native_cache", "insight_usage_log"] {
        let count: i64 = query(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&service.db)
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        assert_eq!(count, 0);
    }
    // Repeated waits remain ready, even while DB access is impossible.
    let _held = service.db.connection().await.unwrap();
    let mut repeated = Box::pin(clone.shutdown_quota_cleanup());
    std::future::poll_fn(|cx| {
        assert!(repeated.as_mut().poll(cx).is_ready());
        std::task::Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn quota_fence_after_transactional_deletion_has_no_redundant_cleanup() {
    let (_dir, service, _) = fixture().await;
    service
        .summarize(
            1,
            request("tldr", "https://example.test/committed"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let _held = service.db.connection().await.unwrap();
    let mut fence = Box::pin(service.shutdown_quota_cleanup());
    std::future::poll_fn(|cx| {
        assert!(fence.as_mut().poll(cx).is_ready());
        std::task::Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn quota_fence_after_explicit_cached_release_has_no_redundant_cleanup() {
    let (_dir, mut service, _) = fixture().await;
    service.config.tldr_limit = 0;
    let r = request("tldr", "https://example.test/released");
    service
        .summarize(1, r.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert!(
        service
            .summarize(1, r, CancellationToken::new())
            .await
            .unwrap()
            .cached
    );
    let _held = service.db.connection().await.unwrap();
    let mut fence = Box::pin(service.shutdown_quota_cleanup());
    std::future::poll_fn(|cx| {
        assert!(fence.as_mut().poll(cx).is_ready());
        std::task::Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn quota_admission_after_empty_shutdown_is_closed_across_clones() {
    let (_dir, service, llm) = fixture().await;
    let clone = service.clone();
    tokio::join!(
        service.shutdown_quota_cleanup(),
        clone.shutdown_quota_cleanup()
    );
    let _held = service.db.connection().await.unwrap();
    for command in ["tldr", "summary"] {
        assert!(matches!(
            clone
                .summarize(
                    1,
                    request(command, "https://youtu.be/abcdefghijk"),
                    CancellationToken::new()
                )
                .await,
            Err(Error::Code("shutting_down"))
        ));
    }
    assert_eq!(llm.0.load(Ordering::SeqCst), 0);
}
