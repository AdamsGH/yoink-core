//! Thin HTTP adapter for Yoink application services.

pub mod activity;
mod admin;
mod auth;
mod configuration;
mod cookie_web;
mod download_history;
mod downloader_settings;
mod groups;
mod health;
mod inbox;
mod insight;
pub use inbox::InboxAuthorization;
pub use insight::LateInsight;
mod m2m;
mod metrics;
mod music;
pub use music::router as music_router;
mod music_stats;
pub use music_stats::LateMusicStats;
mod nsfw;
mod stats;
pub mod stats_import;
pub use stats::{router as stats_router, router_with_imports as stats_router_with_imports};
mod user_session;
pub use auth::{AuthService, AuthState};
pub use user_session::router as user_session_router;

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
    metrics::instrument(
        diagnostics(system.clone()).merge(health::router(system.clone(), None)),
        system,
    )
}

fn diagnostics(system: SystemService) -> Router {
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/api/v1/system", get(system_snapshot))
        .with_state(system)
}

pub fn router_with_auth(system: SystemService, auth: Option<AuthState>) -> Router {
    let diagnostics =
        diagnostics(system.clone()).merge(health::router(system.clone(), auth.clone()));
    let app = match auth {
        Some(state) => diagnostics
            .merge(cookie_web::public_router(state.clone(), system.clone()))
            .merge(metrics::router(system.clone(), state.clone()))
            .merge(authenticated_routes(state, system.clone(), None)),
        None => diagnostics,
    };
    metrics::instrument(app, system)
}

pub fn router_with_identity(
    system: SystemService,
    auth: Option<AuthState>,
    identities: AuthState,
) -> Router {
    router_with_identity_and_user_session(system, auth, identities, None)
}

pub fn router_with_identity_and_user_session(
    system: SystemService,
    auth: Option<AuthState>,
    identities: AuthState,
    client: Option<std::sync::Arc<yoink_user_session::Client>>,
) -> Router {
    let diagnostics =
        diagnostics(system.clone()).merge(health::router(system.clone(), Some(identities.clone())));
    let diagnostics = diagnostics.merge(cookie_web::public_router(
        identities.clone(),
        system.clone(),
    ));
    let app = match auth {
        Some(state) => diagnostics
            .merge(metrics::router(system.clone(), state.clone()))
            .merge(authenticated_routes(state, system.clone(), client)),
        None => diagnostics,
    };
    metrics::instrument(app.merge(m2m::router(identities, system.clone())), system)
}

/// Compose native Inbox dependencies supplied by the application root.
/// Existing constructors remain unchanged; this does not install an Inbox runtime.
/// Inbox routes are absent when bearer authentication is disabled. Store, Insight
/// and GitHub are retained; HTTP authority is replaced with the shared AuthService
/// identity store so a supplied authorization implementation cannot bypass it.
pub fn router_with_identity_user_session_and_inbox(
    system: SystemService,
    auth: Option<AuthState>,
    identities: AuthState,
    client: Option<std::sync::Arc<yoink_user_session::Client>>,
    inbox: yoink_inbox::http::HttpState,
) -> Router {
    let app =
        router_with_identity_and_user_session(system.clone(), auth.clone(), identities, client);
    let app = match auth {
        Some(auth) => app.merge(inbox::router(system.clone(), auth, inbox)),
        None => app,
    };
    metrics::instrument(app, system)
}

fn authenticated_routes(
    state: AuthState,
    system: SystemService,
    client: Option<std::sync::Arc<yoink_user_session::Client>>,
) -> Router {
    let app = auth::router(state.clone(), system.clone());
    let session = user_session_router(state, client).route_layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let ready = system.snapshot().ready;
            async move {
                use axum::response::IntoResponse;
                if ready {
                    next.run(request).await
                } else {
                    auth::ApiError(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable")
                        .into_response()
                }
            }
        },
    ));
    app.merge(session)
}

pub async fn serve_with_identity(
    listener: TcpListener,
    system: SystemService,
    auth: Option<AuthState>,
    identities: AuthState,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    axum::serve(listener, router_with_identity(system, auth, identities))
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
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

/// Serve the authenticated router with its configured user-mode client.
pub async fn serve_with_user_session(
    listener: TcpListener,
    system: SystemService,
    auth: Option<AuthState>,
    identities: AuthState,
    client: Option<std::sync::Arc<yoink_user_session::Client>>,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    serve_with_user_session_and_inbox(listener, system, auth, identities, client, None, shutdown)
        .await
}

/// Inbox is present only after the composition root constructs its real dependencies.
pub async fn serve_with_user_session_and_inbox(
    listener: TcpListener,
    system: SystemService,
    auth: Option<AuthState>,
    identities: AuthState,
    client: Option<std::sync::Arc<yoink_user_session::Client>>,
    inbox: Option<yoink_inbox::http::HttpState>,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let app = match inbox {
        Some(inbox) => {
            router_with_identity_user_session_and_inbox(system, auth, identities, client, inbox)
        }
        None => router_with_identity_and_user_session(system, auth, identities, client),
    };
    axum::serve(listener, app)
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

    #[tokio::test]
    async fn m2m_without_jwt_does_not_register_disabled_auth_routes() {
        let system = SystemService::new("test");
        system.set_ready(true);
        let app = router_with_identity(system, None, AuthState::new(false));
        for endpoint in ["/api/v1/auth/token", "/api/v1/auth/dev"] {
            assert_eq!(
                request(app.clone(), Method::POST, endpoint).await.status(),
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            request(app.clone(), Method::GET, "/api/v1/users/me")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(app, Method::GET, "/api/internal/v1/status")
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
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

/// Late installation preserves diagnostics while dependencies are prepared.
#[derive(Clone, Default)]
pub struct LateInbox(std::sync::Arc<std::sync::OnceLock<axum::Router>>);
impl LateInbox {
    pub fn install(
        &self,
        system: SystemService,
        auth: AuthState,
        state: yoink_inbox::http::HttpState,
    ) -> Result<(), &'static str> {
        self.0
            .set(inbox::router(system, auth, state))
            .map_err(|_| "Inbox already installed")
    }
}
pub async fn serve_with_late_inbox(
    listener: TcpListener,
    system: SystemService,
    auth: Option<AuthState>,
    identities: AuthState,
    client: Option<std::sync::Arc<yoink_user_session::Client>>,
    inbox: LateInbox,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    serve_with_late_products(
        listener,
        system,
        auth,
        identities,
        client,
        inbox,
        LateInsight::default(),
        shutdown,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
pub async fn serve_with_late_products(
    listener: TcpListener,
    system: SystemService,
    auth: Option<AuthState>,
    identities: AuthState,
    client: Option<std::sync::Arc<yoink_user_session::Client>>,
    inbox: LateInbox,
    insight: LateInsight,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let app = router_with_late_products(system, auth, identities, client, inbox, insight);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}
pub fn router_with_late_products(
    system: SystemService,
    auth: Option<AuthState>,
    identities: AuthState,
    client: Option<std::sync::Arc<yoink_user_session::Client>>,
    inbox: LateInbox,
    insight: LateInsight,
) -> Router {
    router_with_late_music_stats(
        system,
        auth,
        identities,
        client,
        inbox,
        insight,
        LateMusicStats::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub async fn serve_with_late_music_stats(
    listener: TcpListener,
    system: SystemService,
    auth: Option<AuthState>,
    identities: AuthState,
    client: Option<std::sync::Arc<yoink_user_session::Client>>,
    inbox: LateInbox,
    insight: LateInsight,
    music_stats: LateMusicStats,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let app = router_with_late_music_stats(
        system,
        auth,
        identities,
        client,
        inbox,
        insight,
        music_stats,
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}

pub fn router_with_late_music_stats(
    system: SystemService,
    auth: Option<AuthState>,
    identities: AuthState,
    client: Option<std::sync::Arc<yoink_user_session::Client>>,
    inbox: LateInbox,
    insight: LateInsight,
    music_stats: LateMusicStats,
) -> Router {
    use tower::ServiceExt;
    let app = router_with_identity_and_user_session(system.clone(), auth, identities, client)
        .fallback(move |request: axum::extract::Request| {
            let inbox = inbox.clone();
            let insight = insight.clone();
            let music_stats = music_stats.clone();
            async move {
                let path = request.uri().path();
                let selected = if let Some(router) = music_stats.routes_for(path) {
                    Some(router)
                } else if path.starts_with("/api/v1/insight/") || path.starts_with("/api/v1/users/")
                {
                    insight.get()
                } else {
                    inbox.0.get()
                };
                match selected {
                    Some(router) => router.clone().oneshot(request).await.unwrap(),
                    None => axum::response::IntoResponse::into_response(
                        axum::http::StatusCode::NOT_FOUND,
                    ),
                }
            }
        });
    metrics::instrument(app, system)
}
