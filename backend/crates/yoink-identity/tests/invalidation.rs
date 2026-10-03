use std::{collections::BTreeMap, time::Duration};
use yoink_access::Role;
use yoink_identity::{
    IdentityStore, TrustedProfile,
    admin::{
        AdminTagMapEntry, AdminUserPatch,
        groups::{GroupCreate, GroupPatch, MemberPatch, ThreadPatch},
    },
};
use yoink_storage::Store;

async fn setup() -> (tempfile::TempDir, IdentityStore) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let database = Store::open(&dir.path().join("changes.db"), true, Duration::from_secs(2))
        .await
        .unwrap();
    database.migrate().await.unwrap();
    let store = IdentityStore::new(database.database().clone(), Some(42), false);
    register(&store, 42).await;
    register(&store, 43).await;
    (dir, store)
}
async fn register(store: &IdentityStore, user_id: i64) {
    store
        .register(&TrustedProfile {
            user_id,
            username: None,
            first_name: None,
            photo_url: None,
        })
        .await
        .unwrap();
}
fn changed(receiver: &mut tokio::sync::watch::Receiver<()>) {
    assert!(
        receiver.has_changed().unwrap(),
        "successful committed mutation must invalidate"
    );
    receiver.borrow_and_update();
}

#[tokio::test]
async fn identity_language_role_grant_and_config_changes_signal_only_after_commit() {
    let (_dir, store) = setup().await;
    let mut receiver = store.subscribe_changes();
    register(&store, 44).await;
    changed(&mut receiver);
    store.update_settings(43, Some("ru"), None).await.unwrap();
    changed(&mut receiver);
    // A subscriber can immediately acquire/read the database, and sees committed
    // state rather than a request's obsolete role/locale snapshot.
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), store.snapshot(43, None))
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .language,
        "ru"
    );
    store.settings(43).await.unwrap();
    store.current_profile(43).await.unwrap();
    assert!(
        !receiver.has_changed().unwrap(),
        "reads must not generate work"
    );
    assert!(
        store
            .update_settings(43, Some("invalid"), None)
            .await
            .is_err()
    );
    assert!(!receiver.has_changed().unwrap());
    store
        .admin_update_user(
            42,
            43,
            &AdminUserPatch {
                role: Some(Role::Admin),
                language: Some("en".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    changed(&mut receiver);
    store
        .admin_grant_permission(42, 43, "dl", "inline", None)
        .await
        .unwrap();
    changed(&mut receiver);
    store
        .admin_revoke_permission(42, 43, "dl", "inline")
        .await
        .unwrap();
    changed(&mut receiver);
    store
        .admin_update_tag_map(
            42,
            &[AdminTagMapEntry {
                tag: "native-test".into(),
                features: vec!["dl:inline".into()],
            }],
        )
        .await
        .unwrap();
    changed(&mut receiver);
    store
        .admin_update_bot_settings(
            42,
            &BTreeMap::from([("bot_access_mode".into(), "approved_only".into())]),
        )
        .await
        .unwrap();
    changed(&mut receiver);
    let revision = store
        .language_preference(42)
        .await
        .unwrap()
        .unwrap()
        .revision;
    store.change_language(42, "ru", revision).await.unwrap();
    changed(&mut receiver);
    assert_eq!(
        store.change_language(42, "en", revision).await,
        Err(yoink_identity::IdentityError::LanguageConflict)
    );
    assert!(
        !receiver.has_changed().unwrap(),
        "failed CAS must not invalidate"
    );
    store
        .admin_update_user(
            42,
            43,
            &AdminUserPatch {
                role: Some(Role::Banned),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    changed(&mut receiver);
    assert_eq!(
        store
            .snapshot(43, None)
            .await
            .unwrap()
            .unwrap()
            .principal
            .role,
        Role::Banned
    );
    assert!(
        store
            .admin_update_user(
                43,
                44,
                &AdminUserPatch {
                    role: Some(Role::Admin),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert!(
        !receiver.has_changed().unwrap(),
        "denied mutation must not invalidate"
    );
}

#[tokio::test]
async fn group_thread_and_membership_mutations_share_the_same_invalidation_contract() {
    let (_dir, store) = setup().await;
    let mut receiver = store.subscribe_changes();
    store
        .group_create(
            42,
            &GroupCreate {
                id: -100,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    changed(&mut receiver);
    store
        .group_update(
            42,
            -100,
            &GroupPatch {
                enabled: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    changed(&mut receiver);
    let thread = store
        .thread_upsert(
            42,
            -100,
            &ThreadPatch {
                thread_id: Some(7),
                name: None,
                enabled: true,
            },
        )
        .await
        .unwrap();
    changed(&mut receiver);
    store
        .member_upsert(
            42,
            -100,
            43,
            &MemberPatch {
                role_override: Some(Role::Restricted),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    changed(&mut receiver);
    store.member_delete(42, -100, 43).await.unwrap();
    changed(&mut receiver);
    store.thread_delete(42, -100, thread.id).await.unwrap();
    changed(&mut receiver);
    store.group_delete(42, -100).await.unwrap();
    changed(&mut receiver);
}

#[tokio::test]
async fn bounded_keyset_enumeration_keeps_blocked_users_and_implicit_group_targets() {
    let (_dir, store) = setup().await;
    for id in 100..235 {
        register(&store, id).await;
    }
    store
        .admin_update_user(
            42,
            100,
            &AdminUserPatch {
                role: Some(Role::Banned),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    store
        .group_create(
            42,
            &GroupCreate {
                id: -100,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    // No subscriber was necessary for any preceding commit; notification state
    // is a bounded coalesced signal, not an accumulating event queue.
    let mut after = (0, i64::MIN);
    let mut targets = vec![];
    loop {
        let page = store.access_targets_after(after).await.unwrap();
        assert!(page.len() <= 128);
        if page.is_empty() {
            break;
        }
        assert!(page.iter().all(|target| *target > after));
        after = *page.last().unwrap();
        targets.extend(page);
    }
    assert_eq!(targets.len(), 137 * 2);
    assert!(
        targets.contains(&(100, 100)),
        "blocked users need revocation"
    );
    assert!(
        targets.contains(&(100, -100)),
        "implicit memberships still use auto-grant"
    );
    assert_eq!(store.access_groups_after(i64::MIN).await.unwrap(), [-100]);
    assert!(store.access_groups_after(-100).await.unwrap().is_empty());
}
