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
        music: false,
        stats: false,
        summary: false,
        tldr: false,
        ingest: false,
        stars: false,
        approved: true,
        owner: false,
        downloads: true,
        group: false,
    };
    let expected = [
        "start", "help", "lang", "download", "video", "audio", "image", "job", "cancel", "nsfw",
        "playlist", "cut", "subs", "list", "settings", "format", "quality", "cookie", "proxy",
    ];
    for language in [Language::English, Language::Russian] {
        assert_eq!(names(&catalog(ordinary, language)), expected);
        let owner = catalog(
            Eligibility {
                music: false,
                stats: false,
                summary: false,
                tldr: false,
                ingest: false,
                stars: false,
                owner: true,
                ..ordinary
            },
            language,
        );
        assert_eq!(owner.len(), 21);
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
        music: false,
        stats: false,
        summary: false,
        tldr: false,
        ingest: false,
        stars: false,
        approved: true,
        owner: false,
        downloads: false,
        group: false,
    };
    assert_eq!(names(&catalog(basic, Language::English)), ["start", "help"]);
    assert_eq!(
        names(&catalog(
            Eligibility {
                music: false,
                stats: false,
                summary: false,
                tldr: false,
                ingest: false,
                stars: false,
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
                music: false,
                stats: false,
                summary: false,
                tldr: false,
                ingest: false,
                stars: false,
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
            music: false,
            stats: false,
            summary: false,
            tldr: false,
            ingest: false,
            stars: false,
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
        Failure::Unreachable(UnavailableScope::Target)
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

#[test]
fn unavailable_targets_are_debug_only_and_real_failures_remain_redacted_warnings() {
    use std::fmt::Write as _;
    use tracing::{Event, Level, Metadata, Subscriber, field::Visit, span};

    type Events = Arc<Mutex<Vec<(Level, String)>>>;
    struct Capture(Events);
    struct Fields(String);
    impl Visit for Fields {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            write!(&mut self.0, " {}={value:?}", field.name()).unwrap();
        }
    }
    impl Subscriber for Capture {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }
        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            self.0
                .lock()
                .unwrap()
                .push((*event.metadata().level(), fields.0));
        }
        fn enter(&self, _: &span::Id) {}
        fn exit(&self, _: &span::Id) {}
    }

    let events = Events::default();
    let retry = Duration::from_secs(1);
    let expected = [
        (ApiError::BotBlocked, "bot_blocked"),
        (ApiError::ChatNotFound, "chat_not_found"),
        (ApiError::BotKicked, "bot_kicked"),
        (ApiError::BotKickedFromSupergroup, "bot_kicked"),
        (ApiError::GroupDeactivated, "group_deactivated"),
        (ApiError::UserDeactivated, "user_deactivated"),
        (ApiError::UserNotFound, "user_not_found"),
        (
            ApiError::Unknown("Bad Request: USER_ID_INVALID".into()),
            "user_id_invalid",
        ),
    ];
    tracing::subscriber::with_default(Capture(events.clone()), || {
        for (error, _) in &expected {
            assert_eq!(
                report_failure(
                    &RequestError::Api(error.clone()),
                    retry,
                    "chat_member",
                    "delete_my_commands"
                ),
                Failure::Unreachable(
                    if matches!(
                        error,
                        ApiError::ChatNotFound
                            | ApiError::BotKicked
                            | ApiError::BotKickedFromSupergroup
                            | ApiError::GroupDeactivated
                    ) {
                        UnavailableScope::Chat
                    } else {
                        UnavailableScope::Target
                    }
                )
            );
        }
        report_failure(
            &RequestError::Api(ApiError::Unknown("token/SECRET".into())),
            retry,
            "global",
            "set_my_commands",
        );
        report_failure(
            &RequestError::RetryAfter(teloxide::types::Seconds::from_seconds(9)),
            retry,
            "chat",
            "delete_my_commands",
        );
    });
    let events = events.lock().unwrap();
    assert_eq!(events.len(), expected.len() + 2);
    for ((level, fields), (_, reason)) in events.iter().zip(&expected) {
        assert_eq!(*level, Level::DEBUG);
        assert!(fields.contains(reason));
        assert!(fields.contains("chat_member"));
        assert!(fields.contains("delete_my_commands"));
    }
    for (level, fields) in &events[expected.len()..] {
        assert_eq!(*level, Level::WARN);
        assert!(!fields.contains("SECRET"));
    }
    assert!(events[expected.len()].1.contains("retryable"));
    assert!(events[expected.len() + 1].1.contains("rate_limited"));
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
            music: false,
            stats: false,
            summary: false,
            tldr: false,
            ingest: false,
            stars: false,
            approved: true,
            owner: false,
            downloads: true,
            group: false,
        },
        Language::Russian,
    );
    let cleaned = CleanedApi::new(api.clone());
    personalized(&cleaned, private(43), ru.clone())
        .await
        .unwrap();
    personalized(&cleaned, private(43), ru.clone())
        .await
        .unwrap();
    let state = api.state.lock().unwrap();
    assert!(!state.saved.contains_key(&(private(43), "en".into())));
    assert!(!state.saved.contains_key(&(private(43), "ru".into())));
    assert_eq!(state.saved[&(private(43), "".into())], ru);
}

#[derive(Clone, Default)]
struct ContactTransport {
    calls: Arc<Mutex<Vec<BotCommandScope>>>,
    failures: Arc<Mutex<HashMap<BotCommandScope, Failure>>>,
}

impl MenuApi for ContactTransport {
    async fn apply(
        &self,
        scope: BotCommandScope,
        _: &str,
        _: Option<Vec<BotCommand>>,
    ) -> Result<(), Failure> {
        self.calls.lock().unwrap().push(scope.clone());
        self.failures
            .lock()
            .unwrap()
            .get(&scope)
            .copied()
            .map_or(Ok(()), Err)
    }
}

#[tokio::test]
async fn successful_cleanup_is_reused_but_sets_invalidate_it() {
    let transport = ContactTransport::default();
    let api = CleanedApi::new(transport.clone());
    personalized(&api, private(43), vec![]).await.unwrap();
    personalized(&api, private(43), vec![]).await.unwrap();
    assert_eq!(transport.calls.lock().unwrap().len(), 4);
    api.apply(private(43), "en", Some(vec![])).await.unwrap();
    personalized(&api, private(43), vec![]).await.unwrap();
    assert_eq!(transport.calls.lock().unwrap().len(), 7);
    personalized(&api, private(44), vec![]).await.unwrap();
    assert_eq!(transport.calls.lock().unwrap().len(), 10);
    let restarted = CleanedApi::new(transport.clone());
    personalized(&restarted, private(43), vec![]).await.unwrap();
    assert_eq!(transport.calls.lock().unwrap().len(), 13);
}

#[tokio::test]
async fn cleanup_cache_overflow_keeps_cleanup_active() {
    let transport = ContactTransport::default();
    let api = CleanedApi::new(transport.clone());
    for user in 1..=MAX_CLEANED_OVERRIDES as i64 {
        api.apply(private(user), "ru", None).await.unwrap();
    }
    let overflow = private(MAX_CLEANED_OVERRIDES as i64 + 1);
    api.apply(overflow.clone(), "ru", None).await.unwrap();
    api.apply(overflow, "ru", None).await.unwrap();
    assert_eq!(api.deleted.lock().unwrap().len(), MAX_CLEANED_OVERRIDES);
    assert_eq!(
        transport.calls.lock().unwrap().len(),
        MAX_CLEANED_OVERRIDES + 2
    );
}

#[tokio::test]
async fn failed_cleanup_and_uncertain_sets_remain_retryable() {
    let transport = ContactTransport::default();
    let api = CleanedApi::new(transport.clone());
    let failure = Failure::Retry(Duration::from_millis(1));
    transport
        .failures
        .lock()
        .unwrap()
        .insert(private(43), failure);
    assert_eq!(api.apply(private(43), "ru", None).await, Err(failure));
    transport.failures.lock().unwrap().clear();
    api.apply(private(43), "ru", None).await.unwrap();
    api.apply(private(43), "ru", None).await.unwrap();
    assert_eq!(transport.calls.lock().unwrap().len(), 2);
    transport
        .failures
        .lock()
        .unwrap()
        .insert(private(43), failure);
    assert_eq!(
        api.apply(private(43), "ru", Some(vec![])).await,
        Err(failure)
    );
    transport.failures.lock().unwrap().clear();
    api.apply(private(43), "ru", None).await.unwrap();
    assert_eq!(transport.calls.lock().unwrap().len(), 4);
}

fn guarded(
    api: ContactTransport,
    store: Arc<IdentityStore>,
    source_id: i64,
) -> ContactApi<ContactTransport> {
    ContactApi {
        api,
        identities: Some(store),
        source_id,
        retry_delay: Duration::from_millis(1),
    }
}

#[tokio::test]
async fn menu_contact_guard_stops_api_calls_across_passes_and_worker_recreation() {
    use yoink_identity::{MenuContact, MenuContactObservation};
    let (_dir, store) = setup().await;
    let transport = ContactTransport::default();
    transport
        .failures
        .lock()
        .unwrap()
        .insert(private(43), Failure::Unreachable(UnavailableScope::Chat));
    let api = guarded(transport.clone(), store.clone(), 123456);
    for _ in 0..3 {
        assert!(personalized(&api, private(43), vec![]).await.is_err());
    }
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
    let api = guarded(transport.clone(), store.clone(), 123456);
    assert!(personalized(&api, private(43), vec![]).await.is_err());
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
    let other_bot = guarded(transport.clone(), store.clone(), 654321);
    assert!(personalized(&other_bot, private(43), vec![]).await.is_err());
    assert_eq!(transport.calls.lock().unwrap().len(), 2);
    transport.failures.lock().unwrap().clear();
    store
        .restore_menu_contact(
            MenuContact {
                source_id: 123456,
                chat_id: 43,
                user_id: 0,
            },
            MenuContactObservation {
                occurred_at: 100,
                update_id: 1,
            },
        )
        .await
        .unwrap();
    personalized(&api, private(43), vec![]).await.unwrap();
    assert_eq!(transport.calls.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn menu_contact_guard_separates_member_rejections_from_missing_chats() {
    let (_dir, store) = setup().await;
    let transport = ContactTransport::default();
    let api = guarded(transport.clone(), store, 123456);
    let member = |user| BotCommandScope::ChatMember {
        chat_id: Recipient::Id(ChatId(-100)),
        user_id: UserId(user),
    };
    transport
        .failures
        .lock()
        .unwrap()
        .insert(member(43), Failure::Unreachable(UnavailableScope::Target));
    assert!(api.apply(member(43), "en", None).await.is_err());
    assert!(api.apply(member(43), "ru", None).await.is_err());
    api.apply(member(44), "en", None).await.unwrap();
    assert_eq!(transport.calls.lock().unwrap().len(), 2);
    transport
        .failures
        .lock()
        .unwrap()
        .insert(member(44), Failure::Unreachable(UnavailableScope::Chat));
    assert!(api.apply(member(44), "ru", None).await.is_err());
    assert!(api.apply(member(45), "en", None).await.is_err());
    assert!(
        api.apply(
            BotCommandScope::ChatAdministrators {
                chat_id: Recipient::Id(ChatId(-100))
            },
            "ru",
            None
        )
        .await
        .is_err()
    );
    assert_eq!(transport.calls.lock().unwrap().len(), 3);
    api.apply(BotCommandScope::AllPrivateChats, "en", Some(vec![]))
        .await
        .unwrap();
    assert_eq!(transport.calls.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn menu_contact_guard_keeps_transient_and_rate_limited_targets_retryable() {
    let (_dir, store) = setup().await;
    let transport = ContactTransport::default();
    let api = guarded(transport.clone(), store, 123456);
    for failure in [
        Failure::Retry(Duration::from_secs(1)),
        Failure::RateLimited(Duration::from_secs(9)),
    ] {
        transport
            .failures
            .lock()
            .unwrap()
            .insert(private(43), failure);
        assert_eq!(api.apply(private(43), "en", None).await, Err(failure));
        assert_eq!(api.apply(private(43), "ru", None).await, Err(failure));
    }
    assert_eq!(transport.calls.lock().unwrap().len(), 4);
}

async fn setup() -> (tempfile::TempDir, Arc<IdentityStore>) {
    let (dir, identities, _) = setup_store(false).await;
    (dir, identities)
}

#[tokio::test]
async fn product_menus_refresh_grants_and_bans_without_downloader_authority() {
    let (_dir, identities) = setup().await;
    let api = Fake::default();
    let mut menus = worker(api.clone(), identities.clone()).with_products(true, true);
    menus.downloads = false;
    identities
        .admin_grant_permission(42, 43, "music", "inline", None)
        .await
        .unwrap();
    menus.target(43, 43).await.unwrap();
    let entries = api.state.lock().unwrap().saved[&(private(43), "".into())].clone();
    assert!(names(&entries).contains(&"music"));
    assert!(names(&entries).contains(&"song"));
    assert!(!names(&entries).contains(&"download"));
    let snapshot = identities.snapshot(43, None).await.unwrap().unwrap();
    let mut restricted = snapshot.clone();
    restricted.principal.role = Role::Restricted;
    let granted = catalog(menus.inbox_eligibility(&restricted), Language::Russian);
    assert!(names(&granted).contains(&"music"));
    assert!(!names(&granted).contains(&"stats"));
    assert_eq!(
        granted
            .iter()
            .find(|c| c.command == "music")
            .unwrap()
            .description,
        translate(Language::Russian, Key::MenuMusic, &[])
    );
    identities
        .admin_update_user(
            42,
            43,
            &AdminUserPatch {
                role: Some(Role::User),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    menus.target(43, 43).await.unwrap();
    assert!(names(&api.state.lock().unwrap().saved[&(private(43), "".into())]).contains(&"stats"));
    identities
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
    menus.target(43, 43).await.unwrap();
    assert!(api.state.lock().unwrap().saved[&(private(43), "".into())].is_empty());
    let disabled = worker(api.clone(), identities);
    disabled.target(42, 42).await.unwrap();
    let state = api.state.lock().unwrap();
    assert!(!names(&state.saved[&(private(42), "".into())]).contains(&"music"));
    assert!(!names(&state.saved[&(private(42), "".into())]).contains(&"stats"));
}
async fn setup_store(
    insight: bool,
) -> (tempfile::TempDir, Arc<IdentityStore>, yoink_storage::Store) {
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
        insight,
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
    (dir, identities, store)
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
        source_id: 123456,
        downloads: true,
        groups: false,
        known_groups: Default::default(),
        inbox: false,
        insight: None,
        music: false,
        stats: false,
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
        menus.known_groups.contains(&-100),
        "durable deletion targets retain cleanup scope across restarts"
    );
}

#[tokio::test]
async fn inbox_menus_are_feature_aware_localized_and_absent_without_transport() {
    let (_dir, identities) = setup().await;
    identities
        .admin_grant_permission(42, 43, "inbox", "gh_sync", None)
        .await
        .unwrap();
    let snapshot = identities.snapshot(43, None).await.unwrap().unwrap();
    let menus = worker(Fake::default(), identities.clone()).with_inbox(true);
    let mut snapshot = snapshot;
    // A restricted principal's explicit grants do not turn on unrelated media.
    snapshot.principal.role = Role::Restricted;
    let entries = catalog(menus.inbox_eligibility(&snapshot), Language::Russian);
    assert!(names(&entries).contains(&"stars"));
    assert!(names(&entries).contains(&"stars_sync"));
    assert!(!names(&entries).contains(&"save"));
    assert!(!names(&entries).contains(&"download"));
    assert!(
        entries
            .iter()
            .find(|c| c.command == "stars")
            .unwrap()
            .description
            .contains("GitHub")
    );
    let unavailable = worker(Fake::default(), identities.clone());
    assert!(catalog(unavailable.inbox_eligibility(&snapshot), Language::English).is_empty());
    snapshot.principal.role = Role::User;
    assert!(
        names(&catalog(
            menus.inbox_eligibility(&snapshot),
            Language::English
        ))
        .contains(&"save")
    );
    snapshot.principal.role = Role::Banned;
    assert!(catalog(menus.inbox_eligibility(&snapshot), Language::English).is_empty());
}

#[tokio::test]
async fn shared_insight_menu_reads_current_grants_toggle_readiness_and_ban() {
    let (_dir, identities, db) = setup_store(true).await;
    let service = yoink_insight::Insight::completion_only(
        db.database().clone(),
        yoink_insight::Config {
            owner_id: Some(42),
            model: "fixture".into(),
            ..Default::default()
        },
        Arc::new(
            yoink_insight::provider::HttpCompletion::new(Duration::from_secs(2), 4096).unwrap(),
        ),
    )
    .unwrap();
    let api = Fake::default();
    let menus = worker(api.clone(), identities.clone()).with_insight(Some(service.clone()));
    menus.target(43, 43).await.unwrap();
    assert!(!names(&api.state.lock().unwrap().saved[&(private(43), "".into())]).contains(&"tldr"));
    identities
        .admin_grant_permission(42, 43, "insight", "summary", None)
        .await
        .unwrap();
    identities
        .admin_grant_permission(42, 43, "insight", "tldr", None)
        .await
        .unwrap();
    menus.target(43, 43).await.unwrap();
    let commands = api.state.lock().unwrap().saved[&(private(43), "".into())].clone();
    for name in ["about", "summary", "tldr", "cancel"] {
        assert!(names(&commands).contains(&name));
    }
    identities
        .admin_revoke_permission(42, 43, "insight", "tldr")
        .await
        .unwrap();
    menus.target(43, 43).await.unwrap();
    assert!(!names(&api.state.lock().unwrap().saved[&(private(43), "".into())]).contains(&"tldr"));
    // Persisted provider grant is recognized only through product settings,
    // never a second resolver in Telegram.
    yoink_db::query("INSERT INTO insight_user_byok(user_id,provider,api_key,model,tested_at,test_error,created_at,updated_at) VALUES(43,'openai','fixture-key','fixture',1,NULL,1,1)").execute(db.database()).await.unwrap();
    yoink_db::query("INSERT INTO bot_settings(key,value,updated_at) VALUES('insight_byok_enabled','true',1) ON CONFLICT(key) DO UPDATE SET value=excluded.value").execute(db.database()).await.unwrap();
    menus.target(43, 43).await.unwrap();
    assert!(names(&api.state.lock().unwrap().saved[&(private(43), "".into())]).contains(&"tldr"));
    yoink_db::query(
        "UPDATE insight_user_byok SET test_error='provider_probe_failed' WHERE user_id=43",
    )
    .execute(db.database())
    .await
    .unwrap();
    menus.target(43, 43).await.unwrap();
    assert!(!names(&api.state.lock().unwrap().saved[&(private(43), "".into())]).contains(&"tldr"));
    identities
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
    menus.target(43, 43).await.unwrap();
    assert!(api.state.lock().unwrap().saved[&(private(43), "".into())].is_empty());
    let disabled = worker(api.clone(), identities);
    disabled.target(42, 42).await.unwrap();
    assert!(!names(&api.state.lock().unwrap().saved[&(private(42), "".into())]).contains(&"tldr"));
}
