//! Frontend metadata for the core configuration contracts.
use crate::auth::{ApiError, AuthState, authenticated_id, no_store};
use axum::{Router, extract::State, http::HeaderMap, response::IntoResponse, routing::get};

pub(crate) fn router() -> Router<AuthState> {
    Router::new()
        .route("/api/v1/_meta/openapi.json", get(openapi))
        .route("/api/v1/threads/status", get(threads_status))
        .route(
            "/api/v1/threads/scan/{group_id}",
            axum::routing::post(scan_unavailable),
        )
}

async fn openapi(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    // This read rechecks current database roles and timed bans. Metadata is admin-only.
    service
        .identities
        .configuration_authorized(actor, false)
        .await?;
    Ok(no_store(
        serde_json::from_str::<serde_json::Value>(include_str!("configuration-openapi.json"))
            .expect("checked-in configuration OpenAPI is valid JSON"),
    ))
}

// Native runtime has no user-mode Telegram session adapter. Do not report bot
// token availability as user-session availability or execute host login commands.
async fn threads_status(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    service
        .identities
        .configuration_authorized(actor, false)
        .await?;
    Ok(no_store(serde_json::json!({"available": false})))
}

async fn scan_unavailable(
    State(state): State<AuthState>,
    headers: HeaderMap,
    path: Result<axum::extract::Path<i64>, axum::extract::rejection::PathRejection>,
) -> Result<axum::response::Response, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    service
        .identities
        .configuration_authorized(actor, true)
        .await?;
    let axum::extract::Path(id) = path.map_err(|_| {
        ApiError(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "group_id: expected a negative Telegram group ID",
        )
    })?;
    if id >= 0 {
        return Err(ApiError(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "group_id: expected a negative Telegram group ID",
        ));
    }
    Err(ApiError(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "User-mode Telegram session is not supported by the native runtime; configure thread policies manually",
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn configuration_schema_matches_registered_contracts() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("configuration-openapi.json")).unwrap();
        assert_eq!(schema["openapi"], "3.1.0");
        assert!(schema["paths"]["/api/v1/bot-settings"]["patch"].is_object());
        assert!(
            schema["paths"]
                .as_object()
                .unwrap()
                .keys()
                .all(|path| !path.contains("restart") && !path.contains("exec"))
        );
    }
}
