//! Late product routes and admission ownership for their direct HTTP work.
use crate::{AuthState, activity::Activity, auth::ApiError};
use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
};
use std::sync::{Arc, OnceLock};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use yoink_application::SystemService;
use yoink_music::NativeMusicPersistence;

#[derive(Clone, Copy, Default)]
struct Requests {
    closed: bool,
    active: usize,
}

#[derive(Clone)]
struct Admission(watch::Sender<Requests>);
impl Default for Admission {
    fn default() -> Self {
        Self(watch::channel(Requests::default()).0)
    }
}
impl Admission {
    fn admit(&self) -> Option<RequestGuard> {
        let mut admitted = false;
        self.0.send_modify(|state| {
            if !state.closed {
                state.active += 1;
                admitted = true;
            }
        });
        admitted.then(|| RequestGuard(self.0.clone()))
    }
    fn stop(&self) {
        self.0.send_modify(|state| state.closed = true);
    }
    async fn drain(&self) {
        self.stop();
        let mut requests = self.0.subscribe();
        loop {
            if requests.borrow_and_update().active == 0 {
                return;
            }
            if requests.changed().await.is_err() {
                return;
            }
        }
    }
}
struct RequestGuard(watch::Sender<Requests>);
impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.0.send_modify(|state| state.active -= 1);
    }
}

#[derive(Clone, Default)]
pub struct LateMusicStats {
    routes: Arc<OnceLock<Router>>,
    admission: Admission,
    cancellation: CancellationToken,
}
impl LateMusicStats {
    pub fn install(
        &self,
        system: SystemService,
        auth: AuthState,
        music: Option<Arc<NativeMusicPersistence>>,
        stats: Option<yoink_stats::Stats>,
        activity: Activity,
    ) -> Result<(), &'static str> {
        self.install_with_stats_imports(system, auth, music, stats, activity, None)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn install_with_stats_imports(
        &self,
        system: SystemService,
        auth: AuthState,
        music: Option<Arc<NativeMusicPersistence>>,
        stats: Option<yoink_stats::Stats>,
        activity: Activity,
        imports: Option<crate::stats_import::ImportJobs>,
    ) -> Result<(), &'static str> {
        let mut routes = activity.router(auth.clone());
        if let Some(music) = music {
            routes = routes.merge(crate::music_router(system.clone(), auth.clone(), music));
        }
        if let Some(stats) = stats {
            routes = routes.merge(match imports {
                Some(jobs) => crate::stats_router_with_imports(system.clone(), auth, stats, jobs),
                None => crate::stats_router(system.clone(), auth, stats),
            });
        }
        self.install_routes(system, routes)
    }
    fn install_routes(&self, system: SystemService, routes: Router) -> Result<(), &'static str> {
        self.routes
            .set(routes.layer(from_fn_with_state(
                (system, self.admission.clone(), self.cancellation.clone()),
                guarded,
            )))
            .map_err(|_| "Music/Stats HTTP routes already installed")
    }
    pub(crate) fn routes_for(&self, path: &str) -> Option<&Router> {
        let activity = path
            .strip_prefix("/api/v1/users/")
            .and_then(|path| path.strip_suffix("/stats"))
            .is_some_and(|id| !id.is_empty() && !id.contains('/'));
        let product = ["/api/v1/music", "/api/v1/stats"].iter().any(|prefix| {
            path == *prefix
                || path
                    .strip_prefix(prefix)
                    .is_some_and(|p| p.starts_with('/'))
        });
        (activity || product).then(|| self.routes.get()).flatten()
    }
    pub fn stop_admission(&self) {
        self.admission.stop();
    }
    /// Cancels active service futures if graceful HTTP draining times out.
    /// Drain must still finish before product or database shutdown.
    pub fn cancel_all(&self) {
        self.stop_admission();
        self.cancellation.cancel();
    }
    /// The parent must await this before Stats search shutdown or database close.
    /// Its global shutdown deadline owns the timeout; cancellation can be retried.
    pub async fn drain(&self) {
        self.admission.drain().await;
    }
}

async fn guarded(
    State((system, admission, cancellation)): State<(SystemService, Admission, CancellationToken)>,
    request: Request,
    next: Next,
) -> Response {
    // These handlers materialize JSON before returning. The permit covers all
    // service/database work, not transmission of the already-built response.
    let mut response = if system.snapshot().ready {
        match admission.admit() {
            Some(_guard) => tokio::select! {
                response = next.run(request) => response,
                () = cancellation.cancelled() => ApiError(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable").into_response(),
            },
            None => {
                ApiError(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable").into_response()
            }
        }
    } else {
        ApiError(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable").into_response()
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::get};
    use std::time::Duration;
    use tokio::sync::Semaphore;
    use tower::ServiceExt;

    async fn get_response(app: &Router, path: &str) -> Response {
        app.clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn late_routes_preserve_precedence_readiness_and_drain_accepted_requests() {
        let system = SystemService::new("fixture");
        let products = LateMusicStats::default();
        let inbox = crate::LateInbox::default();
        inbox
            .0
            .set(Router::new().fallback(|| async { "inbox" }))
            .unwrap();
        let app = crate::router_with_late_music_stats(
            system.clone(),
            None,
            AuthState::new(false),
            None,
            inbox,
            crate::LateInsight::default(),
            products.clone(),
        );
        assert_eq!(
            get_response(&app, "/api/v1/users/me/stats").await.status(),
            StatusCode::NOT_FOUND
        );
        let reached = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let pending = {
            let reached = reached.clone();
            let release = release.clone();
            move || {
                let reached = reached.clone();
                let release = release.clone();
                async move {
                    reached.add_permits(1);
                    release.acquire().await.unwrap().forget();
                    "stats"
                }
            }
        };
        products
            .install_routes(
                system.clone(),
                Router::new()
                    .route("/api/v1/stats/groups", get(pending.clone()))
                    .route("/api/v1/music/me/stats", get(|| async { "music" }))
                    .route("/api/v1/users/{id}/stats", get(|| async { "activity" })),
            )
            .unwrap();
        assert!(
            products
                .install_routes(system.clone(), Router::new())
                .is_err()
        );
        let response = get_response(&app, "/api/v1/music/me/stats").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        system.set_ready(true);
        for path in [
            "/api/v1/users/me/stats",
            "/api/v1/users/42/stats",
            "/api/v1/music/me/stats",
        ] {
            assert_eq!(get_response(&app, path).await.status(), StatusCode::OK);
        }
        assert_eq!(
            get_response(&app, "/api/v1/users/42/other").await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get_response(&app, "/api/v1/music-extra").await.status(),
            StatusCode::OK
        );
        let request_app = app.clone();
        let request =
            tokio::spawn(async move { get_response(&request_app, "/api/v1/stats/groups").await });
        reached.acquire().await.unwrap().forget();
        products.stop_admission();
        let response = get_response(&app, "/api/v1/users/me/stats").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let draining = products.clone();
        let mut drain = tokio::spawn(async move { draining.drain().await });
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut drain)
                .await
                .is_err()
        );
        release.add_permits(1);
        assert_eq!(request.await.unwrap().status(), StatusCode::OK);
        tokio::time::timeout(Duration::from_secs(1), drain)
            .await
            .unwrap()
            .unwrap();

        for abort in [true, false] {
            let cancelled = LateMusicStats::default();
            cancelled
                .install_routes(
                    system.clone(),
                    Router::new().route("/api/v1/stats/groups", get(pending.clone())),
                )
                .unwrap();
            let cancelled_app = cancelled
                .routes_for("/api/v1/stats/groups")
                .unwrap()
                .clone();
            let request =
                tokio::spawn(
                    async move { get_response(&cancelled_app, "/api/v1/stats/groups").await },
                );
            reached.acquire().await.unwrap().forget();
            if abort {
                cancelled.stop_admission();
                request.abort();
                assert!(request.await.unwrap_err().is_cancelled());
            } else {
                cancelled.cancel_all();
                let response = request.await.unwrap();
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            }
            tokio::time::timeout(Duration::from_secs(1), cancelled.drain())
                .await
                .unwrap();
        }

        let admission = Admission::default();
        let guard = admission.admit().unwrap();
        let mut cancelled_drain = Box::pin(admission.drain());
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut cancelled_drain)
                .await
                .is_err()
        );
        drop(cancelled_drain);
        assert!(admission.admit().is_none());
        drop(guard);
        tokio::time::timeout(Duration::from_secs(1), admission.drain())
            .await
            .unwrap();
    }
}
