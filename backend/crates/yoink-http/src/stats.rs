use crate::auth::{ApiError, AuthState, authenticated_id};
use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::Response,
};
use yoink_application::SystemService;
#[path = "stats_imports.rs"]
mod imports;

pub fn router(system: SystemService, auth: AuthState, stats: yoink_stats::Stats) -> Router {
    Router::new().nest(
        "/api/v1/stats",
        yoink_stats::http::router(stats).layer(from_fn_with_state((system, auth), authenticate)),
    )
}

pub fn router_with_imports(
    system: SystemService,
    auth: AuthState,
    stats: yoink_stats::Stats,
    jobs: yoink_stats::imports::ImportJobs,
) -> Router {
    Router::new().nest(
        "/api/v1/stats",
        yoink_stats::http::router(stats)
            .merge(imports::router(jobs))
            .layer(from_fn_with_state((system, auth), authenticate)),
    )
}

async fn authenticate(
    State((system, auth)): State<(SystemService, AuthState)>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if !system.snapshot().ready {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service unavailable",
        ));
    }
    let actor = authenticated_id(auth.service()?, request.headers())?;
    // Stats checks current native authority; JWT roles and caller extensions are not grants.
    request
        .extensions_mut()
        .insert(yoink_stats::http::AuthenticatedUser(actor));
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use std::{sync::Arc, time::Duration};
    use tower::ServiceExt;
    use yoink_auth::{AuthConfig, JwtProfile};
    use yoink_identity::{IdentityStore, TrustedProfile};

    struct UnavailableMembers;
    impl yoink_stats::MembershipProvider for UnavailableMembers {
        fn lookup(&self, _: i64, _: i64) -> yoink_stats::MembershipFuture<'_> {
            Box::pin(async { Err(yoink_stats::Error::Unavailable) })
        }
        fn lookup_name(&self, _: Option<i64>, _: i64) -> yoink_stats::NameFuture<'_> {
            Box::pin(async { Ok(None) })
        }
    }

    async fn get(
        app: &Router,
        path: &str,
        token: Option<&str>,
        extension_actor: i64,
    ) -> StatusCode {
        let mut request = Request::builder().uri(path);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let mut request = request.body(Body::empty()).unwrap();
        request
            .extensions_mut()
            .insert(yoink_stats::http::AuthenticatedUser(extension_actor));
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        response.status()
    }

    #[tokio::test]
    async fn verified_actor_overwrites_extensions_and_current_authority_overrides_jwt_role() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let store =
            yoink_storage::Store::open(&dir.path().join("stats.db"), true, Duration::from_secs(2))
                .await
                .unwrap();
        store.migrate().await.unwrap();
        let identities = Arc::new(IdentityStore::new(store.database().clone(), Some(1), false));
        for user_id in [1, 2] {
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
            "fixture-stats-jwt-secret-32-bytes".into(),
            86400,
            30,
            86400,
        )
        .unwrap();
        let tokens: Vec<_> = [1, 2]
            .into_iter()
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
            auth.install(crate::AuthService {
                identities: identities.clone(),
                crypto
            })
            .is_ok()
        );
        let stats = yoink_stats::Stats::open(
            store.database().clone(),
            identities,
            Arc::new(UnavailableMembers),
            None,
            dir.path().join("search"),
        )
        .await
        .unwrap();
        let system = SystemService::new("test");
        let jobs = yoink_stats::imports::ImportJobs::new(
            stats.clone(),
            yoink_stats::imports::ImportConfig {
                root: dir.path().to_owned(),
                max_bytes: 4096,
                max_messages: 100,
                max_jobs: 4,
                timeout: Duration::from_secs(2),
            },
        )
        .unwrap();
        let app = router_with_imports(system.clone(), auth, stats.clone(), jobs.clone());
        let groups = "/api/v1/stats/groups";
        assert_eq!(
            get(&app, groups, Some(&tokens[0]), 1).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        system.set_ready(true);
        assert_eq!(get(&app, groups, None, 1).await, StatusCode::UNAUTHORIZED);
        assert_eq!(
            get(&app, groups, Some("invalid"), 1).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(get(&app, groups, Some(&tokens[0]), 2).await, StatusCode::OK);
        assert_eq!(
            get(
                &app,
                "/api/v1/stats/top-reactions?chat_id=-100",
                Some(&tokens[1]),
                1
            )
            .await,
            StatusCode::FORBIDDEN
        );
        let boundary = "stats-fixture";
        let multipart = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"result.json\"\r\nContent-Type: application/json\r\n\r\n{{\"messages\":[]}}\r\n--{boundary}--\r\n"
        );
        for (token, expected) in [
            (&tokens[1], StatusCode::FORBIDDEN),
            (&tokens[0], StatusCode::OK),
        ] {
            let request = Request::builder()
                .method("POST")
                .uri("/api/v1/stats/import?chat_id=-100")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart.clone()))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
        for (query, body) in [
            ("chat_id=100", multipart.clone()),
            (
                "chat_id=-100",
                multipart.replace("result.json", "result.txt"),
            ),
            (
                "chat_id=-100",
                multipart.replace("{\"messages\":[]}", &"x".repeat(4097)),
            ),
        ] {
            let request = Request::builder()
                .method("POST")
                .uri(format!("/api/v1/stats/import?{query}"))
                .header(header::AUTHORIZATION, format!("Bearer {}", tokens[0]))
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap();
            assert_eq!(
                app.clone().oneshot(request).await.unwrap().status(),
                StatusCode::BAD_REQUEST
            );
        }
        std::fs::write(dir.path().join("result.json"), b"{\"messages\":[]}").unwrap();
        for (path, expected) in [
            ("result.json", StatusCode::OK),
            ("../private.json", StatusCode::BAD_REQUEST),
        ] {
            let request = Request::builder()
                .method("POST")
                .uri("/api/v1/stats/import/by-path")
                .header(header::AUTHORIZATION, format!("Bearer {}", tokens[0]))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"path":path,"chat_id":-100}).to_string(),
                ))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected);
        }
        jobs.drain().await;
        assert_eq!(
            get(&app, "/api/v1/stats/import/1", Some(&tokens[0]), 2).await,
            StatusCode::OK
        );
        assert_eq!(
            get(&app, "/api/v1/stats/import/1", Some(&tokens[1]), 1).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get(&app, "/api/v1/stats/import/999", Some(&tokens[0]), 2).await,
            StatusCode::NOT_FOUND
        );
        yoink_db::query("UPDATE users SET ban_until=4070908800000000 WHERE id=1")
            .execute(store.database())
            .await
            .unwrap();
        assert_eq!(
            get(&app, groups, Some(&tokens[0]), 2).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get(&app, "/api/v1/stats/import/1", Some(&tokens[0]), 2).await,
            StatusCode::FORBIDDEN
        );
        stats.shutdown().await.unwrap();
        store.close().await.unwrap();
    }
}
