use std::convert::Infallible;
use teloxide::{
    prelude::*,
    types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message},
};
use yoink_domain::DeliveryTarget;
use yoink_downloads::{DownloadService, ProxySettings};
use yoink_i18n::{Key, Language, language_for};

use crate::{DownloadsDependency, TelegramIdentity, download_error_response, translate};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Action {
    Menu,
    Invalid,
    Change(bool, i64),
}

pub(super) fn command(args: &[&str]) -> Action {
    if args.is_empty() {
        Action::Menu
    } else {
        Action::Invalid
    }
}

fn callback(data: &str) -> Option<Action> {
    if data.len() > 64 {
        return None;
    }
    let mut parts = data.split(':');
    if parts.next()? != "proxy" {
        return None;
    }
    let enabled = match parts.next()? {
        "on" => true,
        "off" => false,
        _ => return None,
    };
    let revision_text = parts.next()?;
    if revision_text.is_empty()
        || revision_text.len() > 19
        || !revision_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let revision = revision_text.parse::<i64>().ok()?;
    if revision.to_string() != revision_text {
        return None;
    }
    if revision < 0 || parts.next().is_some() {
        return None;
    }
    Some(Action::Change(enabled, revision))
}

fn render(settings: &ProxySettings, language: Language) -> (String, InlineKeyboardMarkup) {
    let status = translate(
        language,
        if settings.enabled {
            Key::ProxyEnabled
        } else {
            Key::ProxyDisabled
        },
        &[],
    );
    let mut text = translate(language, Key::ProxyTitle, &[("status", &status)]);
    let enabled = !settings.enabled;
    let rows = if enabled && !settings.configured {
        text.push('\n');
        text.push_str(&translate(language, Key::ProxyUnavailable, &[]));
        Vec::new()
    } else {
        let label = translate(
            language,
            if enabled {
                Key::ProxyEnable
            } else {
                Key::ProxyDisable
            },
            &[],
        );
        vec![vec![InlineKeyboardButton::callback(
            label,
            format!(
                "proxy:{}:{}",
                if enabled { "on" } else { "off" },
                settings.revision
            ),
        )]]
    };
    (text, InlineKeyboardMarkup::new(rows))
}

async fn apply(
    service: &DownloadService,
    actor: u64,
    target: DeliveryTarget,
    action: Action,
    language: Language,
) -> Result<(String, InlineKeyboardMarkup), String> {
    match action {
        Action::Invalid => return Err(translate(language, Key::ProxyUsage, &[])),
        Action::Menu => (),
        Action::Change(enabled, revision) => service
            .change_proxy_setting(actor, target, enabled, revision)
            .await
            .map_err(|error| {
                if matches!(error, yoink_downloads::DownloadError::Conflict) {
                    translate(language, Key::ProxyStale, &[])
                } else if matches!(error, yoink_downloads::DownloadError::ProxyUnavailable) {
                    translate(language, Key::ProxyUnavailable, &[])
                } else {
                    download_error_response(error, language)
                }
            })?,
    }
    let settings = service
        .proxy_settings(actor, target)
        .await
        .map_err(|error| download_error_response(error, language))?;
    Ok(render(&settings, language))
}

pub(super) async fn handle_command(
    bot: &Bot,
    message: &Message,
    service: &DownloadService,
    actor: u64,
    target: DeliveryTarget,
    action: Action,
    language: Language,
) {
    match apply(service, actor, target, action, language).await {
        Ok((text, keyboard)) => {
            let _ = bot
                .send_message(message.chat.id, text)
                .reply_markup(keyboard)
                .await;
        }
        Err(text) => {
            let _ = bot.send_message(message.chat.id, text).await;
        }
    }
}

pub(super) async fn handle_callback(
    bot: Bot,
    query: CallbackQuery,
    identity: TelegramIdentity,
    DownloadsDependency(downloads): DownloadsDependency,
) -> Result<(), Infallible> {
    let Some(data) = query.data.as_deref() else {
        return Ok(());
    };
    if !data.starts_with("proxy:") {
        return Ok(());
    }
    let Some(message) = query
        .message
        .as_ref()
        .and_then(|message| message.regular_message())
    else {
        let _ = bot.answer_callback_query(query.id).await;
        return Ok(());
    };
    if query.from.is_bot
        || query.from.id.0 != identity.owner_id
        || !message.chat.is_private()
        || message.chat.id.0 <= 0
        || message.chat.id.0 as u64 != query.from.id.0
        || message.thread_id.is_some()
        || message.sender_chat.is_some()
        || !message
            .from
            .as_ref()
            .is_some_and(|user| user.is_bot && user.id.0 == identity.bot_id)
    {
        let _ = bot.answer_callback_query(query.id).await;
        return Ok(());
    }
    let language = language_for(query.from.language_code.as_deref());
    let Some(service) = downloads.as_ref() else {
        let _ = bot
            .answer_callback_query(query.id)
            .text(translate(language, Key::DownloadsUnavailable, &[]))
            .show_alert(true)
            .await;
        return Ok(());
    };
    let Some(action) = callback(data) else {
        let _ = bot
            .answer_callback_query(query.id)
            .text(translate(language, Key::ProxyStale, &[]))
            .show_alert(true)
            .await;
        return Ok(());
    };
    let Ok(target) = DeliveryTarget::new(identity.bot_id as i64, message.chat.id.0, None, None)
    else {
        let _ = bot.answer_callback_query(query.id).await;
        return Ok(());
    };
    match apply(service, query.from.id.0, target, action, language).await {
        Ok((text, keyboard)) => {
            let _ = bot.answer_callback_query(query.id).await;
            let _ = bot
                .edit_message_text(message.chat.id, message.id, text)
                .reply_markup(keyboard)
                .await;
        }
        Err(text) => {
            let _ = bot
                .answer_callback_query(query.id)
                .text(text)
                .show_alert(true)
                .await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_only_offers_available_action_in_both_languages() {
        for language in [Language::English, Language::Russian] {
            for (enabled, configured, action) in [
                (false, false, None),
                (false, true, Some("on")),
                (true, true, Some("off")),
                (true, false, Some("off")),
            ] {
                let (text, markup) = render(
                    &ProxySettings {
                        enabled,
                        configured,
                        revision: 123,
                    },
                    language,
                );
                let callbacks = markup.inline_keyboard.iter().flatten().collect::<Vec<_>>();
                assert!(!text.contains("{status}"));
                assert!(!text.contains("http"));
                if let Some(action) = action {
                    assert_eq!(callbacks.len(), 1);
                    assert_eq!(
                        *callbacks[0],
                        InlineKeyboardButton::callback(
                            translate(
                                language,
                                if enabled {
                                    Key::ProxyDisable
                                } else {
                                    Key::ProxyEnable
                                },
                                &[]
                            ),
                            format!("proxy:{action}:123"),
                        )
                    );
                } else {
                    assert!(callbacks.is_empty());
                    assert!(text.contains(&translate(language, Key::ProxyUnavailable, &[])));
                }
            }
        }
    }

    #[test]
    fn accepts_only_revision_bounded_callback_contract() {
        assert_eq!(callback("proxy:on:3"), Some(Action::Change(true, 3)));
        assert_eq!(callback("proxy:off:0"), Some(Action::Change(false, 0)));
        for bad in [
            "proxy:on",
            "proxy:on:-1",
            "proxy:on:+1",
            "proxy:on:01",
            "proxy:on:1:x",
            "proxy:maybe:1",
            "proxy:on:999999999999999999999",
        ] {
            assert_eq!(callback(bad), None);
        }
        assert_eq!(command(&[]), Action::Menu);
        assert_eq!(command(&["secret"]), Action::Invalid);
    }
}
