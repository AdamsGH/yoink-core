//! Disk-backed native profile tests.
use std::path::PathBuf;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use yoink_access::{Principal, Role};
use yoink_db::{Database, query, raw_sql};
use yoink_downloads::profile::{NetworkContext, ProfileError, ProfileStore};
use yoink_identity::IdentitySnapshot;

struct TestDir(PathBuf);

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn setup() -> (TestDir, Database, ProfileStore) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let db = Database::open(&dir.path().join("profile.db"), true, Duration::from_secs(2))
        .await
        .unwrap();
    raw_sql(include_str!("profile_schema.sql"))
        .execute(&db)
        .await
        .unwrap();
    let path = dir.keep();
    (TestDir(path), db.clone(), ProfileStore::new(db))
}

fn identity(actor: i64, role: Role) -> IdentitySnapshot {
    IdentitySnapshot {
        principal: Principal {
            user_id: actor,
            role,
            ban_until: None,
        },
        language: "en".into(),
        grants: vec![],
        providers: vec![],
        now: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        group: None,
    }
}

fn context<'a>(
    actor_id: u64,
    domain: &'a str,
    identity: &'a IdentitySnapshot,
) -> NetworkContext<'a> {
    NetworkContext {
        actor_id,
        domain,
        no_cookie_domains: &[],
        identity,
        system_proxy: None,
    }
}

#[tokio::test]
async fn disk_profile_preferences_and_frozen_cookie_credentials() {
    let (_dir, db, store) = setup().await;
    query("INSERT INTO users (id) VALUES (?1)")
        .bind(42_i64)
        .execute(&db)
        .await
        .unwrap();
    query("INSERT INTO dl_user_settings (user_id) VALUES (?1)")
        .bind(42_i64)
        .execute(&db)
        .await
        .unwrap();
    let defaults = store.preferences(42).await.unwrap().unwrap();
    assert_eq!(defaults.quality, "best");
    assert!(!defaults.proxy_enabled);
    assert!(defaults.use_pool_cookies);
    assert_eq!(defaults.youtube_auth_mode, "cookies");
    assert_eq!(defaults.args_json, serde_json::json!({}));
    assert!(store.preferences(43).await.unwrap().is_none());

    let jar = "# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tFALSE\t\tsid\tfixture\n";
    query("INSERT INTO cookies (user_id, domain, content, is_valid, is_pool, updated_at) VALUES (?1, ?2, ?3, 1, 0, ?4)")
        .bind(42_i64)
        .bind("example.com")
        .bind(jar)
        .bind(1_700_000_000_000_000_i64)
        .execute(&db)
        .await
        .unwrap();
    let actor = identity(42, Role::User);
    let selected = store
        .prepare_network(context(42, "sub.example.com", &actor), 0)
        .await
        .unwrap();
    assert!(selected.cookie().is_some());
    let resolved = store
        .resolve(context(42, "sub.example.com", &actor), &selected)
        .await
        .unwrap();
    assert_eq!(resolved.cookie_jar.as_deref(), Some(jar));

    let no_cookie_domains = vec!["example.com".to_owned()];
    let no_cookie = store
        .prepare_network(
            NetworkContext {
                actor_id: 42,
                domain: "sub.example.com",
                no_cookie_domains: &no_cookie_domains,
                identity: &actor,
                system_proxy: None,
            },
            0,
        )
        .await
        .unwrap();
    assert!(no_cookie.cookie().is_none());
    query("UPDATE dl_user_settings SET youtube_auth_mode = 'oauth' WHERE user_id = ?1")
        .bind(42_i64)
        .execute(&db)
        .await
        .unwrap();
    assert!(
        store
            .prepare_network(context(42, "youtube.com", &actor), 0)
            .await
            .unwrap()
            .cookie()
            .is_none()
    );
    query("UPDATE dl_user_settings SET youtube_auth_mode = 'cookies' WHERE user_id = ?1")
        .bind(42_i64)
        .execute(&db)
        .await
        .unwrap();

    query("UPDATE dl_user_settings SET proxy_enabled = 1 WHERE user_id = ?1")
        .bind(42_i64)
        .execute(&db)
        .await
        .unwrap();
    assert!(matches!(
        store
            .resolve(context(42, "sub.example.com", &actor), &selected)
            .await,
        Err(ProfileError::StaleReference)
    ));
    query("UPDATE dl_user_settings SET proxy_enabled = 0, proxy_url = NULL WHERE user_id = ?1")
        .bind(42_i64)
        .execute(&db)
        .await
        .unwrap();
    query("UPDATE dl_user_settings SET proxy_enabled = 1, proxy_url = 'http://one.proxy' WHERE user_id = ?1")
        .bind(42_i64)
        .execute(&db)
        .await
        .unwrap();
    let proxy_selection = store
        .prepare_network(context(42, "sub.example.com", &actor), 0)
        .await
        .unwrap();
    query("UPDATE dl_user_settings SET proxy_url = 'http://two.proxy' WHERE user_id = ?1")
        .bind(42_i64)
        .execute(&db)
        .await
        .unwrap();
    assert!(matches!(
        store
            .resolve(context(42, "sub.example.com", &actor), &proxy_selection)
            .await,
        Err(ProfileError::StaleReference)
    ));
    query("UPDATE dl_user_settings SET proxy_enabled = 0, proxy_url = NULL WHERE user_id = ?1")
        .bind(42_i64)
        .execute(&db)
        .await
        .unwrap();

    assert!(matches!(
        store
            .resolve(context(42, "notexample.com", &actor), &selected)
            .await,
        Err(ProfileError::StaleReference)
    ));
    query("UPDATE cookies SET content = ?1 WHERE id = ?2")
        .bind("# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tFALSE\t\tsid\tchanged\n")
        .bind(selected.cookie().unwrap().id())
        .execute(&db)
        .await
        .unwrap();
    assert!(matches!(
        store
            .resolve(context(42, "example.com", &actor), &selected)
            .await,
        Err(ProfileError::StaleReference)
    ));
}

#[tokio::test]
async fn profile_schema_check_uses_only_required_columns() {
    let (_dir, _db, store) = setup().await;
    store.check_schema().await.unwrap();
}
