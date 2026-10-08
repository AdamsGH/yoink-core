//! Product-owned HTTP routes. Shared authentication middleware must insert
//! AuthenticatedUser only after validating the shared JWT/WebApp/API-key identity.
//! Never deserialize that identity from request JSON or trust frontend roles.
use crate::{
    ByokTest, ByokUpdate, Error, Insight, ModelProbeOptions, Request, SettingsUpdate,
    oauth::DeviceOAuth, prompts::AliasInput,
};
use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Copy)]
pub struct AuthenticatedUser(pub i64);
#[derive(Clone)]
pub struct HttpState {
    pub insight: Insight,
    pub oauth: DeviceOAuth,
    pub shutdown: CancellationToken,
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let code = match &self {
            Error::Code(c) => *c,
            Error::Database => "database_error",
        };
        let status = match code {
            "access_denied" | "byok_disabled" => StatusCode::FORBIDDEN,
            "not_found" => StatusCode::NOT_FOUND,
            "oauth_not_configured" => StatusCode::NOT_IMPLEMENTED,
            "alias_conflict" => StatusCode::CONFLICT,
            "quota_exceeded" => StatusCode::TOO_MANY_REQUESTS,
            "database_error" => StatusCode::INTERNAL_SERVER_ERROR,
            "provider_unavailable" | "oauth_unavailable" => StatusCode::BAD_GATEWAY,
            _ => StatusCode::BAD_REQUEST,
        };
        (status, Json(json!({"detail":code}))).into_response()
    }
}
pub fn router(state: HttpState) -> Router {
    Router::new()
        .route("/me/stats", get(stats))
        .route("/settings/me", get(settings).patch(update_settings))
        .route("/aliases", get(aliases).post(create_alias))
        .route(
            "/aliases/{id}",
            axum::routing::patch(update_alias).delete(delete_alias),
        )
        .route("/byok/me", get(byok).put(save_byok).delete(delete_byok))
        .route("/byok/me/test", post(probe_input))
        .route("/byok/me/refresh-models", post(refresh_models))
        .route("/models", get(models))
        .route("/config/test", get(test_gateway))
        .route("/github/write-token/status", get(write_token_status))
        .route("/config/byok", get(toggle_get).patch(toggle_set))
        .route("/config/tldr", get(config).patch(config_set))
        .route("/access", get(access))
        .route("/access/lookup", get(lookup))
        .route(
            "/access/{uid}",
            post(grant).patch(update_access).delete(revoke),
        )
        .route("/summarize", post(summarize))
        .route("/github/login", post(login))
        .route("/github/login/status", get(login_status))
        .route("/github/token", delete(clear_token))
        .route("/github/upgrade-scope", post(write_login))
        .route("/github/upgrade-scope/status", get(write_status))
        .route("/github/write-token", delete(clear_write_token))
        .layer(DefaultBodyLimit::max(65536))
        .layer(axum::middleware::from_fn(require_identity))
        .with_state(state)
}
// Cancel in-flight provider work if Axum drops the handler on disconnect.
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel()
    }
}
async fn settings(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, Error> {
    s.insight.settings(u.0).await.map(Json)
}
async fn update_settings(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Json(v): Json<SettingsUpdate>,
) -> Result<Json<Value>, Error> {
    s.insight.update_settings(u.0, v).await.map(Json)
}
async fn aliases(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<Vec<crate::prompts::Alias>>, Error> {
    s.insight.aliases(u.0).await.map(Json)
}
async fn create_alias(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Json(v): Json<AliasInput>,
) -> Result<(StatusCode, Json<crate::prompts::Alias>), Error> {
    Ok((
        StatusCode::CREATED,
        Json(s.insight.save_alias(u.0, None, v).await?),
    ))
}
async fn update_alias(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
    Json(v): Json<AliasInput>,
) -> Result<Json<crate::prompts::Alias>, Error> {
    s.insight.save_alias(u.0, Some(id), v).await.map(Json)
}
async fn delete_alias(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, Error> {
    s.insight.delete_alias(u.0, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn byok(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, Error> {
    s.insight.byok_view(u.0).await.map(Json)
}
async fn save_byok(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Json(v): Json<ByokUpdate>,
) -> Result<Json<Value>, Error> {
    let cancel = s.shutdown.child_token();
    let _guard = CancelOnDrop(cancel.clone());
    s.insight
        .save_byok_with_cancel(u.0, v, cancel)
        .await
        .map(Json)
}
async fn delete_byok(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<StatusCode, Error> {
    s.insight.delete_byok(u.0).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn config(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, Error> {
    s.insight.admin_config(u.0).await.map(Json)
}
async fn config_set(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Json(v): Json<Value>,
) -> Result<Json<Value>, Error> {
    s.insight.update_config(u.0, v).await.map(Json)
}
async fn toggle_get(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, Error> {
    s.insight.byok_toggle(u.0, None).await.map(Json)
}
#[derive(Deserialize)]
struct Toggle {
    enabled: bool,
}
async fn toggle_set(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Json(v): Json<Toggle>,
) -> Result<Json<Value>, Error> {
    s.insight.byok_toggle(u.0, Some(v.enabled)).await.map(Json)
}
async fn access(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, Error> {
    s.insight.access_list(u.0).await.map(Json)
}
#[derive(Deserialize)]
struct Lookup {
    #[serde(default)]
    q: String,
}
async fn lookup(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Query(v): Query<Lookup>,
) -> Result<Json<Value>, Error> {
    s.insight.lookup(u.0, &v.q).await.map(Json)
}
#[derive(Deserialize)]
struct Grant {
    #[serde(default = "english")]
    lang: String,
}
fn english() -> String {
    "en".into()
}
async fn grant(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Path(uid): Path<i64>,
    Json(v): Json<Grant>,
) -> Result<(StatusCode, Json<Value>), Error> {
    Ok((
        StatusCode::CREATED,
        Json(s.insight.grant(u.0, uid, &v.lang).await?),
    ))
}
async fn update_access(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Path(uid): Path<i64>,
    Json(v): Json<Grant>,
) -> Result<Json<Value>, Error> {
    s.insight.update_access(u.0, uid, &v.lang).await.map(Json)
}
async fn revoke(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Path(uid): Path<i64>,
) -> Result<StatusCode, Error> {
    s.insight.revoke(u.0, uid).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn summarize(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Json(v): Json<Request>,
) -> Result<Json<crate::Summary>, Error> {
    let cancel = s.shutdown.child_token();
    let _guard = CancelOnDrop(cancel.clone());
    s.insight.summarize(u.0, v, cancel).await.map(Json)
}
async fn login(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<crate::oauth::Status>, Error> {
    s.oauth.start(u.0, false).await.map(Json)
}
async fn write_login(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<crate::oauth::Status>, Error> {
    s.oauth.start(u.0, true).await.map(Json)
}
async fn login_status(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, Error> {
    s.oauth.status(u.0, false).await.map(Json)
}
async fn write_status(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, Error> {
    s.oauth.status(u.0, true).await.map(Json)
}
async fn clear_token(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<StatusCode, Error> {
    s.oauth.cancel(u.0, false).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn clear_write_token(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<StatusCode, Error> {
    s.oauth.cancel(u.0, true).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn stats(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, Error> {
    s.insight.stats(u.0).await.map(Json)
}
async fn probe_input(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Json(v): Json<ByokTest>,
) -> Result<Json<Value>, Error> {
    let cancel = s.shutdown.child_token();
    let _guard = CancelOnDrop(cancel.clone());
    s.insight.test_byok_input(u.0, v, cancel).await.map(Json)
}
async fn refresh_models(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, Error> {
    let cancel = s.shutdown.child_token();
    let _guard = CancelOnDrop(cancel.clone());
    s.insight.refresh_byok_models(u.0, cancel).await.map(Json)
}
async fn models(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Query(options): Query<ModelProbeOptions>,
) -> Result<Json<Value>, Error> {
    let cancel = s.shutdown.child_token();
    let _guard = CancelOnDrop(cancel.clone());
    s.insight
        .models_with_options(u.0, options, cancel)
        .await
        .map(Json)
}
async fn test_gateway(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
    Query(options): Query<ModelProbeOptions>,
) -> Result<Json<Value>, Error> {
    let cancel = s.shutdown.child_token();
    let _guard = CancelOnDrop(cancel.clone());
    s.insight
        .test_gateway_with_options(u.0, options, cancel)
        .await
        .map(Json)
}
async fn write_token_status(
    State(s): State<HttpState>,
    Extension(u): Extension<AuthenticatedUser>,
) -> Result<Json<Value>, Error> {
    let mut status = s.insight.write_token_status(u.0).await?;
    status["configured"] = json!(s.oauth.write_configured());
    Ok(Json(status))
}

async fn require_identity(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if request.extensions().get::<AuthenticatedUser>().is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"detail":"Invalid token"})),
        )
            .into_response();
    }
    next.run(request).await
}
