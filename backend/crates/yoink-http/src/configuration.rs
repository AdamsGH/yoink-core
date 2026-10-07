//! Frontend metadata for the core configuration contracts.
use crate::auth::{ApiError, AuthState, authenticated_id, no_store};
use axum::{Router, extract::State, http::HeaderMap, response::IntoResponse, routing::get};

pub(crate) fn router() -> Router<AuthState> {
    Router::new().route("/api/v1/_meta/openapi.json", get(openapi))
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
