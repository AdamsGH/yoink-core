//! Exact public composition seam, real native storage/identity/JWT; only external
//! Insight and GitHub boundaries are replaced. Network tests use loopback only.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tower::ServiceExt;
use yoink_auth::{AuthConfig, JwtProfile};
use yoink_http::{
    AuthService, AuthState, InboxAuthorization, router_with_identity_user_session_and_inbox,
};
use yoink_identity::{IdentityStore, TrustedProfile};
use yoink_inbox::{
    contracts::{Call, Insight},
    github::{Github, StarSnapshot},
    http::{Actor, HttpState},
};

struct External;
impl Insight for External {
    fn github_token(&self, _: i64, _: bool) -> Call<'_, Option<String>> {
        panic!("unexpected Insight call")
    }
    fn complete<'a>(&'a self, _: i64, _: &'a str) -> Call<'a, String> {
        panic!("unexpected Insight call")
    }
    fn language(&self, _: i64) -> Call<'_, String> {
        panic!("unexpected Insight call")
    }
}
impl Github for External {
    fn stars<'a>(&'a self, _: &'a str, _: Option<&'a str>) -> Call<'a, StarSnapshot> {
        panic!("unexpected GitHub call")
    }
    fn star<'a>(&'a self, _: &'a str, _: &'a str, _: &'a str, _: bool) -> Call<'a, ()> {
        panic!("unexpected GitHub call")
    }
    fn get_json<'a>(&'a self, _: Option<&'a str>, _: &'a str) -> Call<'a, Value> {
        panic!("unexpected GitHub call")
    }
    fn is_starred<'a>(&'a self, _: &'a str, _: &'a str, _: &'a str) -> Call<'a, bool> {
        panic!("unexpected GitHub call")
    }
    fn graphql<'a>(&'a self, _: &'a str, _: &'a str, _: Value, _: bool) -> Call<'a, Value> {
        panic!("unexpected GitHub call")
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    storage: yoink_storage::Store,
    app: Router,
    system: yoink_application::SystemService,
    auth: AuthState,
    inbox: HttpState,
    tokens: Vec<String>,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let storage =
            yoink_storage::Store::open(&dir.path().join("native.db"), true, Duration::from_secs(2))
                .await
                .unwrap();
        storage.migrate().await.unwrap();
        storage.verify_schema().await.unwrap();
        let identities = Arc::new(IdentityStore::new(
            storage.database().clone(),
            Some(1),
            false,
        ));
        for user_id in 1..=4 {
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
            "fixture-only".into(),
            "fixture-inbox-secret-at-least-32-bytes".into(),
            86400,
            30,
            86400,
        )
        .unwrap();
        // Deliberately false owner role claims: only persisted current identity is authority.
        let tokens = (1..=4)
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
        assert!(auth.install_identities(identities.clone()).is_ok());
        assert!(
            auth.install(AuthService {
                identities: identities.clone(),
                crypto
            })
            .is_ok()
        );
        let system = yoink_application::SystemService::new("test");
        system.set_ready(true);
        let inbox = HttpState {
            store: yoink_inbox::store::Store::new(storage.database().clone()),
            auth: Arc::new(InboxAuthorization::new(identities)),
            insight: Arc::new(External),
            github: Arc::new(External),
        };
        let app = router_with_identity_user_session_and_inbox(
            system.clone(),
            Some(auth.clone()),
            auth.clone(),
            None,
            inbox.clone(),
        );
        Self {
            _dir: dir,
            storage,
            app,
            system,
            auth,
            inbox,
            tokens,
        }
    }
    async fn sql(&self, sql: &str) {
        yoink_db::raw_sql(sql)
            .execute(self.storage.database())
            .await
            .unwrap();
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Value,
        actor: Option<i64>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(format!("/api/v1/inbox{path}"))
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let mut req = req.body(Body::from(body.to_string())).unwrap();
        if let Some(actor) = actor {
            req.extensions_mut().insert(Actor(actor));
        }
        let res = self.app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        if res.status() == StatusCode::UNAUTHORIZED {
            assert_eq!(res.headers()[header::WWW_AUTHENTICATE], "Bearer");
        }
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    async fn get(&self, path: &str, user: usize) -> StatusCode {
        self.request("GET", path, Some(&self.tokens[user - 1]), Value::Null, None)
            .await
            .0
    }
}

#[tokio::test]
async fn bearer_bridge_native_routes_foreign_records_and_injected_actor() {
    let f = Fixture::new().await;
    for token in [None, Some("invalid")] {
        assert_eq!(
            f.request("GET", "/items", token, Value::Null, Some(1))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    f.sql("UPDATE users SET role='user' WHERE id IN (2,3)")
        .await;
    let (status, item) = f
        .request(
            "POST",
            "/items",
            Some(&f.tokens[1]),
            json!({"url":"https://example.invalid/article", "user_id":1}),
            Some(1),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let path = format!("/items/{}", item["id"].as_i64().unwrap());
    assert_eq!(f.get(&path, 2).await, StatusCode::OK);
    assert_eq!(f.get(&path, 3).await, StatusCode::NOT_FOUND);
    assert_eq!(
        f.request("GET", &path, Some(&f.tokens[2]), Value::Null, Some(2))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    for path in [
        "/items",
        "/categories",
        "/folders",
        "/rules",
        "/teams",
        "/settings/classify-hint",
    ] {
        assert_eq!(f.get(path, 2).await, StatusCode::OK, "{path}");
    }
    f.system.set_ready(false);
    assert_eq!(f.get("/items", 2).await, StatusCode::SERVICE_UNAVAILABLE);
    f.system.set_ready(true);
    assert_eq!(f.get("/items", 2).await, StatusCode::OK);
}

#[tokio::test]
async fn unchanged_jwt_observes_revoked_grants_and_current_bans() {
    let f = Fixture::new().await;
    f.sql("UPDATE users SET role='restricted' WHERE id=2; INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(2,'inbox','ingest',1)").await;
    assert_eq!(f.get("/items", 2).await, StatusCode::OK);
    f.sql("DELETE FROM user_permissions WHERE user_id=2").await;
    assert_eq!(f.get("/items", 2).await, StatusCode::FORBIDDEN);
    f.sql("UPDATE users SET role='user' WHERE id=2").await;
    assert_eq!(f.get("/items", 2).await, StatusCode::OK);
    assert_eq!(f.get("/gh_stars", 2).await, StatusCode::FORBIDDEN);
    f.sql("INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(2,'inbox','gh_sync',1)").await;
    assert_eq!(f.get("/gh_stars", 2).await, StatusCode::OK);
    f.sql("UPDATE users SET ban_until=CAST((unixepoch()+3600)*1000000 AS INTEGER) WHERE id=2")
        .await;
    assert_eq!(f.get("/items", 2).await, StatusCode::FORBIDDEN);
    assert_eq!(f.get("/gh_stars", 2).await, StatusCode::FORBIDDEN);
    f.sql("UPDATE users SET ban_until=NULL,role='banned' WHERE id=2")
        .await;
    assert_eq!(f.get("/items", 2).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn legacy_admin_is_current_moderator_or_higher_and_bans_win() {
    let f = Fixture::new().await;
    let path = "/admin/classify-prompt";
    f.sql("UPDATE users SET role='user' WHERE id=2; INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(2,'inbox','admin',1)").await;
    assert_eq!(f.get(path, 2).await, StatusCode::FORBIDDEN);
    for role in ["moderator", "admin", "owner"] {
        f.sql(&format!("UPDATE users SET role='{role}' WHERE id=2"))
            .await;
        assert_eq!(f.get(path, 2).await, StatusCode::OK);
    }
    f.sql("UPDATE users SET role='moderator',ban_until=CAST((unixepoch()+3600)*1000000 AS INTEGER) WHERE id=2").await;
    assert_eq!(f.get(path, 2).await, StatusCode::FORBIDDEN);
    f.sql("UPDATE users SET role='banned' WHERE id=1").await;
    assert_eq!(f.get(path, 1).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn mutation_status_proves_only_the_owned_matching_operation_key() {
    let f = Fixture::new().await;
    let store = yoink_inbox::store::Store::new(f.storage.database().clone());
    let intent = store
        .prepare_mutation_keyed(
            1,
            &yoink_inbox::mutations::Mutation::CreateList {
                body: json!({"name":"Proof fixture","is_local":false}),
            },
            Some("fixture-create-key"),
            1,
        )
        .await
        .unwrap();
    store
        .cancel_prepared_mutation(1, &intent.id, 2)
        .await
        .unwrap();
    let oversized = "x".repeat(129);
    for (key, expected, status) in [
        (None, false, StatusCode::OK),
        (Some("unrelated-conflict-key"), false, StatusCode::OK),
        (Some("fixture-create-key"), true, StatusCode::OK),
        (Some(""), false, StatusCode::UNPROCESSABLE_ENTITY),
        (
            Some(oversized.as_str()),
            false,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let mut request = Request::builder()
            .uri(format!("/api/v1/inbox/gh_mutations/{}", intent.id))
            .header(header::AUTHORIZATION, format!("Bearer {}", f.tokens[0]));
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        let response = f
            .app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        if status == StatusCode::OK {
            assert_eq!(body["operation_key_matches"], expected);
            assert_eq!(body["state"], "rejected");
            assert!(body.get("request_key").is_none());
            assert!(
                !String::from_utf8(bytes.to_vec())
                    .unwrap()
                    .contains("fixture-create-key")
            );
        }
    }
    f.sql("UPDATE users SET role='user' WHERE id=2; INSERT INTO user_permissions(user_id,plugin,feature,granted_by) VALUES(2,'inbox','gh_write',1)").await;
    let foreign = Request::builder()
        .uri(format!("/api/v1/inbox/gh_mutations/{}", intent.id))
        .header(header::AUTHORIZATION, format!("Bearer {}", f.tokens[1]))
        .header("Idempotency-Key", "fixture-create-key")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        f.app.oneshot(foreign).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

async fn transport_status(address: std::net::SocketAddr, token: Option<&str>) -> StatusCode {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let authorization = token
            .map(|value| format!("Authorization: Bearer {value}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "GET /api/v1/inbox/items HTTP/1.1\r\nHost: localhost\r\n{authorization}Connection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.take(65_537).read_to_end(&mut response).await.unwrap();
        assert!(response.len() <= 65_536);
        let response = std::str::from_utf8(&response).unwrap();
        let status = response.lines().next().unwrap().split_whitespace().nth(1).unwrap();
        StatusCode::from_u16(status.parse().unwrap()).unwrap()
    })
    .await
    .expect("local HTTP transport exceeded its deadline")
}

#[tokio::test]
async fn actual_http_transport_installs_inbox_observes_bans_and_drains() {
    let f = Fixture::new().await;
    f.sql("UPDATE users SET role='user' WHERE id=2").await;
    for (authentication, installed) in [(true, true), (true, false), (false, true)] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let shutdown = tokio_util::sync::CancellationToken::new();
        let server = tokio::spawn(yoink_http::serve_with_user_session_and_inbox(
            listener,
            f.system.clone(),
            authentication.then(|| f.auth.clone()),
            f.auth.clone(),
            None,
            installed.then(|| f.inbox.clone()),
            shutdown.clone(),
        ));
        if authentication && installed {
            assert_eq!(
                transport_status(address, None).await,
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                transport_status(address, Some(&f.tokens[1])).await,
                StatusCode::OK
            );
            f.sql("UPDATE users SET role='banned' WHERE id=2").await;
            assert_eq!(
                transport_status(address, Some(&f.tokens[1])).await,
                StatusCode::FORBIDDEN
            );
        } else {
            assert_eq!(
                transport_status(address, Some(&f.tokens[1])).await,
                StatusCode::NOT_FOUND
            );
        }
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .expect("HTTP server did not drain")
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn late_install_preserves_diagnostics_and_current_authority() {
    let f = Fixture::new().await;
    f.sql("UPDATE users SET role='user' WHERE id=2").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let inbox = yoink_http::LateInbox::default();
    let server = tokio::spawn(yoink_http::serve_with_late_inbox(
        listener,
        f.system.clone(),
        Some(f.auth.clone()),
        f.auth.clone(),
        None,
        inbox.clone(),
        shutdown.clone(),
    ));
    assert_eq!(
        transport_status(address, Some(&f.tokens[1])).await,
        StatusCode::NOT_FOUND
    );
    inbox
        .install(f.system.clone(), f.auth.clone(), f.inbox.clone())
        .unwrap();
    assert_eq!(
        transport_status(address, None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        transport_status(address, Some(&f.tokens[1])).await,
        StatusCode::OK
    );
    f.sql("UPDATE users SET role='banned' WHERE id=2").await;
    assert_eq!(
        transport_status(address, Some(&f.tokens[1])).await,
        StatusCode::FORBIDDEN
    );
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
