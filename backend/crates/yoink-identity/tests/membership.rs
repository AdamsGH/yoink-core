use chrono::{DateTime, Utc};
use std::time::Duration;
use yoink_access::{AccessPolicy, AccessScope, Role};
use yoink_db::{query, raw_sql};
use yoink_identity::{
    IdentityError, IdentityStore, MemberStatus, MembershipObservation, MembershipOutcome,
    MembershipSubject, ObservedTag, TrustedProfile,
    admin::{
        AdminError, AdminTagMapEntry,
        groups::{GroupCreate, GroupPatch, MemberPatch},
    },
};
use yoink_storage::Store;

async fn setup() -> (tempfile::TempDir, Store, IdentityStore) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Store::open(
        &dir.path().join("membership.db"),
        true,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    raw_sql(include_str!("sql/membership_fixtures.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let identity = IdentityStore::new(store.database().clone(), Some(1), false);
    (dir, store, identity)
}
fn profile(user_id: i64) -> TrustedProfile {
    TrustedProfile {
        user_id,
        username: Some("observed".into()),
        first_name: Some("Observed".into()),
        photo_url: None,
    }
}
fn bot(group: i64, time: i64, present: bool) -> MembershipObservation {
    MembershipObservation {
        source_id: 100,
        update_id: 1,
        occurred_at: DateTime::<Utc>::from_timestamp(time, 0).unwrap(),
        group_id: group,
        title: Some("Observed Group".into()),
        subject: MembershipSubject::Bot,
        status: if present {
            MemberStatus::Administrator
        } else {
            MemberStatus::Left
        },
        present,
        tag: ObservedTag::Unknown,
    }
}
fn user(
    group: i64,
    user: i64,
    time: i64,
    present: bool,
    tag: ObservedTag,
) -> MembershipObservation {
    MembershipObservation {
        subject: MembershipSubject::User(profile(user)),
        tag,
        status: if present {
            MemberStatus::Member
        } else {
            MemberStatus::Left
        },
        ..bot(group, time, present)
    }
}
fn vip() -> ObservedTag {
    ObservedTag::Known(Some("VIP".into()))
}
async fn enable(id: &IdentityStore, group: i64) {
    id.group_update(
        1,
        group,
        &GroupPatch {
            enabled: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
}
async fn grants(id: &IdentityStore, user: i64) -> Vec<(String, String)> {
    id.snapshot(user, None)
        .await
        .unwrap()
        .unwrap()
        .grants
        .into_iter()
        .map(|g| (g.plugin, g.feature))
        .collect()
}
fn tldr() -> (String, String) {
    ("insight".into(), "tldr".into())
}
fn policy() -> AccessPolicy {
    AccessPolicy {
        min_role: Role::User,
        scopes: vec![AccessScope::Group],
        check_group_enabled: true,
        check_thread_policy: true,
    }
}

#[tokio::test]
async fn discovery_ordering_disabled_and_presence_validation() {
    let (_dir, store, id) = setup().await;
    let mut changes = id.subscribe_changes();
    assert_eq!(
        id.observe_membership(&user(-10, 7, 100, true, vip()))
            .await
            .unwrap(),
        MembershipOutcome::UnknownGroup
    );
    assert!(!changes.has_changed().unwrap());
    assert_eq!(
        query(include_str!("sql/membership_state.sql"))
            .bind(7)
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get::<i64, _>("user_count")
            .unwrap(),
        0
    );
    // Unknown bot departure creates an ordering fence, never a group.
    assert_eq!(
        id.observe_membership(&bot(-11, 200, false)).await.unwrap(),
        MembershipOutcome::Applied
    );
    changes.borrow_and_update();
    assert_eq!(
        id.observe_membership(&bot(-11, 100, true)).await.unwrap(),
        MembershipOutcome::Stale
    );
    assert!(id.group_get(1, -11).await.is_err());
    let first = bot(-10, 100, true);
    assert_eq!(
        id.observe_membership(&first).await.unwrap(),
        MembershipOutcome::Applied
    );
    assert!(changes.has_changed().unwrap());
    changes.borrow_and_update();
    assert!(!id.group_get(1, -10).await.unwrap().enabled);
    assert_eq!(
        id.observe_membership(&first).await.unwrap(),
        MembershipOutcome::Duplicate
    );
    assert!(!changes.has_changed().unwrap());
    let mut conflict = first.clone();
    conflict.title = Some("different".into());
    assert_eq!(
        id.observe_membership(&conflict).await,
        Err(IdentityError::MembershipConflict)
    );
    assert!(!changes.has_changed().unwrap());
    let mut reset = bot(-10, 101, true);
    reset.update_id = 0;
    assert_eq!(
        id.observe_membership(&reset).await.unwrap(),
        MembershipOutcome::Applied
    );
    changes.borrow_and_update();
    assert_eq!(
        id.observe_membership(&first).await.unwrap(),
        MembershipOutcome::Stale
    );
    assert!(!changes.has_changed().unwrap());
    let mut invalid = user(-10, 7, 102, false, ObservedTag::Unknown);
    invalid.status = MemberStatus::Member;
    assert_eq!(
        id.observe_membership(&invalid).await,
        Err(IdentityError::InvalidMembership)
    );
    invalid.status = MemberStatus::Restricted;
    id.observe_membership(&invalid).await.unwrap();
    enable(&id, -10).await;
    assert!(!id.membership_eligible(100, -10, 7).await.unwrap());
    assert!(
        !id.snapshot(7, Some((-10, None)))
            .await
            .unwrap()
            .unwrap()
            .authorize(&policy(), None)
            .decision
            .allowed
    );
    let mut invalid_bot = bot(-10, 103, true);
    invalid_bot.tag = vip();
    assert_eq!(
        id.observe_membership(&invalid_bot).await,
        Err(IdentityError::InvalidMembership)
    );
    invalid_bot.tag = ObservedTag::Unknown;
    invalid_bot.title = Some("界".repeat(257));
    assert_eq!(
        id.observe_membership(&invalid_bot).await,
        Err(IdentityError::InvalidMembership)
    );
}

#[tokio::test]
async fn observations_preserve_roles_bans_policies_preferences_and_policy_registration() {
    let (_dir, _store, id) = setup().await;
    id.observe_membership(&bot(-10, 100, true)).await.unwrap();
    enable(&id, -10).await;
    id.member_upsert(
        1,
        -10,
        3,
        &MemberPatch {
            role_override: Some(Role::Moderator),
            allow_pm_override: Some(false),
        },
    )
    .await
    .unwrap();
    id.observe_membership(&user(-10, 3, 101, true, vip()))
        .await
        .unwrap();
    let p = id.current_profile(3).await.unwrap();
    assert_eq!(
        (
            p.role,
            p.language.as_str(),
            p.theme.as_str(),
            p.username.as_deref(),
            p.updated_at.timestamp_micros()
        ),
        (Role::Restricted, "ru", "frappe", Some("original"), 20)
    );
    assert_eq!(
        id.members_list(1, -10).await.unwrap()[0].role_override,
        Some(Role::Moderator)
    );
    assert!(
        id.snapshot(3, Some((-10, None)))
            .await
            .unwrap()
            .unwrap()
            .authorize(&policy(), None)
            .decision
            .allowed
    );
    id.observe_membership(&user(-10, 4, 102, false, ObservedTag::Unknown))
        .await
        .unwrap();
    let banned = id.snapshot(4, None).await.unwrap().unwrap();
    assert_eq!(banned.principal.role, Role::Banned);
    assert_eq!(
        banned.principal.ban_until.unwrap().timestamp_micros(),
        4102444800000000
    );
    id.admin_update_bot_settings(
        1,
        &std::collections::BTreeMap::from([("bot_access_mode".into(), "approved_only".into())]),
    )
    .await
    .unwrap();
    id.observe_membership(&user(-10, 7, 103, true, ObservedTag::Unknown))
        .await
        .unwrap();
    assert_eq!(id.current_profile(7).await.unwrap().role, Role::Restricted);
    id.admin_update_bot_settings(
        1,
        &std::collections::BTreeMap::from([("bot_access_mode".into(), "open".into())]),
    )
    .await
    .unwrap();
    id.observe_membership(&user(-10, 8, 104, true, ObservedTag::Unknown))
        .await
        .unwrap();
    assert_eq!(id.current_profile(8).await.unwrap().role, Role::User);
    // Departure gates execution and menus even for a configured owner.
    id.observe_membership(&user(-10, 1, 105, false, ObservedTag::Unknown))
        .await
        .unwrap();
    assert!(!id.membership_eligible(100, -10, 1).await.unwrap());
    assert!(
        !id.snapshot(1, Some((-10, None)))
            .await
            .unwrap()
            .unwrap()
            .authorize(&policy(), None)
            .decision
            .allowed
    );
    // Imported unknown membership still inherits baseline availability.
    assert!(id.membership_eligible(100, -10, 5).await.unwrap());
}

#[tokio::test]
async fn lifecycle_generation_unknown_tag_and_explicit_clear() {
    let (_dir, _store, id) = setup().await;
    id.observe_membership(&bot(-10, 100, true)).await.unwrap();
    enable(&id, -10).await;
    id.observe_membership(&user(-10, 3, 101, true, vip()))
        .await
        .unwrap();
    assert_eq!(grants(&id, 3).await, vec![tldr()]);
    id.observe_membership(&user(-10, 3, 102, true, ObservedTag::Unknown))
        .await
        .unwrap();
    assert_eq!(grants(&id, 3).await, vec![tldr()]);
    id.observe_membership(&bot(-10, 110, false)).await.unwrap();
    assert!(id.group_get(1, -10).await.unwrap().enabled); // admin setting preserved
    assert!(
        !id.snapshot(1, Some((-10, None)))
            .await
            .unwrap()
            .unwrap()
            .group
            .unwrap()
            .enabled
    );
    assert!(grants(&id, 3).await.is_empty());
    id.observe_membership(&bot(-10, 120, true)).await.unwrap();
    assert!(grants(&id, 3).await.is_empty());
    assert!(!id.membership_eligible(100, -10, 3).await.unwrap());
    // A late user event from before the gap cannot confirm the new generation.
    id.observe_membership(&user(-10, 3, 115, true, vip()))
        .await
        .unwrap();
    assert!(grants(&id, 3).await.is_empty());
    id.observe_membership(&user(-10, 3, 121, true, ObservedTag::Unknown))
        .await
        .unwrap();
    assert!(id.membership_eligible(100, -10, 3).await.unwrap());
    assert!(grants(&id, 3).await.is_empty());
    id.observe_membership(&user(-10, 3, 122, true, vip()))
        .await
        .unwrap();
    assert_eq!(grants(&id, 3).await, vec![tldr()]);
    id.observe_membership(&user(-10, 3, 123, true, ObservedTag::Known(None)))
        .await
        .unwrap();
    assert!(grants(&id, 3).await.is_empty());
}

#[tokio::test]
async fn overlapping_groups_sources_manual_imported_and_owner_suppression() {
    let (_dir, store, id) = setup().await;
    for group in [-10, -11] {
        id.observe_membership(&bot(group, 100, true)).await.unwrap();
        enable(&id, group).await;
        id.observe_membership(&user(group, 3, 101, true, vip()))
            .await
            .unwrap();
    }
    assert_eq!(grants(&id, 3).await, vec![tldr()]);
    assert_eq!(
        query(include_str!("sql/membership_state.sql"))
            .bind(3)
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get::<i64, _>("ledger_count")
            .unwrap(),
        0
    );
    let list = id.admin_permissions(1, Some(3)).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].grant_source, "tag");
    assert!(list[0].id < 0);
    assert_eq!(
        id.admin_permissions(1, Some(3)).await.unwrap()[0].id,
        list[0].id
    );
    assert_eq!(
        id.admin_revoke_permission(2, 3, "insight", "tldr").await,
        Err(AdminError::Forbidden)
    );
    assert!(
        id.admin_grant_permission(2, 3, "insight", "tldr", None)
            .await
            .is_err()
    );
    id.observe_membership(&user(-10, 3, 102, false, ObservedTag::Known(None)))
        .await
        .unwrap();
    assert_eq!(grants(&id, 3).await, vec![tldr()]);
    id.admin_revoke_permission(1, 3, "insight", "tldr")
        .await
        .unwrap();
    assert!(grants(&id, 3).await.is_empty());
    id.observe_membership(&user(-11, 3, 103, true, vip()))
        .await
        .unwrap();
    assert!(grants(&id, 3).await.is_empty());
    id.admin_grant_permission(1, 3, "insight", "tldr", None)
        .await
        .unwrap();
    id.observe_membership(&user(-11, 3, 104, false, ObservedTag::Known(None)))
        .await
        .unwrap();
    assert_eq!(grants(&id, 3).await, vec![tldr()]);
    assert_eq!(id.admin_permissions(1, Some(3)).await.unwrap().len(), 1);
    assert_eq!(
        id.admin_permissions(1, Some(3)).await.unwrap()[0].grant_source,
        "manual"
    );
    assert_eq!(grants(&id, 6).await, vec![tldr()]); // imported tag provenance is not guessed
    assert_eq!(
        query(include_str!("sql/membership_state.sql"))
            .bind(6)
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get::<i64, _>("observations")
            .unwrap(),
        0
    );
    // Multiple bot sources for the same group remain separate contributions.
    let mut second = bot(-11, 200, true);
    second.source_id = 200;
    id.observe_membership(&second).await.unwrap();
    let mut second_user = user(-11, 5, 201, true, vip());
    second_user.source_id = 200;
    id.observe_membership(&second_user).await.unwrap();
    id.observe_membership(&user(-11, 5, 202, true, vip()))
        .await
        .unwrap();
    second_user.occurred_at = DateTime::from_timestamp(203, 0).unwrap();
    second_user.tag = ObservedTag::Known(None);
    id.observe_membership(&second_user).await.unwrap();
    assert_eq!(grants(&id, 5).await, vec![tldr()]);
}

#[tokio::test]
async fn map_updates_and_corruption_are_immediate_and_fail_closed() {
    let (_dir, store, id) = setup().await;
    id.observe_membership(&bot(-10, 100, true)).await.unwrap();
    enable(&id, -10).await;
    id.observe_membership(&user(-10, 3, 101, true, vip()))
        .await
        .unwrap();
    assert_eq!(grants(&id, 3).await, vec![tldr()]);
    id.admin_update_tag_map(
        1,
        &[AdminTagMapEntry {
            tag: "VIP".into(),
            features: vec!["insight:summary".into()],
        }],
    )
    .await
    .unwrap();
    assert_eq!(
        grants(&id, 3).await,
        vec![("insight".into(), "summary".into())]
    );
    query(include_str!("sql/membership_corrupt_map.sql"))
        .execute(store.database())
        .await
        .unwrap();
    assert!(grants(&id, 3).await.is_empty());
    assert!(id.admin_tag_map(1).await.is_err());
    // Corruption does not remove or hide imported provenance.
    assert_eq!(grants(&id, 6).await, vec![tldr()]);
}

#[tokio::test]
async fn deletion_suppression_cleanup_and_generation_survive_restart() {
    let (dir, store, id) = setup().await;
    id.observe_membership(&bot(-10, 100, true)).await.unwrap();
    enable(&id, -10).await;
    id.observe_membership(&user(-10, 3, 101, true, vip()))
        .await
        .unwrap();
    id.group_delete(1, -10).await.unwrap();
    assert_eq!(
        id.observe_membership(&bot(-10, 200, true)).await.unwrap(),
        MembershipOutcome::Suppressed
    );
    assert_eq!(
        id.observe_membership(&bot(-10, 90, true)).await.unwrap(),
        MembershipOutcome::Suppressed
    );
    assert!(grants(&id, 3).await.is_empty());
    drop(id);
    drop(store);
    let store = Store::open(
        &dir.path().join("membership.db"),
        false,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    store.verify_schema().await.unwrap();
    let id = IdentityStore::new(store.database().clone(), Some(1), false);
    assert_eq!(
        id.observe_membership(&bot(-10, 300, true)).await.unwrap(),
        MembershipOutcome::Suppressed
    );
    assert!(
        id.access_groups_after(i64::MIN)
            .await
            .unwrap()
            .contains(&-10)
    );
    assert!(
        id.access_targets_after((0, i64::MIN))
            .await
            .unwrap()
            .contains(&(3, -10))
    );
    assert!(
        id.membership_cleanup_targets(100)
            .await
            .unwrap()
            .contains(&(3, -10))
    );
    assert!(!id.membership_eligible(100, -10, 3).await.unwrap());
    id.group_create(
        1,
        &GroupCreate {
            id: -10,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    enable(&id, -10).await;
    assert!(grants(&id, 3).await.is_empty());
    id.observe_membership(&bot(-10, 400, true)).await.unwrap();
    assert!(grants(&id, 3).await.is_empty());
    id.observe_membership(&user(-10, 3, 401, true, vip()))
        .await
        .unwrap();
    assert_eq!(grants(&id, 3).await, vec![tldr()]);
    // Persisted owner suppression cannot be undone by restart/reobservation.
    id.admin_revoke_permission(1, 3, "insight", "tldr")
        .await
        .unwrap();
    drop(id);
    drop(store);
    let store = Store::open(
        &dir.path().join("membership.db"),
        false,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let id = IdentityStore::new(store.database().clone(), Some(1), false);
    id.observe_membership(&user(-10, 3, 402, true, vip()))
        .await
        .unwrap();
    assert!(grants(&id, 3).await.is_empty());
}

#[tokio::test]
async fn invalid_policy_rolls_back_registration_and_ordering_without_signal() {
    let (_dir, store, id) = setup().await;
    id.observe_membership(&bot(-10, 100, true)).await.unwrap();
    query(include_str!("sql/membership_corrupt_policy.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let mut changes = id.subscribe_changes();
    let o = user(-10, 7, 101, true, vip());
    assert_eq!(
        id.observe_membership(&o).await,
        Err(IdentityError::InvalidPolicy)
    );
    assert!(!changes.has_changed().unwrap());
    let state = query(include_str!("sql/membership_state.sql"))
        .bind(7)
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(state.try_get::<i64, _>("user_count").unwrap(), 0);
    assert_eq!(state.try_get::<i64, _>("observations").unwrap(), 0);
    id.admin_update_bot_settings(
        1,
        &std::collections::BTreeMap::from([("bot_access_mode".into(), "open".into())]),
    )
    .await
    .unwrap();
    changes.borrow_and_update();
    assert_eq!(
        id.observe_membership(&o).await.unwrap(),
        MembershipOutcome::Applied
    );
    assert!(changes.has_changed().unwrap());
    // Connection guard was released and committed state is immediately readable.
    assert!(
        tokio::time::timeout(Duration::from_secs(1), id.snapshot(7, None))
            .await
            .unwrap()
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn database_failure_after_user_insert_rolls_back_everything_without_signal() {
    let (_dir, store, id) = setup().await;
    id.observe_membership(&bot(-10, 100, true)).await.unwrap();
    raw_sql(include_str!("sql/membership_reject_insert.sql"))
        .execute(store.database())
        .await
        .unwrap();
    let mut changes = id.subscribe_changes();
    let observation = user(-10, 8, 101, true, vip());
    assert_eq!(
        id.observe_membership(&observation).await,
        Err(IdentityError::Database)
    );
    assert!(!changes.has_changed().unwrap());
    let state = query(include_str!("sql/membership_state.sql"))
        .bind(8)
        .fetch_one(store.database())
        .await
        .unwrap();
    assert_eq!(state.try_get::<i64, _>("user_count").unwrap(), 0);
    assert_eq!(state.try_get::<i64, _>("observations").unwrap(), 0);
    raw_sql(include_str!("sql/membership_allow_insert.sql"))
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        id.observe_membership(&observation).await.unwrap(),
        MembershipOutcome::Applied
    );
    assert!(changes.has_changed().unwrap());
    changes.borrow_and_update();
    assert_eq!(
        id.observe_membership(&observation).await.unwrap(),
        MembershipOutcome::Duplicate
    );
    assert!(!changes.has_changed().unwrap());
}

#[tokio::test]
async fn known_member_departure_and_restricted_bot_absence_survive_restart() {
    let (dir, store, id) = setup().await;
    id.observe_membership(&bot(-10, 100, true)).await.unwrap();
    enable(&id, -10).await;
    id.observe_membership(&user(-10, 3, 101, true, vip()))
        .await
        .unwrap();
    id.observe_membership(&user(-10, 3, 102, false, ObservedTag::Unknown))
        .await
        .unwrap();
    drop(id);
    drop(store);
    let store = Store::open(
        &dir.path().join("membership.db"),
        false,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let id = IdentityStore::new(store.database().clone(), Some(1), false);
    assert!(!id.membership_eligible(100, -10, 3).await.unwrap());
    assert!(
        !id.snapshot(3, Some((-10, None)))
            .await
            .unwrap()
            .unwrap()
            .authorize(&policy(), None)
            .decision
            .allowed
    );
    assert!(grants(&id, 3).await.is_empty());
    let mut absent = bot(-10, 110, false);
    absent.status = MemberStatus::Restricted;
    id.observe_membership(&absent).await.unwrap();
    assert!(id.group_get(1, -10).await.unwrap().enabled);
    assert!(
        !id.snapshot(1, Some((-10, None)))
            .await
            .unwrap()
            .unwrap()
            .authorize(&policy(), None)
            .decision
            .allowed
    );
    assert!(
        id.membership_cleanup_targets(100)
            .await
            .unwrap()
            .contains(&(1, -10))
    );
    drop(id);
    drop(store);
    let store = Store::open(
        &dir.path().join("membership.db"),
        false,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let id = IdentityStore::new(store.database().clone(), Some(1), false);
    assert!(!id.membership_eligible(100, -10, 1).await.unwrap());
    assert!(
        !id.snapshot(1, Some((-10, None)))
            .await
            .unwrap()
            .unwrap()
            .authorize(&policy(), None)
            .decision
            .allowed
    );
}

#[tokio::test]
async fn automatic_union_is_indefinite_without_rewriting_expiring_manual_grant() {
    let (_dir, store, id) = setup().await;
    id.observe_membership(&bot(-10, 100, true)).await.unwrap();
    enable(&id, -10).await;
    id.observe_membership(&user(-10, 3, 101, true, vip()))
        .await
        .unwrap();
    let expiry = DateTime::from_timestamp(4102444800, 0).unwrap();
    id.admin_grant_permission(1, 3, "insight", "tldr", Some(expiry))
        .await
        .unwrap();
    let permissions = id.admin_permissions(1, Some(3)).await.unwrap();
    assert_eq!(permissions.len(), 1);
    assert_eq!(permissions[0].grant_source, "tag");
    assert_eq!(permissions[0].expires_at, None);
    let snapshot = id.snapshot(3, None).await.unwrap().unwrap();
    assert_eq!(snapshot.grants.len(), 1);
    assert_eq!(snapshot.grants[0].expires_at, None);
    assert!(
        id.admin_feature_access(1, Some(3))
            .await
            .unwrap()
            .into_iter()
            .find(|f| f.plugin == "insight" && f.feature == "tldr")
            .unwrap()
            .effective
    );
    id.observe_membership(&user(-10, 3, 102, false, ObservedTag::Unknown))
        .await
        .unwrap();
    let permissions = id.admin_permissions(1, Some(3)).await.unwrap();
    assert_eq!(permissions[0].grant_source, "manual");
    assert_eq!(permissions[0].expires_at, Some(expiry));
    assert_eq!(
        query(include_str!("sql/membership_state.sql"))
            .bind(3)
            .fetch_one(store.database())
            .await
            .unwrap()
            .try_get::<i64, _>("ledger_count")
            .unwrap(),
        1
    );
}
