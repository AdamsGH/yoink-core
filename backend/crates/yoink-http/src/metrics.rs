use crate::auth::{ApiError, AuthState, authenticated_id, no_store};
use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::get,
};
use yoink_application::{
    SystemService,
    metrics::{Counter, Histogram},
};

pub(crate) fn router(system: SystemService, auth: AuthState) -> Router {
    Router::new()
        .route("/metrics", get(snapshot))
        .with_state((system, auth))
}
async fn snapshot(
    State((system, auth)): State<(SystemService, AuthState)>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    if !system.snapshot().ready {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service unavailable",
        ));
    }
    let service = auth.service()?;
    let actor = authenticated_id(service, &headers)?;
    service
        .identities
        .configuration_authorized(actor, false)
        .await?;
    Ok(no_store(system.metrics().snapshot()))
}
#[derive(Clone, Copy)]
struct Observed;
pub(crate) fn instrument(app: Router, system: SystemService) -> Router {
    app.layer(from_fn_with_state(system, observe))
}
async fn observe(
    State(system): State<SystemService>,
    mut request: Request,
    next: Next,
) -> Response {
    // Composed compatibility constructors may nest this layer. Count at the outer boundary only.
    if matches!(request.uri().path(), "/health" | "/metrics")
        || request.extensions().get::<Observed>().is_some()
    {
        return next.run(request).await;
    }
    request.extensions_mut().insert(Observed);
    let metrics = system.metrics();
    metrics.increment(Counter::ApiRequests, 1);
    let start = std::time::Instant::now();
    let response = next.run(request).await;
    metrics.observe(Histogram::ApiResponse, start.elapsed().as_secs_f64());
    if response.status().as_u16() >= 400 {
        metrics.increment(Counter::ApiErrors, 1);
    }
    response
}
#[cfg(test)]
mod tests;
