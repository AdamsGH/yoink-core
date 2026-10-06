//! Existing downloader frontend paths, authenticated IDs only (never JWT roles).
use crate::auth::{ApiError, AuthState, authenticated_id, no_store};
use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use yoink_identity::nsfw::{NsfwImport, NsfwRuleInput, RuleKind};

pub(crate) fn router() -> Router<AuthState> {
    Router::new()
        .route("/api/v1/dl/nsfw/domains", get(domains).post(add_domain))
        .route(
            "/api/v1/dl/nsfw/domains/{id}",
            axum::routing::patch(update_domain).delete(delete_domain),
        )
        .route("/api/v1/dl/nsfw/keywords", get(keywords).post(add_keyword))
        .route(
            "/api/v1/dl/nsfw/keywords/{id}",
            axum::routing::patch(update_keyword).delete(delete_keyword),
        )
        .route("/api/v1/dl/nsfw/check", post(check))
        .route("/api/v1/dl/nsfw/import", post(import))
        .route("/api/v1/dl/nsfw/export", get(export))
}
fn invalid() -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request")
}
fn revision(h: &HeaderMap) -> Result<Option<i64>, ApiError> {
    h.get("if-match")
        .map(|v| {
            v.to_str()
                .map_err(|_| invalid())?
                .trim_matches('"')
                .parse::<i64>()
                .map_err(|_| invalid())
        })
        .transpose()
}
macro_rules! handlers {
    ($list:ident,$add:ident,$update:ident,$delete:ident,$kind:expr) => {
        async fn $list(
            State(s): State<AuthState>,
            h: HeaderMap,
        ) -> Result<impl IntoResponse, ApiError> {
            let svc = s.service()?;
            let a = authenticated_id(svc, &h)?;
            let (rows, revision) = svc.identities.nsfw_list(a, $kind).await?;
            Ok(([("etag", format!("\"{revision}\""))], no_store(rows)))
        }
        async fn $add(
            State(s): State<AuthState>,
            h: HeaderMap,
            b: Result<Json<NsfwRuleInput>, JsonRejection>,
        ) -> Result<impl IntoResponse, ApiError> {
            let svc = s.service()?;
            let a = authenticated_id(svc, &h)?;
            let Json(input) = b.map_err(|_| invalid())?;
            let row = svc
                .identities
                .nsfw_mutate(a, $kind, None, Some(&input), revision(&h)?)
                .await?;
            Ok((StatusCode::CREATED, no_store(row)))
        }
        async fn $update(
            State(s): State<AuthState>,
            h: HeaderMap,
            Path(id): Path<i64>,
            b: Result<Json<NsfwRuleInput>, JsonRejection>,
        ) -> Result<impl IntoResponse, ApiError> {
            let svc = s.service()?;
            let a = authenticated_id(svc, &h)?;
            let Json(input) = b.map_err(|_| invalid())?;
            if id <= 0 {
                return Err(invalid());
            }
            Ok(no_store(
                svc.identities
                    .nsfw_mutate(a, $kind, Some(id), Some(&input), revision(&h)?)
                    .await?,
            ))
        }
        async fn $delete(
            State(s): State<AuthState>,
            h: HeaderMap,
            Path(id): Path<i64>,
        ) -> Result<impl IntoResponse, ApiError> {
            let svc = s.service()?;
            let a = authenticated_id(svc, &h)?;
            if id <= 0 {
                return Err(invalid());
            }
            svc.identities
                .nsfw_mutate(a, $kind, Some(id), None, revision(&h)?)
                .await?;
            Ok(StatusCode::NO_CONTENT)
        }
    };
}
handlers!(
    domains,
    add_domain,
    update_domain,
    delete_domain,
    RuleKind::Domain
);
handlers!(
    keywords,
    add_keyword,
    update_keyword,
    delete_keyword,
    RuleKind::Keyword
);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckInput {
    url: String,
}
async fn check(
    State(s): State<AuthState>,
    h: HeaderMap,
    b: Result<Json<CheckInput>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    let Json(input) = b.map_err(|_| invalid())?;
    Ok(no_store(svc.identities.nsfw_check(a, &input.url).await?))
}
async fn import(
    State(s): State<AuthState>,
    h: HeaderMap,
    b: Result<Json<NsfwImport>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    let Json(input) = b.map_err(|_| invalid())?;
    let (d, k) = svc.identities.nsfw_import(a, &input, revision(&h)?).await?;
    Ok(no_store(json!({"domains_added":d,"keywords_added":k})))
}
async fn export(State(s): State<AuthState>, h: HeaderMap) -> Result<impl IntoResponse, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    let (domains, keywords, revision) = svc.identities.nsfw_export(a).await?;
    Ok((
        [("etag", format!("\"{revision}\""))],
        no_store(json!({
            "domains":domains.into_iter().map(|r|json!({"domain":r.domain,"note":r.note})).collect::<Vec<_>>(),
            "keywords":keywords.into_iter().map(|r|json!({"keyword":r.keyword,"note":r.note})).collect::<Vec<_>>()
        })),
    ))
}
