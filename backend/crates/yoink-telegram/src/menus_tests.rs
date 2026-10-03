use super::*;
use std::{collections::HashMap, sync::Mutex};
use tokio::sync::Notify;
use yoink_identity::{TrustedProfile, admin::AdminUserPatch};

type Saved = HashMap<(BotCommandScope, String), Vec<BotCommand>>;
#[derive(Default)]
struct FakeState {
    saved: Saved,
    hold: bool,
    fail: bool,
}
#[derive(Clone, Default)]
struct Fake {
    state: Arc<Mutex<FakeState>>,
    held: Arc<Notify>,
    release: Arc<Notify>,
    applied: Arc<Notify>,
}
fn private(user: i64) -> BotCommandScope {
    BotCommandScope::Chat {
        chat_id: Recipient::Id(ChatId(user)),
    }
}
impl MenuApi for Fake {
    async fn apply(
        &self,
        scope: BotCommandScope,
        locale: &str,
        commands: Option<Vec<BotCommand>>,
    ) -> Result<(), Failure> {
        let (hold, fail) = {
            let mut state = self.state.lock().unwrap();
            let target = scope == private(43) && commands.is_some();
            let hold = target && std::mem::take(&mut state.hold);
            let fail = target && std::mem::take(&mut state.fail);
            (hold, fail)
        };
        if hold {
            self.held.notify_one();
            self.release.notified().await;
        }
        {
            let mut state = self.state.lock().unwrap();
            let key = (scope, locale.to_owned());
            if let Some(commands) = commands {
                state.saved.insert(key, commands);
            } else {
                state.saved.remove(&key);
            }
        }
        self.applied.notify_one();
        // Applied-but-lost response: retry must remain dirty even without a new event.
        if fail {
            self.state.lock().unwrap().hold = true;
            Err(Failure::Retry(Duration::from_millis(1)))
        } else {
            Ok(())
        }
    }
}
fn names(commands: &[BotCommand]) -> Vec<&str> {
    commands
        .iter()
        .map(|command| command.command.as_str())
        .collect()
}

#[test]
fn catalog_is_native_localized_unique_and_never_elevates_ordinary_users() {
    let ordinary = Eligibility {
        approved: true,
        owner: false,
        downloads: true,
        group: false,
    };
    let expected = [
        "start", "help", "download", "video", "audio", "image", "job", "cancel",
    ];
    for language in [Language::English, Language::Russian] {
        assert_eq!(names(&catalog(ordinary, language)), expected);
        let owner = catalog(
            Eligibility {
                owner: true,
                ..ordinary
            },
            language,
        );
        assert_eq!(owner.len(), 20);
        let mut unique = std::collections::HashSet::new();
        for command in &owner {
            assert!(unique.insert(&command.command));
            assert!((1..=256).contains(&command.description.chars().count()));
            assert!(crate::parse_command(&format!("/{}", command.command), "bot").is_some());
        }
        for unported in [
            "block",
            "unblock",
            "ban_time",
            "broadcast",
            "runtime",
            "group",
            "thread",
            "inline",
            "tldr",
            "search",
        ] {
            assert!(!names(&owner).contains(&unported));
        }
    }
    let en = catalog(ordinary, Language::English);
    let ru = catalog(ordinary, Language::Russian);
    assert!(
        en.iter()
            .zip(&ru)
            .all(|(en, ru)| en.description != ru.description)
    );
}

#[test]
fn catalog_respects_disabled_downloads_blocking_and_group_private_boundaries() {
    let basic = Eligibility {
        approved: true,
        owner: false,
        downloads: false,
        group: false,
    };
    assert_eq!(names(&catalog(basic, Language::English)), ["start", "help"]);
    assert_eq!(
        names(&catalog(
            Eligibility {
                owner: true,
                ..basic
            },
            Language::English
        )),
        ["start", "help", "status"]
    );
    assert!(
        catalog(
            Eligibility {
                approved: false,
                owner: true,
                downloads: true,
                group: false
            },
            Language::English
        )
        .is_empty()
    );
    let group = catalog(
        Eligibility {
            group: true,
            downloads: true,
            owner: true,
            ..basic
        },
        Language::English,
    );
    assert!(names(&group).contains(&"playlist"));
    for private in [
        "status", "list", "settings", "format", "quality", "proxy", "uncache", "cookie", "lang",
    ] {
        assert!(!names(&group).contains(&private));
    }
}

#[test]
fn failures_are_safe_fixed_categories_and_rate_limit_is_not_shortened() {
    use teloxide::types::Seconds;
    let retry = Duration::from_secs(1);
    assert_eq!(
        classify(&RequestError::RetryAfter(Seconds::from_seconds(9)), retry),
        Failure::RateLimited(Duration::from_secs(9))
    );
    assert_eq!(
        classify(&RequestError::Api(ApiError::BotBlocked), retry),
        Failure::Unreachable
    );
    assert_eq!(
        classify(
            &RequestError::Api(ApiError::Unknown("token/SECRET".into())),
            retry
        ),
        Failure::Retry(retry)
    );
    assert!(
        !format!(
            "{:?}",
            classify(
                &RequestError::Api(ApiError::Unknown("token/SECRET".into())),
                retry
            )
        )
        .contains("SECRET")
    );
    assert!(MenuConfig::default().valid());
    assert!(
        !MenuConfig {
            request_timeout: Duration::ZERO,
            ..MenuConfig::default()
        }
        .valid()
    );
}

#[tokio::test]
async fn personalized_language_removes_both_higher_precedence_overrides() {
    let api = Fake::default();
    for locale in ["", "en", "ru"] {
        api.apply(
            private(43),
            locale,
            Some(vec![BotCommand::new("broadcast", "stale")]),
        )
        .await
        .unwrap();
    }
    let ru = catalog(
        Eligibility {
            approved: true,
            owner: false,
            downloads: true,
            group: false,
        },
        Language::Russian,
    );
    personalized(&api, private(43), ru.clone()).await.unwrap();
    let state = api.state.lock().unwrap();
    assert!(!state.saved.contains_key(&(private(43), "en".into())));
    assert!(!state.saved.contains_key(&(private(43), "ru".into())));
    assert_eq!(state.saved[&(private(43), "".into())], ru);
}

async fn setup() -> (tempfile::TempDir, Arc<IdentityStore>) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store =
        yoink_storage::Store::open(&dir.path().join("menus.db"), true, Duration::from_secs(2))
            .await
            .unwrap();
    store.migrate().await.unwrap();
    let identities = Arc::new(IdentityStore::new(
        store.database().clone(),
        Some(42),
        false,
    ));
    for user_id in [42, 43, 44] {
        identities
            .register(&TrustedProfile {
                user_id,
                username: None,
                first_name: None,
                photo_url: None,
            })
            .await
            .unwrap();
    }
    (dir, identities)
}
fn worker(api: Fake, identities: Arc<IdentityStore>) -> Menus<Fake> {
    Menus {
        changes: Some(identities.subscribe_changes()),
        identities: Some(identities),
        api,
        config: MenuConfig {
            retry_delay: Duration::from_millis(1),
            reconcile_interval: Duration::from_secs(3600),
            ..MenuConfig::default()
        },
        owner: 42,
        downloads: true,
        groups: false,
        known_groups: Default::default(),
    }
}
async fn until(api: &Fake, predicate: impl Fn(&Saved) -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let notified = api.applied.notified();
            if predicate(&api.state.lock().unwrap().saved) {
                return;
            }
            notified.await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn startup_replaces_offline_and_banned_persisted_menus() {
    let (_dir, identities) = setup().await;
    identities
        .admin_update_user(
            42,
            44,
            &AdminUserPatch {
                role: Some(Role::Banned),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    identities
        .update_settings(43, Some("ru"), None)
        .await
        .unwrap();
    let api = Fake::default();
    for user in [43, 44] {
        for locale in ["", "en", "ru"] {
            api.apply(
                private(user),
                locale,
                Some(vec![BotCommand::new("broadcast", "stale")]),
            )
            .await
            .unwrap();
        }
    }
    worker(api.clone(), identities).initialize().await;
    let state = api.state.lock().unwrap();
    assert!(state.saved[&(private(44), "".into())].is_empty());
    assert_eq!(
        state.saved[&(private(43), "".into())][0].description,
        translate(Language::Russian, Key::MenuStart, &[])
    );
    for user in [43, 44] {
        for locale in ["en", "ru"] {
            assert!(!state.saved.contains_key(&(private(user), locale.into())));
        }
    }
}

#[tokio::test]
async fn held_external_request_does_not_block_commit_and_newer_state_wins() {
    let (_dir, identities) = setup().await;
    let api = Fake::default();
    let mut menus = worker(api.clone(), identities.clone());
    menus.initialize().await;
    api.state.lock().unwrap().hold = true;
    identities
        .update_settings(43, Some("ru"), None)
        .await
        .unwrap();
    let task = tokio::spawn(menus.run());
    tokio::time::timeout(Duration::from_secs(2), api.held.notified())
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        identities.admin_update_user(
            42,
            43,
            &AdminUserPatch {
                role: Some(Role::Restricted),
                language: Some("en".into()),
                ..Default::default()
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    api.release.notify_one();
    until(&api, |saved| {
        saved
            .get(&(private(43), "".into()))
            .is_some_and(Vec::is_empty)
    })
    .await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn failed_applied_update_is_retried_without_another_notification() {
    let (_dir, identities) = setup().await;
    let api = Fake::default();
    let mut menus = worker(api.clone(), identities.clone());
    menus.initialize().await;
    api.state.lock().unwrap().fail = true;
    identities
        .update_settings(43, Some("ru"), None)
        .await
        .unwrap();
    let task = tokio::spawn(menus.run());
    until(&api, |saved| {
        saved
            .get(&(private(43), "".into()))
            .is_some_and(|commands| {
                commands[0].description == translate(Language::Russian, Key::MenuStart, &[])
            })
    })
    .await;
    // Hold the retry itself to prove the worker did not merely apply then forget
    // the failed update. No mutation or reconciliation timer supplies this retry.
    tokio::time::timeout(Duration::from_secs(2), api.held.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}
use yoink_identity::GroupContext;

#[tokio::test]
async fn current_roles_and_feature_grants_do_not_invent_or_elevate_handlers() {
    let (_dir, identities) = setup().await;
    identities
        .admin_grant_permission(42, 43, "dl", "inline", None)
        .await
        .unwrap();
    let mut snapshot = identities.snapshot(43, None).await.unwrap().unwrap();
    assert!(!snapshot.grants.is_empty());
    let ordinary = names(&catalog(
        eligibility(&snapshot, 42, true, false),
        Language::English,
    ))
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    for role in [Role::User, Role::Moderator, Role::Admin, Role::Owner] {
        snapshot.principal.role = role;
        assert_eq!(
            names(&catalog(
                eligibility(&snapshot, 42, true, false),
                Language::English
            )),
            ordinary
        );
    }
    for role in [Role::Banned, Role::Restricted] {
        snapshot.principal.role = role;
        assert!(catalog(eligibility(&snapshot, 42, true, false), Language::English).is_empty());
    }
    snapshot.principal.role = Role::User;
    snapshot.principal.ban_until = Some(snapshot.now + Duration::from_secs(1));
    assert!(catalog(eligibility(&snapshot, 42, true, false), Language::English).is_empty());
    snapshot.now += Duration::from_secs(1);
    assert_eq!(
        names(&catalog(
            eligibility(&snapshot, 42, true, false),
            Language::English
        )),
        ordinary
    );

    snapshot.principal.role = Role::Restricted;
    snapshot.group = Some(GroupContext {
        group_id: -100,
        enabled: true,
        role_override: None,
        auto_grant_role: Role::User,
        thread_allowed: false,
    });
    let group = catalog(eligibility(&snapshot, 42, true, true), Language::English);
    assert!(
        names(&group).contains(&"video"),
        "topic-sensitive execution remains gated; menu can describe allowed topics"
    );
    snapshot.group.as_mut().unwrap().role_override = Some(Role::Restricted);
    assert!(catalog(eligibility(&snapshot, 42, true, true), Language::English).is_empty());
    snapshot.principal.role = Role::Banned;
    snapshot.group.as_mut().unwrap().role_override = Some(Role::Owner);
    assert!(catalog(eligibility(&snapshot, 42, true, true), Language::English).is_empty());
}

#[tokio::test]
async fn group_deletion_revokes_member_scopes_using_only_retained_scope_ids() {
    use yoink_identity::admin::groups::{GroupCreate, GroupPatch};
    let (_dir, identities) = setup().await;
    identities
        .group_create(
            42,
            &GroupCreate {
                id: -100,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    identities
        .group_update(
            42,
            -100,
            &GroupPatch {
                enabled: Some(true),
                auto_grant_role: Some(Role::User),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let api = Fake::default();
    let mut menus = worker(api.clone(), identities.clone());
    menus.groups = true;
    menus.initialize().await;
    let scope = BotCommandScope::ChatMember {
        chat_id: Recipient::Id(ChatId(-100)),
        user_id: UserId(43),
    };
    assert!(
        names(&api.state.lock().unwrap().saved[&(scope.clone(), "".into())]).contains(&"video")
    );
    identities.group_delete(42, -100).await.unwrap();
    menus.pass().await.unwrap();
    assert!(api.state.lock().unwrap().saved[&(scope, "".into())].is_empty());
    assert!(
        menus.known_groups.is_empty(),
        "successful revocation releases scope metadata"
    );
}
