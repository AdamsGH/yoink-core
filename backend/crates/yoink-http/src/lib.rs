//! Thin HTTP adapter for Yoink application services.

mod admin;
mod auth;
pub use auth::{AuthService, AuthState};

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde::Serialize;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use yoink_application::{SystemService, SystemSnapshot};

#[derive(Serialize)]
struct HealthStatus {
    status: &'static str,
}

/// Build the diagnostics HTTP router.
pub fn router(system: SystemService) -> Router {
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/api/v1/system", get(system_snapshot))
        .with_state(system)
}

pub fn router_with_auth(system: SystemService, auth: Option<AuthState>) -> Router {
    let diagnostics = router(system.clone());
    match auth {
        Some(state) => diagnostics.merge(auth::router(state, system)),
        None => diagnostics,
    }
}

pub async fn serve_with_auth(
    listener: TcpListener,
    system: SystemService,
    auth: Option<AuthState>,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    axum::serve(listener, router_with_auth(system, auth))
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}

async fn live() -> Json<HealthStatus> {
    Json(HealthStatus { status: "live" })
}

async fn ready(State(system): State<SystemService>) -> (StatusCode, Json<HealthStatus>) {
    let is_ready = system.snapshot().ready;
    let status = if is_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = HealthStatus {
        status: if is_ready { "ready" } else { "not_ready" },
    };
    (status, Json(body))
}

async fn system_snapshot(State(system): State<SystemService>) -> Json<SystemSnapshot> {
    Json(system.snapshot())
}

/// Serve HTTP on a listener bound by the composition root, stopping on cancellation.
pub async fn serve(
    listener: TcpListener,
    system: SystemService,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    axum::serve(listener, router(system))
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request},
    };
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;

    async fn request(app: Router, method: Method, uri: &str) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    }

    async fn json(response: axum::response::Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn live_is_always_successful() {
        let app = router(SystemService::new("test"));
        let response = request(app, Method::GET, "/health/live").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json(response).await, serde_json::json!({"status": "live"}));
    }

    #[tokio::test]
    async fn readiness_tracks_shared_service_transitions() {
        let system = SystemService::new("test");
        let app = router(system.clone());
        let response = request(app.clone(), Method::GET, "/health/ready").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            json(response).await,
            serde_json::json!({"status": "not_ready"})
        );

        system.set_ready(true);
        let response = request(app.clone(), Method::GET, "/health/ready").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json(response).await, serde_json::json!({"status": "ready"}));

        system.set_ready(false);
        let response = request(app, Method::GET, "/health/ready").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn system_endpoint_returns_snapshot_and_planned_catalog() {
        let system = SystemService::new("test-version");
        system.set_ready(true);
        let response = request(router(system), Method::GET, "/api/v1/system").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json(response).await;
        assert_eq!(body["version"], "test-version");
        assert_eq!(body["ready"], true);
        let modules = body["modules"].as_array().unwrap();
        assert_eq!(modules.len(), 5);
        assert!(modules.iter().all(|module| module["status"] == "planned"));
        assert_eq!(
            modules
                .iter()
                .map(|module| module["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["downloads", "inbox", "insight", "music", "stats"]
        );
    }

    #[tokio::test]
    async fn unknown_routes_and_methods_use_axum_defaults() {
        let app = router(SystemService::new("test"));
        assert_eq!(
            request(app.clone(), Method::GET, "/missing").await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(app, Method::POST, "/health/live").await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[tokio::test]
    async fn serve_exits_after_cancellation() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(serve(
            listener,
            SystemService::new("test"),
            shutdown.clone(),
        ));
        shutdown.cancel();
        assert!(task.await.unwrap().is_ok());
    }
}
