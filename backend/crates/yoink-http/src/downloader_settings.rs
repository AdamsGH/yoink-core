//! Existing frontend downloader settings contract, authenticated caller only.
use crate::auth::{ApiError, AuthState, authenticated_id, no_store};
use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use yoink_downloads::{DownloadError, UserSettingsPatch};

pub(crate) fn router() -> Router<AuthState> {
    Router::new().route("/api/v1/dl/settings", get(read).patch(patch))
}
fn invalid() -> ApiError {
    ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Invalid downloader settings",
    )
}
fn map_error(e: DownloadError) -> ApiError {
    match e {
        DownloadError::Denied => ApiError(StatusCode::FORBIDDEN, "Access denied"),
        DownloadError::Conflict => ApiError(StatusCode::CONFLICT, "Settings changed; reload"),
        DownloadError::InvalidPreferences => invalid(),
        _ => ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Downloader settings unavailable",
        ),
    }
}
async fn read(State(s): State<AuthState>, h: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    let value = s
        .downloader_settings()?
        .user_settings(actor as u64)
        .await
        .map_err(map_error)?;
    Ok((
        [("etag", format!("\"{}\"", value.revision))],
        no_store(value),
    ))
}
async fn patch(
    State(s): State<AuthState>,
    h: HeaderMap,
    body: Result<Json<UserSettingsPatch>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    let Json(body) = body.map_err(|_| invalid())?;
    let revision = h
        .get("if-match")
        .map(|v| {
            let text = v.to_str().map_err(|_| invalid())?.trim_matches('"');
            if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid());
            }
            text.parse::<i64>().map_err(|_| invalid())
        })
        .transpose()?;
    let value = s
        .downloader_settings()?
        .patch_user_settings(actor as u64, body, revision)
        .await
        .map_err(map_error)?;
    Ok((
        [("etag", format!("\"{}\"", value.revision))],
        no_store(value),
    ))
}
