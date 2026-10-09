//! Mount below /stats. Shared WebApp/JWT middleware inserts this identity only after verification.
use crate::{Error, Stats, analytics::Params};
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
#[derive(Clone, Copy)]
pub struct AuthenticatedUser(pub i64);
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::Invalid => StatusCode::BAD_REQUEST,
            Self::Unavailable | Self::Session(yoink_user_session::Error::Unavailable) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::Session(_) => StatusCode::BAD_GATEWAY,
            Self::Search(yoink_stats_search::SearchError::InvalidInput(_)) => {
                StatusCode::BAD_REQUEST
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({"detail":self.to_string()}))).into_response()
    }
}
pub fn router(stats: Stats) -> Router {
    Router::new()
        .route("/groups", get(groups))
        .route("/members/sync", post(sync))
        .route("/{endpoint}", get(analytics))
        .layer(axum::middleware::from_fn(require_identity))
        .with_state(stats)
}
async fn require_identity(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if req.extensions().get::<AuthenticatedUser>().is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"detail":"authentication_required"})),
        )
            .into_response();
    }
    next.run(req).await
}
async fn groups(
    State(s): State<Stats>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, Error> {
    s.groups(u.0).await.map(Json)
}
async fn sync(
    State(s): State<Stats>,
    Extension(u): Extension<AuthenticatedUser>,
    Query(p): Query<Params>,
) -> Result<Json<Value>, Error> {
    s.sync_members(u.0, p.chat_id).await.map(Json)
}
async fn analytics(
    State(s): State<Stats>,
    Extension(u): Extension<AuthenticatedUser>,
    Path(endpoint): Path<String>,
    Query(p): Query<Params>,
) -> Result<Json<Value>, Error> {
    s.analytics(u.0, &endpoint, p).await.map(Json)
}
