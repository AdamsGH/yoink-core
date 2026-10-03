use std::convert::Infallible;
use teloxide::{
    prelude::*,
    types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message},
};
use yoink_domain::DeliveryTarget;
use yoink_downloads::{DownloadService, EditableSettings, SettingChange, SettingField};
use yoink_i18n::{Key, Language};

use crate::{
    DownloadsDependency, TelegramIdentity, download_error_response, reply_language, translate,
};

const LANGUAGES: &[&str] = &[
    "en", "ru", "zh", "ja", "ar", "hi", "de", "fr", "es", "pt", "zh-Hans",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Language(&'static str, i64),
    Automatic(bool, i64),
}

fn parse_callback(data: &str) -> Option<Action> {
    if data.len() > 64 {
        return None;
    }
    let fields = data.split(':').collect::<Vec<_>>();
    if fields.len() != 4 || fields[0] != "subs" {
        return None;
    }
    let revision_text = fields[3];
    if revision_text.is_empty() || !revision_text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let revision = revision_text.parse::<i64>().ok()?;
    match fields[1] {
        "lang" => Some(Action::Language(
            LANGUAGES.iter().find(|&&code| code == fields[2])?,
            revision,
        )),
        "auto" => Some(Action::Automatic(
            match fields[2] {
                "true" => true,
                "false" => false,
                _ => return None,
            },
            revision,
        )),
        _ => None,
    }
}

fn safe_tag(value: &str) -> bool {
    SettingChange::new(SettingField::SubtitleLanguage, value).is_ok()
}

fn render(settings: &EditableSettings, language: Language) -> (String, InlineKeyboardMarkup) {
    let value = |field| {
        settings
            .values
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    };
    let current_language = if safe_tag(value(SettingField::SubtitleLanguage)) {
        value(SettingField::SubtitleLanguage).to_owned()
    } else {
        translate(language, Key::SubsInvalidLanguage, &[])
    };
    let automatic = value(SettingField::SubtitleAutomatic) == "true";
    let current_auto = translate(
        language,
        if automatic {
            Key::SettingsOn
        } else {
            Key::SettingsOff
        },
        &[],
    );
    let values = format!(
        "{}: {current_language}\n{}: {current_auto}",
        translate(language, Key::SettingsSubtitleLanguage, &[]),
        translate(language, Key::SettingsSubtitleAutomatic, &[])
    );
    let text = translate(language, Key::SubsTitle, &[("values", &values)]);
    let buttons = LANGUAGES
        .iter()
        .map(|code| {
            let label = if *code == current_language {
                format!("{code} ✓")
            } else {
                (*code).to_owned()
            };
            InlineKeyboardButton::callback(label, format!("subs:lang:{code}:{}", settings.revision))
        })
        .collect::<Vec<_>>();
    let mut rows = buttons
        .chunks(3)
        .map(|row| row.to_vec())
        .collect::<Vec<_>>();
    let automatic_value = (!automatic).to_string();
    rows.push(vec![InlineKeyboardButton::callback(
        translate(
            language,
            if automatic {
                Key::SubsAutomaticOff
            } else {
                Key::SubsAutomaticOn
            },
            &[],
        ),
        format!("subs:auto:{automatic_value}:{}", settings.revision),
    )]);
    (text, InlineKeyboardMarkup::new(rows))
}

async fn show(
    service: &DownloadService,
    actor: u64,
    target: DeliveryTarget,
    action: Option<Action>,
    language: Language,
) -> Result<(String, InlineKeyboardMarkup), String> {
    if let Some(action) = action {
        let (field, value, revision) = match action {
            Action::Language(code, revision) => (SettingField::SubtitleLanguage, code, revision),
            Action::Automatic(enabled, revision) => (
                SettingField::SubtitleAutomatic,
                if enabled { "true" } else { "false" },
                revision,
            ),
        };
        service
            .change_setting(
                actor,
                target,
                SettingChange::new(field, value)
                    .map_err(|e| download_error_response(e, language))?,
                Some(revision),
            )
            .await
            .map_err(|e| {
                if matches!(e, yoink_downloads::DownloadError::Conflict) {
                    translate(language, Key::SubsStale, &[])
                } else {
                    download_error_response(e, language)
                }
            })?;
    }
    let settings = service
        .settings(actor, target)
        .await
        .map_err(|e| download_error_response(e, language))?;
    Ok(render(&settings, language))
}

pub(super) async fn handle_command(
    bot: &Bot,
    message: &Message,
    service: Option<&DownloadService>,
    actor: u64,
    bot_id: u64,
    language: Language,
) {
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
    match show(service, actor, target, None, language).await {
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

fn authorized(query: &CallbackQuery, message: &Message, identity: TelegramIdentity) -> bool {
    !query.from.is_bot
        && query.from.id.0 == identity.owner_id
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

pub(super) async fn handle_callback(
    bot: Bot,
    query: CallbackQuery,
    identity: TelegramIdentity,
    DownloadsDependency(downloads): DownloadsDependency,
) -> Result<(), Infallible> {
    let Some(message) = query.message.as_ref().and_then(|m| m.regular_message()) else {
        let _ = bot.answer_callback_query(query.id).await;
        return Ok(());
    };
    if !authorized(&query, message, identity) {
        let _ = bot.answer_callback_query(query.id).await;
        return Ok(());
    }
    let language = reply_language(
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
    let action = query.data.as_deref().and_then(parse_callback);
    let Some(action) = action else {
        let _ = bot
            .answer_callback_query(query.id)
            .text(translate(language, Key::SubsStale, &[]))
            .show_alert(true)
            .await;
        return Ok(());
    };
    let Ok(target) = DeliveryTarget::new(identity.bot_id as i64, message.chat.id.0, None, None)
    else {
        let _ = bot.answer_callback_query(query.id).await;
        return Ok(());
    };
    match show(service, query.from.id.0, target, Some(action), language).await {
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

    fn menu_settings(language: &str, automatic: bool) -> EditableSettings {
        EditableSettings {
            values: vec![
                (SettingField::SubtitleLanguage, language.into()),
                (SettingField::SubtitleAutomatic, automatic.to_string()),
            ],
            revision: 9,
        }
    }
    #[test]
    fn callback_contract_is_closed_and_strict() {
        assert_eq!(
            parse_callback("subs:lang:zh-Hans:0"),
            Some(Action::Language("zh-Hans", 0))
        );
        assert_eq!(
            parse_callback("subs:auto:true:42"),
            Some(Action::Automatic(true, 42))
        );
        for bad in [
            "subs:toggle:1",
            "subs:lang:xx:1",
            "subs:auto:yes:1",
            "subs:lang:en:-1",
            "subs:lang:en:+1",
            "subs:lang:en:01:x",
            "prefs:subs_lang:en:0",
            "subs:lang:en:9223372036854775808",
        ] {
            assert_eq!(parse_callback(bad), None);
        }
        assert_eq!(
            parse_callback(&format!("subs:lang:en:{}", "1".repeat(60))),
            None
        );
    }
    #[test]
    fn render_uses_bounded_buttons_and_safe_saved_text() {
        for language in [Language::English, Language::Russian] {
            let settings = menu_settings("x<&script", false);
            let (text, markup) = render(&settings, language);
            assert!(!text.contains("x<&script"));
            assert!(text.contains(translate(language, Key::SubsInvalidLanguage, &[]).as_str()));
            assert!(text.contains(match language {
                Language::English => "Standalone subtitle preferences",
                Language::Russian => "Настройки отдельных файлов субтитров",
            }));
            let json = serde_json::to_value(markup).unwrap();
            for row in json["inline_keyboard"].as_array().unwrap() {
                for button in row.as_array().unwrap() {
                    let data = button["callback_data"].as_str().unwrap();
                    assert!(data.len() <= 64);
                    assert!(parse_callback(data).is_some());
                }
            }
            assert!(
                json["inline_keyboard"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|row| row.as_array().unwrap())
                    .all(|button| !button["text"]
                        .as_str()
                        .unwrap()
                        .to_lowercase()
                        .contains("embedding"))
            );
        }
        assert!(safe_tag("zh-Hans"));
        assert!(!safe_tag("<script>"));
    }

    #[test]
    fn automatic_controls_show_the_action_label_and_matching_callback_in_both_languages() {
        for language in [Language::English, Language::Russian] {
            for (enabled, english_label, russian_label, callback) in [
                (
                    true,
                    "Turn automatic fallback off",
                    "Отключить автоподстановку",
                    "subs:auto:false:9",
                ),
                (
                    false,
                    "Turn automatic fallback on",
                    "Включить автоподстановку",
                    "subs:auto:true:9",
                ),
            ] {
                let (text, markup) = render(&menu_settings("ru", enabled), language);
                let rows = serde_json::to_value(markup).unwrap();
                let button = rows["inline_keyboard"].as_array().unwrap().last().unwrap()[0].clone();
                assert_eq!(
                    button["text"],
                    match language {
                        Language::English => english_label,
                        Language::Russian => russian_label,
                    }
                );
                assert_eq!(button["callback_data"], callback);
                assert!(text.contains(match language {
                    Language::English => "Subtitle language: ru",
                    Language::Russian => "Язык субтитров: ru",
                }));
                assert!(
                    text.contains(
                        translate(
                            language,
                            if enabled {
                                Key::SettingsOn
                            } else {
                                Key::SettingsOff
                            },
                            &[],
                        )
                        .as_str()
                    )
                );
                assert!(
                    rows["inline_keyboard"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .flat_map(|row| row.as_array().unwrap())
                        .any(|button| button["text"] == "ru ✓")
                );
                assert!(
                    rows["inline_keyboard"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .flat_map(|row| row.as_array().unwrap())
                        .all(|button| !button["text"]
                            .as_str()
                            .unwrap()
                            .to_lowercase()
                            .contains("embedding"))
                );
            }
        }
    }

    #[test]
    fn callback_authorization_requires_owner_private_non_thread_user_and_current_bot_message() {
        let identity = TelegramIdentity {
            owner_id: 42,
            bot_id: 123,
            groups_enabled: false,
            identity_enabled: false,
        };
        let message_json = serde_json::json!({
            "message_id": 7, "date": 0,
            "from": {"id": 123, "is_bot": true, "first_name": "Bot"},
            "chat": {"id": 42, "type": "private"}, "text": "settings"
        });
        let query_json = serde_json::json!({
            "id": "query", "from": {"id": 42, "is_bot": false, "first_name": "Owner"},
            "chat_instance": "instance", "data": "subs:auto:true:0"
        });
        let message: Message = serde_json::from_value(message_json.clone()).unwrap();
        let query: CallbackQuery = serde_json::from_value(query_json.clone()).unwrap();
        assert!(authorized(&query, &message, identity));

        let mut cases = Vec::new();
        let mut changed = query_json.clone();
        changed["from"]["id"] = 43.into();
        cases.push((changed, message_json.clone())); // non-owner
        let mut changed = query_json.clone();
        changed["from"]["is_bot"] = true.into();
        cases.push((changed, message_json.clone())); // current bot as callback actor
        let mut changed = message_json.clone();
        changed["chat"] = serde_json::json!({"id": -100, "type": "supergroup"});
        cases.push((query_json.clone(), changed)); // group
        let mut changed = message_json.clone();
        changed["chat"]["id"] = 43.into();
        cases.push((query_json.clone(), changed)); // mismatched private chat
        let mut changed = message_json.clone();
        changed["message_thread_id"] = 2.into();
        cases.push((query_json.clone(), changed)); // topic
        let mut changed = message_json.clone();
        changed["sender_chat"] = serde_json::json!({"id": 42, "type": "private"});
        cases.push((query_json.clone(), changed)); // sender_chat
        let mut changed = message_json.clone();
        changed["from"]["id"] = 124.into();
        cases.push((query_json.clone(), changed)); // stale/other bot
        for (query, message) in cases {
            let query: CallbackQuery = serde_json::from_value(query).unwrap();
            let message: Message = serde_json::from_value(message).unwrap();
            assert!(!authorized(&query, &message, identity));
        }

        let inaccessible: CallbackQuery = serde_json::from_value(serde_json::json!({
            "id": "query", "from": {"id": 42, "is_bot": false, "first_name": "Owner"},
            "chat_instance": "instance",
            "message": {"date": 0, "chat": {"id": 42, "type": "private"}, "message_id": 7}
        }))
        .unwrap();
        assert!(
            inaccessible
                .message
                .as_ref()
                .and_then(|m| m.regular_message())
                .is_none()
        );
    }
}
