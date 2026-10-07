use std::{path::Path, time::Duration};
use yoink_access::Role;
use yoink_identity::{
    IdentityStore,
    admin::{
        AdminError,
        groups::{GroupCreate, GroupPatch, MemberPatch, ThreadPatch},
    },
};
use yoink_storage::Store;
#[cfg(unix)]
fn secure(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
}
#[cfg(not(unix))]
fn secure(_: &Path) {}
async fn setup() -> (tempfile::TempDir, yoink_db::Database, IdentityStore) {
    let d = tempfile::tempdir().unwrap();
    secure(d.path());
    let s = Store::open(&d.path().join("groups.db"), true, Duration::from_secs(2))
        .await
        .unwrap();
    s.migrate().await.unwrap();
    yoink_db::raw_sql(include_str!("fixtures.sql"))
        .execute(s.database())
        .await
        .unwrap();
    (
        d,
        s.database().clone(),
        IdentityStore::new(s.database().clone(), Some(8000000000011), false),
    )
}
#[tokio::test]
async fn group_topic_and_member_policy_admin_contracts() {
    let (_d, _database, db) = setup().await;
    let gid = -8000000000021;
    let other = -8000000000022;
    let created = db
        .group_create(
            8000000000011,
            &GroupCreate {
                id: gid,
                title: Some("Test".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(created.id, gid);
    assert!(
        db.groups_list(8000000000011)
            .await
            .unwrap()
            .iter()
            .any(|g| g.id == gid)
    );
    assert_eq!(
        db.group_create(
            8000000000011,
            &GroupCreate {
                id: gid,
                ..Default::default()
            }
        )
        .await
        .unwrap_err(),
        AdminError::Conflict
    );
    db.group_create(
        8000000000011,
        &GroupCreate {
            id: other,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let blank = db
        .thread_upsert(
            8000000000011,
            gid,
            &ThreadPatch {
                thread_id: None,
                name: None,
                enabled: false,
            },
        )
        .await
        .unwrap();
    let blank2 = db
        .thread_upsert(
            8000000000011,
            gid,
            &ThreadPatch {
                thread_id: None,
                name: Some("Main".into()),
                enabled: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(blank.id, blank2.id);
    assert_eq!(blank2.name.as_deref(), Some("Main"));
    db.thread_upsert(
        8000000000011,
        gid,
        &ThreadPatch {
            thread_id: Some(25),
            name: None,
            enabled: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        db.group_get(8000000000011, gid)
            .await
            .unwrap()
            .thread_policies
            .len(),
        2
    );
    assert!(
        db.threads_list(8000000000011, other)
            .await
            .unwrap()
            .is_empty()
    );
    let m = db
        .member_upsert(
            8000000000011,
            gid,
            8000000000012,
            &MemberPatch {
                role_override: Some(Role::Moderator),
                allow_pm_override: Some(false),
            },
        )
        .await
        .unwrap();
    assert_eq!(m.role_override, Some(Role::Moderator));
    assert_eq!(db.members_list(8000000000011, gid).await.unwrap().len(), 1);
    assert!(
        db.members_list(8000000000011, other)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        db.member_upsert(8000000000011, gid, 8000000000013, &MemberPatch::default())
            .await
            .unwrap()
            .allow_pm_override,
        None
    );
    assert!(
        db.group_update(
            8000000000011,
            gid,
            &GroupPatch {
                enabled: Some(true),
                ..Default::default()
            }
        )
        .await
        .unwrap()
        .enabled
    );
    db.thread_delete(8000000000011, gid, blank.id)
        .await
        .unwrap();
    db.member_delete(8000000000011, gid, 8000000000012)
        .await
        .unwrap();
    db.group_delete(8000000000011, gid).await.unwrap();
    assert_eq!(
        db.group_get(8000000000011, gid).await.unwrap_err(),
        AdminError::NotFound
    );
    assert_eq!(
        db.group_get(8000000000011, 0).await.unwrap_err(),
        AdminError::InvalidInput
    );
}
#[tokio::test]
async fn admin_authorization_is_fresh_and_global_bans_are_not_overridden() {
    let (_d, database, db) = setup().await;
    let gid = -8000000000031;
    assert_eq!(
        db.groups_list(8000000000012).await.unwrap_err(),
        AdminError::Forbidden
    );
    let policy = db
        .member_upsert(
            8000000000011,
            -8000000000011,
            8000000000013,
            &MemberPatch {
                role_override: Some(Role::Admin),
                allow_pm_override: Some(true),
            },
        )
        .await
        .unwrap();
    assert_eq!(policy.role_override, Some(Role::Admin));
    let snapshot = db
        .snapshot(8000000000013, Some((-8000000000011, None)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.principal.role, Role::Banned);
    assert!(snapshot.principal.is_blocked(snapshot.now));
    yoink_db::query(include_str!("sql/group_admin_ban_actor.sql"))
        .bind(8000000000011_i64)
        .execute(&database)
        .await
        .unwrap();
    assert_eq!(
        db.groups_list(8000000000011).await.unwrap_err(),
        AdminError::Forbidden
    );
    assert_eq!(
        db.group_create(
            8000000000011,
            &GroupCreate {
                id: gid,
                ..Default::default()
            }
        )
        .await
        .unwrap_err(),
        AdminError::Forbidden
    );
}

#[tokio::test]
async fn user_session_scan_is_atomic_preserves_policy_and_rechecks_owner() {
    let (_d, database, db) = setup().await;
    let owner = 8000000000011;
    let gid = -8000000000099;
    db.group_create(
        owner,
        &GroupCreate {
            id: gid,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.thread_upsert(
        owner,
        gid,
        &ThreadPatch {
            thread_id: Some(7),
            name: Some("manual".into()),
            enabled: false,
        },
    )
    .await
    .unwrap();
    db.thread_upsert(
        owner,
        gid,
        &ThreadPatch {
            thread_id: None,
            name: Some("default".into()),
            enabled: false,
        },
    )
    .await
    .unwrap();
    db.scan_thread_names(owner, gid, &[(7, "new".into()), (8, "topic".into())])
        .await
        .unwrap();
    let rows = db.threads_list(owner, gid).await.unwrap();
    assert!(
        rows.iter()
            .any(|r| r.thread_id == Some(7) && !r.enabled && r.name.as_deref() == Some("new"))
    );
    assert!(
        rows.iter()
            .any(|r| r.thread_id.is_none() && !r.enabled && r.name.as_deref() == Some("default"))
    );
    assert!(rows.iter().any(|r| r.thread_id == Some(8) && r.enabled));
    assert_eq!(
        db.scan_thread_names(owner, gid, &[(9, "valid".into()), (0, "invalid".into())])
            .await
            .unwrap_err(),
        AdminError::InvalidInput
    );
    assert_eq!(db.threads_list(owner, gid).await.unwrap(), rows);
    yoink_db::raw_sql(include_str!("sql/user_session_ban_owner.sql"))
        .execute(&database)
        .await
        .unwrap();
    assert_eq!(
        db.scan_thread_names(owner, gid, &[(9, "blocked".into())])
            .await
            .unwrap_err(),
        AdminError::Forbidden
    );
}
