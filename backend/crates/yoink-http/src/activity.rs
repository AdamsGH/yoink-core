//! Shared authorized activity aggregation. Providers receive only the subject ID.
use crate::auth::{ApiError, AuthState, authenticated_id, no_store};
use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, RwLock},
};
use yoink_access::{AccessPolicy, AccessScope, Role};
pub type ActivityCall<'a> = Pin<Box<dyn Future<Output = Result<Value, ActivityError>> + Send + 'a>>;
#[derive(Debug)]
pub enum ActivityError {
    Forbidden,
    Unavailable,
}
pub trait ActivityProvider: Send + Sync {
    /// Numeric last_at values use native microseconds; HTTP emits RFC3339 timestamps.
    fn activity(&self, subject: i64) -> ActivityCall<'_>;
}
impl ActivityProvider for yoink_insight::Insight {
    fn activity(&self, subject: i64) -> ActivityCall<'_> {
        Box::pin(async move {
            self.activity(subject).await.map_err(|e| match e {
                yoink_insight::Error::Database => ActivityError::Unavailable,
                _ => ActivityError::Forbidden,
            })
        })
    }
}
impl ActivityProvider for yoink_music::NativeMusicPersistence {
    fn activity(&self, subject: i64) -> ActivityCall<'_> {
        Box::pin(async move {
            let activity = self
                .activity(subject)
                .await
                .map_err(|_| ActivityError::Unavailable)?;
            let mut activity =
                serde_json::to_value(activity).map_err(|_| ActivityError::Unavailable)?;
            activity["extra"] = json!({});
            Ok(activity)
        })
    }
}
#[derive(Clone, Default)]
pub struct Activity(Arc<RwLock<BTreeMap<String, Arc<dyn ActivityProvider>>>>);
impl Activity {
    pub fn install(
        &self,
        name: &str,
        provider: Arc<dyn ActivityProvider>,
    ) -> Result<(), &'static str> {
        let mut providers = self
            .0
            .write()
            .map_err(|_| "activity registry unavailable")?;
        if providers.contains_key(name) {
            return Err("activity provider already installed");
        }
        providers.insert(name.to_owned(), provider);
        Ok(())
    }
    pub fn install_insight(&self, insight: yoink_insight::Insight) -> Result<(), &'static str> {
        self.install("insight", Arc::new(insight))
    }
    pub fn router(&self, auth: AuthState) -> Router {
        Router::new()
            .route("/api/v1/users/me/stats", get(me))
            .route("/api/v1/users/{id}/stats", get(user))
            .with_state((auth, self.clone()))
    }
    async fn collect(&self, auth: &AuthState, actor: i64, subject: i64) -> Result<Value, ApiError> {
        let identities = &auth.service()?.identities;
        let policy = AccessPolicy {
            min_role: if actor == subject {
                Role::Restricted
            } else {
                Role::Admin
            },
            scopes: vec![AccessScope::Private],
            check_group_enabled: false,
            check_thread_policy: false,
        };
        let snapshot = identities
            .snapshot(actor, None)
            .await
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable"))?
            .ok_or(ApiError(StatusCode::FORBIDDEN, "Access denied"))?;
        if !snapshot.authorize(&policy, None).decision.allowed {
            return Err(ApiError(StatusCode::FORBIDDEN, "Access denied"));
        }
        let profile_error = |e| match e {
            yoink_identity::IdentityError::Unauthorized => {
                ApiError(StatusCode::NOT_FOUND, "Not found")
            }
            yoink_identity::IdentityError::Forbidden => {
                ApiError(StatusCode::FORBIDDEN, "Access denied")
            }
            _ => ApiError(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable"),
        };
        identities
            .current_profile(subject)
            .await
            .map_err(profile_error)?;
        let mut plugins = vec![];
        let providers: Vec<_> = self
            .0
            .read()
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable"))?
            .values()
            .cloned()
            .collect();
        for provider in providers {
            let mut activity = provider.activity(subject).await.map_err(|e| match e {
                ActivityError::Unavailable => {
                    ApiError(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable")
                }
                ActivityError::Forbidden => ApiError(StatusCode::FORBIDDEN, "Access denied"),
            })?;
            if let Some(timestamp) = activity["last_at"].as_i64() {
                activity["last_at"] = chrono::DateTime::from_timestamp_micros(timestamp)
                    .ok_or(ApiError(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Service unavailable",
                    ))?
                    .to_rfc3339()
                    .into();
            }
            plugins.push(activity);
        }
        let profile = identities
            .current_profile(subject)
            .await
            .map_err(profile_error)?;
        // Recheck the viewing principal after provider I/O: demotion/ban cannot
        // authorize an already in-flight request for another subject.
        let current = identities
            .snapshot(actor, None)
            .await
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable"))?
            .ok_or(ApiError(StatusCode::FORBIDDEN, "Access denied"))?;
        if !current.authorize(&policy, None).decision.allowed {
            return Err(ApiError(StatusCode::FORBIDDEN, "Access denied"));
        }
        let dl = plugins.iter().find(|v| v["plugin"] == "dl");
        let music = plugins.iter().find(|v| v["plugin"] == "music");
        let ai = plugins.iter().find(|v| v["plugin"] == "insight");
        Ok(
            json!({"total":dl.map(|v|v["total"].clone()).unwrap_or(json!(0)),"this_week":dl.map(|v|v["extra"]["this_week"].clone()).unwrap_or(json!(0)),"today":dl.map(|v|v["extra"]["today"].clone()).unwrap_or(json!(0)),"top_domains":dl.map(|v|v["extra"]["top_domains"].clone()).unwrap_or(json!([])),"by_category":dl.map(|v|v["extra"]["by_category"].clone()).unwrap_or(json!({})),"member_since":profile.created_at,"dl_last_at":dl.map(|v|v["last_at"].clone()).unwrap_or(Value::Null),"music_total":music.map(|v|v["total"].clone()).unwrap_or(json!(0)),"music_last_at":music.map(|v|v["last_at"].clone()).unwrap_or(Value::Null),"ai_total":ai.map(|v|v["total"].clone()).unwrap_or(json!(0)),"ai_last_at":ai.map(|v|v["last_at"].clone()).unwrap_or(Value::Null),"plugins":plugins}),
        )
    }
}
async fn me(
    State((auth, activity)): State<(AuthState, Activity)>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let actor = authenticated_id(auth.service()?, &headers)?;
    Ok(no_store(activity.collect(&auth, actor, actor).await?))
}
async fn user(
    State((auth, activity)): State<(AuthState, Activity)>,
    headers: HeaderMap,
    Path(subject): Path<i64>,
) -> Result<impl axum::response::IntoResponse, ApiError> {
    let actor = authenticated_id(auth.service()?, &headers)?;
    Ok(no_store(activity.collect(&auth, actor, subject).await?))
}
