use crate::{MessageDependencies, MusicRuntime, StatsRuntime, TelegramIdentity, UpdateObserver};
use std::{sync::Arc, time::Duration};
use teloxide::{prelude::*, types::Message};
use tokio_util::sync::CancellationToken;
use yoink_access::{AccessPolicy, AccessScope, Role};
use yoink_i18n::Key;

/// Retain at the composition root and drain after adapter abort/join, before DB close.
#[derive(Clone)]
pub struct TelegramHandlerLifecycle {
    pub(crate) handlers: crate::insight_lifecycle::Handlers,
    pub(crate) stop: CancellationToken,
}
impl TelegramHandlerLifecycle {
    pub(crate) fn new() -> Self {
        Self {
            handlers: crate::insight_lifecycle::Handlers::new(),
            stop: CancellationToken::new(),
        }
    }
    pub fn stop_admission(&self) {
        self.handlers.close();
    }
    pub fn cancel_all(&self) {
        self.stop_admission();
        self.stop.cancel();
    }
    pub async fn drain_handlers(&self) {
        self.cancel_all();
        self.handlers.drain().await;
    }
}

pub(crate) struct ProductStop(
    pub Option<(MusicRuntime, Duration)>,
    pub Option<StatsRuntime>,
);
impl ProductStop {
    pub(crate) fn cancel(&self) {
        if let Some((music, _)) = &self.0 {
            music.cancel_all();
        }
        if let Some(stats) = &self.1 {
            stats.stop_admission();
        }
    }
}
impl Drop for ProductStop {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub(crate) fn compose_observers(
    existing: Option<UpdateObserver>,
    stats: Option<UpdateObserver>,
) -> Option<UpdateObserver> {
    match (existing, stats) {
        (None, observer) | (observer, None) => observer,
        (Some(existing), Some(stats)) => Some(Arc::new(move |raw| {
            let existing = existing.clone();
            let stats = stats.clone();
            Box::pin(async move {
                existing(raw.clone()).await?;
                stats(raw).await
            })
        })),
    }
}

pub(crate) fn command<'a>(text: &'a str, username: &str, name: &str) -> Option<Vec<&'a str>> {
    let mut words = text.split_whitespace();
    let head = words.next()?.strip_prefix('/')?;
    let (head, suffix) = head
        .split_once('@')
        .map_or((head, None), |(h, s)| (h, Some(s)));
    (head.eq_ignore_ascii_case(name) && suffix.is_none_or(|s| s.eq_ignore_ascii_case(username)))
        .then(|| words.collect())
}

pub(crate) async fn music_allowed(
    deps: &MessageDependencies,
    actor: i64,
    group: Option<(i64, Option<i64>)>,
) -> bool {
    let Some(store) = &deps.identities else {
        return false;
    };
    let policy = AccessPolicy {
        min_role: Role::Restricted,
        scopes: vec![AccessScope::Private, AccessScope::Group],
        check_group_enabled: true,
        check_thread_policy: true,
    };
    store
        .snapshot(actor, group)
        .await
        .ok()
        .flatten()
        .is_some_and(|s| {
            s.authorize(&policy, Some(("music", "inline")))
                .decision
                .allowed
        })
}

async fn help(
    bot: &Bot,
    message: &Message,
    actor: u64,
    identity: TelegramIdentity,
    deps: &MessageDependencies,
    language: yoink_i18n::Language,
    system: &yoink_application::SystemService,
) -> bool {
    let group = (message.chat.id.0 < 0).then_some((
        message.chat.id.0,
        message.thread_id.map(|t| i64::from(t.0.0)),
    ));
    let Some(store) = &deps.identities else {
        return false;
    };
    if group.is_some()
        && !store
            .membership_eligible(identity.bot_id as i64, message.chat.id.0, actor as i64)
            .await
            .unwrap_or(false)
    {
        return false;
    }
    let music = deps.music.is_some() && music_allowed(deps, actor as i64, group).await;
    let policy = AccessPolicy {
        min_role: Role::User,
        scopes: vec![AccessScope::Private, AccessScope::Group],
        check_group_enabled: true,
        check_thread_policy: true,
    };
    let snapshot = store.snapshot(actor as i64, group).await.ok().flatten();
    let approved = snapshot
        .as_ref()
        .is_some_and(|s| s.authorize(&policy, None).decision.allowed);
    let owner = actor == identity.owner_id
        && snapshot
            .as_ref()
            .is_some_and(|s| s.principal.role == Role::Owner);
    let stats = deps.stats.is_some() && approved;
    if !music && !stats {
        return false;
    }
    let mut sections = Vec::new();
    if approved && group.is_none() {
        sections.push(if owner {
            crate::response(
                crate::Command::Help,
                language,
                system,
                deps.downloads.is_some(),
            )
        } else {
            crate::translate(
                language,
                if deps.downloads.is_some() {
                    Key::UserHelpEnabled
                } else {
                    Key::UserHelpBasic
                },
                &[],
            )
        });
    }
    if let Some(insight) = &deps.insight {
        let text = insight.help_for(message, actor as i64, language).await;
        if !text.is_empty() {
            sections.push(text);
        }
    }
    // Recheck after other products' help lookups, before advertising capabilities.
    if music && music_allowed(deps, actor as i64, group).await {
        sections.push(crate::translate(language, Key::MusicHelp, &[]));
    }
    if stats
        && store
            .snapshot(actor as i64, group)
            .await
            .ok()
            .flatten()
            .is_some_and(|s| s.authorize(&policy, None).decision.allowed)
    {
        sections.push(format!(
            "/stats - {}",
            crate::translate(language, Key::MenuStats, &[])
        ));
    }
    if sections.is_empty() {
        return true;
    }
    let mut send = bot.send_message(message.chat.id, sections.join("\n\n"));
    if let Some(thread) = message.thread_id {
        send = send.message_thread_id(thread);
    }
    let _ = send.await;
    true
}

pub(crate) async fn message(
    bot: &Bot,
    message: &Message,
    actor: u64,
    identity: TelegramIdentity,
    username: &str,
    deps: &MessageDependencies,
    system: &yoink_application::SystemService,
) -> bool {
    let text = message.text().unwrap_or_default();
    let language = crate::reply_language_with_identity(
        deps.downloads.as_ref(),
        deps.identities.as_deref(),
        actor,
        message
            .from
            .as_ref()
            .and_then(|u| u.language_code.as_deref()),
    )
    .await;
    let group = (message.chat.id.0 < 0).then_some((
        message.chat.id.0,
        message.thread_id.map(|t| i64::from(t.0.0)),
    ));
    if ["start", "help"]
        .into_iter()
        .any(|name| command(text, username, name).is_some_and(|args| args.is_empty()))
        && help(bot, message, actor, identity, deps, language, system).await
    {
        return true;
    }
    let reply = if let Some(args) = command(text, username, "start")
        && args == ["music_help"]
        && deps.music.is_some()
    {
        Some(crate::translate(
            language,
            if music_allowed(deps, actor as i64, group).await {
                Key::MusicHelp
            } else {
                Key::ErrorDenied
            },
            &[],
        ))
    } else if let Some(args) = command(text, username, "stats")
        && let Some(stats) = &deps.stats
    {
        let policy = AccessPolicy {
            min_role: Role::User,
            scopes: vec![AccessScope::Private, AccessScope::Group],
            check_group_enabled: true,
            check_thread_policy: true,
        };
        let allowed = if let Some(store) = &deps.identities {
            store
                .snapshot(actor as i64, group)
                .await
                .ok()
                .flatten()
                .is_some_and(|s| s.authorize(&policy, None).decision.allowed)
                && (group.is_none()
                    || store
                        .membership_eligible(
                            identity.bot_id as i64,
                            message.chat.id.0,
                            actor as i64,
                        )
                        .await
                        .unwrap_or(false))
        } else {
            false
        };
        if !allowed {
            return true;
        }
        let target = crate::stats_runtime::CommandTarget {
            actor: actor as i64,
            chat: message.chat.id.0,
            bot: identity.bot_id as i64,
            message: message.id.0,
            topic: message.thread_id.map(|t| t.0.0),
        };
        let token = format!(
            "stats:{}:{}:{}:{}:{}",
            target.bot,
            target.chat,
            target.topic.unwrap_or(0),
            target.message,
            target.actor
        );
        match stats
            .command(
                &token,
                target,
                &args.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                language,
            )
            .await
        {
            Ok(_) => return true,
            Err(error) => Some(crate::translate(
                language,
                match error {
                    crate::stats_runtime::RuntimeError::Stats(yoink_stats::Error::Forbidden) => {
                        Key::ErrorDenied
                    }
                    crate::stats_runtime::RuntimeError::Stats(yoink_stats::Error::Invalid) => {
                        Key::ErrorInvalidInput
                    }
                    _ => Key::ErrorOperation,
                },
                &[],
            )),
        }
    } else if let Some((music, max_age)) = &deps.music {
        let explicit =
            command(text, username, "music").is_some() || command(text, username, "song").is_some();
        let passive_music =
            yoink_music::extract_links(text, &crate::music_runtime::hidden_links(message), 10)
                .iter()
                .any(|link| link.provider != yoink_worker_protocol::MusicProvider::Youtube);
        let result = if explicit {
            music.handle_command(message, actor as i64, username).await
        } else {
            music
                .handle_observed_message(message, actor as i64, chrono::Utc::now(), *max_age)
                .await
        };
        match result {
            Ok(true) => return true,
            Ok(false) => return false,
            Err(error) if explicit => Some(
                yoink_music::error_message(
                    error,
                    if language == yoink_i18n::Language::Russian {
                        "ru"
                    } else {
                        "en"
                    },
                )
                .to_owned(),
            ),
            Err(error)
                if passive_music
                    && !matches!(
                        error,
                        yoink_music::MusicError::Denied | yoink_music::MusicError::Cancelled
                    )
                    && music_allowed(deps, actor as i64, group).await =>
            {
                Some(
                    yoink_music::error_message(
                        error,
                        if language == yoink_i18n::Language::Russian {
                            "ru"
                        } else {
                            "en"
                        },
                    )
                    .to_owned(),
                )
            }
            Err(_) => return false,
        }
    } else {
        return false;
    };
    if let Some(reply) = reply {
        let mut send = bot.send_message(message.chat.id, reply);
        if let Some(thread) = message.thread_id {
            send = send.message_thread_id(thread);
        }
        let _ = send.await;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_preserve_arguments_and_reject_foreign_bot() {
        assert_eq!(
            command("/stats@Own 7 -100", "own", "stats"),
            Some(vec!["7", "-100"])
        );
        assert!(command("/song@Other https://example.com", "own", "song").is_none());
        assert_eq!(
            command("/start music_help", "own", "start"),
            Some(vec!["music_help"])
        );
    }
    #[tokio::test]
    async fn observer_composition_preserves_raw_and_propagates_failure() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let make = |name| {
            let calls = calls.clone();
            Arc::new(move |raw: serde_json::Value| {
                calls.lock().unwrap().push((name, raw));
                Box::pin(async { Ok(()) }) as futures::future::BoxFuture<'static, _>
            }) as UpdateObserver
        };
        let raw = serde_json::json!({"update_id":23,"message":{"message_thread_id":7}});
        compose_observers(Some(make("existing")), Some(make("stats"))).unwrap()(raw.clone())
            .await
            .unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            vec![("existing", raw.clone()), ("stats", raw)]
        );
        let fail: UpdateObserver =
            Arc::new(|_| Box::pin(async { Err(crate::TelegramError::PollingFailed) }));
        assert!(
            compose_observers(Some(fail), Some(make("not-called"))).unwrap()(
                serde_json::Value::Null
            )
            .await
            .is_err()
        );
        assert_eq!(calls.lock().unwrap().len(), 2);
    }
    #[tokio::test]
    async fn lifecycle_closes_admission_and_waits_for_original_handler() {
        let lifecycle = TelegramHandlerLifecycle::new();
        let handler = lifecycle.handlers.admit().unwrap();
        let drain = lifecycle.drain_handlers();
        tokio::pin!(drain);
        assert!(futures::poll!(&mut drain).is_pending());
        assert!(lifecycle.handlers.admit().is_none());
        assert!(lifecycle.stop.is_cancelled());
        drop(handler);
        drain.await;
    }
}
