use std::convert::Infallible;
use teloxide::{
    prelude::*,
    types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message},
};
use yoink_domain::DeliveryTarget;
use yoink_downloads::{DownloadService, EditableSettings, SettingChange, SettingField};
use yoink_i18n::{Key, Language};

use crate::{DownloadsDependency, TelegramIdentity, download_error_response, translate};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Action {
    Invalid,
    Menu,
    Field(SettingField),
    Change(SettingChange, Option<i64>),
}

pub(super) fn command(args: &[&str]) -> Option<Action> {
    match args {
        [] => Some(Action::Menu),
        [name, value] => Some(Action::Change(
            SettingChange::new(SettingField::parse(name)?, value).ok()?,
            None,
        )),
        _ => None,
    }
}

fn callback(data: &str) -> Option<Action> {
    if data.len() > 64 {
        return None;
    }
    if data == "prefs:menu" {
        return Some(Action::Menu);
    }
    let mut parts = data.split(':');
    if parts.next()? != "prefs" {
        return None;
    }
    let field = SettingField::parse(parts.next()?)?;
    let value = parts.next()?;
    if value == "_menu" {
        if parts.next().is_some() {
            return None;
        }
        Some(Action::Field(field))
    } else {
        let revision = parts.next()?.parse::<i64>().ok()?;
        if revision < 0 || parts.next().is_some() {
            return None;
        }
        Some(Action::Change(
            SettingChange::new(field, value).ok()?,
            Some(revision),
        ))
    }
}

fn label(field: SettingField) -> Key {
    match field {
        SettingField::Quality => Key::SettingsQuality,
        SettingField::Codec => Key::SettingsCodec,
        SettingField::Container => Key::SettingsContainer,
        SettingField::AudioCodec => Key::SettingsAudioCodec,
        SettingField::SendAsFile => Key::SettingsSendAsFile,
        SettingField::GalleryZip => Key::SettingsGalleryZip,
        SettingField::SubtitleLanguage => Key::SettingsSubtitleLanguage,
        SettingField::SubtitleAutomatic => Key::SettingsSubtitleAutomatic,
    }
}

fn value_label(value: &str, language: Language) -> String {
    match value {
        "true" => translate(language, Key::SettingsOn, &[]),
        "false" => translate(language, Key::SettingsOff, &[]),
        "best" => translate(language, Key::SettingsBest, &[]),
        _ => value.to_owned(),
    }
}

fn render(
    settings: &EditableSettings,
    page: Option<SettingField>,
    language: Language,
) -> (String, InlineKeyboardMarkup) {
    let values = settings
        .values
        .iter()
        .map(|(field, value)| {
            format!(
                "{}: {}",
                translate(language, label(*field), &[]),
                value_label(value, language)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let text = translate(language, Key::SettingsTitle, &[("values", &values)]);
    let rows = if let Some(field) = page {
        let current = settings
            .values
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, value)| value.as_str());
        let buttons = field
            .choices()
            .iter()
            .map(|value| {
                let selected = if current == Some(value) { " ✓" } else { "" };
                InlineKeyboardButton::callback(
                    format!("{}{selected}", value_label(value, language)),
                    format!("prefs:{}:{value}:{}", field.name(), settings.revision),
                )
            })
            .collect::<Vec<_>>();
        let mut rows = buttons
            .chunks(3)
            .map(|row| row.to_vec())
            .collect::<Vec<_>>();
        rows.push(vec![InlineKeyboardButton::callback(
            translate(language, Key::SettingsBack, &[]),
            "prefs:menu",
        )]);
        rows
    } else {
        SettingField::ALL
            .into_iter()
            .map(|field| {
                vec![InlineKeyboardButton::callback(
                    translate(language, label(field), &[]),
                    format!("prefs:{}:_menu", field.name()),
                )]
            })
            .collect()
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
    let page = match action {
        Action::Invalid => return Err(translate(language, Key::SettingsUsage, &[])),
        Action::Menu => None,
        Action::Field(field) => Some(field),
        Action::Change(change, revision) => {
            service
                .change_setting(actor, target, change, revision)
                .await
                .map_err(|e| {
                    if matches!(e, yoink_downloads::DownloadError::Conflict) {
                        translate(language, Key::SettingsStale, &[])
                    } else {
                        download_error_response(e, language)
                    }
                })?;
            None
        }
    };
    let settings = service
        .settings(actor, target)
        .await
        .map_err(|e| download_error_response(e, language))?;
    Ok(render(&settings, page, language))
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
    if !data.starts_with("prefs:") {
        return Ok(());
    }
    // Answer once, including revoked, stale and invalid callbacks. Never echo input.
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
    let language = crate::reply_language(
        downloads.as_ref(),
        query.from.id.0,
        query.from.language_code.as_deref(),
    )
    .await;
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
            .text(translate(language, Key::SettingsStale, &[]))
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
    fn callback_and_command_grammar_is_closed() {
        assert_eq!(callback("prefs:menu"), Some(Action::Menu));
        for field in SettingField::ALL {
            assert_eq!(
                callback(&format!("prefs:{}:_menu", field.name())),
                Some(Action::Field(field))
            );
            for value in field.choices() {
                assert_eq!(
                    callback(&format!("prefs:{}:{value}:123", field.name())),
                    Some(Action::Change(
                        SettingChange::new(field, value).unwrap(),
                        Some(123)
                    ))
                );
            }
        }
        for bad in [
            "prefs:quality:ask:0",
            "prefs:quality:720",
            "prefs:quality:720:-1",
            "prefs:quality:720:123:extra",
            "prefs:quality:720:extra",
            "prefs:role:owner",
            "prefs:proxy_url:https://fake",
            "fmt:q:720",
            "prefs:subs_lang:eng-abcdefgh",
        ] {
            assert_eq!(callback(bad), None);
        }
    }
    #[test]
    fn both_languages_render_all_fields_and_bounded_callbacks() {
        let settings = EditableSettings {
            values: SettingField::ALL
                .into_iter()
                .map(|field| (field, field.choices()[0].into()))
                .collect(),
            revision: i64::MAX,
        };
        for language in [Language::English, Language::Russian] {
            for page in [
                None,
                Some(SettingField::Quality),
                Some(SettingField::SubtitleAutomatic),
            ] {
                let (text, keyboard) = render(&settings, page, language);
                assert!(text.len() < 4096);
                assert!(!text.contains("[telegram."));
                let json = serde_json::to_value(keyboard).unwrap();
                for row in json["inline_keyboard"].as_array().unwrap() {
                    for button in row.as_array().unwrap() {
                        assert!(callback(button["callback_data"].as_str().unwrap()).is_some());
                    }
                }
            }
        }
    }
}
