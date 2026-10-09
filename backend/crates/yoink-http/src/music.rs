use crate::auth::{ApiError, AuthState, authenticated_id, no_store};
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use std::sync::Arc;
use yoink_access::{AccessPolicy, AccessScope, Role};
use yoink_application::SystemService;
use yoink_music::NativeMusicPersistence;

#[derive(Clone)]
struct MusicState {
    system: SystemService,
    auth: AuthState,
    persistence: Arc<NativeMusicPersistence>,
}

pub fn router(
    system: SystemService,
    auth: AuthState,
    persistence: Arc<NativeMusicPersistence>,
) -> Router {
    Router::new()
        .route("/api/v1/music/me/stats", get(stats))
        .with_state(MusicState {
            system,
            auth,
            persistence,
        })
}

async fn authorize(state: &MusicState, user: i64) -> Result<(), ApiError> {
    let snapshot = state
        .auth
        .service()?
        .identities
        .snapshot(user, None)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable"))?
        .ok_or(ApiError(StatusCode::FORBIDDEN, "Access denied"))?;
    let policy = AccessPolicy {
        min_role: Role::Restricted,
        scopes: vec![AccessScope::Private],
        check_group_enabled: false,
        check_thread_policy: false,
    };
    // Historical account usage remains readable without a current Music grant.
    if snapshot.authorize(&policy, None).decision.allowed {
        Ok(())
    } else {
        Err(ApiError(StatusCode::FORBIDDEN, "Access denied"))
    }
}

async fn stats(
    State(state): State<MusicState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    if !state.system.snapshot().ready {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service unavailable",
        ));
    }
    let actor = authenticated_id(state.auth.service()?, &headers)?;
    authorize(&state, actor).await?;
    let result = state
        .persistence
        .stats(actor)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "Service unavailable"))?;
    authorize(&state, actor).await?;
    Ok(no_store(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::Value;
    use std::time::Duration;
    use tower::ServiceExt;
    use yoink_auth::{AuthConfig, JwtProfile};
    use yoink_identity::{IdentityStore, TrustedProfile};

    struct BanDuringActivity(yoink_db::Database);
    impl crate::activity::ActivityProvider for BanDuringActivity {
        fn activity(&self, subject: i64) -> crate::activity::ActivityCall<'_> {
            Box::pin(async move {
                yoink_db::query("UPDATE users SET ban_until=4070908800000000 WHERE id=?")
                    .bind(subject)
                    .execute(&self.0)
                    .await
                    .map_err(|_| crate::activity::ActivityError::Unavailable)?;
                Ok(serde_json::json!({"plugin":"test","total":0,"last_at":null}))
            })
        }
    }

    async fn get(app: &Router, path: &str, token: Option<&str>) -> (StatusCode, Value) {
        let mut request = Request::builder().uri(path);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.headers()["cache-control"], "no-store");
        let status = response.status();
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn historical_usage_preserves_subject_scope_readiness_and_current_authority() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let store = Arc::new(
            yoink_storage::Store::open(&dir.path().join("music.db"), true, Duration::from_secs(2))
                .await
                .unwrap(),
        );
        store.migrate().await.unwrap();
        let identities = Arc::new(IdentityStore::new(store.database().clone(), Some(1), false));
        for user_id in 1..=3 {
            identities
                .register(&TrustedProfile {
                    user_id,
                    username: None,
                    first_name: None,
                    photo_url: None,
                })
                .await
                .unwrap();
        }
        let crypto = AuthConfig::new(
            "fixture".into(),
            "fixture-music-jwt-secret-32-bytes".into(),
            86400,
            30,
            86400,
        )
        .unwrap();
        let tokens: Vec<_> = (1..=3)
            .map(|user_id| {
                crypto
                    .issue_token(
                        JwtProfile {
                            user_id,
                            role: "owner",
                            username: None,
                            first_name: None,
                        },
                        chrono::Utc::now().timestamp(),
                    )
                    .unwrap()
            })
            .collect();
        let auth = AuthState::new(false);
        assert!(
            auth.install(crate::AuthService { identities, crypto })
                .is_ok()
        );
        for user in [2, 3, 3] {
            yoink_db::query("INSERT INTO music_resolve_log(user_id,source_platform,artist,title,platforms_found,created_at) VALUES(?,'spotify','Artist','Track',2,?)")
                .bind(user).bind(chrono::Utc::now().timestamp_micros()).execute(store.database()).await.unwrap();
        }
        yoink_db::query("UPDATE users SET role='restricted' WHERE id=2")
            .execute(store.database())
            .await
            .unwrap();
        let persistence = Arc::new(NativeMusicPersistence {
            database: store.database().clone(),
            store: store.clone(),
        });
        let activity = crate::activity::Activity::default();
        activity.install("music", persistence.clone()).unwrap();
        let system = SystemService::new("test");
        let app = router(system.clone(), auth.clone(), persistence).merge(activity.router(auth));
        let path = "/api/v1/music/me/stats";
        assert_eq!(
            get(&app, path, Some(&tokens[1])).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        system.set_ready(true);
        assert_eq!(get(&app, path, None).await.0, StatusCode::UNAUTHORIZED);
        let (status, stats) = get(&app, "/api/v1/music/me/stats?user_id=3", Some(&tokens[1])).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(stats["total"], 1);
        assert_eq!(stats["today"], 1);
        assert_eq!(stats["top_artists"][0]["artist"], "Artist");
        let (_, account) = get(&app, "/api/v1/users/me/stats", Some(&tokens[1])).await;
        assert_eq!(account["music_total"], 1);
        assert_eq!(
            account["plugins"]
                .as_array()
                .unwrap()
                .iter()
                .find(|plugin| plugin["plugin"] == "music")
                .unwrap(),
            &serde_json::json!({
                "plugin": "music",
                "total": 1,
                "last_at": account["music_last_at"],
                "extra": {}
            })
        );
        assert!(
            chrono::DateTime::parse_from_rfc3339(account["music_last_at"].as_str().unwrap())
                .is_ok()
        );
        assert_eq!(
            get(&app, "/api/v1/users/3/stats", Some(&tokens[1])).await.0,
            StatusCode::FORBIDDEN
        );
        let (_, subject) = get(&app, "/api/v1/users/3/stats", Some(&tokens[0])).await;
        assert_eq!(subject["music_total"], 2);
        activity
            .install(
                "zz-ban",
                Arc::new(BanDuringActivity(store.database().clone())),
            )
            .unwrap();
        assert_eq!(
            get(&app, "/api/v1/users/3/stats", Some(&tokens[0])).await.0,
            StatusCode::FORBIDDEN
        );
        yoink_db::query("UPDATE users SET ban_until=4070908800000000 WHERE id=2")
            .execute(store.database())
            .await
            .unwrap();
        assert_eq!(
            get(&app, path, Some(&tokens[1])).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get(&app, "/api/v1/users/2/stats", Some(&tokens[0])).await.0,
            StatusCode::FORBIDDEN
        );
        store.close().await.unwrap();
    }
}
