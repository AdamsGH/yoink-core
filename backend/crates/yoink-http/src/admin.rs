use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use yoink_access::Role;
use yoink_identity::admin::{
    AdminError, AdminUserPatch, AdminUsersQuery,
    api_keys::{API_KEY_SCOPES, ApiKeyCreate},
};

use crate::auth::{ApiError, AuthState, authenticated_id, no_store};

pub(crate) fn router() -> Router<AuthState> {
    Router::new()
        .route("/api/v1/api-keys", get(api_keys_list).post(api_key_create))
        .route("/api/v1/api-keys/scopes", get(api_key_scopes))
        .route(
            "/api/v1/api-keys/{key_id}",
            axum::routing::delete(api_key_revoke),
        )
        .route("/api/v1/users", get(list_users))
        .route("/api/v1/users/{id}", get(get_user).patch(update_user))
        .route("/api/v1/features", get(features))
        .route("/api/v1/feature-access/me", get(my_feature_access))
        .route("/api/v1/users/{id}/feature-access", get(feature_access))
        .route(
            "/api/v1/users/{id}/permissions",
            get(user_permissions).post(grant_permission),
        )
        .route(
            "/api/v1/users/{id}/permissions/{plugin}/{feature}",
            axum::routing::delete(revoke_permission),
        )
        .route("/api/v1/permissions/all", get(all_permissions))
        .route(
            "/api/v1/bot-settings",
            get(bot_settings).patch(update_bot_settings),
        )
        .route(
            "/api/v1/bot-settings/tag-map",
            get(tag_map).put(update_tag_map),
        )
        .route(
            "/api/v1/bot-settings/available-features",
            get(available_features),
        )
}

impl From<AdminError> for ApiError {
    fn from(error: AdminError) -> Self {
        match error {
            AdminError::Unauthorized => Self(StatusCode::UNAUTHORIZED, "Invalid identity"),
            AdminError::Forbidden => Self(StatusCode::FORBIDDEN, "Access denied"),
            AdminError::NotFound => Self(StatusCode::NOT_FOUND, "Not found"),
            AdminError::Conflict => Self(StatusCode::CONFLICT, "Already exists"),
            AdminError::InvalidInput => invalid_request(),
            AdminError::InvalidScope => Self(StatusCode::BAD_REQUEST, "Invalid API key scope"),
            AdminError::Unavailable => Self(
                StatusCode::SERVICE_UNAVAILABLE,
                "Identity service unavailable",
            ),
        }
    }
}

fn invalid_request() -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request")
}

async fn api_key_scopes(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    service.identities.api_keys_authorized(actor).await?;
    Ok(no_store(serde_json::json!({"scopes": API_KEY_SCOPES})))
}

async fn api_keys_list(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    Ok(no_store(service.identities.api_keys_list(actor).await?))
}

fn api_key_create_body(value: Value) -> Result<ApiKeyCreate, ApiError> {
    let object = value.as_object().ok_or_else(invalid_request)?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "name" | "scopes" | "expires_at"))
    {
        return Err(invalid_request());
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(invalid_request)?
        .to_owned();
    let scopes = match object.get("scopes") {
        None => None,
        Some(Value::Array(values)) => Some(
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(invalid_request)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        _ => return Err(invalid_request()),
    };
    let expires_at = match object.get("expires_at") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(
            DateTime::parse_from_rfc3339(value)
                .map_err(|_| invalid_request())?
                .with_timezone(&Utc),
        ),
        _ => return Err(invalid_request()),
    };
    Ok(ApiKeyCreate {
        name,
        scopes,
        expires_at,
    })
}

async fn api_key_create(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    let Json(body) = body.map_err(|_| invalid_request())?;
    let input = api_key_create_body(body)?;
    let created = service.identities.api_key_create(actor, &input).await?;
    Ok((StatusCode::CREATED, no_store(created)))
}

async fn api_key_revoke(
    State(state): State<AuthState>,
    headers: HeaderMap,
    path: Result<Path<i64>, PathRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    let Path(key_id) = path.map_err(|_| invalid_request())?;
    service.identities.api_key_revoke(actor, key_id).await?;
    Ok((StatusCode::NO_CONTENT, ()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsersQuery {
    offset: Option<i64>,
    page: Option<i64>,
    limit: Option<i64>,
    search: Option<String>,
    role: Option<String>,
    status: Option<String>,
    sort: Option<String>,
    direction: Option<String>,
    period: Option<String>,
}

impl UsersQuery {
    fn domain(self) -> Result<AdminUsersQuery, ApiError> {
        let limit = self.limit.unwrap_or(50);
        if !(1..=200).contains(&limit) {
            return Err(invalid_request());
        }
        let page_offset = self
            .page
            .map(|page| {
                page.checked_sub(1)
                    .filter(|value| *value >= 0)
                    .and_then(|value| value.checked_mul(limit))
                    .ok_or_else(invalid_request)
            })
            .transpose()?;
        if self.offset.is_some_and(|offset| offset < 0)
            || matches!((self.offset, page_offset), (Some(offset), Some(page)) if offset != page)
        {
            return Err(invalid_request());
        }
        Ok(AdminUsersQuery {
            offset: self.offset.or(page_offset).unwrap_or(0),
            limit,
            search: self.search,
            role: self.role,
            status: self.status,
            sort: self.sort.unwrap_or_else(|| "dl_count".into()),
            direction: self.direction.unwrap_or_else(|| "desc".into()),
            period: self.period.filter(|value| value != "all"),
        })
    }
}

async fn list_users(
    State(state): State<AuthState>,
    headers: HeaderMap,
    query: Result<Query<UsersQuery>, QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    let Query(query) = query.map_err(|_| invalid_request())?;
    let query = query.domain()?;
    Ok(no_store(
        service.identities.admin_list_users(actor, &query).await?,
    ))
}

fn user_id(path: Result<Path<i64>, PathRejection>) -> Result<i64, ApiError> {
    let Path(id) = path.map_err(|_| invalid_request())?;
    if id <= 0 {
        return Err(invalid_request());
    }
    Ok(id)
}

async fn get_user(
    State(state): State<AuthState>,
    headers: HeaderMap,
    path: Result<Path<i64>, PathRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    Ok(no_store(
        service
            .identities
            .admin_get_user(actor, user_id(path)?)
            .await?,
    ))
}

// A JSON value retains the difference between an omitted ban and an explicit null.
fn user_patch(value: Value) -> Result<AdminUserPatch, ApiError> {
    let object = value.as_object().ok_or_else(invalid_request)?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "role" | "language" | "ban_until"))
    {
        return Err(invalid_request());
    }
    let role = match object.get("role") {
        None | Some(Value::Null) => None,
        Some(Value::String(role)) => Some(role.parse::<Role>().map_err(|_| invalid_request())?),
        _ => return Err(invalid_request()),
    };
    let language = match object.get("language") {
        None | Some(Value::Null) => None,
        Some(Value::String(language)) if matches!(language.as_str(), "en" | "ru") => {
            Some(language.clone())
        }
        _ => return Err(invalid_request()),
    };
    let ban_until = match object.get("ban_until") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(value)) => Some(Some(
            DateTime::parse_from_rfc3339(value)
                .map_err(|_| invalid_request())?
                .with_timezone(&Utc),
        )),
        _ => return Err(invalid_request()),
    };
    Ok(AdminUserPatch {
        role,
        language,
        ban_until,
    })
}

async fn update_user(
    State(state): State<AuthState>,
    headers: HeaderMap,
    path: Result<Path<i64>, PathRejection>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    let id = user_id(path)?;
    let Json(body) = body.map_err(|_| invalid_request())?;
    let patch = user_patch(body)?;
    Ok(no_store(
        service
            .identities
            .admin_update_user(actor, id, &patch)
            .await?,
    ))
}

async fn features(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    Ok(no_store(service.identities.admin_features(actor).await?))
}

async fn my_feature_access(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    Ok(no_store(
        service.identities.admin_feature_access(actor, None).await?,
    ))
}

async fn feature_access(
    State(state): State<AuthState>,
    headers: HeaderMap,
    path: Result<Path<i64>, PathRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    Ok(no_store(
        service
            .identities
            .admin_feature_access(actor, Some(user_id(path)?))
            .await?,
    ))
}

async fn all_permissions(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    Ok(no_store(
        service.identities.admin_permissions(actor, None).await?,
    ))
}

async fn user_permissions(
    State(state): State<AuthState>,
    headers: HeaderMap,
    path: Result<Path<i64>, PathRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    Ok(no_store(
        service
            .identities
            .admin_permissions(actor, Some(user_id(path)?))
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantRequest {
    plugin: String,
    feature: String,
    expires_at: Option<DateTime<Utc>>,
}

async fn grant_permission(
    State(state): State<AuthState>,
    headers: HeaderMap,
    path: Result<Path<i64>, PathRejection>,
    body: Result<Json<GrantRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    let id = user_id(path)?;
    let Json(body) = body.map_err(|_| invalid_request())?;
    Ok(no_store(
        service
            .identities
            .admin_grant_permission(actor, id, &body.plugin, &body.feature, body.expires_at)
            .await?,
    ))
}

async fn revoke_permission(
    State(state): State<AuthState>,
    headers: HeaderMap,
    path: Result<Path<(i64, String, String)>, PathRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    let Path((id, plugin, feature)) = path.map_err(|_| invalid_request())?;
    if id <= 0 {
        return Err(invalid_request());
    }
    service
        .identities
        .admin_revoke_permission(actor, id, &plugin, &feature)
        .await?;
    Ok(no_store(serde_json::json!({"ok": true})))
}

async fn tag_map(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    Ok(no_store(service.identities.admin_tag_map(actor).await?))
}

fn tag_map_entries(value: Value) -> Result<Vec<yoink_identity::admin::AdminTagMapEntry>, ApiError> {
    let entries = value.as_array().ok_or_else(invalid_request)?;
    entries
        .iter()
        .map(|entry| {
            let object = entry.as_object().ok_or_else(invalid_request)?;
            if object.len() != 2 || !object.contains_key("tag") || !object.contains_key("features")
            {
                return Err(invalid_request());
            }
            let tag = object
                .get("tag")
                .and_then(Value::as_str)
                .ok_or_else(invalid_request)?;
            let features = object
                .get("features")
                .and_then(Value::as_array)
                .ok_or_else(invalid_request)?;
            let features = features
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(invalid_request)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(yoink_identity::admin::AdminTagMapEntry {
                tag: tag.to_owned(),
                features,
            })
        })
        .collect()
}

async fn update_tag_map(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    let Json(body) = body.map_err(|_| invalid_request())?;
    let entries = tag_map_entries(body)?;
    Ok(no_store(
        service
            .identities
            .admin_update_tag_map(actor, &entries)
            .await?,
    ))
}

async fn available_features(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    Ok(no_store(
        service.identities.admin_available_features(actor).await?,
    ))
}

async fn bot_settings(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    let settings = service.identities.admin_bot_settings(actor).await?;
    // Legacy DEFAULTS uses null for unset storage IDs, not empty strings.
    let settings: std::collections::BTreeMap<_, _> = settings
        .into_iter()
        .map(|(key, value)| {
            let unset = matches!(
                key.as_str(),
                "inline_storage_chat_id" | "inline_storage_thread_id"
            ) && value.is_empty();
            (key, if unset { None } else { Some(value) })
        })
        .collect();
    Ok(no_store(settings))
}

async fn update_bot_settings(
    State(state): State<AuthState>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state.service()?;
    let actor = authenticated_id(service, &headers)?;
    let Json(body) = body.map_err(|_| invalid_request())?;
    let patch = yoink_identity::admin::configuration::BotSettingsPatch::parse(&body)
        .map_err(|detail| ApiError(StatusCode::UNPROCESSABLE_ENTITY, detail))?;
    service
        .identities
        .admin_patch_configuration(actor, &patch)
        .await?;
    Ok(no_store(serde_json::json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_patch_distinguishes_absent_and_null_bans_and_rejects_invalid_fields() {
        assert_eq!(user_patch(serde_json::json!({})).unwrap().ban_until, None);
        assert_eq!(
            user_patch(serde_json::json!({"ban_until":null}))
                .unwrap()
                .ban_until,
            Some(None)
        );
        assert!(
            user_patch(serde_json::json!({"ban_until":"2030-01-01T00:00:00Z"}))
                .unwrap()
                .ban_until
                .unwrap()
                .is_some()
        );
        for body in [
            serde_json::json!({"ban_until":"2030-01-01"}),
            serde_json::json!({"language":"xx"}),
            serde_json::json!({"role":"superadmin"}),
            serde_json::json!({"id":42}),
            Value::Null,
        ] {
            assert!(user_patch(body.clone()).is_err(), "{body}");
        }
    }

    #[test]
    fn page_alias_is_checked_and_never_silently_disagrees_with_offset() {
        let query: UsersQuery =
            serde_json::from_value(serde_json::json!({"page":2,"limit":25})).unwrap();
        assert_eq!(query.domain().unwrap().offset, 25);
        for body in [
            serde_json::json!({"page":0}),
            serde_json::json!({"page":i64::MAX}),
            serde_json::json!({"page":2,"offset":0}),
            serde_json::json!({"offset":-1}),
            serde_json::json!({"limit":201}),
        ] {
            let query: UsersQuery = serde_json::from_value(body.clone()).unwrap();
            assert!(query.domain().is_err(), "{body}");
        }
    }
}
