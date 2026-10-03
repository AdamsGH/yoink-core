use axum::{
    Json, Router,
    body::Body,
    extract::{
        DefaultBodyLimit, Path, Query, Request, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::Value;
use yoink_identity::{
    admin::AdminError,
    m2m::{M2mEventCreate, M2mKey},
};

use crate::AuthState;

#[derive(Clone)]
struct M2mState {
    auth: AuthState,
    system: yoink_application::SystemService,
}

#[derive(Debug)]
struct M2mError(StatusCode, &'static str, bool);

impl IntoResponse for M2mError {
    fn into_response(self) -> Response {
        let mut response = (self.0, Json(serde_json::json!({"detail": self.1}))).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
        if self.2 {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("ApiKey"),
            );
        }
        response
    }
}

impl From<AdminError> for M2mError {
    fn from(error: AdminError) -> Self {
        match error {
            AdminError::InvalidInput => {
                Self(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request", false)
            }
            AdminError::NotFound => Self(StatusCode::NOT_FOUND, "Not found", false),
            _ => Self(
                StatusCode::SERVICE_UNAVAILABLE,
                "Identity service unavailable",
                false,
            ),
        }
    }
}

pub(crate) fn router(auth: AuthState, system: yoink_application::SystemService) -> Router {
    Router::new()
        .route("/api/internal/v1/status", get(status))
        .route("/api/internal/v1/users", get(users))
        .route("/api/internal/v1/users/{user_id}", get(user))
        .route("/api/internal/v1/groups", get(groups))
        .route("/api/internal/v1/events", get(events).post(create_event))
        .route_layer(middleware::from_fn_with_state(
            system.clone(),
            readiness_and_no_store,
        ))
        .layer(DefaultBodyLimit::max(20 * 1024))
        .with_state(M2mState { auth, system })
}

async fn readiness_and_no_store(
    State(system): State<yoink_application::SystemService>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let mut response = if system.snapshot().ready {
        next.run(request).await
    } else {
        M2mError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service unavailable",
            false,
        )
        .into_response()
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

async fn key(state: &M2mState, headers: &HeaderMap, required: &str) -> Result<M2mKey, M2mError> {
    let value = headers.get("X-Api-Key").and_then(|v| v.to_str().ok());
    let Some(raw) = value.filter(|v| !v.is_empty()) else {
        return Err(M2mError(
            StatusCode::UNAUTHORIZED,
            "Missing X-Api-Key header",
            true,
        ));
    };
    let identities = state.auth.identities().map_err(|_| {
        M2mError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Identity service unavailable",
            false,
        )
    })?;
    let key = identities
        .m2m_authenticate(raw)
        .await
        .map_err(|_| {
            M2mError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Identity service unavailable",
                false,
            )
        })?
        .ok_or(M2mError(StatusCode::UNAUTHORIZED, "Invalid API key", true))?;
    if !has_scope(&key.scopes, required) {
        return Err(M2mError(
            StatusCode::FORBIDDEN,
            "API key missing required scope",
            false,
        ));
    }
    Ok(key)
}

fn has_scope(scopes: &[String], required: &str) -> bool {
    scopes.iter().any(|scope| {
        scope == "*"
            || scope == required
            || required
                .split_once(':')
                .is_some_and(|(namespace, _)| scope == &format!("{namespace}:*"))
    })
}

#[derive(serde::Serialize)]
struct StatusResponse {
    bot_running: bool,
    bot_username: Option<String>,
    uptime_seconds: f64,
    total_users: i64,
    total_groups: i64,
    active_groups: i64,
}

async fn status(
    State(state): State<M2mState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, M2mError> {
    key(&state, &headers, "health:read").await?;
    let identities = state.auth.identities().map_err(|_| {
        M2mError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Identity service unavailable",
            false,
        )
    })?;
    let counts = identities.m2m_counts().await?;
    let username = state.system.bot_username();
    Ok(Json(StatusResponse {
        bot_running: state.system.bot_running(),
        bot_username: username,
        uptime_seconds: state.system.uptime_seconds(),
        total_users: counts.total_users,
        total_groups: counts.total_groups,
        active_groups: counts.active_groups,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsersQuery {
    offset: Option<i64>,
    limit: Option<i64>,
    role: Option<String>,
}

async fn users(
    State(state): State<M2mState>,
    headers: HeaderMap,
    query: Result<Query<UsersQuery>, QueryRejection>,
) -> Result<impl IntoResponse, M2mError> {
    key(&state, &headers, "users:read").await?;
    let Query(query) =
        query.map_err(|_| M2mError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request", false))?;
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50);
    if offset < 0 || !(1..=200).contains(&limit) {
        return Err(M2mError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            false,
        ));
    }
    let role = query.role.as_deref().filter(|role| {
        matches!(
            *role,
            "owner" | "admin" | "moderator" | "user" | "restricted" | "banned"
        )
    });
    Ok(Json(
        state
            .auth
            .identities()
            .map_err(|_| {
                M2mError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Identity service unavailable",
                    false,
                )
            })?
            .m2m_users(role, offset, limit)
            .await?,
    ))
}

async fn user(
    State(state): State<M2mState>,
    headers: HeaderMap,
    path: Result<Path<i64>, PathRejection>,
) -> Result<impl IntoResponse, M2mError> {
    key(&state, &headers, "users:read").await?;
    let Path(id) =
        path.map_err(|_| M2mError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request", false))?;
    let found = state
        .auth
        .identities()
        .map_err(|_| {
            M2mError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Identity service unavailable",
                false,
            )
        })?
        .m2m_user(id)
        .await?
        .ok_or(M2mError(StatusCode::NOT_FOUND, "User not found", false))?;
    Ok(Json(found))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupsQuery {
    #[serde(default, deserialize_with = "legacy_bool")]
    enabled_only: Option<bool>,
}

fn legacy_bool<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    let value = String::deserialize(deserializer)?;
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" | "y" | "t" => Ok(Some(true)),
        "false" | "0" | "no" | "off" | "n" | "f" => Ok(Some(false)),
        _ => Err(serde::de::Error::custom("Invalid boolean")),
    }
}

async fn groups(
    State(state): State<M2mState>,
    headers: HeaderMap,
    query: Result<Query<GroupsQuery>, QueryRejection>,
) -> Result<impl IntoResponse, M2mError> {
    key(&state, &headers, "groups:read").await?;
    let Query(query) =
        query.map_err(|_| M2mError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request", false))?;
    Ok(Json(
        state
            .auth
            .identities()
            .map_err(|_| {
                M2mError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Identity service unavailable",
                    false,
                )
            })?
            .m2m_groups(query.enabled_only.unwrap_or(false))
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventsQuery {
    offset: Option<i64>,
    limit: Option<i64>,
    plugin: Option<String>,
    event_type: Option<String>,
}

async fn events(
    State(state): State<M2mState>,
    headers: HeaderMap,
    query: Result<Query<EventsQuery>, QueryRejection>,
) -> Result<impl IntoResponse, M2mError> {
    key(&state, &headers, "events:read").await?;
    let Query(query) =
        query.map_err(|_| M2mError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request", false))?;
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50);
    if offset < 0 || !(1..=200).contains(&limit) {
        return Err(M2mError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            false,
        ));
    }
    Ok(Json(
        state
            .auth
            .identities()
            .map_err(|_| {
                M2mError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Identity service unavailable",
                    false,
                )
            })?
            .m2m_events(
                query.plugin.as_deref().filter(|value| !value.is_empty()),
                query
                    .event_type
                    .as_deref()
                    .filter(|value| !value.is_empty()),
                offset,
                limit,
            )
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventRequest {
    plugin: String,
    event_type: String,
    user_id: Option<i64>,
    group_id: Option<i64>,
    thread_id: Option<i64>,
    payload: Option<Value>,
}

async fn create_event(
    State(state): State<M2mState>,
    headers: HeaderMap,
    body: Result<Json<EventRequest>, JsonRejection>,
) -> Result<impl IntoResponse, M2mError> {
    key(&state, &headers, "events:write").await?;
    let Json(body) =
        body.map_err(|_| M2mError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request", false))?;
    if body
        .payload
        .as_ref()
        .is_some_and(|value| !value.is_object())
    {
        return Err(M2mError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            false,
        ));
    }
    let event = state
        .auth
        .identities()
        .map_err(|_| {
            M2mError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Identity service unavailable",
                false,
            )
        })?
        .m2m_event_create(M2mEventCreate {
            plugin: &body.plugin,
            event_type: &body.event_type,
            user_id: body.user_id,
            group_id: body.group_id,
            thread_id: body.thread_id,
            payload: body.payload.as_ref(),
        })
        .await?;
    Ok((StatusCode::CREATED, Json(event)))
}

#[cfg(test)]
mod tests {
    use std::{path::Path, time::Duration};

    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request},
    };
    use tower::ServiceExt;
    use yoink_identity::admin::api_keys::ApiKeyCreate;
    use yoink_storage::Store;

    use super::*;

    #[cfg(unix)]
    fn secure(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(not(unix))]
    fn secure(_: &Path) {}

    async fn setup(scopes: Vec<String>) -> (tempfile::TempDir, Store, Router, String) {
        let dir = tempfile::tempdir().unwrap();
        secure(dir.path());
        let store = Store::open(
            &dir.path().join("m2m-http.db"),
            true,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        store.migrate().await.unwrap();
        yoink_db::raw_sql(include_str!(
            "../../yoink-identity/tests/sql/api_keys_users.sql"
        ))
        .execute(store.database())
        .await
        .unwrap();
        let identities = std::sync::Arc::new(yoink_identity::IdentityStore::new(
            store.database().clone(),
            Some(7101),
            false,
        ));
        let issued = identities
            .api_key_create(
                7102,
                &ApiKeyCreate {
                    name: "integration".into(),
                    scopes: Some(scopes),
                    expires_at: None,
                },
            )
            .await
            .unwrap();
        let auth = AuthState::new(false);
        assert!(auth.install_identities(identities).is_ok());
        let system = yoink_application::SystemService::new("test");
        system.set_ready(true);
        (dir, store, router(auth, system), issued.key)
    }

    async fn call(
        app: Router,
        method: Method,
        uri: &str,
        key: &str,
        body: &'static str,
    ) -> Response {
        app.oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("X-Api-Key", key)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[test]
    fn scope_matching_is_exact_or_namespace_wildcard_only() {
        assert!(has_scope(&["*".into()], "users:read"));
        assert!(has_scope(&["users:*".into()], "users:read"));
        assert!(has_scope(&["users:read".into()], "users:read"));
        assert!(!has_scope(&["user:*".into()], "users:read"));
        assert!(!has_scope(&["users:write".into()], "users:read"));
    }

    #[tokio::test]
    async fn m2m_routes_enforce_scope_update_usage_and_keep_key_challenge_separate() {
        let (_dir, store, app, key) = setup(vec!["users:read".into()]).await;
        let response = call(
            app.clone(),
            Method::GET,
            "/api/internal/v1/users?offset=0&limit=1&role=made-up",
            &key,
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        let response = call(
            app.clone(),
            Method::POST,
            "/api/internal/v1/events",
            &key,
            r#"{"plugin":"dl","event_type":"x","payload":{}}"#,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!response.headers().contains_key(header::WWW_AUTHENTICATE));
        let used: Option<chrono::DateTime<chrono::Utc>> = yoink_db::query(include_str!(
            "../../yoink-identity/tests/sql/m2m_last_used.sql"
        ))
        .bind(1_i64)
        .fetch_one(store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
        assert!(
            used.is_some(),
            "valid but scope-denied requests update last_used_at"
        );
        let jwt_only = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/internal/v1/status")
                    .header(header::AUTHORIZATION, "Bearer token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(jwt_only.status(), StatusCode::UNAUTHORIZED);
        assert_ne!(
            jwt_only.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer"
        );
        store.close().await.unwrap();
    }

    #[tokio::test]
    async fn groups_accept_legacy_boolean_spellings() {
        let (_dir, store, app, key) = setup(vec!["groups:read".into()]).await;
        for value in [
            "true", "1", "yes", "on", "y", "t", "TRUE", "false", "0", "no", "off", "n", "f",
            "FALSE",
        ] {
            let response = call(
                app.clone(),
                Method::GET,
                &format!("/api/internal/v1/groups?enabled_only={value}"),
                &key,
                "",
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "{value}");
        }
        let response = call(
            app,
            Method::GET,
            "/api/internal/v1/groups?enabled_only=invalid",
            &key,
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        store.close().await.unwrap();
    }

    #[tokio::test]
    async fn event_post_returns_201_and_persists_object_payload() {
        let (_dir, store, app, key) = setup(vec!["events:write".into()]).await;
        let response = call(app, Method::POST, "/api/internal/v1/events", &key,
            r#"{"plugin":"dl","event_type":"join","user_id":7103,"thread_id":55,"payload":{"ok":true}}"#).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let data = to_bytes(response.into_body(), 100_000).await.unwrap();
        let body: Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(body["payload"]["ok"], true);
        assert_eq!(body["user_id"], 7103);
        store.close().await.unwrap();
    }
}
