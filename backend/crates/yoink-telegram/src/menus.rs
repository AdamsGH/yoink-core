//! Native command menus. One serialized reconciler, bounded watch signal and
//! keyset-paged fresh identity reads; no retained roles, grants or locale snapshots.
#[cfg(test)]
#[path = "menus_tests.rs"]
mod tests;
use std::{sync::Arc, time::Duration};
use teloxide::{
    ApiError, RequestError,
    prelude::*,
    types::{BotCommand, BotCommandScope, Recipient, UserId},
};
use tokio::sync::watch;
use yoink_access::{AccessPolicy, AccessScope, Role};
use yoink_i18n::{Key, Language};
use yoink_identity::{IdentitySnapshot, IdentityStore, MenuContact};

use crate::translate;

/// Operational bounds for command-menu reconciliation. The composition root's
/// startup/shutdown deadlines also bound the initial pass and worker lifetime.
#[derive(Clone, Copy)]
pub struct MenuConfig {
    pub request_timeout: Duration,
    pub retry_delay: Duration,
    pub reconcile_interval: Duration,
}
impl Default for MenuConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(5),
            retry_delay: Duration::from_secs(1),
            reconcile_interval: Duration::from_secs(60),
        }
    }
}

impl MenuConfig {
    pub(crate) fn valid(self) -> bool {
        !self.request_timeout.is_zero()
            && self.request_timeout <= Duration::from_secs(300)
            && !self.retry_delay.is_zero()
            && self.retry_delay <= Duration::from_secs(300)
            && !self.reconcile_interval.is_zero()
            && self.reconcile_interval <= Duration::from_secs(3600)
    }
}

#[derive(Clone, Copy)]
struct Eligibility {
    music: bool,
    stats: bool,
    summary: bool,
    tldr: bool,
    ingest: bool,
    stars: bool,
    approved: bool,
    owner: bool,
    downloads: bool,
    group: bool,
}

const BASIC: &[(&str, Key)] = &[("start", Key::MenuStart), ("help", Key::MenuHelp)];
const MEDIA: &[(&str, Key)] = &[
    ("download", Key::MenuDownload),
    ("video", Key::MenuVideo),
    ("audio", Key::MenuAudio),
    ("image", Key::MenuImage),
    ("job", Key::MenuJob),
    ("cancel", Key::MenuCancel),
    ("nsfw", Key::MenuNsfw),
];
const EXTENDED: &[(&str, Key)] = &[
    ("playlist", Key::MenuPlaylist),
    ("cut", Key::MenuCut),
    ("subs", Key::MenuSubs),
];
const PRIVATE_CONTROLS: &[(&str, Key)] = &[
    ("list", Key::MenuList),
    ("settings", Key::MenuSettings),
    ("format", Key::MenuSettings),
    ("quality", Key::MenuQuality),
    ("cookie", Key::MenuCookie),
    ("proxy", Key::MenuProxy),
];
const OWNER_CONTROLS: &[(&str, Key)] = &[("uncache", Key::MenuUncache)];

fn catalog(access: Eligibility, language: Language) -> Vec<BotCommand> {
    if !access.music
        && !access.stats
        && !access.approved
        && !access.ingest
        && !access.stars
        && !access.summary
        && !access.tldr
    {
        return vec![];
    }
    let mut entries = BASIC.to_vec();
    if access.music {
        entries.extend_from_slice(&[("music", Key::MenuMusic), ("song", Key::MenuSong)]);
    }
    if access.stats {
        entries.push(("stats", Key::MenuStats));
    }
    if access.summary {
        entries.extend_from_slice(&[("about", Key::MenuHelp), ("summary", Key::MenuHelp)]);
    }
    if access.tldr {
        entries.push(("tldr", Key::MenuHelp));
    }
    if (access.summary || access.tldr) && !(access.approved && access.downloads) {
        entries.push(("cancel", Key::MenuCancel));
    }
    if access.ingest {
        entries.extend_from_slice(&[("save", Key::MenuSave), ("inbox", Key::MenuInbox)]);
    }
    if access.stars {
        entries.extend_from_slice(&[
            ("stars", Key::MenuStars),
            ("stars_sync", Key::MenuStarsSync),
        ]);
    }
    if access.owner && !access.group {
        entries.push(("status", Key::MenuStatus));
    }
    if access.approved && access.downloads {
        if !access.group {
            entries.push(("lang", Key::MenuLang));
        }
        entries.extend_from_slice(MEDIA);
        entries.extend_from_slice(EXTENDED);
        if !access.group {
            entries.extend_from_slice(PRIVATE_CONTROLS);
            if access.owner {
                entries.extend_from_slice(OWNER_CONTROLS);
            }
        }
    }
    entries
        .into_iter()
        .map(|(command, key)| {
            let description = match (command, language == Language::Russian) {
                ("about", false) => "About a YouTube video",
                ("about", true) => "О видео YouTube",
                ("summary", false) => "Summarize a YouTube video",
                ("summary", true) => "Краткое содержание видео YouTube",
                ("tldr", false) => "Summarize a URL",
                ("tldr", true) => "Краткое содержание по ссылке",
                _ => return BotCommand::new(command, translate(language, key, &[])),
            };
            BotCommand::new(command, description)
        })
        .collect()
}

fn eligibility(
    snapshot: &IdentitySnapshot,
    owner: i64,
    downloads: bool,
    groups: bool,
) -> Eligibility {
    let group = snapshot.group.is_some();
    // Commands in a group may be usable only in enabled topics. Telegram cannot
    // scope a menu to a topic; execution still enforces the full thread policy.
    let policy = AccessPolicy {
        min_role: Role::User,
        scopes: vec![AccessScope::Private, AccessScope::Group],
        check_group_enabled: true,
        check_thread_policy: false,
    };
    Eligibility {
        music: false,
        stats: false,
        summary: false,
        tldr: false,
        ingest: false,
        stars: false,
        approved: snapshot.authorize(&policy, None).decision.allowed
            && (!group || (groups && downloads)),
        owner: snapshot.principal.user_id == owner,
        downloads,
        group,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Failure {
    Unreachable(UnavailableScope),
    Retry(Duration),
    RateLimited(Duration),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnavailableScope {
    Chat,
    Target,
}
fn unavailable_reason(error: &RequestError) -> Option<&'static str> {
    match error {
        RequestError::Api(ApiError::BotBlocked) => Some("bot_blocked"),
        RequestError::Api(ApiError::ChatNotFound) => Some("chat_not_found"),
        RequestError::Api(ApiError::BotKicked | ApiError::BotKickedFromSupergroup) => {
            Some("bot_kicked")
        }
        RequestError::Api(ApiError::GroupDeactivated) => Some("group_deactivated"),
        RequestError::Api(ApiError::UserDeactivated) => Some("user_deactivated"),
        RequestError::Api(ApiError::UserNotFound) => Some("user_not_found"),
        RequestError::Api(ApiError::Unknown(description))
            if description == "Bad Request: USER_ID_INVALID" =>
        {
            Some("user_id_invalid")
        }
        _ => None,
    }
}

fn classify(error: &RequestError, retry: Duration) -> Failure {
    if unavailable_reason(error).is_some() {
        return Failure::Unreachable(match error {
            RequestError::Api(
                ApiError::ChatNotFound
                | ApiError::BotKicked
                | ApiError::BotKickedFromSupergroup
                | ApiError::GroupDeactivated,
            ) => UnavailableScope::Chat,
            _ => UnavailableScope::Target,
        });
    }
    match error {
        RequestError::RetryAfter(seconds) => {
            Failure::RateLimited(Duration::from_secs(u64::from(seconds.seconds())).max(retry))
        }
        _ => Failure::Retry(retry),
    }
}

fn report_failure(
    error: &RequestError,
    retry: Duration,
    scope: &'static str,
    method: &'static str,
) -> Failure {
    let failure = classify(error, retry);
    if let Some(reason) = unavailable_reason(error) {
        // Persisted targets can block the bot or disappear between passes.
        tracing::debug!(
            reason,
            scope,
            method,
            "Telegram command menu target unavailable"
        );
    } else {
        let category = match failure {
            Failure::RateLimited(_) => "rate_limited",
            _ => "retryable",
        };
        tracing::warn!(
            category,
            scope,
            method,
            "Telegram command menu request failed"
        );
    }
    failure
}

pub(crate) trait MenuApi {
    fn apply(
        &self,
        scope: BotCommandScope,
        locale: &str,
        commands: Option<Vec<BotCommand>>,
    ) -> impl std::future::Future<Output = Result<(), Failure>> + Send;
}

pub(crate) struct ContactApi<A> {
    api: A,
    identities: Option<Arc<IdentityStore>>,
    source_id: i64,
    retry_delay: Duration,
}

impl<A: MenuApi + Sync> MenuApi for ContactApi<A> {
    async fn apply(
        &self,
        scope: BotCommandScope,
        locale: &str,
        commands: Option<Vec<BotCommand>>,
    ) -> Result<(), Failure> {
        let contact = match &scope {
            BotCommandScope::Chat {
                chat_id: Recipient::Id(chat_id),
            }
            | BotCommandScope::ChatAdministrators {
                chat_id: Recipient::Id(chat_id),
            } => Some(MenuContact {
                source_id: self.source_id,
                chat_id: chat_id.0,
                user_id: 0,
            }),
            BotCommandScope::ChatMember {
                chat_id: Recipient::Id(chat_id),
                user_id,
            } => Some(MenuContact {
                source_id: self.source_id,
                chat_id: chat_id.0,
                user_id: i64::try_from(user_id.0)
                    .map_err(|_| Failure::Unreachable(UnavailableScope::Target))?,
            }),
            _ => None,
        };
        let lease = if let (Some(store), Some(contact)) = (&self.identities, contact) {
            match store.begin_menu_contact(contact).await {
                Ok(Some(lease)) => Some(lease),
                Ok(None) => return Err(Failure::Unreachable(UnavailableScope::Target)),
                Err(_) => {
                    tracing::warn!(
                        category = "contact_state",
                        "Telegram command menu state unavailable"
                    );
                    return Err(Failure::Retry(self.retry_delay));
                }
            }
        } else {
            None
        };
        let result = self.api.apply(scope, locale, commands).await;
        if let (Some(store), Some(lease), Err(Failure::Unreachable(kind))) =
            (&self.identities, lease, result)
        {
            store
                .reject_menu_contact(lease, kind == UnavailableScope::Chat)
                .await
                .map_err(|_| {
                    tracing::warn!(
                        category = "contact_state",
                        "Telegram command menu invalidation failed"
                    );
                    Failure::Retry(self.retry_delay)
                })?;
        }
        result
    }
}

pub(crate) struct BotApi {
    bot: Bot,
    config: MenuConfig,
}
impl MenuApi for BotApi {
    async fn apply(
        &self,
        scope: BotCommandScope,
        locale: &str,
        commands: Option<Vec<BotCommand>>,
    ) -> Result<(), Failure> {
        let method = if commands.is_some() {
            "set_my_commands"
        } else {
            "delete_my_commands"
        };
        let scope_kind = match &scope {
            BotCommandScope::Chat { .. } => "chat",
            BotCommandScope::ChatMember { .. } => "chat_member",
            BotCommandScope::ChatAdministrators { .. } => "chat_administrators",
            _ => "global",
        };
        let request = async {
            match commands {
                Some(commands) => {
                    self.bot
                        .set_my_commands(commands)
                        .scope(scope)
                        .language_code(locale)
                        .await
                }
                None => {
                    self.bot
                        .delete_my_commands()
                        .scope(scope)
                        .language_code(locale)
                        .await
                }
            }
            .map(|_| ())
        };
        match tokio::time::timeout(self.config.request_timeout, request).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(report_failure(
                &error,
                self.config.retry_delay,
                scope_kind,
                method,
            )),
            Err(_) => {
                tracing::warn!(
                    category = "timeout",
                    scope = scope_kind,
                    method,
                    "Telegram command menu request failed"
                );
                Err(Failure::Retry(self.config.retry_delay))
            }
        }
    }
}

/// Language-specific chat scopes outrank the base chat scope. Always remove
/// both supported overrides, then install only the freshly persisted locale as
/// a base menu; it wins over localized all-private/all-group fallbacks.
async fn personalized(
    api: &impl MenuApi,
    scope: BotCommandScope,
    commands: Vec<BotCommand>,
) -> Result<(), Failure> {
    for locale in ["en", "ru"] {
        api.apply(scope.clone(), locale, None).await?;
    }
    api.apply(scope, "", Some(commands)).await
}

pub(crate) struct Menus<A = ContactApi<BotApi>> {
    api: A,
    config: MenuConfig,
    identities: Option<Arc<IdentityStore>>,
    changes: Option<watch::Receiver<()>>,
    owner: i64,
    source_id: i64,
    downloads: bool,
    groups: bool,
    // Scope identifiers only, never role/locale snapshots. Keep deleted groups
    // until revocation succeeds so removal cannot orphan personalized menus.
    known_groups: std::collections::BTreeSet<i64>,
    inbox: bool,
    insight: Option<yoink_insight::Insight>,
    music: bool,
    stats: bool,
}
impl Menus {
    pub(crate) fn new(
        bot: Bot,
        identities: Option<Arc<IdentityStore>>,
        owner: u64,
        source_id: i64,
        downloads: bool,
        groups: bool,
        config: MenuConfig,
    ) -> Self {
        let changes = identities.as_ref().map(|store| store.subscribe_changes());
        Self {
            api: ContactApi {
                api: BotApi { bot, config },
                identities: identities.clone(),
                source_id,
                retry_delay: config.retry_delay,
            },
            config,
            identities,
            changes,
            owner: owner as i64,
            source_id,
            downloads,
            groups,
            known_groups: Default::default(),
            inbox: false,
            insight: None,
            music: false,
            stats: false,
        }
    }
}
impl<A: MenuApi> Menus<A> {
    pub(crate) fn with_products(mut self, music: bool, stats: bool) -> Self {
        self.music = music;
        self.stats = stats;
        self
    }
    pub(crate) fn with_insight(mut self, insight: Option<yoink_insight::Insight>) -> Self {
        self.insight = insight;
        self
    }
    pub(crate) fn with_inbox(mut self, enabled: bool) -> Self {
        self.inbox = enabled;
        self
    }
    fn inbox_eligibility(&self, snapshot: &IdentitySnapshot) -> Eligibility {
        let mut access = eligibility(snapshot, self.owner, self.downloads, self.groups);
        let policy = AccessPolicy {
            min_role: Role::Restricted,
            scopes: vec![AccessScope::Private, AccessScope::Group],
            check_group_enabled: true,
            check_thread_policy: false,
        };
        if !access.group || self.groups {
            access.music = self.music
                && snapshot
                    .authorize(&policy, Some(("music", "inline")))
                    .decision
                    .allowed;
            let stats_policy = AccessPolicy {
                min_role: Role::User,
                ..policy.clone()
            };
            access.stats = self.stats && snapshot.authorize(&stats_policy, None).decision.allowed;
        }
        if self.inbox && (!access.group || self.groups) {
            access.ingest = snapshot
                .authorize(&policy, Some(("inbox", "ingest")))
                .decision
                .allowed;
            access.stars = snapshot
                .authorize(&policy, Some(("inbox", "gh_sync")))
                .decision
                .allowed;
        }
        access
    }
    async fn defaults(&self) -> Result<(), Failure> {
        for locale in ["", "en", "ru"] {
            for scope in [
                BotCommandScope::Default,
                BotCommandScope::AllChatAdministrators,
            ] {
                self.api.apply(scope, locale, None).await?;
            }
            let language = yoink_i18n::language_for(Some(locale));
            let commands = catalog(
                Eligibility {
                    music: false,
                    stats: false,
                    summary: false,
                    tldr: false,
                    ingest: false,
                    stars: false,
                    approved: self.identities.is_some(),
                    owner: false,
                    downloads: false,
                    group: false,
                },
                language,
            );
            self.api
                .apply(BotCommandScope::AllPrivateChats, locale, Some(commands))
                .await?;
            // No universally authorized group actor. Personalized member scopes
            // below advertise only policy-allowed commands, including auto-grant.
            self.api
                .apply(BotCommandScope::AllGroupChats, locale, Some(vec![]))
                .await?;
        }
        Ok(())
    }

    async fn target(&self, user: i64, chat: i64) -> Result<(), Failure> {
        let group = chat < 0;
        if group
            && let Some(store) = &self.identities
            && !store
                .membership_eligible(self.source_id, chat, user)
                .await
                .map_err(|_| Failure::Retry(self.config.retry_delay))?
        {
            return personalized(
                &self.api,
                BotCommandScope::ChatMember {
                    chat_id: Recipient::Id(ChatId(chat)),
                    user_id: UserId(user as u64),
                },
                vec![],
            )
            .await;
        }
        let (mut access, language) = match &self.identities {
            Some(store) => match store.snapshot(user, group.then_some((chat, None))).await {
                Ok(Some(snapshot)) => (
                    self.inbox_eligibility(&snapshot),
                    crate::lang::resolve_language(Some(&snapshot.language), None),
                ),
                Ok(None) => (
                    Eligibility {
                        music: false,
                        stats: false,
                        summary: false,
                        tldr: false,
                        ingest: false,
                        stars: false,
                        approved: user == self.owner && !group,
                        owner: user == self.owner,
                        downloads: false,
                        group,
                    },
                    Language::English,
                ),
                Err(_) => {
                    tracing::warn!(
                        category = "identity",
                        "Telegram command menu state unavailable"
                    );
                    return Err(Failure::Retry(self.config.retry_delay));
                }
            },
            None => (
                Eligibility {
                    music: false,
                    stats: false,
                    summary: false,
                    tldr: false,
                    ingest: false,
                    stars: false,
                    approved: user == self.owner && !group,
                    owner: user == self.owner,
                    downloads: false,
                    group,
                },
                Language::English,
            ),
        };
        if let Some(insight) = &self.insight
            && let Some(store) = &self.identities
            && let Ok(Some(snapshot)) = store.snapshot(user, group.then_some((chat, None))).await
        {
            let policy = AccessPolicy {
                min_role: Role::Restricted,
                scopes: vec![AccessScope::Private, AccessScope::Group],
                check_group_enabled: true,
                check_thread_policy: false,
            };
            if (!group || self.groups) && snapshot.authorize(&policy, None).decision.allowed {
                match insight.settings(user).await {
                    Ok(settings) => {
                        access.summary = settings["has_gemini_access"] == true;
                        access.tldr = settings["has_tldr_access"] == true;
                    }
                    Err(_) => {
                        access.summary = false;
                        access.tldr = false;
                    }
                }
            }
        }
        let scope = if group {
            BotCommandScope::ChatMember {
                chat_id: Recipient::Id(ChatId(chat)),
                user_id: UserId(user as u64),
            }
        } else {
            BotCommandScope::Chat {
                chat_id: Recipient::Id(ChatId(chat)),
            }
        };
        personalized(&self.api, scope, catalog(access, language)).await
    }

    async fn revoke_group(&self, chat: i64) -> Result<(), Failure> {
        for scope in [
            BotCommandScope::Chat {
                chat_id: Recipient::Id(ChatId(chat)),
            },
            BotCommandScope::ChatAdministrators {
                chat_id: Recipient::Id(ChatId(chat)),
            },
        ] {
            for locale in ["", "en", "ru"] {
                self.api.apply(scope.clone(), locale, None).await?;
            }
        }
        if let Some(store) = &self.identities {
            let mut after = (0, i64::MIN);
            loop {
                let targets = store
                    .access_targets_after(after)
                    .await
                    .map_err(|_| Failure::Retry(self.config.retry_delay))?;
                if targets.is_empty() {
                    break;
                }
                for (user, target_chat) in targets {
                    after = (user, target_chat);
                    if target_chat <= 0 {
                        continue;
                    }
                    let scope = BotCommandScope::ChatMember {
                        chat_id: Recipient::Id(ChatId(chat)),
                        user_id: UserId(user as u64),
                    };
                    match personalized(&self.api, scope, vec![]).await {
                        Err(Failure::Unreachable(_)) => {}
                        result => result?,
                    }
                }
            }
        }
        Ok(())
    }

    // A failed pass remains dirty. Later passes reload every state rather than
    // retaining an obsolete payload. Continue past unreachable/failed users so
    // a blocked account cannot starve subsequent users' updates.
    async fn pass(&mut self) -> Result<(), Duration> {
        let mut retry = None;
        let mut record = |result: Result<(), Failure>| {
            if let Err(Failure::Retry(delay)) = result {
                retry = Some(retry.unwrap_or(Duration::ZERO).max(delay));
            }
        };
        let result = self.target(self.owner, self.owner).await;
        if let Err(Failure::RateLimited(delay)) = result {
            return Err(delay);
        }
        record(result);
        if let Some(store) = &self.identities {
            let mut current_groups = std::collections::BTreeSet::new();
            let mut after_group = i64::MIN;
            loop {
                let groups = store
                    .access_groups_after(after_group)
                    .await
                    .map_err(|_| self.config.retry_delay)?;
                if groups.is_empty() {
                    break;
                }
                for chat in groups {
                    after_group = chat;
                    current_groups.insert(chat);
                    self.known_groups.insert(chat);
                    // Legacy group chat/admin overrides outrank global fallbacks.
                    for scope in [
                        BotCommandScope::Chat {
                            chat_id: Recipient::Id(ChatId(chat)),
                        },
                        BotCommandScope::ChatAdministrators {
                            chat_id: Recipient::Id(ChatId(chat)),
                        },
                    ] {
                        for locale in ["", "en", "ru"] {
                            let result = self.api.apply(scope.clone(), locale, None).await;
                            if let Err(Failure::RateLimited(delay)) = result {
                                return Err(delay);
                            }
                            record(result);
                        }
                    }
                }
            }
            let removed: Vec<_> = self
                .known_groups
                .difference(&current_groups)
                .copied()
                .collect();
            for chat in removed {
                let result = self.revoke_group(chat).await;
                if let Err(Failure::RateLimited(delay)) = result {
                    return Err(delay);
                }
                if matches!(result, Ok(()) | Err(Failure::Unreachable(_))) {
                    self.known_groups.remove(&chat);
                }
                record(result);
            }
            let mut after = (0, i64::MIN);
            loop {
                let targets = store
                    .access_targets_after(after)
                    .await
                    .map_err(|_| self.config.retry_delay)?;
                if targets.is_empty() {
                    break;
                }
                for (user, chat) in targets {
                    after = (user, chat);
                    if user == self.owner && chat == self.owner {
                        continue;
                    }
                    let result = self.target(user, chat).await;
                    if let Err(Failure::RateLimited(delay)) = result {
                        return Err(delay);
                    }
                    record(result);
                }
            }
        }
        retry.map_or(Ok(()), Err)
    }

    /// Initial reconciliation is part of startup readiness and externally
    /// deadline-bounded. Watch subscription precedes the first state read.
    pub(crate) async fn initialize(&mut self) {
        loop {
            if let Some(changes) = &mut self.changes {
                changes.borrow_and_update();
            }
            let result = match self.defaults().await {
                Ok(()) => self.pass().await,
                Err(Failure::Retry(delay) | Failure::RateLimited(delay)) => Err(delay),
                Err(Failure::Unreachable(_)) => Err(self.config.retry_delay),
            };
            match result {
                Err(delay) => tokio::time::sleep(delay).await,
                Ok(()) if self.changed() => continue,
                Ok(()) => break,
            }
        }
    }
    fn changed(&self) -> bool {
        self.changes
            .as_ref()
            .is_some_and(|changes| changes.has_changed().unwrap_or(false))
    }

    /// Dropping this future cancels pending requests/timers, so shutdown cannot
    /// leave a detached worker running after the adapter exits.
    pub(crate) async fn run(mut self) {
        loop {
            if !self.changed() {
                tokio::select! {
                    _ = tokio::time::sleep(self.config.reconcile_interval) => {},
                    _ = async { match &mut self.changes { Some(changes) => { let _ = changes.changed().await; }, None => std::future::pending::<()>().await } } => {},
                }
            }
            if let Some(changes) = &mut self.changes {
                changes.borrow_and_update();
            }
            if let Err(delay) = self.pass().await {
                // Preserve a dirty retry independently of whether watch has a
                // newer event. Do not let changes bypass Telegram retry_after.
                tokio::time::sleep(delay).await;
                loop {
                    if let Some(changes) = &mut self.changes {
                        changes.borrow_and_update();
                    }
                    match self.pass().await {
                        Ok(()) => break,
                        Err(delay) => tokio::time::sleep(delay).await,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod classification_tests {
    use super::*;

    #[test]
    fn unavailable_menu_user_does_not_retry_the_entire_pass() {
        let retry = Duration::from_secs(1);
        for description in [
            "Bad Request: USER_ID_INVALID",
            "Bad Request: user not found",
        ] {
            let error = serde_json::from_value::<ApiError>(serde_json::json!(description)).unwrap();
            assert_eq!(
                classify(&RequestError::Api(error), retry),
                Failure::Unreachable(UnavailableScope::Target)
            );
        }
        let error = RequestError::Api(ApiError::Unknown("Bad Request: BOT_COMMAND_INVALID".into()));
        assert_eq!(classify(&error, retry), Failure::Retry(retry));
    }
}
