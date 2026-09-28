use std::{path::Path, time::Duration};

use yoink_access::{AccessPolicy, AccessScope, FeatureSpec, GrantSource, Role, effective_grant};
use yoink_db::{query, raw_sql};
use yoink_identity::{IdentityError, IdentityStore, registered_features};
use yoink_storage::Store;

const USER: i64 = 8_000_000_000_011;
const RESTRICTED: i64 = 8_000_000_000_012;
const BANNED: i64 = 8_000_000_000_013;
const TIMED_BAN: i64 = 8_000_000_000_014;
const EXPIRED_AT_NOW: i64 = 8_000_000_000_015;
const EXPIRED_BEFORE_NOW: i64 = 8_000_000_000_016;
const EXPIRED_BAN: i64 = 8_000_000_000_017;
const GROUP: i64 = -8_000_000_000_011;
const DISABLED_GROUP: i64 = -8_000_000_000_012;

#[cfg(unix)]
fn secure_directory(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) {}

#[tokio::test]
async fn native_identity_integration_serial() {
    let directory = tempfile::tempdir().unwrap();
    secure_directory(directory.path());
    let database_path = directory.path().join("identity.db");
    let migration_store = Store::open(&database_path, true, Duration::from_secs(2))
        .await
        .unwrap();
    migration_store.migrate().await.unwrap();
    let database = migration_store.database().clone();

    raw_sql(include_str!("fixtures.sql"))
        .execute(&database)
        .await
        .unwrap();
    query(include_str!("sql/global_setting_set.sql"))
        .bind("true")
        .execute(&database)
        .await
        .unwrap();

    let store = IdentityStore::new(database.clone(), Some(USER), true);
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
        IdentityStore::new(database.clone(), Some(0), false)
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
    let banned_owner = IdentityStore::new(database.clone(), Some(BANNED), false)
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

    let timed_ban = IdentityStore::new(database.clone(), None, false)
        .snapshot(TIMED_BAN, None)
        .await
        .unwrap()
        .unwrap();
    assert!(timed_ban.principal.ban_until.unwrap() > timed_ban.now);
    let expired_at_now = IdentityStore::new(database.clone(), None, false)
        .snapshot(EXPIRED_AT_NOW, None)
        .await
        .unwrap()
        .unwrap();
    assert!(expired_at_now.principal.ban_until.is_none());
    let expired_ban = IdentityStore::new(database.clone(), None, false)
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
            &IdentityStore::new(database.clone(), None, false)
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
    let role_only = IdentityStore::new(database.clone(), None, false)
        .snapshot(RESTRICTED, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        effective_grant(&role_only.principal, &inline, &[], &[], role_only.now),
        None
    );
    let user_role = IdentityStore::new(database.clone(), None, false)
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
    let group_auth = IdentityStore::new(database.clone(), None, false)
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
    let byok_user = IdentityStore::new(database.clone(), None, true)
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
    let boundary = IdentityStore::new(database.clone(), None, false)
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
    let already_expired = IdentityStore::new(database.clone(), None, false)
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

    let restricted = IdentityStore::new(database.clone(), None, false)
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
    let denied_thread = IdentityStore::new(database.clone(), None, false)
        .snapshot(RESTRICTED, Some((GROUP, Some(17))))
        .await
        .unwrap()
        .unwrap();
    assert!(!denied_thread.group.unwrap().thread_allowed);
    let allowed_thread = IdentityStore::new(database.clone(), None, false)
        .snapshot(RESTRICTED, Some((GROUP, Some(18))))
        .await
        .unwrap()
        .unwrap();
    assert!(allowed_thread.group.unwrap().thread_allowed);
    let no_allow_policy = IdentityStore::new(database.clone(), None, false)
        .snapshot(RESTRICTED, Some((GROUP, Some(99))))
        .await
        .unwrap()
        .unwrap();
    assert!(!no_allow_policy.group.unwrap().thread_allowed);
    let absent_group = IdentityStore::new(database.clone(), None, false)
        .snapshot(RESTRICTED, Some((GROUP - 100, None)))
        .await;
    assert!(matches!(absent_group, Err(IdentityError::Database)));
    let disabled = IdentityStore::new(database.clone(), None, false)
        .snapshot(RESTRICTED, Some((DISABLED_GROUP, None)))
        .await
        .unwrap()
        .unwrap();
    assert!(!disabled.group.unwrap().enabled);

    // The snapshot transaction must be read-only and must not persist group-local role elevation.
    let persisted_role: String = query(include_str!("sql/user_role_get.sql"))
        .bind(RESTRICTED)
        .fetch_one(&database)
        .await
        .unwrap()
        .try_get("role")
        .unwrap();
    assert_eq!(persisted_role, "restricted");

    // The native backend has no column ACL. Verify a read-only connection still
    // resolves BYOK readiness and does not expose credential values in diagnostics.
    query("PRAGMA query_only = 1")
        .execute(&database)
        .await
        .unwrap();
    let readonly_snapshot = IdentityStore::new(database.clone(), None, true)
        .snapshot(USER, None)
        .await
        .unwrap()
        .unwrap();
    assert!(
        readonly_snapshot
            .providers
            .iter()
            .any(|provider| provider.feature == "tldr")
    );
    assert_eq!(
        effective_grant(
            &readonly_snapshot.principal,
            &tldr,
            &[],
            &readonly_snapshot.providers,
            readonly_snapshot.now,
        ),
        Some(GrantSource::Provider)
    );
    let snapshot_debug = format!("{readonly_snapshot:?}");
    for secret in ["integration-fake-key", "fake-model", "https://fake.example"] {
        assert!(!snapshot_debug.contains(secret));
    }
    assert!(
        query(include_str!("sql/byok_set_status.sql"))
            .bind(false)
            .bind(Option::<&str>::None)
            .bind(USER)
            .execute(&database)
            .await
            .is_err()
    );
    query("PRAGMA query_only = 0")
        .execute(&database)
        .await
        .unwrap();

    // Renaming the BYOK table demonstrates disabled Insight avoids that lookup.
    raw_sql("ALTER TABLE insight_user_byok RENAME TO insight_user_byok_hidden")
        .execute(&database)
        .await
        .unwrap();
    let disabled_insight = IdentityStore::new(database.clone(), None, false)
        .snapshot(USER, None)
        .await
        .unwrap()
        .unwrap();
    assert!(disabled_insight.providers.is_empty());
    assert!(matches!(
        IdentityStore::new(database.clone(), None, true)
            .snapshot(USER, None)
            .await,
        Err(IdentityError::Database)
    ));
    raw_sql("ALTER TABLE insight_user_byok_hidden RENAME TO insight_user_byok")
        .execute(&database)
        .await
        .unwrap();

    // BYOK global toggle, untested state, probe failure, success, and disabled implementation.
    query(include_str!("sql/global_setting_set.sql"))
        .bind("true")
        .execute(&database)
        .await
        .unwrap();
    let byok_store = IdentityStore::new(database.clone(), None, true);
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
    query(include_str!("sql/byok_set_status.sql"))
        .bind(false)
        .bind(Option::<&str>::None)
        .bind(USER)
        .execute(&database)
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
    query(include_str!("sql/byok_set_status.sql"))
        .bind(true)
        .bind(Some("invalid key"))
        .bind(USER)
        .execute(&database)
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
    query(include_str!("sql/byok_set_status.sql"))
        .bind(true)
        .bind(Option::<&str>::None)
        .bind(USER)
        .execute(&database)
        .await
        .unwrap();
    query(include_str!("sql/global_setting_set.sql"))
        .bind("false")
        .execute(&database)
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
        IdentityStore::new(database.clone(), None, false)
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

    migration_store.close().await.unwrap();
}
