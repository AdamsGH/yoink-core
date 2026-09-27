use sqlx::{Row, postgres::PgPoolOptions};
use yoink_access::{AccessPolicy, AccessScope, FeatureSpec, GrantSource, Role, effective_grant};
use yoink_identity::{IdentityError, IdentityStore, registered_features};

const USER: i64 = 8_000_000_000_011;
const RESTRICTED: i64 = 8_000_000_000_012;
const BANNED: i64 = 8_000_000_000_013;
const TIMED_BAN: i64 = 8_000_000_000_014;
const EXPIRED_AT_NOW: i64 = 8_000_000_000_015;
const EXPIRED_BEFORE_NOW: i64 = 8_000_000_000_016;
const EXPIRED_BAN: i64 = 8_000_000_000_017;
const GROUP: i64 = -8_000_000_000_011;
const DISABLED_GROUP: i64 = -8_000_000_000_012;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL migrated through Alembic"]
async fn postgres_identity_integration_serial() {
    let url = std::env::var("YOINK_TEST_DATABASE_URL").expect(
        "YOINK_TEST_DATABASE_URL must point to the migrated disposable PostgreSQL database",
    );
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connect to YOINK_TEST_DATABASE_URL");

    // Remove stale data from an interrupted prior run before capturing global fixture state.
    sqlx::raw_sql(include_str!("cleanup.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let original_setting = sqlx::query(include_str!("sql/global_setting_get.sql"))
        .fetch_optional(&pool)
        .await
        .unwrap()
        .map(|row| row.get::<Option<String>, _>("value"));

    sqlx::raw_sql(include_str!("fixtures.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(include_str!("sql/global_setting_set.sql"))
        .bind("true")
        .execute(&pool)
        .await
        .unwrap();

    let store = IdentityStore::new(pool.clone(), Some(USER), true);
    assert!(store.snapshot(USER + 100, None).await.unwrap().is_none());
    assert!(matches!(
        store.snapshot(0, None).await,
        Err(IdentityError::InvalidId)
    ));
    assert!(matches!(
        store.snapshot(-1, None).await,
        Err(IdentityError::InvalidId)
    ));
    assert!(matches!(
        IdentityStore::new(pool.clone(), Some(0), false)
            .snapshot(USER, None)
            .await,
        Err(IdentityError::InvalidId)
    ));
    assert!(matches!(
        store.snapshot(USER, Some((GROUP.abs(), None))).await,
        Err(IdentityError::InvalidId)
    ));
    assert!(matches!(
        store.snapshot(USER, Some((GROUP, Some(0)))).await,
        Err(IdentityError::InvalidId)
    ));

    let owner = store.snapshot(USER, None).await.unwrap().unwrap();
    assert_eq!(owner.principal.role, Role::Owner);
    let banned_owner = IdentityStore::new(pool.clone(), Some(BANNED), false)
        .snapshot(BANNED, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(banned_owner.principal.role, Role::Banned);
    assert!(banned_owner.principal.is_blocked(banned_owner.now));
    let banned_grant = FeatureSpec {
        plugin: "dl".into(),
        feature: "inline".into(),
        default_min_role: Some(Role::User),
    };
    assert_eq!(
        effective_grant(
            &banned_owner.principal,
            &banned_grant,
            &banned_owner.grants,
            &[],
            banned_owner.now
        ),
        None
    );

    let timed_ban = IdentityStore::new(pool.clone(), None, false)
        .snapshot(TIMED_BAN, None)
        .await
        .unwrap()
        .unwrap();
    assert!(timed_ban.principal.ban_until.unwrap() > timed_ban.now);
    let expired_at_now = IdentityStore::new(pool.clone(), None, false)
        .snapshot(EXPIRED_AT_NOW, None)
        .await
        .unwrap()
        .unwrap();
    assert!(expired_at_now.principal.ban_until.is_none());
    let expired_ban = IdentityStore::new(pool.clone(), None, false)
        .snapshot(EXPIRED_BAN, None)
        .await
        .unwrap()
        .unwrap();
    assert!(expired_ban.principal.ban_until.unwrap() < expired_ban.now);
    assert!(!expired_ban.principal.is_blocked(expired_ban.now));

    // Owner, explicit, role, and provider are the resolver's four grant sources.
    let inline = FeatureSpec {
        plugin: "dl".into(),
        feature: "inline".into(),
        default_min_role: Some(Role::User),
    };
    assert_eq!(
        effective_grant(
            &owner.principal,
            &inline,
            &owner.grants,
            &owner.providers,
            owner.now
        ),
        Some(GrantSource::Owner)
    );
    assert_eq!(
        effective_grant(
            &IdentityStore::new(pool.clone(), None, false)
                .snapshot(USER, None)
                .await
                .unwrap()
                .unwrap()
                .principal,
            &inline,
            &owner.grants,
            &[],
            owner.now
        ),
        Some(GrantSource::Explicit)
    );
    let role_only = IdentityStore::new(pool.clone(), None, false)
        .snapshot(RESTRICTED, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        effective_grant(&role_only.principal, &inline, &[], &[], role_only.now),
        None
    );
    let user_role = IdentityStore::new(pool.clone(), None, false)
        .snapshot(EXPIRED_AT_NOW, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        effective_grant(&user_role.principal, &inline, &[], &[], user_role.now),
        Some(GrantSource::Role)
    );
    assert!(
        owner
            .providers
            .iter()
            .any(|g| g.plugin == "insight" && g.feature == "tldr")
    );
    let group_auth = IdentityStore::new(pool.clone(), None, false)
        .snapshot(RESTRICTED, Some((GROUP, None)))
        .await
        .unwrap()
        .unwrap()
        .authorize(
            &AccessPolicy {
                min_role: Role::User,
                scopes: vec![AccessScope::Group],
                check_group_enabled: true,
                check_thread_policy: true,
            },
            Some(("stats", "stats")),
        );
    assert!(group_auth.decision.allowed);
    assert_eq!(group_auth.decision.effective_role, Role::Admin);
    assert_eq!(group_auth.grant_source, Some(GrantSource::Role));
    let tldr = FeatureSpec {
        plugin: "insight".into(),
        feature: "tldr".into(),
        default_min_role: None,
    };
    let byok_user = IdentityStore::new(pool.clone(), None, true)
        .snapshot(USER, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        effective_grant(
            &byok_user.principal,
            &tldr,
            &[],
            &byok_user.providers,
            byok_user.now
        ),
        Some(GrantSource::Provider)
    );
    assert_eq!(
        effective_grant(
            &byok_user.principal,
            &inline,
            &byok_user.grants,
            &byok_user.providers,
            byok_user.now
        ),
        Some(GrantSource::Explicit)
    );
    assert_eq!(
        effective_grant(
            &user_role.principal,
            &tldr,
            &owner.grants,
            &user_role.providers,
            user_role.now
        ),
        None
    );
    assert_eq!(
        effective_grant(&owner.principal, &inline, &owner.grants, &[], owner.now),
        Some(GrantSource::Owner)
    );
    assert!(owner.grants.iter().any(|g| g.expires_at.is_none()));
    assert!(
        role_only
            .grants
            .iter()
            .any(|g| g.expires_at.is_some_and(|expiry| expiry > role_only.now))
    );
    let boundary = IdentityStore::new(pool.clone(), None, false)
        .snapshot(EXPIRED_AT_NOW, None)
        .await
        .unwrap()
        .unwrap();
    assert!(
        boundary
            .grants
            .iter()
            .any(|g| g.expires_at.is_some_and(|expiry| expiry <= boundary.now))
    );
    let no_role_fallback = FeatureSpec {
        plugin: "dl".into(),
        feature: "inline".into(),
        default_min_role: None,
    };
    assert_eq!(
        effective_grant(
            &boundary.principal,
            &no_role_fallback,
            &boundary.grants,
            &[],
            boundary.now
        ),
        None
    );
    let already_expired = IdentityStore::new(pool.clone(), None, false)
        .snapshot(EXPIRED_BEFORE_NOW, None)
        .await
        .unwrap()
        .unwrap();
    assert!(already_expired.grants.iter().any(|g| {
        g.expires_at
            .is_some_and(|expiry| expiry < already_expired.now)
    }));
    assert_eq!(
        effective_grant(
            &already_expired.principal,
            &no_role_fallback,
            &already_expired.grants,
            &[],
            already_expired.now
        ),
        None
    );

    let restricted = IdentityStore::new(pool.clone(), None, false)
        .snapshot(RESTRICTED, Some((GROUP, None)))
        .await
        .unwrap()
        .unwrap();
    let group = restricted.group.unwrap();
    assert!(group.enabled);
    assert_eq!(group.auto_grant_role, Role::Moderator);
    assert_eq!(group.role_override, Some(Role::Admin));
    assert!(group.thread_allowed);
    assert_eq!(restricted.principal.role, Role::Restricted);
    let denied_thread = IdentityStore::new(pool.clone(), None, false)
        .snapshot(RESTRICTED, Some((GROUP, Some(17))))
        .await
        .unwrap()
        .unwrap();
    assert!(!denied_thread.group.unwrap().thread_allowed);
    let allowed_thread = IdentityStore::new(pool.clone(), None, false)
        .snapshot(RESTRICTED, Some((GROUP, Some(18))))
        .await
        .unwrap()
        .unwrap();
    assert!(allowed_thread.group.unwrap().thread_allowed);
    let no_allow_policy = IdentityStore::new(pool.clone(), None, false)
        .snapshot(RESTRICTED, Some((GROUP, Some(99))))
        .await
        .unwrap()
        .unwrap();
    assert!(!no_allow_policy.group.unwrap().thread_allowed);
    let absent_group = IdentityStore::new(pool.clone(), None, false)
        .snapshot(RESTRICTED, Some((GROUP - 100, None)))
        .await;
    assert!(matches!(absent_group, Err(IdentityError::Database)));
    let disabled = IdentityStore::new(pool.clone(), None, false)
        .snapshot(RESTRICTED, Some((DISABLED_GROUP, None)))
        .await
        .unwrap()
        .unwrap();
    assert!(!disabled.group.unwrap().enabled);

    // The snapshot transaction must be read-only and must not persist group-local role elevation.
    let persisted_role: String = sqlx::query(include_str!("sql/user_role_get.sql"))
        .bind(RESTRICTED)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("role");
    assert_eq!(persisted_role, "restricted");

    // A least-privilege role can resolve BYOK readiness without reading credential columns or writing.
    let reader_role = format!("yoink_identity_reader_{}", std::process::id());
    let setup = include_str!("sql/read_role_setup.sql").replace("__ROLE__", &reader_role);
    sqlx::raw_sql(&setup).execute(&pool).await.unwrap();
    let byok_grant = include_str!("sql/read_role_byok_grant.sql").replace("__ROLE__", &reader_role);
    sqlx::raw_sql(&byok_grant).execute(&pool).await.unwrap();
    let set_role = include_str!("sql/set_role.sql").replace("__ROLE__", &reader_role);
    let reader_pool = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |conn, _| {
            let set_role = set_role.clone();
            Box::pin(async move {
                sqlx::query(&set_role).execute(conn).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .expect("connect least-privilege reader pool");
    let role_name: String = sqlx::query(include_str!("sql/role_check.sql"))
        .fetch_one(&reader_pool)
        .await
        .unwrap()
        .get("current_user");
    assert_eq!(role_name, reader_role);
    let column_access = sqlx::query(include_str!("sql/role_has_column_access.sql"))
        .fetch_one(&reader_pool)
        .await
        .unwrap();
    assert!(!column_access.get::<bool, _>("api_key"));
    assert!(!column_access.get::<bool, _>("base_url"));
    assert!(!column_access.get::<bool, _>("model"));
    let reader_snapshot = IdentityStore::new(reader_pool.clone(), None, true)
        .snapshot(USER, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        effective_grant(
            &reader_snapshot.principal,
            &tldr,
            &[],
            &reader_snapshot.providers,
            reader_snapshot.now
        ),
        Some(GrantSource::Provider)
    );
    assert!(
        sqlx::query(include_str!("sql/role_mutation_check.sql"))
            .bind(USER)
            .execute(&reader_pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query(include_str!("sql/role_secret_check.sql"))
            .bind(USER)
            .fetch_one(&reader_pool)
            .await
            .is_err()
    );
    reader_pool.close().await;
    let cleanup_role = include_str!("sql/read_role_cleanup.sql").replace("__ROLE__", &reader_role);
    sqlx::raw_sql(&cleanup_role).execute(&pool).await.unwrap();

    // Disabled Insight must skip BYOK readiness lookup even when a valid BYOK row and global flag exist.
    let no_byok_role = format!("yoink_identity_no_byok_{}", std::process::id());
    let setup = include_str!("sql/read_role_setup_no_byok.sql").replace("__ROLE__", &no_byok_role);
    sqlx::raw_sql(&setup).execute(&pool).await.unwrap();
    let set_role = include_str!("sql/set_role.sql").replace("__ROLE__", &no_byok_role);
    let no_byok_pool = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |conn, _| {
            let set_role = set_role.clone();
            Box::pin(async move {
                sqlx::query(&set_role).execute(conn).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .expect("connect reader without BYOK table privileges");
    let has_byok_privilege: bool = sqlx::query(include_str!("sql/role_has_byok_privilege.sql"))
        .fetch_one(&no_byok_pool)
        .await
        .unwrap()
        .get("allowed");
    assert!(!has_byok_privilege);
    let disabled_insight = IdentityStore::new(no_byok_pool.clone(), None, false)
        .snapshot(USER, None)
        .await
        .unwrap()
        .unwrap();
    assert!(disabled_insight.providers.is_empty());
    no_byok_pool.close().await;
    let cleanup_role = include_str!("sql/read_role_cleanup.sql").replace("__ROLE__", &no_byok_role);
    sqlx::raw_sql(&cleanup_role).execute(&pool).await.unwrap();

    // BYOK global toggle, untested state, probe failure, success, and disabled implementation.
    sqlx::query(include_str!("sql/global_setting_set.sql"))
        .bind("true")
        .execute(&pool)
        .await
        .unwrap();
    let byok_store = IdentityStore::new(pool.clone(), None, true);
    assert!(
        byok_store
            .snapshot(USER, None)
            .await
            .unwrap()
            .unwrap()
            .providers
            .iter()
            .any(|p| p.feature == "tldr")
    );
    sqlx::query(include_str!("sql/byok_set_status.sql"))
        .bind(false)
        .bind(Option::<&str>::None)
        .bind(USER)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        byok_store
            .snapshot(USER, None)
            .await
            .unwrap()
            .unwrap()
            .providers
            .is_empty()
    );
    sqlx::query(include_str!("sql/byok_set_status.sql"))
        .bind(true)
        .bind(Some("invalid key"))
        .bind(USER)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        byok_store
            .snapshot(USER, None)
            .await
            .unwrap()
            .unwrap()
            .providers
            .is_empty()
    );
    sqlx::query(include_str!("sql/byok_set_status.sql"))
        .bind(true)
        .bind(Option::<&str>::None)
        .bind(USER)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(include_str!("sql/global_setting_set.sql"))
        .bind("false")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        byok_store
            .snapshot(USER, None)
            .await
            .unwrap()
            .unwrap()
            .providers
            .is_empty()
    );
    assert!(
        IdentityStore::new(pool.clone(), None, false)
            .snapshot(USER, None)
            .await
            .unwrap()
            .unwrap()
            .providers
            .is_empty()
    );

    // Inbox catalog is exact, including minimums and absence of a fabricated download feature.
    let catalog = registered_features();
    let inbox: Vec<_> = catalog
        .iter()
        .filter(|feature| feature.plugin == "inbox")
        .collect();
    assert_eq!(inbox.len(), 7);
    let inbox_minimums = [
        ("ingest", Some(Role::User)),
        ("classify", None),
        ("share", Some(Role::User)),
        ("gh_sync", None),
        ("gh_organise", None),
        ("gh_write", None),
        ("admin", Some(Role::Admin)),
    ];
    for (feature, minimum) in inbox_minimums {
        assert_eq!(
            inbox
                .iter()
                .find(|spec| spec.feature == feature)
                .unwrap()
                .default_min_role,
            minimum
        );
    }
    assert!(
        !catalog
            .iter()
            .any(|feature| feature.plugin == "dl" && feature.feature == "download")
    );

    // Restore the globally-scoped setting exactly as found, then remove only fixture rows.
    if let Some(value) = original_setting {
        sqlx::query(include_str!("sql/global_setting_set.sql"))
            .bind(value)
            .execute(&pool)
            .await
            .unwrap();
    } else {
        sqlx::raw_sql(include_str!("sql/global_setting_delete.sql"))
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::raw_sql(include_str!("cleanup.sql"))
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
