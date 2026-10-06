use std::convert::Infallible;
use teloxide::{
    prelude::*,
    types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message},
};
use yoink_domain::DeliveryTarget;
use yoink_downloads::DownloadService;
use yoink_i18n::{Key, Language};

use crate::{DownloadsDependency, TelegramIdentity, reply_language, translate};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Action {
    Menu,
    Invalid,
}

pub(super) fn command(args: &[&str]) -> Action {
    if args.is_empty() {
        Action::Menu
    } else {
        Action::Invalid
    }
}

/// Stored preferences deliberately accept only supported bases and well-formed optional subtags.
pub(super) fn saved_language(code: &str) -> Option<Language> {
    let mut parts = code.split('-');
    let base = parts.next()?;
    let language = if base.eq_ignore_ascii_case("en") {
        Language::English
    } else if base.eq_ignore_ascii_case("ru") {
        Language::Russian
    } else {
        return None;
    };
    if let Some(subtag) = parts.next()
        && (!(2..=8).contains(&subtag.len())
            || !subtag.bytes().all(|b| b.is_ascii_alphanumeric())
            || parts.next().is_some())
    {
        return None;
    }
    Some(language)
}

pub(super) fn resolve_language(preferred: Option<&str>, fallback: Option<&str>) -> Language {
    preferred
        .and_then(saved_language)
        .unwrap_or_else(|| yoink_i18n::language_for(fallback))
}

fn keyboard(language: Language, revision: i64) -> InlineKeyboardMarkup {
    let rows: Vec<_> = ["en", "ru"]
        .into_iter()
        .map(|value| {
            let selected = (value == "en" && language == Language::English)
                || (value == "ru" && language == Language::Russian);
            let label = translate(
                language,
                if value == "en" {
                    Key::LangEnglish
                } else {
                    Key::LangRussian
                },
                &[],
            );
            let label = if selected {
                format!("{label} ✓")
            } else {
                label
            };
            vec![InlineKeyboardButton::callback(
                label,
                format!("lang:{value}:{revision}"),
            )]
        })
        .collect();
    InlineKeyboardMarkup::new(rows)
}

fn callback(data: &str) -> Option<(&'static str, i64)> {
    if data.len() > 64 {
        return None;
    }
    let mut parts = data.split(':');
    if parts.next()? != "lang" {
        return None;
    }
    let value = match parts.next()? {
        "en" => "en",
        "ru" => "ru",
        _ => return None,
    };
    let revision_text = parts.next()?;
    if revision_text.is_empty() || !revision_text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let revision = revision_text.parse::<i64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((value, revision))
}

fn authorized(query: &CallbackQuery, message: &Message, identity: TelegramIdentity) -> bool {
    !query.from.is_bot
        && message.chat.is_private()
        && message.chat.id.0 > 0
        && message.chat.id.0 as u64 == query.from.id.0
        && message.thread_id.is_none()
        && message.sender_chat.is_none()
        && message
            .from
            .as_ref()
            .is_some_and(|u| u.is_bot && u.id.0 == identity.bot_id)
}

pub(super) async fn handle_command(
    bot: &Bot,
    message: &Message,
    bot_id: u64,
    service: Option<&DownloadService>,
    actor: u64,
    action: Action,
    language: Language,
) {
    if action == Action::Invalid {
        let _ = bot
            .send_message(message.chat.id, translate(language, Key::LangUsage, &[]))
            .await;
        return;
    }
    let Some(service) = service else {
        let _ = bot
            .send_message(
                message.chat.id,
                translate(language, Key::DownloadsUnavailable, &[]),
            )
            .await;
        return;
    };
    let Ok(target) = DeliveryTarget::new(bot_id as i64, message.chat.id.0, None, None) else {
        return;
    };
    let Ok(pref) = service.language_settings(actor, target).await else {
        let _ = bot
            .send_message(
                message.chat.id,
                translate(language, Key::DownloadsUnavailable, &[]),
            )
            .await;
        return;
    };
    let language = saved_language(&pref.code).unwrap_or(language);
    let text = translate(language, Key::LangChoose, &[]);
    let _ = bot
        .send_message(message.chat.id, text)
        .reply_markup(keyboard(language, pref.revision))
        .await;
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
    let Some(message) = query.message.as_ref().and_then(|m| m.regular_message()) else {
        let _ = bot.answer_callback_query(query.id).await;
        return Ok(());
    };
    if !authorized(&query, message, identity) {
        let _ = bot.answer_callback_query(query.id).await;
        return Ok(());
    }
    let fallback = query.from.language_code.as_deref();
    let current = reply_language(downloads.as_ref(), query.from.id.0, fallback).await;
    let Some(service) = downloads.as_ref() else {
        let _ = bot
            .answer_callback_query(query.id)
            .text(translate(current, Key::DownloadsUnavailable, &[]))
            .show_alert(true)
            .await;
        return Ok(());
    };
    let Some((selected, revision)) = callback(data) else {
        let _ = bot
            .answer_callback_query(query.id)
            .text(translate(current, Key::LangStale, &[]))
            .show_alert(true)
            .await;
        return Ok(());
    };
    let Ok(target) = DeliveryTarget::new(identity.bot_id as i64, message.chat.id.0, None, None)
    else {
        let _ = bot.answer_callback_query(query.id).await;
        return Ok(());
    };
    match service
        .change_language(query.from.id.0, target, selected, revision)
        .await
    {
        Ok(pref) => {
            let new_language = saved_language(&pref.code).unwrap_or(current);
            let display = translate(
                new_language,
                if selected == "en" {
                    Key::LangEnglish
                } else {
                    Key::LangRussian
                },
                &[],
            );
            let text = translate(new_language, Key::LangSet, &[("language", &display)]);
            let _ = bot
                .edit_message_text(message.chat.id, message.id, text)
                .reply_markup(InlineKeyboardMarkup::new(
                    Vec::<Vec<InlineKeyboardButton>>::new(),
                ))
                .await;
            let _ = bot.answer_callback_query(query.id).await;
        }
        Err(_) => {
            let latest = reply_language(Some(service), query.from.id.0, fallback).await;
            let _ = bot
                .answer_callback_query(query.id)
                .text(translate(latest, Key::LangStale, &[]))
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
    fn commands_are_bare_only_and_callbacks_require_versioned_exact_wire_fields() {
        assert_eq!(command(&[]), Action::Menu);
        assert_eq!(command(&["ru"]), Action::Invalid);
        for valid in ["lang:en:0", "lang:ru:42"] {
            assert!(callback(valid).is_some(), "{valid}");
        }
        for invalid in [
            "lang:en",
            "lang:ru:-1",
            "lang:fr:0",
            "lang:en:1:extra",
            "lang:en:+1",
            "lang:en:9223372036854775808",
        ] {
            assert!(callback(invalid).is_none(), "{invalid}");
        }
        assert!(callback(&format!("lang:en:{}", "1".repeat(64))).is_none());
    }

    #[test]
    fn supported_saved_preference_wins_and_invalid_or_missing_falls_back() {
        assert_eq!(
            resolve_language(Some("ru"), Some("en-US")),
            Language::Russian
        );
        assert_eq!(
            resolve_language(Some("en-GB"), Some("ru")),
            Language::English
        );
        assert_eq!(
            resolve_language(Some("xx"), Some("ru-RU")),
            Language::Russian
        );
        assert_eq!(
            resolve_language(Some("ru--RU"), Some("en")),
            Language::English
        );
        assert_eq!(resolve_language(None, Some("ru")), Language::Russian);
        assert_eq!(resolve_language(Some("fr"), None), Language::English);
    }

    #[test]
    fn stored_codes_allow_only_supported_base_and_one_valid_optional_subtag() {
        assert_eq!(saved_language("RU-ru"), Some(Language::Russian));
        assert_eq!(saved_language("en-US"), Some(Language::English));
        for invalid in ["fr", "rubbish", "ru_ RU", "ru--RU", "en-US-extra", "en-"] {
            assert_eq!(saved_language(invalid), None, "{invalid}");
        }
    }
}
