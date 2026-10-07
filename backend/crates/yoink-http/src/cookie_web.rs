//! Existing cookie frontend and extension routes; bearer claims supply only IDs.
use crate::auth::{ApiError, AuthState, authenticated_id, no_store};
use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
};
use yoink_downloads::{
    DownloadError, DownloadService,
    cookie_web::{CookieInput, CookieSubmit},
};
fn error(e: DownloadError) -> ApiError {
    match e {
        DownloadError::Denied => ApiError(StatusCode::FORBIDDEN, "Access denied"),
        DownloadError::Capacity => ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Cookie control capacity exceeded",
        ),
        DownloadError::Executor => ApiError(StatusCode::BAD_GATEWAY, "Provider request failed"),
        DownloadError::UnsupportedAuthentication => ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "OAuth credentials cannot be validated as Netscape cookies",
        ),
        DownloadError::InvalidInput => {
            ApiError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid cookie data")
        }
        DownloadError::PersonalCookieCapacity => {
            ApiError(StatusCode::CONFLICT, "Cookie capacity exceeded")
        }
        _ => ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Cookie service unavailable",
        ),
    }
}
fn service(s: &AuthState) -> Result<&DownloadService, ApiError> {
    s.cookies.get().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Cookie service unavailable",
    ))
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn invalid() -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid cookie data")
}
pub(crate) fn router() -> Router<AuthState> {
    Router::new()
        .route("/api/v1/dl/cookies/{id}/validate", post(validate_cookie))
        .route(
            "/api/v1/dl/cookies/pool/refresh-labels",
            post(refresh_labels),
        )
        .route("/api/v1/dl/cookies/yttv/start", post(oauth_start))
        .route("/api/v1/dl/cookies/yttv/poll/{session}", get(oauth_poll))
        .route("/api/v1/dl/cookies/all", get(all))
        .route("/api/v1/dl/cookies/pool", get(pool).post(add_pool))
        .route("/api/v1/dl/cookies/pool/{id}", delete(remove_pool))
        .route("/api/v1/dl/cookies", get(list).post(upload))
        .route("/api/v1/dl/cookies/upload", post(upload))
        .route("/api/v1/dl/cookies/token", post(token).delete(revoke))
        .route("/api/v1/dl/cookies/by-id/{id}", delete(remove))
        .route("/api/v1/dl/cookies/{domain}", delete(remove_domain))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
}
async fn list(State(s): State<AuthState>, h: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    Ok(no_store(
        service(&s)?
            .web_cookie_list(actor, now())
            .await
            .map_err(error)?,
    ))
}
async fn token(State(s): State<AuthState>, h: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    Ok(no_store(
        service(&s)?
            .issue_cookie_token(actor, now())
            .await
            .map_err(error)?,
    ))
}
async fn revoke(State(s): State<AuthState>, h: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    service(&s)?
        .revoke_cookie_tokens(actor, now())
        .await
        .map_err(error)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn submit(
    State(s): State<AuthState>,
    b: Result<Json<CookieSubmit>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(body) = b.map_err(|_| invalid())?;
    service(&s)?
        .submit_cookie_token(&body, now())
        .await
        .map_err(|e| {
            if matches!(e, DownloadError::Denied) {
                ApiError(StatusCode::UNAUTHORIZED, "Invalid or expired token")
            } else {
                error(e)
            }
        })?;
    Ok(StatusCode::NO_CONTENT)
}
async fn upload(
    State(s): State<AuthState>,
    h: HeaderMap,
    b: Result<Json<CookieInput>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    let Json(body) = b.map_err(|_| invalid())?;
    let svc = service(&s)?;
    svc.upload_web_cookie(actor, &body, now())
        .await
        .map_err(error)?;
    let row = svc
        .web_cookie_list(actor, now())
        .await
        .map_err(error)?
        .into_iter()
        .find(|r| {
            r.domain
                .trim_matches('.')
                .eq_ignore_ascii_case(body.domain.trim_matches('.'))
        })
        .ok_or(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Cookie service unavailable",
        ))?;
    Ok((StatusCode::CREATED, no_store(row)))
}
async fn remove(
    State(s): State<AuthState>,
    h: HeaderMap,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    if !service(&s)?
        .web_cookie_delete(actor, Some(id), None, now())
        .await
        .map_err(error)?
    {
        return Err(ApiError(StatusCode::NOT_FOUND, "Cookie not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn remove_domain(
    State(s): State<AuthState>,
    h: HeaderMap,
    Path(domain): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    if !service(&s)?
        .web_cookie_delete(actor, None, Some(&domain), now())
        .await
        .map_err(error)?
    {
        return Err(ApiError(StatusCode::NOT_FOUND, "Cookie not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn all(State(s): State<AuthState>, h: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    Ok(no_store(
        service(&s)?
            .web_cookie_scoped_list(actor, 1, now())
            .await
            .map_err(error)?,
    ))
}
#[derive(serde::Deserialize)]
struct PoolQuery {
    domain: Option<String>,
}
async fn pool(
    State(s): State<AuthState>,
    h: HeaderMap,
    q: Result<Query<PoolQuery>, QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    let Query(q) = q.map_err(|_| invalid())?;
    let domain = q
        .domain
        .as_deref()
        .map(yoink_downloads::CookieDomain::parse)
        .transpose()
        .map_err(error)?;
    let mut rows = service(&s)?
        .web_cookie_scoped_list(actor, 2, now())
        .await
        .map_err(error)?;
    if let Some(domain) = domain {
        rows.retain(|r| {
            r.domain
                .trim_matches('.')
                .eq_ignore_ascii_case(domain.as_str())
        });
    }
    Ok(no_store(rows))
}
async fn remove_pool(
    State(s): State<AuthState>,
    h: HeaderMap,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    if !service(&s)?
        .web_pool_delete(actor, id, now())
        .await
        .map_err(error)?
    {
        return Err(ApiError(StatusCode::NOT_FOUND, "Cookie not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn add_pool(
    State(s): State<AuthState>,
    h: HeaderMap,
    b: Result<Json<CookieInput>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    let Json(body) = b.map_err(|_| invalid())?;
    let svc = service(&s)?;
    let id = svc
        .web_pool_upload(actor, &body, now())
        .await
        .map_err(error)?;
    let row = svc
        .web_cookie_scoped_list(actor, 2, now())
        .await
        .map_err(error)?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Cookie service unavailable",
        ))?;
    Ok((StatusCode::CREATED, no_store(row)))
}
async fn oauth_start(
    State(s): State<AuthState>,
    h: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    Ok(no_store(
        service(&s)?
            .oauth_start(actor, now())
            .await
            .map_err(error)?,
    ))
}
async fn oauth_poll(
    State(s): State<AuthState>,
    h: HeaderMap,
    Path(session): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    Ok(no_store(
        service(&s)?
            .oauth_poll(actor, &session, now())
            .await
            .map_err(error)?,
    ))
}
async fn validate_cookie(
    State(s): State<AuthState>,
    h: HeaderMap,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    let svc = service(&s)?;
    svc.web_cookie_validate(actor, id, now())
        .await
        .map_err(error)?;
    let row = svc
        .web_cookie_list(actor, now())
        .await
        .map_err(error)?
        .into_iter()
        .find(|r| r.id == id);
    let row = if let Some(row) = row {
        row
    } else {
        svc.web_cookie_scoped_list(actor, 1, now())
            .await
            .map_err(error)?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or(ApiError(StatusCode::NOT_FOUND, "Cookie not found"))?
    };
    Ok(no_store(row))
}
async fn refresh_labels(
    State(s): State<AuthState>,
    h: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authenticated_id(s.service()?, &h)?;
    Ok(no_store(
        serde_json::json!({"updated":service(&s)?.web_pool_refresh_labels(actor,now()).await.map_err(error)?}),
    ))
}
/// Browser sync is credentialed by its one-time token even when JWT auth is
/// disabled. Keep it outside bearer-only router composition/body limits.
pub(crate) fn public_router(state: AuthState, system: yoink_application::SystemService) -> Router {
    Router::new()
        .route("/api/v1/dl/cookies/submit", post(submit))
        .route_layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let ready = system.snapshot().ready;
                async move {
                    let mut response = if ready {
                        next.run(request).await
                    } else {
                        ApiError(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "Cookie service unavailable",
                        )
                        .into_response()
                    };
                    response.headers_mut().insert(
                        axum::http::header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static("no-store"),
                    );
                    response
                }
            },
        ))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(state)
}
