use chrono::Utc;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use yoink_access::{Principal, Role};
use yoink_db::{Database, query, raw_sql};
use yoink_downloads::profile::{
    NetworkContext, ProfileError, ProfileStore, oauth, oauth_refresh::OAuthRefreshProvider,
};
use yoink_identity::IdentitySnapshot;
use yoink_worker_protocol::youtube_auth::YoutubeOAuth;

struct Provider {
    db: Database,
    action: &'static str,
}
impl OAuthRefreshProvider for Provider {
    fn exchange<'a>(
        &'a self,
        _tokens: &'a YoutubeOAuth,
    ) -> Pin<Box<dyn Future<Output = Result<YoutubeOAuth, ProfileError>> + Send + 'a>> {
        Box::pin(async move {
            // This write proves no DB connection/transaction is held across networking.
            if self.action == "pending" {
                std::future::pending::<()>().await;
            }
            match self.action {
                "revoke" => {
                    query("UPDATE cookies SET is_valid=0")
                        .execute(&self.db)
                        .await?;
                }
                "ban" => {
                    query("UPDATE users SET role='banned'")
                        .execute(&self.db)
                        .await?;
                }
                _ => {}
            }
            Ok(tokens("refreshed", "2099-01-01T00:00:00Z"))
        })
    }
}
fn tokens(access: &str, expiry: &str) -> YoutubeOAuth {
    YoutubeOAuth {
        access_token: access.into(),
        refresh_token: "synthetic-refresh".into(),
        expires_at: expiry.into(),
    }
}
fn content(tokens: &YoutubeOAuth) -> String {
    format!(
        "{}{}",
        oauth::CONTENT_PREFIX,
        serde_json::to_string(tokens).unwrap()
    )
}
fn identity() -> IdentitySnapshot {
    IdentitySnapshot {
        principal: Principal {
            user_id: 42,
            role: Role::User,
            ban_until: None,
        },
        language: "en".into(),
        grants: vec![],
        providers: vec![],
        now: Utc::now(),
        group: None,
    }
}
fn context(identity: &IdentitySnapshot) -> NetworkContext<'_> {
    NetworkContext {
        actor_id: 42,
        domain: "youtube.com",
        identity,
        no_cookie_domains: &[],
        system_proxy_domains: &[],
        system_proxy: None,
    }
}
async fn setup(action: &'static str, expiry: &str) -> (tempfile::TempDir, Database, ProfileStore) {
    let dir = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let db = Database::open(&dir.path().join("test.db"), true, Duration::from_secs(2))
        .await
        .unwrap();
    raw_sql(include_str!("profile_schema.sql"))
        .execute(&db)
        .await
        .unwrap();
    raw_sql("ALTER TABLE users ADD COLUMN role TEXT DEFAULT 'user'; ALTER TABLE users ADD COLUMN ban_until INTEGER; INSERT INTO users(id) VALUES(42); INSERT INTO dl_user_settings(user_id,youtube_auth_mode) VALUES(42,'oauth');").execute(&db).await.unwrap();
    query("INSERT INTO cookies(user_id,domain,content,is_valid,is_pool,updated_at) VALUES(42,'youtube.com',?1,1,0,1)").bind(content(&tokens("old",expiry))).execute(&db).await.unwrap();
    let store = ProfileStore::new(db.clone()).with_oauth_refresh(Arc::new(Provider {
        db: db.clone(),
        action,
    }));
    (dir, db, store)
}
#[tokio::test]
async fn youtube_auth_refresh_is_pre_admission_and_old_refs_are_immutable() {
    let (_dir, db, store) = setup("ok", "2099-01-01T00:00:00Z").await;
    let actor = identity();
    let old = store.prepare_network(context(&actor), 0).await.unwrap();
    query("UPDATE cookies SET content=?1")
        .bind(content(&tokens("old", "2000-01-01T00:00:00Z")))
        .execute(&db)
        .await
        .unwrap();
    let fresh = tokio::time::timeout(
        Duration::from_secs(3),
        store.prepare_network(context(&actor), 0),
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(
        old.cookie().unwrap().version(),
        fresh.cookie().unwrap().version()
    );
    assert!(matches!(
        store.resolve(context(&actor), &old).await,
        Err(ProfileError::StaleReference)
    ));
    let resolved = store.resolve(context(&actor), &fresh).await.unwrap();
    assert!(resolved.cookie_jar.is_none());
    assert_eq!(resolved.youtube_oauth.unwrap().access_token, "refreshed");
}
#[tokio::test]
async fn youtube_auth_refresh_cannot_replace_revoked_or_banned_work() {
    for action in ["revoke", "ban"] {
        let (_dir, _db, store) = setup(action, "2000-01-01T00:00:00Z").await;
        assert!(matches!(
            store.prepare_network(context(&identity()), 0).await,
            Err(ProfileError::StaleReference)
        ));
    }
}
#[tokio::test]
async fn youtube_auth_cookie_fallback_is_real_netscape() {
    let (_dir, db, store) = setup("ok", "2099-01-01T00:00:00Z").await;
    let jar =
        "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t1700000000\tsid\tsynthetic\n";
    query("UPDATE cookies SET content=?1")
        .bind(jar)
        .execute(&db)
        .await
        .unwrap();
    let actor = identity();
    let selected = store.prepare_network(context(&actor), 0).await.unwrap();
    let resolved = store.resolve(context(&actor), &selected).await.unwrap();
    assert_eq!(resolved.cookie_jar.as_deref(), Some(jar));
    assert!(resolved.youtube_oauth.is_none());
}

#[tokio::test]
async fn youtube_auth_refresh_timeout_is_bounded_without_row_replacement() {
    let (_dir, db, store) = setup("pending", "2000-01-01T00:00:00Z").await;
    let store = store
        .with_oauth_refresh_timeout(Duration::from_millis(10))
        .unwrap();
    assert!(matches!(
        store.prepare_network(context(&identity()), 0).await,
        Err(ProfileError::Unavailable)
    ));
    let row = query("SELECT content FROM cookies")
        .fetch_one(&db)
        .await
        .unwrap();
    let persisted: String = row.try_get("content").unwrap();
    assert_eq!(oauth::decode(&persisted).unwrap().access_token, "old");
}
