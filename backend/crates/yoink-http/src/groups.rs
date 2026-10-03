use crate::auth::{ApiError, AuthState, authenticated_id, no_store};
use axum::{
    Json, Router,
    extract::{
        Path, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde_json::Value;
use yoink_access::Role;
use yoink_identity::admin::groups::{GroupCreate, GroupPatch, MemberPatch, ThreadPatch};

pub(crate) fn router() -> Router<AuthState> {
    Router::new()
        .route("/api/v1/groups", get(list).post(create))
        .route(
            "/api/v1/groups/{id}",
            get(get_one).patch(update).delete(delete),
        )
        .route(
            "/api/v1/groups/{id}/threads",
            get(threads).post(upsert_thread),
        )
        .route(
            "/api/v1/groups/{id}/threads/{policy}",
            axum::routing::delete(delete_thread),
        )
        .route("/api/v1/groups/{id}/members", get(members))
        .route(
            "/api/v1/groups/{id}/members/{user}",
            axum::routing::put(set_member).delete(delete_member),
        )
}
fn invalid() -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request")
}
fn parse_role(v: &Value) -> Result<Option<Role>, ApiError> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.parse().map_err(|_| invalid())?)),
        _ => Err(invalid()),
    }
}
fn object(v: Value, allowed: &[&str]) -> Result<serde_json::Map<String, Value>, ApiError> {
    let m = v.as_object().ok_or_else(invalid)?;
    if m.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(invalid());
    }
    Ok(m.clone())
}
fn id(p: Result<Path<i64>, PathRejection>, negative: bool) -> Result<i64, ApiError> {
    let Path(v) = p.map_err(|_| invalid())?;
    if (negative && v >= 0) || (!negative && v <= 0) {
        Err(invalid())
    } else {
        Ok(v)
    }
}
async fn list(
    State(s): State<AuthState>,
    h: axum::http::HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    Ok(no_store(svc.identities.groups_list(a).await?))
}
async fn get_one(
    State(s): State<AuthState>,
    h: axum::http::HeaderMap,
    p: Result<Path<i64>, PathRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    Ok(no_store(svc.identities.group_get(a, id(p, true)?).await?))
}
async fn create(
    State(s): State<AuthState>,
    h: axum::http::HeaderMap,
    b: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    let Json(v) = b.map_err(|_| invalid())?;
    let m = object(
        v,
        &[
            "id",
            "title",
            "enabled",
            "auto_grant_role",
            "allow_pm",
            "nsfw_allowed",
            "storage_chat_id",
            "storage_thread_id",
        ],
    )?;
    let gid = m.get("id").and_then(Value::as_i64).ok_or_else(invalid)?;
    if gid >= 0 {
        return Err(invalid());
    }
    let title = match m.get("title") {
        None | Some(Value::Null) => None,
        Some(Value::String(x)) => Some(x.clone()),
        _ => return Err(invalid()),
    };
    let role = match m.get("auto_grant_role") {
        None => None,
        Some(Value::String(v)) => Some(v.parse().map_err(|_| invalid())?),
        _ => return Err(invalid()),
    };
    let boolf = |k: &str| -> Result<Option<bool>, ApiError> {
        match m.get(k) {
            None => Ok(None),
            Some(Value::Bool(v)) => Ok(Some(*v)),
            _ => Err(invalid()),
        }
    };
    let out = svc
        .identities
        .group_create(
            a,
            &GroupCreate {
                id: gid,
                title,
                auto_grant_role: role,
                allow_pm: boolf("allow_pm")?,
                nsfw_allowed: boolf("nsfw_allowed")?,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, no_store(out)))
}
async fn update(
    State(s): State<AuthState>,
    h: axum::http::HeaderMap,
    p: Result<Path<i64>, PathRejection>,
    b: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    let gid = id(p, true)?;
    let Json(v) = b.map_err(|_| invalid())?;
    let m = object(
        v,
        &[
            "title",
            "enabled",
            "auto_grant_role",
            "allow_pm",
            "nsfw_allowed",
            "storage_chat_id",
            "storage_thread_id",
        ],
    )?;
    let title = match m.get("title") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(x)) => Some(Some(x.clone())),
        _ => return Err(invalid()),
    };
    let bval = |k: &str| -> Result<Option<bool>, ApiError> {
        match m.get(k) {
            None => Ok(None),
            Some(Value::Bool(v)) => Ok(Some(*v)),
            _ => Err(invalid()),
        }
    };
    let intval = |k: &str, chat: bool| -> Result<Option<Option<i64>>, ApiError> {
        match m.get(k) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(Value::Number(n)) => {
                let x = n.as_i64().ok_or_else(invalid)?;
                if x == 0 || (!chat && x < 0) {
                    return Err(invalid());
                }
                Ok(Some(Some(x)))
            }
            _ => Err(invalid()),
        }
    };
    let role = m
        .get("auto_grant_role")
        .map(parse_role)
        .transpose()?
        .flatten();
    let g = svc
        .identities
        .group_update(
            a,
            gid,
            &GroupPatch {
                title,
                enabled: bval("enabled")?,
                auto_grant_role: role,
                allow_pm: bval("allow_pm")?,
                nsfw_allowed: bval("nsfw_allowed")?,
                storage_chat_id: intval("storage_chat_id", true)?,
                storage_thread_id: intval("storage_thread_id", false)?,
            },
        )
        .await?;
    Ok(no_store(g))
}
async fn delete(
    State(s): State<AuthState>,
    h: axum::http::HeaderMap,
    p: Result<Path<i64>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    svc.identities.group_delete(a, id(p, true)?).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn threads(
    State(s): State<AuthState>,
    h: axum::http::HeaderMap,
    p: Result<Path<i64>, PathRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    Ok(no_store(
        svc.identities.threads_list(a, id(p, true)?).await?,
    ))
}
async fn upsert_thread(
    State(s): State<AuthState>,
    h: axum::http::HeaderMap,
    p: Result<Path<i64>, PathRejection>,
    b: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    let gid = id(p, true)?;
    let Json(v) = b.map_err(|_| invalid())?;
    let m = object(v, &["thread_id", "name", "enabled"])?;
    let tid = match m.get("thread_id") {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => Some(n.as_i64().filter(|x| *x > 0).ok_or_else(invalid)?),
        _ => return Err(invalid()),
    };
    let name = match m.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(x)) => Some(x.clone()),
        _ => return Err(invalid()),
    };
    let enabled = match m.get("enabled") {
        None => true,
        Some(Value::Bool(v)) => *v,
        _ => return Err(invalid()),
    };
    let r = svc
        .identities
        .thread_upsert(
            a,
            gid,
            &ThreadPatch {
                thread_id: tid,
                name,
                enabled,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, no_store(r)))
}
async fn delete_thread(
    State(s): State<AuthState>,
    h: axum::http::HeaderMap,
    p: Result<Path<(i64, i64)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    let Path((g, t)) = p.map_err(|_| invalid())?;
    svc.identities.thread_delete(a, g, t).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn members(
    State(s): State<AuthState>,
    h: axum::http::HeaderMap,
    p: Result<Path<i64>, PathRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    Ok(no_store(
        svc.identities.members_list(a, id(p, true)?).await?,
    ))
}
async fn set_member(
    State(s): State<AuthState>,
    h: axum::http::HeaderMap,
    p: Result<Path<(i64, i64)>, PathRejection>,
    b: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    let Path((g, u)) = p.map_err(|_| invalid())?;
    if g >= 0 || u <= 0 {
        return Err(invalid());
    }
    let Json(v) = b.map_err(|_| invalid())?;
    let m = object(v, &["role_override", "allow_pm_override"])?;
    let role = m
        .get("role_override")
        .map(parse_role)
        .transpose()?
        .flatten();
    let pm = match m.get("allow_pm_override") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(x)) => Some(*x),
        _ => return Err(invalid()),
    };
    Ok(no_store(
        svc.identities
            .member_upsert(
                a,
                g,
                u,
                &MemberPatch {
                    role_override: role,
                    allow_pm_override: pm,
                },
            )
            .await?,
    ))
}
async fn delete_member(
    State(s): State<AuthState>,
    h: axum::http::HeaderMap,
    p: Result<Path<(i64, i64)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let svc = s.service()?;
    let a = authenticated_id(svc, &h)?;
    let Path((g, u)) = p.map_err(|_| invalid())?;
    svc.identities.member_delete(a, g, u).await?;
    Ok(StatusCode::NO_CONTENT)
}
