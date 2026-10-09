use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Request, header},
    routing::get,
};
use std::{sync::Arc, time::Duration};
use tower::ServiceExt;
use yoink_auth::{AuthConfig, JwtProfile};
use yoink_identity::{IdentityStore, TrustedProfile};
#[tokio::test]
async fn observations_exclude_legacy_health_metrics_and_count_nested_routes_once() {
    let system = SystemService::new("test");
    let app = instrument(
        instrument(
            Router::new()
                .route("/ok", get(|| async { StatusCode::OK }))
                .route("/bad", get(|| async { StatusCode::BAD_REQUEST }))
                .route("/health", get(|| async { StatusCode::OK }))
                .route("/metrics", get(|| async { StatusCode::OK })),
            system.clone(),
        ),
        system.clone(),
    );
    let mut tasks = tokio::task::JoinSet::new();
    for path in ["/ok", "/bad", "/missing", "/health", "/metrics"] {
        let app = app.clone();
        tasks.spawn(async move {
            app.oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap()
        });
    }
    while tasks.join_next().await.is_some() {}
    let s = system.metrics().snapshot();
    assert_eq!(s.counters["api_requests_total"], 3);
    assert_eq!(s.counters["api_errors_total"], 2);
    assert_eq!(s.histograms["api_response_seconds"].count, 3);
}
#[tokio::test]
async fn snapshot_requires_readiness_and_current_admin_authority_without_counting_itself() {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store =
        yoink_storage::Store::open(&dir.path().join("metrics.db"), true, Duration::from_secs(2))
            .await
            .unwrap();
    store.migrate().await.unwrap();
    let identity = Arc::new(IdentityStore::new(store.database().clone(), Some(1), false));
    for user_id in [1, 2, 3] {
        identity
            .register(&TrustedProfile {
                user_id,
                username: None,
                first_name: None,
                photo_url: None,
            })
            .await
            .unwrap();
    }
    yoink_db::query("UPDATE users SET role='admin' WHERE id=3")
        .execute(store.database())
        .await
        .unwrap();
    let crypto = AuthConfig::new(
        "fixture".into(),
        "fixture-metrics-secret-32-bytes-long".into(),
        86400,
        30,
        86400,
    )
    .unwrap();
    let tokens: Vec<_> = (1..=3)
        .map(|user_id| {
            crypto
                .issue_token(
                    JwtProfile {
                        user_id,
                        role: "owner",
                        username: None,
                        first_name: None,
                    },
                    chrono::Utc::now().timestamp(),
                )
                .unwrap()
        })
        .collect();
    let auth = AuthState::new(false);
    assert!(
        auth.install(crate::AuthService {
            identities: identity,
            crypto
        })
        .is_ok()
    );
    let system = SystemService::new("test");
    let app = crate::router_with_auth(system.clone(), Some(auth));
    async fn call(app: &Router, token: Option<&str>) -> Response {
        let mut r = Request::builder().uri("/metrics");
        if let Some(t) = token {
            r = r.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        app.clone()
            .oneshot(r.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
    async fn metadata(app: &Router, token: Option<&str>) -> Response {
        let mut request = Request::builder().uri("/api/v1/_meta/openapi.json");
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        app.clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
    assert_eq!(
        call(&app, Some(&tokens[0])).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    system.set_ready(true);
    assert_eq!(call(&app, None).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        call(&app, Some(&tokens[1])).await.status(),
        StatusCode::FORBIDDEN
    );
    for token in [&tokens[0], &tokens[2]] {
        let response = call(&app, Some(token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let v: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert!(v["counters"].as_object().unwrap().is_empty());
        assert!(v["histograms"].as_object().unwrap().is_empty());
        assert!(v["uptime_seconds"].is_number());
    }
    assert_eq!(
        metadata(&app, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        metadata(&app, Some(&tokens[1])).await.status(),
        StatusCode::FORBIDDEN
    );
    for token in [&tokens[0], &tokens[2]] {
        let response = metadata(&app, Some(token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let document: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap())
                .unwrap();
        assert_eq!(document["openapi"], "3.1.0");
        assert_eq!(
            document["components"]["securitySchemes"]["HTTPBearer"]["scheme"],
            "bearer"
        );
        assert!(document["paths"]["/metrics"]["get"].is_object());
        assert!(document["paths"]["/api/v1/music/me/stats"]["get"].is_object());
        assert!(document["paths"]["/api/v1/stats/import"]["post"].is_object());
    }
    yoink_db::query("UPDATE users SET ban_until=4070908800000000 WHERE id=3")
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        call(&app, Some(&tokens[2])).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        metadata(&app, Some(&tokens[2])).await.status(),
        StatusCode::FORBIDDEN
    );
    yoink_db::query("UPDATE users SET role='user',ban_until=NULL WHERE id=3")
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        call(&app, Some(&tokens[2])).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        metadata(&app, Some(&tokens[2])).await.status(),
        StatusCode::FORBIDDEN
    );
    store.close().await.unwrap();
}
