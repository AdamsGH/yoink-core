//! Bridge only: the module reauthorizes against current DB policy on every call.
use crate::auth::{ApiError, AuthState, authenticated_id};
use axum::{
    Router,
    extract::Request,
    http::{StatusCode, header},
    middleware::{self, Next},
    response::IntoResponse,
};
pub(crate) fn router(
    system: yoink_application::SystemService,
    auth: AuthState,
    state: yoink_insight::http::HttpState,
) -> Router {
    Router::new()
        .nest("/api/v1/insight", yoink_insight::http::router(state))
        .route_layer(middleware::from_fn(
            move |mut request: Request, next: Next| {
                let auth = auth.clone();
                let ready = system.snapshot().ready;
                async move {
                    let verified = if ready {
                        auth.service()
                            .and_then(|s| authenticated_id(s, request.headers()))
                    } else {
                        Err(ApiError(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "Service unavailable",
                        ))
                    };
                    let mut response = match verified {
                        Ok(id) => {
                            request
                                .extensions_mut()
                                .insert(yoink_insight::http::AuthenticatedUser(id));
                            next.run(request).await
                        }
                        Err(e) => e.into_response(),
                    };
                    response.headers_mut().insert(
                        header::CACHE_CONTROL,
                        header::HeaderValue::from_static("no-store"),
                    );
                    if response.status() == StatusCode::UNAUTHORIZED {
                        response.headers_mut().insert(
                            header::WWW_AUTHENTICATE,
                            header::HeaderValue::from_static("Bearer"),
                        );
                    }
                    response
                }
            },
        ))
}
#[derive(Clone, Default)]
pub struct LateInsight(std::sync::Arc<std::sync::OnceLock<Router>>);
impl LateInsight {
    pub fn install(
        &self,
        system: yoink_application::SystemService,
        auth: AuthState,
        state: yoink_insight::http::HttpState,
    ) -> Result<(), &'static str> {
        let activity = crate::activity::Activity::default();
        activity.install_insight(state.insight.clone())?;
        self.install_with_activity(system, auth, state, activity)
    }
    pub fn install_with_activity(
        &self,
        system: yoink_application::SystemService,
        auth: AuthState,
        state: yoink_insight::http::HttpState,
        activity: crate::activity::Activity,
    ) -> Result<(), &'static str> {
        let readiness = system.clone();
        let activity = activity
            .router(auth.clone())
            .route_layer(middleware::from_fn(move |request: Request, next: Next| {
                let ready = readiness.snapshot().ready;
                async move {
                    if ready {
                        next.run(request).await
                    } else {
                        StatusCode::SERVICE_UNAVAILABLE.into_response()
                    }
                }
            }));
        self.0
            .set(router(system, auth, state).merge(activity))
            .map_err(|_| "Insight already installed")
    }
    pub(crate) fn get(&self) -> Option<&Router> {
        self.0.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use std::{sync::Arc, time::Duration};
    use tower::ServiceExt;
    #[tokio::test]
    async fn shared_jwt_current_authority_redaction_and_activity() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let db =
            yoink_storage::Store::open(&dir.path().join("native.db"), true, Duration::from_secs(1))
                .await
                .unwrap();
        db.migrate().await.unwrap();
        let identities = Arc::new(yoink_identity::IdentityStore::new(
            db.database().clone(),
            Some(1),
            true,
        ));
        for user_id in [1, 2] {
            identities
                .register(&yoink_identity::TrustedProfile {
                    user_id,
                    username: None,
                    first_name: None,
                    photo_url: None,
                })
                .await
                .unwrap();
        }
        let crypto = yoink_auth::AuthConfig::new(
            "fixture".into(),
            "fixture-shared-secret".into(),
            60,
            0,
            600,
        )
        .unwrap();
        let token = |id| {
            crypto
                .issue_token(
                    yoink_auth::JwtProfile {
                        user_id: id,
                        role: "owner",
                        username: None,
                        first_name: None,
                    },
                    chrono::Utc::now().timestamp(),
                )
                .unwrap()
        };
        let owner = token(1);
        let user = token(2);
        let auth = AuthState::new(false);
        assert!(
            auth.install(crate::AuthService { identities, crypto })
                .is_ok()
        );
        let service = yoink_insight::Insight::completion_only(
            db.database().clone(),
            yoink_insight::Config {
                owner_id: Some(1),
                gateway_key: "credential-marker".into(),
                ..Default::default()
            },
            Arc::new(
                yoink_insight::provider::HttpCompletion::new(Duration::from_secs(1), 4096).unwrap(),
            ),
        )
        .unwrap();
        let shutdown = tokio_util::sync::CancellationToken::new();
        let oauth = yoink_insight::oauth::DeviceOAuth::new(
            service.clone(),
            "fixture-client".into(),
            None,
            shutdown.child_token(),
        )
        .unwrap();
        let system = yoink_application::SystemService::new("test");
        let late = LateInsight::default();
        assert!(late.get().is_none());
        late.install(
            system.clone(),
            auth,
            yoink_insight::http::HttpState {
                insight: service,
                oauth: oauth.clone(),
                shutdown: shutdown.clone(),
            },
        )
        .unwrap();
        let app = late.get().unwrap().clone();
        async fn get(app: &Router, path: &str, token: Option<&str>) -> (StatusCode, String) {
            let mut req = Request::builder().uri(path);
            if let Some(t) = token {
                req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
            }
            let response = app
                .clone()
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let body = String::from_utf8(
                to_bytes(response.into_body(), 100000)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            (status, body)
        }
        assert_eq!(
            get(&app, "/api/v1/insight/config/tldr", Some(&owner))
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        system.set_ready(true);
        assert_eq!(
            get(&app, "/api/v1/insight/config/tldr", None).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (status, body) = get(&app, "/api/v1/insight/config/tldr", Some(&owner)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body.contains("credential-marker"));
        let config: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(config["gateway_api_key"], "");
        assert_eq!(config["gateway_api_key_set"], true);
        assert_eq!(
            get(&app, "/api/v1/users/1/stats", Some(&user)).await.0,
            StatusCode::FORBIDDEN,
            "JWT role is not authority"
        );
        yoink_db::query("INSERT INTO insight_usage_log(user_id,command,lang,status,created_at) VALUES(2,'summary','en','ok',1700000000000000)").execute(db.database()).await.unwrap();
        let (status, body) = get(&app, "/api/v1/users/2/stats", Some(&owner)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["plugins"][0]["plugin"],
            "insight"
        );
        let stats: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(stats["ai_total"], 1);
        assert_eq!(stats["plugins"][0]["total"], 1);
        yoink_db::query("UPDATE users SET ban_until=4070908800000000 WHERE id=1")
            .execute(db.database())
            .await
            .unwrap();
        assert_eq!(
            get(&app, "/api/v1/insight/config/tldr", Some(&owner))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get(&app, "/api/v1/users/2/stats", Some(&owner)).await.0,
            StatusCode::FORBIDDEN
        );
        yoink_db::query("UPDATE users SET ban_until=NULL WHERE id=1")
            .execute(db.database())
            .await
            .unwrap();
        yoink_db::query("UPDATE users SET ban_until=4070908800000000 WHERE id=2")
            .execute(db.database())
            .await
            .unwrap();
        assert_eq!(
            get(&app, "/api/v1/users/2/stats", Some(&owner)).await.0,
            StatusCode::FORBIDDEN,
            "activity subject must also have fresh authority"
        );
        shutdown.cancel();
        oauth.shutdown().await;
        db.close().await.unwrap();
    }
}

#[cfg(test)]
mod disabled_tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;
    #[tokio::test]
    async fn absent_products_preserve_diagnostics_and_do_not_install_routes() {
        let app = crate::router_with_late_products(
            yoink_application::SystemService::new("test"),
            None,
            AuthState::new(false),
            None,
            crate::LateInbox::default(),
            LateInsight::default(),
        );
        for path in [
            "/api/v1/insight/config/tldr",
            "/api/v1/insight/byok/me",
            "/api/v1/users/me/stats",
            "/api/v1/inbox/items",
        ] {
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
        assert_eq!(
            app.oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/system")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
            StatusCode::OK
        );
    }
}
