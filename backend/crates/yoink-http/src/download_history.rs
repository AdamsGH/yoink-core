//! Download history routes for the existing SPA.
use crate::auth::{ApiError, AuthState, authenticated_id, no_store};
use axum::{
    Router,
    extract::{Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use yoink_downloads::{DownloadError, history::HistoryQuery};

pub(crate) fn router() -> Router<AuthState> {
    Router::new()
        .route("/api/v1/dl/downloads", get(list))
        .route("/api/v1/dl/downloads/domains", get(domains))
        .route("/api/v1/dl/downloads/{id}/retry", post(retry))
}
fn error(e: DownloadError) -> ApiError {
    match e {
        DownloadError::Denied => ApiError(StatusCode::FORBIDDEN, "Access denied"),
        DownloadError::InvalidInput => {
            ApiError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid history query")
        }
        DownloadError::QuotaExceeded { .. } | DownloadError::Capacity => {
            ApiError(StatusCode::TOO_MANY_REQUESTS, "Download capacity exceeded")
        }
        _ => ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Download history unavailable",
        ),
    }
}
async fn list(
    State(s): State<AuthState>,
    h: HeaderMap,
    filters: Result<Query<HistoryQuery>, QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    let Query(filters) = filters.map_err(|_| error(DownloadError::InvalidInput))?;
    Ok(no_store(
        s.downloader_settings()?
            .download_history(actor as u64, filters)
            .await
            .map_err(error)?,
    ))
}
async fn domains(State(s): State<AuthState>, h: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    Ok(no_store(
        s.downloader_settings()?
            .download_domains(actor as u64)
            .await
            .map_err(error)?,
    ))
}
async fn retry(
    State(s): State<AuthState>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)? as u64;
    let source = s
        .downloader_settings()?
        .history_source(actor, &id)
        .await
        .map_err(error)?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "Download not found"))?;
    let service = s.cookies.get().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Downloader unavailable",
    ))?;
    // The authenticated actor's private chat is the only retry delivery target.
    let admission = service
        .retry_download(actor, &source)
        .await
        .map_err(error)?;
    Ok(no_store(
        serde_json::json!({"status":"queued","operation_id":admission.operation_id}),
    ))
}
