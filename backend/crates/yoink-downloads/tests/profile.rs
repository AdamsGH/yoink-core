//! Read-only profile contracts against a disposable migrated native database.
use std::time::Duration;
use yoink_access::Role;
use yoink_db::{Database, query, raw_sql};
use yoink_downloads::profile::{NetworkContext, ProfileError, ProfileStore};
use yoink_identity::IdentityStore;

const USER: i64 = 8_000_000_000_201;
const POOL_OWNER: i64 = 8_000_000_000_202;
const BAN_USER: i64 = 8_000_000_000_203;
const PREFS_USER: i64 = 8_000_000_000_204;
const SECRET: &str = "# Netscape HTTP Cookie File\n#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1700000000\tsid\tfake-cookie-value\n";
const TWITTER_COOKIE: &str =
    "# Netscape HTTP Cookie File\n.twitter.com\tTRUE\t/\tFALSE\t\tsid\tfake-twitter-value\n";

async fn identity(pool: &Database, actor: i64) -> yoink_identity::IdentitySnapshot {
    IdentityStore::new(pool.clone(), None, false)
        .snapshot(actor, None)
        .await
        .unwrap()
        .unwrap()
}
fn context<'a>(
    actor_id: u64,
    domain: &'a str,
    identity: &'a yoink_identity::IdentitySnapshot,
    no_cookie_domains: &'a [String],
    system_proxy: Option<&'a str>,
) -> NetworkContext<'a> {
    NetworkContext {
        actor_id,
        domain,
        no_cookie_domains,
        identity,
        system_proxy,
    }
}
async fn insert_cookie(
    pool: &Database,
    owner: i64,
    domain: &str,
    content: &str,
    valid: bool,
    is_pool: bool,
) -> i32 {
    query(include_str!("sql/cookie_insert.sql"))
        .bind(owner)
        .bind(domain)
        .bind(content)
        .bind(valid)
        .bind(is_pool)
        .fetch_one(pool)
        .await
        .unwrap()
        .try_get("id")
        .unwrap()
}
async fn set_setting(pool: &Database, actor: i64, column: &str, value: &str) {
    // Keep SQL static and parameterized; only enumerate fixture-controlled known columns.
    let statement = match column {
        "proxy_enabled" => include_str!("sql/set_proxy_enabled.sql"),
        "proxy_url" => include_str!("sql/set_proxy_url.sql"),
        "use_pool_cookies" => include_str!("sql/set_pool_enabled.sql"),
        "youtube_auth_mode" => include_str!("sql/set_auth_mode.sql"),
        _ => unreachable!(),
    };
    let setting = query(statement);
    let setting = if matches!(column, "proxy_enabled" | "use_pool_cookies") {
        setting.bind(match value {
            "true" => true,
            "false" => false,
            _ => panic!("fixture boolean setting must be true or false"),
        })
    } else {
        setting.bind(value)
    };
    setting.bind(actor).execute(pool).await.unwrap();
}

#[tokio::test]
async fn native_profile_contracts_serial() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = directory.path().join("profile.db");
    let storage = yoink_storage::Store::open(&path, true, Duration::from_secs(2))
        .await
        .unwrap();
    storage.migrate().await.unwrap();
    let pool = storage.database().clone();
    raw_sql(include_str!("fixtures.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let store = ProfileStore::new(pool.clone());

    // User absence differs from an existing row without a settings row.
    assert!(
        store
            .preferences((USER + 99) as u64)
            .await
            .unwrap()
            .is_none()
    );
    let defaults = store.preferences(PREFS_USER as u64).await.unwrap().unwrap();
    assert_eq!(defaults.quality, "best");
    assert_eq!(defaults.codec, "avc1");
    assert_eq!(defaults.container, "mp4");
    assert_eq!(defaults.args_json, serde_json::json!({}));

    let personal = insert_cookie(&pool, USER, "youtube.com", SECRET, true, false).await;
    let pool_cookie = insert_cookie(&pool, POOL_OWNER, "youtube.com", SECRET, true, true).await;
    let twitter = insert_cookie(&pool, USER, "twitter.com", TWITTER_COOKIE, true, false).await;
    let base = identity(&pool, USER).await;
    let none = Vec::new();
    let prepared = store
        .prepare_network(
            context(USER as u64, "https://youtu.be/watch", &base, &none, None),
            0,
        )
        .await
        .unwrap();
    assert_eq!(prepared.cookie().unwrap().id(), personal);
    let resolved = store
        .resolve(
            context(USER as u64, "youtube.com", &base, &none, None),
            &prepared,
        )
        .await
        .unwrap();
    assert_eq!(resolved.cookie_jar.as_deref(), Some(SECRET));
    assert!(format!("{prepared:?}").find("fake-cookie-value").is_none());

    // Enforce read-only operations on the shared connection for this isolated section.
    raw_sql(include_str!("sql/read_only_session.sql"))
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        query(include_str!("sql/set_proxy_enabled.sql"))
            .bind(true)
            .bind(USER)
            .execute(&pool)
            .await
            .is_err()
    );
    let read_only = ProfileStore::new(pool.clone());
    read_only.check_schema().await.unwrap();
    assert!(read_only.preferences(USER as u64).await.unwrap().is_some());
    let read_identity = identity(&pool, USER).await;
    let read_selection = read_only
        .prepare_network(
            context(USER as u64, "youtube.com", &read_identity, &none, None),
            0,
        )
        .await
        .unwrap();
    assert_eq!(read_selection, prepared);
    assert_eq!(
        read_only
            .resolve(
                context(USER as u64, "youtube.com", &read_identity, &none, None),
                &read_selection
            )
            .await
            .unwrap()
            .cookie_jar
            .as_deref(),
        Some(SECRET)
    );

    raw_sql("PRAGMA query_only = 0;")
        .execute(&pool)
        .await
        .unwrap();

    // The database query itself handles aliases and suffix matching, while excluding lookalike hosts.
    let twitter_selection = store
        .prepare_network(context(USER as u64, "m.twitter.com", &base, &none, None), 0)
        .await
        .unwrap();
    assert_eq!(twitter_selection.cookie().unwrap().id(), twitter);
    assert!(matches!(
        store
            .resolve(
                context(USER as u64, "notyoutube.com", &base, &none, None),
                &prepared
            )
            .await,
        Err(ProfileError::StaleReference)
    ));

    // No-cookie policy bypasses both lookup and reference use.
    let bypass = vec!["youtube.com".to_owned()];
    let bypass_selection = store
        .prepare_network(context(USER as u64, "youtube.com", &base, &bypass, None), 0)
        .await
        .unwrap();
    assert!(bypass_selection.cookie().is_none());
    assert!(
        store
            .resolve(
                context(USER as u64, "youtube.com", &base, &bypass, None),
                &bypass_selection
            )
            .await
            .unwrap()
            .cookie_jar
            .is_none()
    );
    assert!(matches!(
        store
            .resolve(
                context(USER as u64, "youtube.com", &base, &bypass, None),
                &prepared
            )
            .await,
        Err(ProfileError::StaleReference)
    ));

    // Seeded selection is repeatable, and a later candidate cannot replace a frozen reference.
    let again = store
        .prepare_network(context(USER as u64, "youtube.com", &base, &none, None), 0)
        .await
        .unwrap();
    assert_eq!(again.cookie().unwrap().id(), personal);
    insert_cookie(
        &pool,
        USER,
        "youtu.be",
        "# Netscape HTTP Cookie File\n.youtu.be\tTRUE\t/\tFALSE\t\tsid\tsecond-fake-cookie\n",
        true,
        false,
    )
    .await;
    let still_frozen = store
        .resolve(
            context(USER as u64, "youtube.com", &base, &none, None),
            &prepared,
        )
        .await
        .unwrap();
    assert_eq!(still_frozen.cookie_jar.as_deref(), Some(SECRET));

    // Owning a pool cookie does not bypass shared-cookie authorization.
    insert_cookie(&pool, USER, "youtube.com", SECRET, true, true).await;
    // Pool cookies require a currently refreshed explicit grant; revocation takes effect on refresh.
    assert!(
        matches!(store.prepare_network(context(USER as u64, "youtube.com", &base, &none, None), 1).await.unwrap().cookie(),
        Some(c) if !c.is_pool())
    );
    query(include_str!("sql/grant_shared.sql"))
        .bind(USER)
        .execute(&pool)
        .await
        .unwrap();
    let granted = identity(&pool, USER).await;
    let shared = store
        .prepare_network(
            context(USER as u64, "youtube.com", &granted, &none, None),
            1,
        )
        .await
        .unwrap();
    assert_eq!(shared.cookie().unwrap().id(), pool_cookie);
    assert_eq!(
        store
            .resolve(
                context(USER as u64, "youtube.com", &granted, &none, None),
                &shared
            )
            .await
            .unwrap()
            .cookie_jar
            .as_deref(),
        Some(SECRET)
    );
    query(include_str!("sql/revoke_shared.sql"))
        .bind(USER)
        .execute(&pool)
        .await
        .unwrap();
    let revoked = identity(&pool, USER).await;
    assert!(matches!(
        store
            .resolve(
                context(USER as u64, "youtube.com", &revoked, &none, None),
                &shared
            )
            .await,
        Err(ProfileError::SharedAccessDenied)
    ));

    // Banned principals cannot consume personal, shared, proxy, or NONE references.
    insert_cookie(&pool, BAN_USER, "youtube.com", SECRET, true, false).await;
    set_setting(&pool, BAN_USER, "proxy_url", "http://personal.proxy.test").await;
    set_setting(&pool, BAN_USER, "proxy_enabled", "true").await;
    let before_ban = identity(&pool, BAN_USER).await;
    let personal_selection = store
        .prepare_network(
            context(BAN_USER as u64, "youtube.com", &before_ban, &none, None),
            0,
        )
        .await
        .unwrap();
    let proxy_selection = store
        .prepare_network(
            context(
                BAN_USER as u64,
                "example.org",
                &before_ban,
                &none,
                Some("http://system.proxy.test"),
            ),
            0,
        )
        .await
        .unwrap();
    query(include_str!("sql/set_banned.sql"))
        .bind(BAN_USER)
        .execute(&pool)
        .await
        .unwrap();
    let banned = identity(&pool, BAN_USER).await;
    assert_eq!(banned.principal.role, Role::Banned);
    for selection in [
        &personal_selection,
        &proxy_selection,
        &yoink_domain::NetworkSelection::NONE,
    ] {
        assert!(matches!(
            store
                .resolve(
                    context(
                        BAN_USER as u64,
                        "youtube.com",
                        &banned,
                        &none,
                        Some("http://system.proxy.test")
                    ),
                    selection
                )
                .await,
            Err(ProfileError::SharedAccessDenied)
        ));
    }

    // A queued direct route must fail closed after the user requires a proxy.
    let direct = store
        .prepare_network(context(USER as u64, "example.org", &base, &none, None), 0)
        .await
        .unwrap();
    assert!(direct.proxy().is_none());
    assert!(
        store
            .resolve(
                context(USER as u64, "example.org", &base, &none, None),
                &direct
            )
            .await
            .unwrap()
            .proxy_url
            .is_none()
    );
    set_setting(&pool, USER, "proxy_enabled", "true").await;
    assert_eq!(
        store
            .resolve(
                context(USER as u64, "example.org", &base, &none, None),
                &direct
            )
            .await
            .err(),
        Some(ProfileError::StaleReference)
    );
    set_setting(&pool, USER, "proxy_url", "http://personal.proxy.test").await;
    assert_eq!(
        store
            .resolve(
                context(USER as u64, "example.org", &base, &none, None),
                &direct
            )
            .await
            .err(),
        Some(ProfileError::StaleReference)
    );
    set_setting(&pool, USER, "proxy_enabled", "false").await;
    assert!(
        store
            .resolve(
                context(USER as u64, "example.org", &base, &none, None),
                &direct
            )
            .await
            .unwrap()
            .proxy_url
            .is_none()
    );

    // Proxy source priority, explicit system fallback, and ref invalidation.
    set_setting(&pool, USER, "proxy_url", "http://personal.proxy.test").await;
    set_setting(&pool, USER, "proxy_enabled", "true").await;
    let proxy_selection = store
        .prepare_network(
            context(
                USER as u64,
                "example.org",
                &base,
                &none,
                Some("http://system.proxy.test"),
            ),
            0,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .resolve(
                context(
                    USER as u64,
                    "example.org",
                    &base,
                    &none,
                    Some("http://system.proxy.test")
                ),
                &proxy_selection
            )
            .await
            .unwrap()
            .proxy_url
            .as_deref(),
        Some("http://personal.proxy.test")
    );
    set_setting(&pool, USER, "proxy_url", "").await;
    assert!(matches!(
        store
            .resolve(
                context(
                    USER as u64,
                    "example.org",
                    &base,
                    &none,
                    Some("http://system.proxy.test")
                ),
                &proxy_selection
            )
            .await,
        Err(ProfileError::StaleReference)
    ));
    let system_selection = store
        .prepare_network(
            context(
                USER as u64,
                "example.org",
                &base,
                &none,
                Some("http://system.proxy.test"),
            ),
            0,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .resolve(
                context(
                    USER as u64,
                    "example.org",
                    &base,
                    &none,
                    Some("http://system.proxy.test")
                ),
                &system_selection
            )
            .await
            .unwrap()
            .proxy_url
            .as_deref(),
        Some("http://system.proxy.test")
    );
    assert!(matches!(
        store
            .resolve(
                context(USER as u64, "example.org", &base, &none, None),
                &system_selection
            )
            .await,
        Err(ProfileError::StaleReference)
    ));
    set_setting(&pool, USER, "proxy_url", "http://changed.proxy.test").await;
    assert!(matches!(
        store
            .resolve(
                context(
                    USER as u64,
                    "example.org",
                    &base,
                    &none,
                    Some("http://system.proxy.test")
                ),
                &system_selection
            )
            .await,
        Err(ProfileError::StaleReference)
    ));
    let changed_proxy = store
        .prepare_network(context(USER as u64, "example.org", &base, &none, None), 0)
        .await
        .unwrap();
    set_setting(&pool, USER, "proxy_enabled", "false").await;
    assert!(matches!(
        store
            .resolve(
                context(USER as u64, "example.org", &base, &none, None),
                &changed_proxy
            )
            .await,
        Err(ProfileError::StaleReference)
    ));
    set_setting(&pool, USER, "proxy_enabled", "true").await;
    set_setting(&pool, USER, "proxy_url", "").await;
    assert!(matches!(
        store
            .prepare_network(context(USER as u64, "example.org", &base, &none, None), 0)
            .await,
        Err(ProfileError::ProxyUnavailable)
    ));

    // OAuth cases must not be masked by the preceding missing-proxy fixture.
    set_setting(&pool, USER, "proxy_enabled", "false").await;
    // OAuth and unknown modes are rejected by the public database-backed boundary.
    set_setting(&pool, USER, "youtube_auth_mode", "oauth2").await;
    assert!(matches!(
        store
            .prepare_network(context(USER as u64, "youtube.com", &base, &none, None), 0)
            .await,
        Err(ProfileError::UnsupportedAuthentication)
    ));
    let youtube_bypass = vec!["youtube.com".to_owned()];
    assert!(matches!(
        store
            .prepare_network(
                context(USER as u64, "youtube.com", &base, &youtube_bypass, None),
                0
            )
            .await,
        Err(ProfileError::UnsupportedAuthentication)
    ));
    set_setting(&pool, USER, "youtube_auth_mode", "not-a-mode").await;
    assert!(matches!(
        store
            .prepare_network(context(USER as u64, "example.org", &base, &none, None), 0)
            .await,
        Err(ProfileError::InvalidPreferences)
    ));

    // Restore independent settings before testing cookie lifecycle failures.
    set_setting(&pool, USER, "youtube_auth_mode", "cookies").await;
    set_setting(&pool, USER, "proxy_enabled", "false").await;

    // Current cookie content invalidation and deletion are stale, never replacement credentials.
    query(include_str!("sql/cookie_update.sql"))
        .bind(personal)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store
            .resolve(
                context(USER as u64, "youtube.com", &base, &none, None),
                &prepared
            )
            .await,
        Err(ProfileError::StaleReference)
    ));
    query(include_str!("sql/cookie_restore.sql"))
        .bind("# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tFALSE\t\tsid\trestored-fake-value\n")
        .bind(personal)
        .execute(&pool)
        .await
        .unwrap();
    let restored = store
        .prepare_network(context(USER as u64, "youtube.com", &base, &none, None), 0)
        .await
        .unwrap();
    query(include_str!("sql/cookie_invalidate.sql"))
        .bind(personal)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store
            .resolve(
                context(USER as u64, "youtube.com", &base, &none, None),
                &restored
            )
            .await,
        Err(ProfileError::StaleReference)
    ));
    query(include_str!("sql/cookie_delete.sql"))
        .bind(personal)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store
            .resolve(
                context(USER as u64, "youtube.com", &base, &none, None),
                &prepared
            )
            .await,
        Err(ProfileError::StaleReference)
    ));

    // Capacity is explicit, and oversized JSON is reported without returning the large value.
    query(include_str!("sql/grant_shared.sql"))
        .bind(USER)
        .execute(&pool)
        .await
        .unwrap();
    let capacity_identity = identity(&pool, USER).await;
    query(include_str!("sql/cookie_overflow.sql"))
        .bind(POOL_OWNER)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store
            .prepare_network(
                context(
                    USER as u64,
                    "capacity.test",
                    &capacity_identity,
                    &none,
                    None
                ),
                0
            )
            .await,
        Err(ProfileError::CandidateCapacityExceeded)
    ));
    query(include_str!("sql/args_oversize.sql"))
        .bind(USER)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store.preferences(USER as u64).await,
        Err(ProfileError::InvalidPreferences)
    ));

    // check_schema uses metadata-free zero-row reads. All fixtures are removed by bounded IDs.
    store.check_schema().await.unwrap();
    raw_sql(include_str!("sql/cleanup.sql"))
        .execute(&pool)
        .await
        .unwrap();
    storage.close().await.unwrap();
}
