use std::time::Duration;
use yoink_identity::{IdentityStore, MenuContact, MenuContactObservation, TrustedProfile};
use yoink_storage::Store;

async fn open(path: &std::path::Path, create: bool) -> (Store, IdentityStore) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path.parent().unwrap(),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    let store = Store::open(path, create, Duration::from_secs(2))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let identity = IdentityStore::new(store.database().clone(), Some(42), false);
    (store, identity)
}

fn contact(chat_id: i64, user_id: i64) -> MenuContact {
    MenuContact {
        source_id: 123456,
        chat_id,
        user_id,
    }
}

fn event(update_id: u32) -> MenuContactObservation {
    MenuContactObservation {
        occurred_at: 100 + i64::from(update_id),
        update_id,
    }
}

#[tokio::test]
async fn menu_contacts_survive_reopen_without_changing_schema_authority_or_admin_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("contacts.db");
    let (store, identity) = open(&path, true).await;
    for user_id in [42, 43] {
        identity
            .register(&TrustedProfile {
                user_id,
                username: None,
                first_name: None,
                photo_url: None,
            })
            .await
            .unwrap();
    }
    let before = identity.snapshot(43, None).await.unwrap().unwrap();
    let target = contact(43, 0);
    let lease = identity.begin_menu_contact(target).await.unwrap().unwrap();
    identity.reject_menu_contact(lease, true).await.unwrap();
    assert!(identity.begin_menu_contact(target).await.unwrap().is_none());
    identity.notify_committed_access_change();
    assert!(identity.begin_menu_contact(target).await.unwrap().is_none());
    assert!(
        identity
            .begin_menu_contact(MenuContact {
                source_id: 654321,
                ..target
            })
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        !identity
            .admin_bot_settings(42)
            .await
            .unwrap()
            .keys()
            .any(|key| key.starts_with("mc:"))
    );
    let after = identity.snapshot(43, None).await.unwrap().unwrap();
    assert_eq!(before.principal, after.principal);
    assert_eq!(before.grants, after.grants);
    store.verify_schema().await.unwrap();
    store.close().await.unwrap();
    drop(identity);
    drop(store);

    let (store, identity) = open(&path, false).await;
    assert!(identity.begin_menu_contact(target).await.unwrap().is_none());
    assert!(
        identity
            .restore_menu_contact(target, event(1))
            .await
            .unwrap()
    );
    assert!(identity.begin_menu_contact(target).await.unwrap().is_some());
    store.verify_schema().await.unwrap();
}

#[tokio::test]
async fn menu_contact_member_and_chat_rejections_have_different_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, identity) = open(&dir.path().join("contacts.db"), true).await;
    let member = contact(-100, 43);
    let other = contact(-100, 44);
    let lease = identity.begin_menu_contact(member).await.unwrap().unwrap();
    identity.reject_menu_contact(lease, false).await.unwrap();
    assert!(identity.begin_menu_contact(member).await.unwrap().is_none());
    assert!(identity.begin_menu_contact(other).await.unwrap().is_some());
    assert!(
        identity
            .begin_menu_contact(contact(-200, 43))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        identity
            .begin_menu_contact(contact(43, 0))
            .await
            .unwrap()
            .is_some()
    );

    let lease = identity.begin_menu_contact(other).await.unwrap().unwrap();
    identity.reject_menu_contact(lease, true).await.unwrap();
    assert!(identity.begin_menu_contact(other).await.unwrap().is_none());
    identity
        .restore_menu_contact(member, event(1))
        .await
        .unwrap();
    assert!(identity.begin_menu_contact(member).await.unwrap().is_none());
    identity
        .restore_menu_contact(contact(-100, 0), event(2))
        .await
        .unwrap();
    assert!(identity.begin_menu_contact(other).await.unwrap().is_some());
    assert!(identity.begin_menu_contact(member).await.unwrap().is_some());
}

#[tokio::test]
async fn menu_contact_replay_and_late_failures_cannot_reverse_newer_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, identity) = open(&dir.path().join("contacts.db"), true).await;
    let member = contact(-100, 43);
    identity
        .restore_menu_contact(member, event(2))
        .await
        .unwrap();
    let old = identity.begin_menu_contact(member).await.unwrap().unwrap();
    identity
        .restore_menu_contact(member, event(3))
        .await
        .unwrap();
    identity.reject_menu_contact(old, false).await.unwrap();
    assert!(identity.begin_menu_contact(member).await.unwrap().is_some());

    let old = identity.begin_menu_contact(member).await.unwrap().unwrap();
    identity
        .restore_menu_contact(contact(-100, 0), event(4))
        .await
        .unwrap();
    identity.reject_menu_contact(old, true).await.unwrap();
    assert!(identity.begin_menu_contact(member).await.unwrap().is_some());

    let current = identity.begin_menu_contact(member).await.unwrap().unwrap();
    identity.reject_menu_contact(current, false).await.unwrap();
    for stale in [event(2), event(3)] {
        assert!(!identity.restore_menu_contact(member, stale).await.unwrap());
        assert!(identity.begin_menu_contact(member).await.unwrap().is_none());
    }
    assert!(
        identity
            .restore_menu_contact(member, event(5))
            .await
            .unwrap()
    );
    identity
        .observe_menu_contact(member, event(6), false)
        .await
        .unwrap();
    assert!(
        !identity
            .restore_menu_contact(member, event(5))
            .await
            .unwrap()
    );
    assert!(identity.begin_menu_contact(member).await.unwrap().is_none());
}

#[tokio::test]
async fn menu_contact_state_changes_signal_recovery_not_every_observation() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, identity) = open(&dir.path().join("contacts.db"), true).await;
    let mut changes = identity.subscribe_changes();
    let target = contact(43, 0);
    identity
        .restore_menu_contact(target, event(1))
        .await
        .unwrap();
    assert!(!changes.has_changed().unwrap());
    let lease = identity.begin_menu_contact(target).await.unwrap().unwrap();
    identity.reject_menu_contact(lease, true).await.unwrap();
    assert!(!changes.has_changed().unwrap());
    identity
        .restore_menu_contact(target, event(2))
        .await
        .unwrap();
    assert!(changes.has_changed().unwrap());
    changes.borrow_and_update();
    identity
        .restore_menu_contact(target, event(2))
        .await
        .unwrap();
    assert!(!changes.has_changed().unwrap());

    let longest = MenuContact {
        source_id: i64::MAX,
        chat_id: i64::MIN,
        user_id: i64::MAX,
    };
    let lease = identity.begin_menu_contact(longest).await.unwrap().unwrap();
    identity.reject_menu_contact(lease, false).await.unwrap();
    assert!(
        identity
            .begin_menu_contact(longest)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn menu_contact_rejection_fences_only_the_scope_being_invalidated() {
    let dir = tempfile::tempdir().unwrap();
    let (_store, identity) = open(&dir.path().join("contacts.db"), true).await;
    let member = contact(-100, 43);
    let lease = identity.begin_menu_contact(member).await.unwrap().unwrap();
    identity
        .restore_menu_contact(contact(-100, 0), event(1))
        .await
        .unwrap();
    identity.reject_menu_contact(lease, false).await.unwrap();
    assert!(identity.begin_menu_contact(member).await.unwrap().is_none());
    identity
        .restore_menu_contact(member, event(2))
        .await
        .unwrap();
    let lease = identity.begin_menu_contact(member).await.unwrap().unwrap();
    identity
        .restore_menu_contact(member, event(3))
        .await
        .unwrap();
    identity.reject_menu_contact(lease, true).await.unwrap();
    assert!(
        identity
            .begin_menu_contact(contact(-100, 44))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn menu_contact_membership_commit_restores_only_new_evidence_and_the_correct_scope() {
    use yoink_identity::{
        MemberStatus, MembershipObservation, MembershipOutcome, MembershipSubject, ObservedTag,
    };
    let dir = tempfile::tempdir().unwrap();
    let (_store, identity) = open(&dir.path().join("contacts.db"), true).await;
    let observation = |update_id, subject, present| MembershipObservation {
        source_id: 123456,
        group_id: -100,
        update_id,
        occurred_at: chrono::DateTime::from_timestamp(100 + i64::from(update_id), 0).unwrap(),
        title: Some("Group".into()),
        subject,
        status: if present {
            MemberStatus::Member
        } else {
            MemberStatus::Left
        },
        present,
        tag: ObservedTag::Unknown,
    };
    identity
        .observe_membership(&observation(1, MembershipSubject::Bot, true))
        .await
        .unwrap();
    let user = || {
        MembershipSubject::User(TrustedProfile {
            user_id: 43,
            username: None,
            first_name: None,
            photo_url: None,
        })
    };
    let joined = observation(2, user(), true);
    assert_eq!(
        identity.observe_membership(&joined).await.unwrap(),
        MembershipOutcome::Applied
    );
    let member = contact(-100, 43);
    let lease = identity.begin_menu_contact(member).await.unwrap().unwrap();
    identity.reject_menu_contact(lease, false).await.unwrap();
    assert_eq!(
        identity.observe_membership(&joined).await.unwrap(),
        MembershipOutcome::Duplicate
    );
    assert!(identity.begin_menu_contact(member).await.unwrap().is_none());
    identity
        .observe_membership(&observation(3, user(), true))
        .await
        .unwrap();
    assert!(identity.begin_menu_contact(member).await.unwrap().is_some());
    identity
        .observe_membership(&observation(4, MembershipSubject::Bot, false))
        .await
        .unwrap();
    assert!(identity.begin_menu_contact(member).await.unwrap().is_none());
    identity
        .observe_membership(&observation(5, user(), true))
        .await
        .unwrap();
    assert!(identity.begin_menu_contact(member).await.unwrap().is_none());
    identity
        .observe_membership(&observation(6, MembershipSubject::Bot, true))
        .await
        .unwrap();
    assert!(identity.begin_menu_contact(member).await.unwrap().is_some());

    identity
        .register(&TrustedProfile {
            user_id: 42,
            username: None,
            first_name: None,
            photo_url: None,
        })
        .await
        .unwrap();
    identity.group_delete(42, -100).await.unwrap();
    let lease = identity.begin_menu_contact(member).await.unwrap().unwrap();
    identity.reject_menu_contact(lease, true).await.unwrap();
    assert_eq!(
        identity
            .observe_membership(&observation(7, MembershipSubject::Bot, true))
            .await
            .unwrap(),
        MembershipOutcome::Suppressed
    );
    assert!(identity.begin_menu_contact(member).await.unwrap().is_some());
    assert!(matches!(
        identity.group_get(42, -100).await,
        Err(yoink_identity::admin::AdminError::NotFound)
    ));
    assert!(
        !identity
            .membership_eligible(123456, -100, 43)
            .await
            .unwrap()
    );
}
