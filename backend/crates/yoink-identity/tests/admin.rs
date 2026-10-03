use std::{collections::BTreeMap, path::Path, time::Duration};

use chrono::{DateTime, Utc};
use yoink_access::Role;
use yoink_db::{Database, query, raw_sql};
use yoink_identity::admin::{AdminError, AdminUserPatch, AdminUsersQuery};
use yoink_identity::{IdentityError, IdentityStore, TrustedProfile};
use yoink_storage::Store;

#[cfg(unix)]
fn secure(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
#[cfg(not(unix))]
fn secure(_: &Path) {}

async fn setup(
    name: &str,
    fixture: &str,
    owner_id: i64,
) -> (tempfile::TempDir, Database, IdentityStore) {
    let dir = tempfile::tempdir().unwrap();
    secure(dir.path());
    let store = Store::open(&dir.path().join(name), true, Duration::from_secs(2))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let db = store.database().clone();
    raw_sql(fixture).execute(&db).await.unwrap();
    let identity = IdentityStore::new(db.clone(), Some(owner_id), false);
    (dir, db, identity)
}

fn stamp(micros: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(micros).unwrap()
}

#[tokio::test]
async fn user_updates_enforce_admin_owner_ban_and_atomic_patch_rules() {
    let (_dir, db, store) =
        setup("admin-update.db", include_str!("sql/admin_users.sql"), 1001).await;
    let admin = AdminUserPatch {
        role: Some(Role::Banned),
        ..Default::default()
    };
    let banned = store.admin_update_user(1001, 1004, &admin).await.unwrap();
    assert_eq!(banned.role, Role::Banned);
    assert_eq!(banned.ban_until, None);

    let timed = AdminUserPatch {
        ban_until: Some(Some(stamp(4_102_444_800_123_456))),
        ..Default::default()
    };
    let updated = store.admin_update_user(1001, 1005, &timed).await.unwrap();
    assert_eq!(updated.ban_until, Some(stamp(4_102_444_800_123_456)));
    assert_eq!(
        store.admin_get_user(1001, 1001).await.unwrap().role,
        Role::Owner
    );
    assert_eq!(
        store
            .admin_update_user(
                1001,
                1001,
                &AdminUserPatch {
                    role: Some(Role::User),
                    ..Default::default()
                }
            )
            .await
            .unwrap_err(),
        AdminError::Forbidden
    );
    assert_eq!(
        store
            .admin_update_user(
                1002,
                1003,
                &AdminUserPatch {
                    role: Some(Role::Admin),
                    ..Default::default()
                }
            )
            .await
            .unwrap_err(),
        AdminError::Forbidden
    );
    assert_eq!(
        store
            .admin_update_user(
                1002,
                1002,
                &AdminUserPatch {
                    role: Some(Role::Admin),
                    ..Default::default()
                }
            )
            .await
            .unwrap_err(),
        AdminError::Forbidden
    );
    assert_eq!(
        store
            .admin_update_user(
                1001,
                1004,
                &AdminUserPatch {
                    role: Some(Role::Owner),
                    ..Default::default()
                }
            )
            .await
            .unwrap_err(),
        AdminError::Forbidden
    );
    let promoted = store
        .admin_update_user(
            1001,
            1004,
            &AdminUserPatch {
                role: Some(Role::Admin),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(promoted.role, Role::Admin);
    let rev_before: i64 = query(include_str!("sql/admin_revision.sql"))
        .fetch_one(&db)
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    let language = store
        .admin_update_user(
            1001,
            1004,
            &AdminUserPatch {
                language: Some("ru".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(language.language, "ru");
    let rev_after: i64 = query(include_str!("sql/admin_revision.sql"))
        .fetch_one(&db)
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert!(
        rev_after > rev_before,
        "language change advances profile revision"
    );
    let before = query(include_str!("sql/admin_role_language.sql"))
        .fetch_one(&db)
        .await
        .unwrap();
    let before: (String, String) = (before.try_get(0).unwrap(), before.try_get(1).unwrap());
    assert_eq!(
        store
            .admin_update_user(
                1001,
                1004,
                &AdminUserPatch {
                    role: Some(Role::User),
                    language: Some("xx".into()),
                    ..Default::default()
                }
            )
            .await
            .unwrap_err(),
        AdminError::InvalidInput
    );
    let after = query(include_str!("sql/admin_role_language.sql"))
        .fetch_one(&db)
        .await
        .unwrap();
    let after: (String, String) = (after.try_get(0).unwrap(), after.try_get(1).unwrap());
    assert_eq!(
        after, before,
        "invalid combined patch does not partially write"
    );
    let preserved = store
        .admin_update_user(
            1001,
            1005,
            &AdminUserPatch {
                language: Some("ru".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        preserved.ban_until,
        Some(stamp(4_102_444_800_123_456)),
        "omitted ban field preserves timed ban"
    );
    let cleared = store
        .admin_update_user(
            1001,
            1005,
            &AdminUserPatch {
                ban_until: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(cleared.ban_until, None, "explicit null clears timed ban");
    let actor_banned = store
        .admin_update_user(
            1001,
            1002,
            &AdminUserPatch {
                role: Some(Role::Banned),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(actor_banned.role, Role::Banned);
    assert_eq!(
        store.admin_get_user(1002, 1002).await.unwrap_err(),
        AdminError::Forbidden
    );
    query(include_str!("sql/admin_ban_owner.sql"))
        .execute(&db)
        .await
        .unwrap();
    let configured_banned = IdentityStore::new(db, Some(1001), false)
        .snapshot(1001, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(configured_banned.principal.role, Role::User);
    assert!(
        configured_banned
            .principal
            .is_blocked(configured_banned.now),
        "configured owner remains blocked by a timed ban"
    );
}

#[tokio::test]
async fn grants_are_idempotent_expiring_and_respect_tag_ownership() {
    let (_dir, db, store) = setup(
        "admin-grants.db",
        include_str!("sql/admin_grants.sql"),
        2001,
    )
    .await;
    let expiry = stamp(4_102_444_800_654_321);
    let first = store
        .admin_grant_permission(2001, 2003, "dl", "inline", Some(expiry))
        .await
        .unwrap();
    let again = store
        .admin_grant_permission(2001, 2003, "dl", "inline", None)
        .await
        .unwrap();
    assert_eq!(first.id, again.id);
    assert_eq!(again.expires_at, None);
    assert_eq!(again.grant_source, "manual");
    assert_eq!(
        store
            .admin_permissions(2001, Some(2003))
            .await
            .unwrap()
            .len(),
        1
    );
    store
        .admin_revoke_permission(2001, 2003, "dl", "inline")
        .await
        .unwrap();
    store
        .admin_revoke_permission(2001, 2003, "dl", "inline")
        .await
        .unwrap();
    assert!(
        store
            .admin_permissions(2001, Some(2003))
            .await
            .unwrap()
            .is_empty()
    );
    let expired_grant = store
        .admin_grant_permission(
            2001,
            2003,
            "insight",
            "tldr",
            Some(stamp(1_700_000_000_000_000)),
        )
        .await
        .unwrap();
    assert_eq!(expired_grant.grant_source, "manual");
    let access = store.admin_feature_access(2001, Some(2003)).await.unwrap();
    let tldr = access
        .iter()
        .find(|x| x.plugin == "insight" && x.feature == "tldr")
        .unwrap();
    assert!(!tldr.access_via_grant);
    assert!(!tldr.effective);
    let admin_grant = store
        .admin_grant_permission(2002, 2003, "dl", "shared_cookies", None)
        .await
        .unwrap();
    assert_eq!(admin_grant.granted_by, 2002);
    store
        .admin_revoke_permission(2002, 2003, "dl", "shared_cookies")
        .await
        .unwrap();
    assert_eq!(
        store
            .admin_grant_permission(2001, 2003, "dl", "download", None)
            .await
            .unwrap_err(),
        AdminError::InvalidInput
    );
    let tag = store
        .admin_grant_permission(2001, 2003, "stats", "stats", None)
        .await
        .unwrap();
    query(include_str!("sql/admin_tag_grant.sql"))
        .bind(tag.id)
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        store.admin_permissions(2001, Some(2003)).await.unwrap()[0].grant_source,
        "tag"
    );
    assert_eq!(
        store
            .admin_grant_permission(2002, 2003, "stats", "stats", None)
            .await
            .unwrap_err(),
        AdminError::Forbidden
    );
    assert_eq!(
        store
            .admin_revoke_permission(2002, 2003, "stats", "stats")
            .await
            .unwrap_err(),
        AdminError::Forbidden
    );
    assert_eq!(
        store
            .admin_grant_permission(2001, 2003, "stats", "stats", None)
            .await
            .unwrap()
            .grant_source,
        "manual"
    );
    store
        .admin_revoke_permission(2001, 2003, "stats", "stats")
        .await
        .unwrap();
    let remaining = store.admin_permissions(2001, Some(2003)).await.unwrap();
    assert!(
        remaining.is_empty(),
        "expired grants are omitted from public lists"
    );
    assert!(
        store
            .admin_permissions(2001, None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn bot_policy_defaults_validation_and_patch_preserve_unknown_settings() {
    let (_dir, db, store) = setup(
        "admin-policy.db",
        include_str!("sql/admin_policy.sql"),
        3001,
    )
    .await;
    let defaults = store.admin_bot_settings(3001).await.unwrap();
    assert_eq!(
        defaults.get("bot_access_mode").map(String::as_str),
        Some("open")
    );
    assert_eq!(
        defaults.get("browser_cookies_min_role").map(String::as_str),
        Some("owner")
    );
    let mut patch = BTreeMap::new();
    patch.insert("bot_access_mode".into(), "approved_only".into());
    patch.insert("inline_storage_chat_id".into(), "-100123456".into());
    store.admin_update_bot_settings(3001, &patch).await.unwrap();
    let changed = store.admin_bot_settings(3001).await.unwrap();
    assert_eq!(
        changed.get("bot_access_mode").map(String::as_str),
        Some("approved_only")
    );
    assert_eq!(
        changed.get("inline_storage_chat_id").map(String::as_str),
        Some("-100123456")
    );
    assert_eq!(changed.get("unrelated").map(String::as_str), Some("keep"));
    let registration = store
        .register(&TrustedProfile {
            user_id: 3003,
            username: Some("new_user".into()),
            first_name: Some("New User".into()),
            photo_url: None,
        })
        .await
        .unwrap();
    assert_eq!(
        registration.role,
        Role::Restricted,
        "approved-only policy restricts new trusted registrations"
    );
    query(include_str!("sql/admin_corrupt_policy.sql"))
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        store.admin_bot_settings(3001).await.unwrap_err(),
        AdminError::Unavailable
    );
    assert_eq!(
        store
            .register(&TrustedProfile {
                user_id: 3004,
                username: None,
                first_name: Some("Rejected".into()),
                photo_url: None,
            })
            .await
            .unwrap_err(),
        IdentityError::InvalidPolicy
    );
    let features = store.admin_features(3001).await.unwrap();
    assert_eq!(features.len(), 14);
    assert!(
        features
            .iter()
            .any(|x| x.plugin == "dl" && x.feature == "inline")
    );
    let catalog: std::collections::BTreeSet<_> = features
        .iter()
        .map(|x| (x.plugin.as_str(), x.feature.as_str()))
        .collect();
    let expected: std::collections::BTreeSet<_> = [
        ("dl", "inline"),
        ("dl", "shared_cookies"),
        ("stats", "stats"),
        ("insight", "summary"),
        ("insight", "tldr"),
        ("insight", "search"),
        ("music", "inline"),
        ("inbox", "ingest"),
        ("inbox", "classify"),
        ("inbox", "share"),
        ("inbox", "gh_sync"),
        ("inbox", "gh_organise"),
        ("inbox", "gh_write"),
        ("inbox", "admin"),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        catalog, expected,
        "native 14-feature catalog excludes download aliases"
    );
}

#[tokio::test]
async fn tag_map_validates_authorizes_sorts_and_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    secure(dir.path());
    let path = dir.path().join("admin-tag-map.db");
    let db_store = Store::open(&path, true, Duration::from_secs(2))
        .await
        .unwrap();
    db_store.migrate().await.unwrap();
    raw_sql(include_str!("sql/admin_policy.sql"))
        .execute(db_store.database())
        .await
        .unwrap();
    let db = db_store.database().clone();
    raw_sql(include_str!("sql/admin_imported_tag_map.sql"))
        .execute(&db)
        .await
        .unwrap();
    let identity = IdentityStore::new(db.clone(), Some(3001), false);
    use yoink_identity::admin::AdminTagMapEntry;
    let expected = vec![
        AdminTagMapEntry {
            tag: "Alpha".into(),
            features: vec!["dl:inline".into()],
        },
        AdminTagMapEntry {
            tag: "Бета".into(),
            features: vec!["stats:stats".into()],
        },
        AdminTagMapEntry {
            tag: "Я".repeat(129),
            features: vec![],
        },
    ];
    assert_eq!(
        identity
            .admin_update_tag_map(3002, &expected)
            .await
            .unwrap_err(),
        AdminError::Forbidden
    );
    assert_eq!(
        identity
            .admin_update_tag_map(9999, &expected)
            .await
            .unwrap_err(),
        AdminError::Unauthorized
    );
    assert_eq!(identity.admin_tag_map(3002).await.unwrap(), expected);
    let written = identity
        .admin_update_tag_map(3001, &expected)
        .await
        .unwrap();
    assert_eq!(written, expected);
    assert_eq!(identity.admin_tag_map(3002).await.unwrap(), expected);
    for invalid in [
        vec![AdminTagMapEntry {
            tag: " ".into(),
            features: vec![],
        }],
        vec![
            AdminTagMapEntry {
                tag: "duplicate".into(),
                features: vec![],
            },
            AdminTagMapEntry {
                tag: "duplicate".into(),
                features: vec![],
            },
        ],
        vec![AdminTagMapEntry {
            tag: "unknown".into(),
            features: vec!["missing:feature".into()],
        }],
    ] {
        assert_eq!(
            identity
                .admin_update_tag_map(3001, &invalid)
                .await
                .unwrap_err(),
            AdminError::InvalidInput
        );
        assert_eq!(identity.admin_tag_map(3001).await.unwrap(), expected);
    }
    drop(identity);
    drop(db);
    db_store.close().await.unwrap();
    drop(db_store);
    let reopened = Store::open(&path, false, Duration::from_secs(2))
        .await
        .unwrap();
    let identity = IdentityStore::new(reopened.database().clone(), Some(3001), false);
    assert_eq!(identity.admin_tag_map(3001).await.unwrap(), expected);
    query(include_str!("sql/admin_corrupt_tag_map.sql"))
        .execute(reopened.database())
        .await
        .unwrap();
    assert_eq!(
        identity.admin_tag_map(3001).await.unwrap_err(),
        AdminError::Unavailable
    );
    let stored: String = query(include_str!("sql/admin_read_tag_map.sql"))
        .fetch_one(reopened.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(
        stored, "not-json",
        "a failed read never repairs corrupt persisted state"
    );
}

#[tokio::test]
async fn banned_configured_owner_cannot_read_or_replace_tag_map() {
    let (_dir, db, identity) = setup(
        "admin-banned-tag-owner.db",
        include_str!("sql/admin_policy.sql"),
        3001,
    )
    .await;
    query(include_str!("sql/admin_ban_tag_owner.sql"))
        .execute(&db)
        .await
        .unwrap();
    use yoink_identity::admin::AdminTagMapEntry;
    let entries = vec![AdminTagMapEntry {
        tag: "keep".into(),
        features: vec!["dl:inline".into()],
    }];
    assert_eq!(
        identity.admin_tag_map(3001).await.unwrap_err(),
        AdminError::Forbidden
    );
    assert_eq!(
        identity
            .admin_update_tag_map(3001, &entries)
            .await
            .unwrap_err(),
        AdminError::Forbidden
    );
    let stored: i64 = query(include_str!("sql/admin_tag_map_count.sql"))
        .fetch_one(&db)
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(stored, 0);
}

#[tokio::test]
async fn user_listing_filters_searches_paginates_and_uses_download_fixtures() {
    let (_dir, _db, store) =
        setup("admin-list.db", include_str!("sql/admin_listing.sql"), 4001).await;
    let query = AdminUsersQuery {
        sort: "dl_count".into(),
        direction: "desc".into(),
        limit: 2,
        ..Default::default()
    };
    let first = store.admin_list_users(4001, &query).await.unwrap();
    assert_eq!(first.total, 6);
    assert_eq!(
        first.items.iter().map(|x| x.id).collect::<Vec<_>>(),
        vec![4002, 4003]
    );
    assert!(first.has_more);
    let second = store
        .admin_list_users(
            4001,
            &AdminUsersQuery {
                offset: 2,
                ..query.clone()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        second.items.iter().map(|x| x.id).collect::<Vec<_>>(),
        vec![4001, 4006]
    );
    assert_eq!(first.items[0].dl_count, 2);
    let searched = store
        .admin_list_users(
            4001,
            &AdminUsersQuery {
                search: Some("@ALPHA".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        searched.items.iter().map(|x| x.id).collect::<Vec<_>>(),
        vec![4002]
    );
    let banned = store
        .admin_list_users(
            4001,
            &AdminUsersQuery {
                status: Some("banned".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        banned.items.iter().map(|x| x.id).collect::<Vec<_>>(),
        vec![4006, 4005]
    );
    let active = store
        .admin_list_users(
            4001,
            &AdminUsersQuery {
                status: Some("active".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        !active
            .items
            .iter()
            .any(|u| u.id == 4004 || u.id == 4005 || u.id == 4006)
    );
    let role = store
        .admin_list_users(
            4001,
            &AdminUsersQuery {
                role: Some("restricted".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        role.items.iter().map(|x| x.id).collect::<Vec<_>>(),
        vec![4004]
    );
    let recent = store
        .admin_list_users(
            4001,
            &AdminUsersQuery {
                period: Some("7".into()),
                sort: "dl_count".into(),
                direction: "desc".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        recent.items.iter().find(|u| u.id == 4002).unwrap().dl_count,
        1
    );
    assert_eq!(
        recent.items.iter().find(|u| u.id == 4003).unwrap().dl_count,
        0
    );
    assert_eq!(
        recent.items.iter().find(|u| u.id == 4001).unwrap().dl_count,
        1
    );
}
