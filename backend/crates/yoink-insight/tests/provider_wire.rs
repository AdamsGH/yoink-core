use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use yoink_insight::{
    Error,
    provider::{Completion, HttpCompletion, Route, Shape, public_ip},
};
async fn fixture(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, handle)
}
fn route(base: &str, shape: Shape) -> Route {
    Route {
        shape,
        base: base.into(),
        key: "fixture-key-not-live".into(),
        model: "test".into(),
        via: "gateway",
    }
}
#[tokio::test]
async fn real_openai_request() {
    async fn answer(headers: axum::http::HeaderMap, Json(body): Json<Value>) -> Json<Value> {
        assert_eq!(headers["authorization"], "Bearer fixture-key-not-live");
        assert_eq!(body["messages"][0]["content"], "hello");
        assert_eq!(body["stream"], false);
        Json(json!({"choices":[{"message":{"content":"fixture completion"}}]}))
    }
    let (base, handle) = fixture(Router::new().route("/chat/completions", post(answer))).await;
    let client = HttpCompletion::new(Duration::from_secs(2), 4096).unwrap();
    assert_eq!(
        client
            .complete(
                &[route(&base, Shape::OpenAi)],
                "hello",
                CancellationToken::new()
            )
            .await
            .unwrap(),
        "fixture completion"
    );
    handle.abort();
}
#[tokio::test]
async fn real_anthropic_and_gemini() {
    async fn anthropic(headers: axum::http::HeaderMap, Json(body): Json<Value>) -> Json<Value> {
        assert_eq!(headers["x-api-key"], "fixture-key-not-live");
        assert_eq!(headers["anthropic-version"], "2023-06-01");
        assert_eq!(body["max_tokens"], 8192);
        Json(json!({"content":[{"type":"text","text":"A"}]}))
    }
    async fn gemini(headers: axum::http::HeaderMap, Json(body): Json<Value>) -> Json<Value> {
        assert_eq!(headers["x-goog-api-key"], "fixture-key-not-live");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hello");
        Json(json!({"candidates":[{"content":{"parts":[{"text":"G"}]}}]}))
    }
    let (base, handle) = fixture(
        Router::new()
            .route("/messages", post(anthropic))
            .route("/models/test:generateContent", post(gemini)),
    )
    .await;
    let client = HttpCompletion::new(Duration::from_secs(2), 4096).unwrap();
    for (shape, text) in [(Shape::Anthropic, "A"), (Shape::Gemini, "G")] {
        assert_eq!(
            client
                .complete(&[route(&base, shape)], "hello", CancellationToken::new())
                .await
                .unwrap(),
            text
        )
    }
    handle.abort();
}
#[tokio::test]
async fn fallback_only_on_eligible_errors() {
    async fn fail(State(code): State<u16>) -> StatusCode {
        StatusCode::from_u16(code).unwrap()
    }
    async fn answer(State(c): State<Arc<AtomicUsize>>) -> Json<Value> {
        c.fetch_add(1, Ordering::SeqCst);
        Json(json!({"choices":[{"message":{"content":"fallback"}}]}))
    }
    let count = Arc::new(AtomicUsize::new(0));
    let (backup, b) = fixture(
        Router::new()
            .route("/chat/completions", post(answer))
            .with_state(count.clone()),
    )
    .await;
    let client = HttpCompletion::new(Duration::from_secs(2), 4096).unwrap();
    for (code, retries) in [
        (503, true),
        (404, true),
        (401, false),
        (429, false),
        (402, false),
    ] {
        let (base, h) = fixture(
            Router::new()
                .route("/chat/completions", post(fail))
                .with_state(code),
        )
        .await;
        let before = count.load(Ordering::SeqCst);
        let result = client
            .complete(
                &[route(&base, Shape::OpenAi), route(&backup, Shape::OpenAi)],
                "hello",
                CancellationToken::new(),
            )
            .await;
        assert_eq!(result.is_ok(), retries);
        assert_eq!(count.load(Ordering::SeqCst) - before, usize::from(retries));
        h.abort()
    }
    b.abort();
}
#[tokio::test]
async fn bounded_response_and_cancellation() {
    async fn huge() -> String {
        "x".repeat(2000)
    }
    async fn slow() -> Json<Value> {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Json(json!({}))
    }
    let (base, h) = fixture(Router::new().route("/chat/completions", post(huge))).await;
    let client = HttpCompletion::new(Duration::from_secs(1), 1024).unwrap();
    assert!(matches!(
        client
            .complete(
                &[route(&base, Shape::OpenAi)],
                "hello",
                CancellationToken::new()
            )
            .await,
        Err(Error::Code("response_too_large"))
    ));
    h.abort();
    let (base, h) = fixture(Router::new().route("/chat/completions", post(slow))).await;
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        child.cancel()
    });
    assert!(matches!(
        client
            .complete(&[route(&base, Shape::OpenAi)], "hello", cancel)
            .await,
        Err(Error::Code("cancelled"))
    ));
    h.abort();
}
#[test]
fn ssrf_addresses() {
    for ip in [
        "127.0.0.1",
        "10.0.0.1",
        "169.254.169.254",
        "100.64.0.1",
        "::1",
        "::ffff:127.0.0.1",
        "2001:db8::1",
        "2002:7f00:1::",
    ] {
        assert!(!public_ip(ip.parse().unwrap()), "{ip}")
    }
    assert!(public_ip("8.8.8.8".parse().unwrap()));
}
#[tokio::test]
async fn independent_responses_research_requires_citations() {
    use yoink_insight::research::{Research, ResponsesResearch};
    async fn answer(Json(body): Json<Value>) -> Json<Value> {
        assert_eq!(body["tools"][0]["type"], "web_search_preview");
        assert_eq!(body["stream"], false);
        Json(
            json!({"output":[{"type":"message","content":[{"type":"output_text","text":"The independently checked claim matches upstream docs.","annotations":[{"type":"url_citation","url":"https://docs.example.test/primary","title":"Primary evidence","start_index":0,"end_index":40}]}]}]}),
        )
    }
    let (base, h) = fixture(Router::new().route("/responses", post(answer))).await;
    let research = ResponsesResearch::new(Duration::from_secs(1), 4096).unwrap();
    let citations = research
        .search(
            &route(&base, Shape::OpenAi),
            "source under review",
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(citations.len(), 1);
    assert_eq!(citations[0].url, "https://docs.example.test/primary");
    assert!(
        yoink_insight::research::citations(
            &json!({"output":[{"content":[{"text":"Uncited claim"}]}]})
        )
        .is_err()
    );
    h.abort();
}

#[tokio::test]
async fn catalogue_consumes_bounded_pages_without_inventing_gemini_search() {
    use axum::{extract::Query, routing::get};
    use std::collections::HashMap;
    async fn page(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
        if q.contains_key("pageToken") {
            assert_eq!(q["pageToken"], "next & token");
            Json(
                json!({"models":[{"name":"models/gemini-two","supportedGenerationMethods":["generateContent"]}]}),
            )
        } else {
            Json(
                json!({"models":[{"name":"models/gemini-one","supportedGenerationMethods":["generateContent"]},{"name":"models/embedding","supportedGenerationMethods":["embedContent"]}],"nextPageToken":"next & token"}),
            )
        }
    }
    let (base, h) = fixture(Router::new().route("/models", get(page))).await;
    let client = HttpCompletion::new(Duration::from_secs(2), 4096).unwrap();
    let models = client
        .models(&route(&base, Shape::Gemini), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["gemini-one", "gemini-two"]
    );
    assert!(models.iter().all(|m| !m.supports_websearch));
    h.abort();
    async fn anthropic(Query(q): Query<HashMap<String, String>>) -> Json<Value> {
        if q.contains_key("after_id") {
            Json(json!({"data":[{"id":"second"}],"has_more":false}))
        } else {
            Json(json!({"data":[{"id":"first"}],"has_more":true,"last_id":"first"}))
        }
    }
    let (base, h) = fixture(Router::new().route("/models", get(anthropic))).await;
    assert_eq!(
        client
            .models(&route(&base, Shape::Anthropic), CancellationToken::new())
            .await
            .unwrap()
            .len(),
        2
    );
    h.abort();
}

#[tokio::test]
async fn catalogue_repeated_cursor_and_byte_budget_fail_closed() {
    use axum::routing::get;
    let client = HttpCompletion::new(Duration::from_secs(2), 1024).unwrap();
    let (base, h) = fixture(Router::new().route(
        "/models",
        get(|| async { Json(json!({"models":[],"nextPageToken":"same"})) }),
    ))
    .await;
    assert!(matches!(
        client
            .models(&route(&base, Shape::Gemini), CancellationToken::new())
            .await,
        Err(Error::Code("catalogue_pagination_invalid"))
    ));
    h.abort();
    let (base, h) =
        fixture(Router::new().route("/models", get(|| async { "x".repeat(1025) }))).await;
    assert!(matches!(
        client
            .models(&route(&base, Shape::OpenAi), CancellationToken::new())
            .await,
        Err(Error::Code("response_too_large"))
    ));
    h.abort();
}

#[tokio::test]
async fn credential_redirect_is_not_followed_and_custom_loopback_is_denied() {
    use axum::response::Redirect;
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let (target, t) = fixture(Router::new().route(
        "/leak",
        post(move || {
            let h = h.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                StatusCode::OK
            }
        }),
    ))
    .await;
    let (base, b) = fixture(Router::new().route(
        "/chat/completions",
        post(move || {
            let url = format!("{target}/leak");
            async move { Redirect::temporary(&url) }
        }),
    ))
    .await;
    let client = HttpCompletion::new(Duration::from_secs(2), 4096).unwrap();
    assert!(matches!(
        client
            .complete(
                &[route(&base, Shape::OpenAi)],
                "prompt",
                CancellationToken::new()
            )
            .await,
        Err(Error::Code("llm_error"))
    ));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    let mut custom = route(&base, Shape::OpenAi);
    custom.via = "byok";
    assert!(matches!(
        client
            .complete(&[custom], "prompt", CancellationToken::new())
            .await,
        Err(Error::Code("unsafe_endpoint"))
    ));
    t.abort();
    b.abort();
}

#[tokio::test]
async fn public_openrouter_catalogue_cannot_validate_invalid_keys() {
    async fn denied() -> StatusCode {
        StatusCode::UNAUTHORIZED
    }
    async fn public_models() -> Json<Value> {
        Json(json!({"data":[{"id":"public-model"}]}))
    }
    let (base, server) = fixture(
        Router::new()
            .route("/key", axum::routing::get(denied))
            .route("/models", axum::routing::get(public_models)),
    )
    .await;
    let client = HttpCompletion::new(Duration::from_secs(2), 4096).unwrap();
    let mut selected = route(&base, Shape::OpenAi);
    selected.via = "openrouter"; // trusted loopback operator fixture, not BYOK
    assert!(matches!(
        client.models(&selected, CancellationToken::new()).await,
        Err(Error::Code("auth_failed"))
    ));
    server.abort();
}

#[tokio::test]
async fn perplexity_authenticates_with_tiny_sonar_chat_without_catalogue() {
    async fn ping(Json(body): Json<Value>) -> Json<Value> {
        assert_eq!(body["model"], "sonar");
        assert_eq!(body["max_tokens"], 1);
        Json(json!({"choices":[{"message":{"content":"pong"}}]}))
    }
    let (base, server) = fixture(Router::new().route("/chat/completions", post(ping))).await;
    let client = HttpCompletion::new(Duration::from_secs(2), 4096).unwrap();
    let mut selected = route(&base, Shape::OpenAi);
    selected.via = "perplexity";
    let models = client
        .models(&selected, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(models.len(), 5);
    assert!(models.iter().all(|model| model.supports_websearch));
    server.abort();
}

#[test]
fn all_seven_byok_provider_urls_are_executable_and_confined() {
    use yoink_insight::provider::{byok, contract};
    for (provider, base, endpoint) in [
        ("openai", None, "https://api.openai.com/v1/chat/completions"),
        (
            "openrouter",
            None,
            "https://openrouter.ai/api/v1/chat/completions",
        ),
        (
            "perplexity",
            None,
            "https://api.perplexity.ai/chat/completions",
        ),
        (
            "gemini",
            Some("https://generativelanguage.googleapis.com/v1beta/openai"),
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
        ),
        (
            "anthropic",
            Some("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages",
        ),
        (
            "custom_openai",
            Some("https://fixture.example/v1"),
            "https://fixture.example/v1/chat/completions",
        ),
        (
            "custom_anthropic",
            Some("https://fixture.example/v1/messages"),
            "https://fixture.example/v1/messages",
        ),
    ] {
        let selected = byok(
            provider,
            base,
            "fixture-secret".into(),
            "fixture-model".into(),
        )
        .unwrap();
        assert!(selected.is_byok());
        assert_eq!(contract(&selected, "prompt").unwrap().0, endpoint);
    }
}
