//! Legacy user-mode proxy routes. The tdlight service remains external.
use crate::auth::{ApiError, AuthState, authenticated_id, no_store};
use axum::{
    Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::sync::Arc;
use yoink_user_session::{Client, Error, Method};
#[derive(Clone)]
struct SessionState {
    auth: AuthState,
    client: Option<Arc<Client>>,
}
pub fn router(auth: AuthState, client: Option<Arc<Client>>) -> Router {
    Router::new()
        .route("/api/v1/threads/status", get(status))
        .route("/api/v1/threads/scan/{chat}", post(scan))
        .route("/api/v1/forum/topics/{chat}", get(topics))
        .route("/api/v1/forum/topics/{chat}/{thread}", get(topic))
        .route("/api/v1/forum/topics/{chat}/{thread}/link", get(topic_link))
        .route("/api/v1/forum/search/{chat}", get(search))
        .route("/api/v1/forum/history/{chat}", get(history))
        .route("/api/v1/messages/{chat}/{message}/viewers", get(viewers))
        .route("/api/v1/messages/{chat}/{message}/link", get(message_link))
        .route(
            "/api/v1/messages/{chat}/{message}/read-date",
            get(read_date),
        )
        .route(
            "/api/v1/messages/{chat}/{message}/thread",
            get(message_thread),
        )
        .route("/api/v1/messages/{chat}/by-date", get(by_date))
        .with_state(SessionState { auth, client })
}
fn error(e: Error) -> ApiError {
    ApiError(
        match e {
            Error::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            Error::Unauthorized => StatusCode::UNAUTHORIZED,
            Error::Forbidden => StatusCode::FORBIDDEN,
            Error::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::BAD_GATEWAY,
        },
        match e {
            Error::Unavailable => "User-mode session not available",
            Error::Unauthorized => "User session unauthorized",
            _ => "User session request failed",
        },
    )
}
async fn authority(s: &SessionState, h: &HeaderMap, owner: bool) -> Result<i64, ApiError> {
    let svc = s.auth.service()?;
    let id = authenticated_id(svc, h)?;
    svc.identities.configuration_authorized(id, owner).await?;
    Ok(id)
}
async fn status(
    State(s): State<SessionState>,
    h: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    authority(&s, &h, false).await?;
    let available = match &s.client {
        Some(c) => c.available().await.map_err(error)?,
        None => false,
    };
    Ok(no_store(json!({"available":available})))
}
async fn scan(
    State(s): State<SessionState>,
    h: HeaderMap,
    path: Result<Path<i64>, axum::extract::rejection::PathRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let a = authority(&s, &h, true).await?;
    let Path(chat) = path.map_err(|_| invalid())?;
    if chat >= 0 {
        return Err(invalid());
    }
    let c = s.client.as_ref().ok_or_else(|| error(Error::Unavailable))?;
    let svc = s.auth.service()?;
    svc.identities.group_get(a, chat).await?;
    let topics = c
        .scan_checked(chat, || async {
            authority(&s, &h, true).await.map(|_| ()).map_err(|e| {
                if e.0 == StatusCode::FORBIDDEN {
                    Error::Forbidden
                } else {
                    Error::Network
                }
            })
        })
        .await
        .map_err(error)?;
    let names = topics
        .iter()
        .map(|t| {
            (
                t["message_thread_id"].as_i64().unwrap(),
                t["name"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect::<Vec<_>>();
    let upserted = svc.identities.scan_thread_names(a, chat, &names).await?;
    Ok(no_store(
        json!({"group_id":chat,"total_count":topics.len(),"upserted":upserted,"topics":topics}),
    ))
}
fn invalid() -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request")
}
async fn proxy(
    s: SessionState,
    h: HeaderMap,
    m: Method,
    mut q: std::collections::HashMap<String, String>,
    base: &[(&str, i64)],
    defaults: &[(&str, &str)],
    allowed: &[&str],
) -> Result<impl IntoResponse + use<>, ApiError> {
    authority(&s, &h, true).await?;
    if q.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(invalid());
    }
    for (k, v) in defaults {
        q.entry(k.to_string()).or_insert(v.to_string());
    }
    for (k, v) in &q {
        if matches!(k.as_str(), "query" | "filter") {
            if v.len() > 4096 {
                return Err(invalid());
            }
            continue;
        }
        if matches!(k.as_str(), "only_local" | "for_album" | "in_message_thread") {
            if !matches!(v.as_str(), "true" | "false") {
                return Err(invalid());
            }
            continue;
        }
        let n = v.parse::<i64>().map_err(|_| invalid())?;
        if (k == "limit" && !(1..=100).contains(&n)) || (k == "offset" && n < 0) {
            return Err(invalid());
        }
    }
    if let Some(v) = q.remove("thread_id")
        && v != "0"
    {
        q.insert("message_thread_id".into(), v);
    }
    if q.get("filter").is_some_and(|v| v.is_empty()) {
        q.remove("filter");
    }
    for (k, v) in base {
        q.insert(k.to_string(), v.to_string());
    }
    let c = s.client.as_ref().ok_or_else(|| error(Error::Unavailable))?;
    let value: Value = c
        .call(m, &q.into_iter().collect::<Vec<_>>())
        .await
        .map_err(error)?;
    if matches!(m, Method::GetMessageViewers) {
        if !value.is_array() {
            return Err(error(Error::Response));
        }
    } else if !value.is_object() {
        return Err(error(Error::Response));
    }
    Ok(no_store(value))
}
async fn topics(
    State(s): State<SessionState>,
    h: HeaderMap,
    Path(chat): Path<i64>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    proxy(
        s,
        h,
        Method::GetForumTopics,
        q,
        &[("chat_id", chat)],
        &[
            ("query", ""),
            ("limit", "100"),
            ("offset_date", "0"),
            ("offset_message_id", "0"),
            ("offset_forum_topic_id", "0"),
        ],
        &[
            "query",
            "limit",
            "offset_date",
            "offset_message_id",
            "offset_forum_topic_id",
        ],
    )
    .await
}
async fn topic(
    State(s): State<SessionState>,
    h: HeaderMap,
    Path((chat, item)): Path<(i64, i64)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    proxy(
        s,
        h,
        Method::GetForumTopic,
        q,
        &[("chat_id", chat), ("message_thread_id", item)],
        &[],
        &[],
    )
    .await
}
async fn topic_link(
    State(s): State<SessionState>,
    h: HeaderMap,
    Path((chat, item)): Path<(i64, i64)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    proxy(
        s,
        h,
        Method::GetForumTopicLink,
        q,
        &[("chat_id", chat), ("message_thread_id", item)],
        &[],
        &[],
    )
    .await
}
async fn search(
    State(s): State<SessionState>,
    h: HeaderMap,
    Path(chat): Path<i64>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    proxy(
        s,
        h,
        Method::SearchChatMessages,
        q,
        &[("chat_id", chat)],
        &[
            ("query", ""),
            ("thread_id", "0"),
            ("from_message_id", "0"),
            ("offset", "0"),
            ("limit", "50"),
            ("filter", ""),
        ],
        &[
            "query",
            "thread_id",
            "from_message_id",
            "offset",
            "limit",
            "filter",
        ],
    )
    .await
}
async fn history(
    State(s): State<SessionState>,
    h: HeaderMap,
    Path(chat): Path<i64>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    proxy(
        s,
        h,
        Method::GetChatHistory,
        q,
        &[("chat_id", chat)],
        &[
            ("from_message_id", "0"),
            ("offset", "0"),
            ("limit", "50"),
            ("only_local", "false"),
        ],
        &["from_message_id", "offset", "limit", "only_local"],
    )
    .await
}
async fn viewers(
    State(s): State<SessionState>,
    h: HeaderMap,
    Path((chat, item)): Path<(i64, i64)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    proxy(
        s,
        h,
        Method::GetMessageViewers,
        q,
        &[("chat_id", chat), ("message_id", item)],
        &[],
        &[],
    )
    .await
}
async fn message_link(
    State(s): State<SessionState>,
    h: HeaderMap,
    Path((chat, item)): Path<(i64, i64)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    proxy(
        s,
        h,
        Method::GetMessageLink,
        q,
        &[("chat_id", chat), ("message_id", item)],
        &[
            ("media_timestamp", "0"),
            ("for_album", "false"),
            ("in_message_thread", "false"),
        ],
        &["media_timestamp", "for_album", "in_message_thread"],
    )
    .await
}
async fn read_date(
    State(s): State<SessionState>,
    h: HeaderMap,
    Path((chat, item)): Path<(i64, i64)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    proxy(
        s,
        h,
        Method::GetMessageReadDate,
        q,
        &[("chat_id", chat), ("message_id", item)],
        &[],
        &[],
    )
    .await
}
async fn message_thread(
    State(s): State<SessionState>,
    h: HeaderMap,
    Path((chat, item)): Path<(i64, i64)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    proxy(
        s,
        h,
        Method::GetMessageThread,
        q,
        &[("chat_id", chat), ("message_id", item)],
        &[],
        &[],
    )
    .await
}
async fn by_date(
    State(s): State<SessionState>,
    h: HeaderMap,
    Path(chat): Path<i64>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    if !q.contains_key("date") {
        return Err(invalid());
    }
    proxy(
        s,
        h,
        Method::GetChatMessageByDate,
        q,
        &[("chat_id", chat)],
        &[],
        &["date"],
    )
    .await
}
