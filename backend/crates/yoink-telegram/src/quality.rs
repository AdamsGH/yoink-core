use std::convert::Infallible;

use teloxide::{
    prelude::*,
    types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message},
};
use uuid::Uuid;
use yoink_domain::DeliveryTarget;
use yoink_downloads::{
    DownloadError, DownloadService, QualityChoice, QualitySession, QualityStage,
};
use yoink_i18n::{Key, Language};
use yoink_storage::Admission;

use crate::{
    DownloadsDependency, TelegramIdentity, admitted_response, download_error_response, translate,
};

const HEIGHTS: &[&str] = &["2160", "1440", "1080", "720", "480", "360", "240", "144"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Callback {
    Select(QualityChoice, Uuid, i64),
    Cancel(Uuid, i64),
}

fn parse_choice(value: &str) -> Option<QualityChoice> {
    if value == "best" || value == "audio" || HEIGHTS.contains(&value) {
        QualityChoice::parse(value).ok()
    } else {
        None
    }
}

fn callback(data: &str) -> Option<Callback> {
    if data.len() > 64 {
        return None;
    }
    let mut parts = data.split(':');
    if parts.next()? != "ql" {
        return None;
    }
    let action = parts.next()?;
    let id_text = parts.next()?;
    let revision_text = parts.next()?;
    if parts.next().is_some()
        || id_text.len() != 32
        || !id_text.bytes().all(|byte| byte.is_ascii_hexdigit())
        || revision_text.is_empty()
        || !revision_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let id = Uuid::parse_str(id_text).ok()?;
    let revision = revision_text.parse::<i64>().ok()?;
    if revision <= 0 || revision.to_string() != revision_text {
        return None;
    }
    if action == "cancel" {
        Some(Callback::Cancel(id, revision))
    } else {
        Some(Callback::Select(parse_choice(action)?, id, revision))
    }
}

fn callback_data(choice: &str, id: Uuid, revision: i64) -> String {
    format!("ql:{choice}:{}:{revision}", id.simple())
}

fn choice_label(choice: &QualityChoice, language: Language) -> String {
    match choice.as_str() {
        "best" => translate(language, Key::QualityBest, &[]),
        "audio" => translate(language, Key::QualityAudio, &[]),
        height => translate(language, Key::QualityHeight, &[("height", height)]),
    }
}

fn keyboard(session: &QualitySession, language: Language) -> InlineKeyboardMarkup {
    keyboard_values(
        session.stage,
        session.id,
        session.revision,
        session.selected,
        language,
    )
}

fn keyboard_values(
    stage: QualityStage,
    id: Uuid,
    revision: i64,
    selected: Option<QualityChoice>,
    language: Language,
) -> InlineKeyboardMarkup {
    let rows = match stage {
        QualityStage::Choosing => {
            let mut buttons = vec![InlineKeyboardButton::callback(
                choice_label(
                    &QualityChoice::parse("best").expect("supported choice"),
                    language,
                ),
                callback_data("best", id, revision),
            )];
            let audio = parse_choice("audio").expect("supported choice");
            buttons.extend(HEIGHTS.iter().map(|height| {
                let choice = QualityChoice::parse(height).expect("supported height");
                InlineKeyboardButton::callback(
                    choice_label(&choice, language),
                    callback_data(height, id, revision),
                )
            }));
            let mut rows = buttons
                .chunks(3)
                .map(|row| row.to_vec())
                .collect::<Vec<_>>();
            rows.push(vec![
                InlineKeyboardButton::callback(
                    choice_label(&audio, language),
                    callback_data("audio", id, revision),
                ),
                InlineKeyboardButton::callback(
                    translate(language, Key::QualityCancel, &[]),
                    callback_data("cancel", id, revision),
                ),
            ]);
            rows
        }
        QualityStage::Confirming => selected
            .as_ref()
            .map(|selected| {
                vec![vec![InlineKeyboardButton::callback(
                    choice_label(selected, language),
                    callback_data(selected.as_str(), id, revision),
                )]]
            })
            .unwrap_or_default(),
        QualityStage::Submitted | QualityStage::Cancelled => Vec::new(),
    };
    InlineKeyboardMarkup::new(rows)
}

fn render(session: &QualitySession, language: Language) -> String {
    match session.stage {
        QualityStage::Choosing => translate(language, Key::QualityTitle, &[]),
        QualityStage::Confirming => {
            let choice = session
                .selected
                .as_ref()
                .map(|choice| choice_label(choice, language))
                .unwrap_or_default();
            translate(language, Key::QualityConfirming, &[("choice", &choice)])
        }
        QualityStage::Submitted => {
            let id = session
                .operation_id
                .map(|id| id.to_string())
                .unwrap_or_default();
            translate(language, Key::QualitySubmitted, &[("id", &id)])
        }
        QualityStage::Cancelled => translate(language, Key::QualityCancelled, &[]),
    }
}

fn callback_error(error: DownloadError, language: Language) -> String {
    match error {
        DownloadError::Conflict => translate(language, Key::QualityStale, &[]),
        DownloadError::Database | DownloadError::Executor | DownloadError::Task => {
            translate(language, Key::QualityRetry, &[])
        }
        other => download_error_response(other, language),
    }
}

pub(super) async fn send_session(
    bot: &Bot,
    message: &Message,
    session: &QualitySession,
    language: Language,
) {
    let text = render(session, language);
    let mut send = bot.send_message(message.chat.id, text);
    if session.stage == QualityStage::Choosing || session.stage == QualityStage::Confirming {
        send = send.reply_markup(keyboard(session, language));
    }
    let _ = send.await;
}

pub(super) async fn handle_command(
    bot: &Bot,
    message: &Message,
    service: Option<&DownloadService>,
    actor: u64,
    target: DeliveryTarget,
    valid: bool,
    language: Language,
) {
    if !valid {
        let _ = bot
            .send_message(message.chat.id, translate(language, Key::QualityUsage, &[]))
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
    match service.quality_current(actor, target).await {
        Ok(Some(session)) => send_session(bot, message, &session, language).await,
        Ok(None) => {
            let _ = bot
                .send_message(message.chat.id, translate(language, Key::QualityUsage, &[]))
                .await;
        }
        Err(error) => {
            let _ = bot
                .send_message(message.chat.id, download_error_response(error, language))
                .await;
        }
    }
}

fn authorized(query: &CallbackQuery, message: &Message, identity: TelegramIdentity) -> bool {
    callback_context_allowed(
        !query.from.is_bot,
        query.from.id.0 == identity.owner_id,
        message.chat.is_private(),
        message.chat.id.0 > 0 && message.chat.id.0 as u64 == query.from.id.0,
        message.thread_id.is_none(),
        message.sender_chat.is_none(),
        message
            .from
            .as_ref()
            .is_some_and(|user| user.is_bot && user.id.0 == identity.bot_id),
    )
}

fn callback_context_allowed(
    from_human: bool,
    owner: bool,
    private: bool,
    chat_matches_sender: bool,
    no_thread: bool,
    no_sender_chat: bool,
    sent_by_this_bot: bool,
) -> bool {
    from_human
        && owner
        && private
        && chat_matches_sender
        && no_thread
        && no_sender_chat
        && sent_by_this_bot
}

pub(super) async fn handle_callback(
    bot: Bot,
    query: CallbackQuery,
    identity: TelegramIdentity,
    DownloadsDependency(downloads): DownloadsDependency,
) -> Result<(), Infallible> {
    let Some(data) = query
        .data
        .as_deref()
        .filter(|data| *data == "ql" || data.starts_with("ql:"))
    else {
        return Ok(());
    };
    let language = crate::reply_language(
        downloads.as_ref(),
        query.from.id.0,
        query.from.language_code.as_deref(),
    )
    .await;
    let Some(message) = query
        .message
        .as_ref()
        .and_then(|message| message.regular_message())
    else {
        answer(&bot, query.id, translate(language, Key::QualityStale, &[])).await;
        return Ok(());
    };
    if !authorized(&query, message, identity) {
        answer(&bot, query.id, translate(language, Key::ErrorDenied, &[])).await;
        return Ok(());
    }
    let Some(service) = downloads else {
        answer(
            &bot,
            query.id,
            translate(language, Key::DownloadsUnavailable, &[]),
        )
        .await;
        return Ok(());
    };
    let Some(action) = callback(data) else {
        answer(&bot, query.id, translate(language, Key::QualityStale, &[])).await;
        return Ok(());
    };
    let target = match DeliveryTarget::new(identity.bot_id as i64, message.chat.id.0, None, None) {
        Ok(target) => target,
        Err(_) => {
            let _ = bot.answer_callback_query(query.id).await;
            return Ok(());
        }
    };
    let actor = query.from.id.0;
    let session_id = match action {
        Callback::Select(_, id, _) | Callback::Cancel(id, _) => id,
    };
    let result = match action {
        Callback::Select(choice, id, revision) => service
            .quality_confirm(actor, target, id, revision, choice)
            .await
            .map(Outcome::Admitted),
        Callback::Cancel(id, revision) => service
            .quality_cancel(actor, target, id, revision)
            .await
            .map(|session| Outcome::Updated(Box::new(session))),
    };
    match result {
        Ok(Outcome::Updated(session)) => {
            let text = render(&session, language);
            let _ = bot.answer_callback_query(query.id).await;
            let _ = bot
                .edit_message_text(message.chat.id, message.id, text)
                .reply_markup(keyboard(&session, language))
                .await;
        }
        Ok(Outcome::Admitted(admission)) => {
            let operation_id = admission.operation_id;
            let text = admitted_response(admission, language);
            let _ = bot.answer_callback_query(query.id).await;
            if let Ok(Some(session)) = service.quality_current(actor, target).await
                && session.id == session_id
            {
                let _ = bot
                    .edit_message_text(message.chat.id, message.id, render(&session, language))
                    .reply_markup(keyboard(&session, language))
                    .await;
            }
            match bot.send_message(message.chat.id, text).await {
                Ok(sent)
                    if sent.chat.id == message.chat.id
                        && sent.id.0 > 0
                        && sent.thread_id.is_none() =>
                {
                    let locale = if language == Language::Russian {
                        "ru"
                    } else {
                        "en"
                    };
                    if service
                        .register_progress_message(actor, operation_id, sent.id.0, locale)
                        .await
                        .is_err()
                    {
                        tracing::warn!("Telegram quality admission progress registration failed");
                    }
                }
                _ => tracing::warn!("Telegram quality admission response delivery failed"),
            }
        }
        Err(error) => {
            let text = callback_error(error, language);
            answer(&bot, query.id, text).await;
            if let Ok(Some(session)) = service.quality_current(actor, target).await
                && session.id == session_id
            {
                let text = render(&session, language);
                let _ = bot
                    .edit_message_text(message.chat.id, message.id, text)
                    .reply_markup(keyboard(&session, language))
                    .await;
            }
        }
    }
    Ok(())
}

enum Outcome {
    Updated(Box<QualitySession>),
    Admitted(Admission),
}

async fn answer(bot: &Bot, id: teloxide::types::CallbackQueryId, text: String) {
    let _ = bot
        .answer_callback_query(id)
        .text(text.chars().take(180).collect::<String>())
        .show_alert(true)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_wire_format_is_closed_canonical_and_bounded() {
        let id = Uuid::parse_str("38e4f36a14da4f7b8d1d12d2191d8db1").unwrap();
        for choice in [
            "best", "audio", "2160", "1440", "1080", "720", "480", "360", "240", "144",
        ] {
            let data = callback_data(choice, id, 1);
            assert!(data.len() <= 64);
            assert!(callback(&data).is_some());
        }
        for invalid in [
            "ql",
            "ql:best:38e4f36a14da4f7b8d1d12d2191d8db1:0",
            "ql:best:38e4f36a14da4f7b8d1d12d2191d8db1:01",
            "ql:best:38e4f36a14da4f7b8d1d12d2191d8db1:-1",
            "ql:720:38e4f36a14da4f7b8d1d12d2191d8db1:1:extra",
            "ql:0720:38e4f36a14da4f7b8d1d12d2191d8db1:1",
            "ql:ask:38e4f36a14da4f7b8d1d12d2191d8db1:1",
            "ql:unknown:38e4f36a14da4f7b8d1d12d2191d8db1:1",
            "ql:720:uuid:1",
            "quality:720:38e4f36a14da4f7b8d1d12d2191d8db1:1",
            "ql:cancel:38e4f36a14da4f7b8d1d12d2191d8db1:9223372036854775808",
        ] {
            assert!(callback(invalid).is_none(), "{invalid}");
        }
        assert!(callback(&format!("ql:best:{}:1", "a".repeat(65))).is_none());
    }

    #[test]
    fn callback_context_requires_owner_private_bot_message_without_thread_or_sender_chat() {
        let allowed = [true; 7];
        assert!(callback_context_allowed(
            allowed[0], allowed[1], allowed[2], allowed[3], allowed[4], allowed[5], allowed[6]
        ));
        for rejected in 0..allowed.len() {
            let mut context = allowed;
            context[rejected] = false;
            assert!(!callback_context_allowed(
                context[0], context[1], context[2], context[3], context[4], context[5], context[6]
            ));
        }
    }

    #[test]
    fn keyboards_match_closed_callback_choices_in_both_locales_and_stale_errors_are_localized() {
        let id = Uuid::parse_str("38e4f36a14da4f7b8d1d12d2191d8db1").unwrap();
        for language in [Language::English, Language::Russian] {
            let text = translate(language, Key::QualityTitle, &[]);
            assert!(text.contains(if language == Language::English {
                "Choose a download option"
            } else {
                "вариант загрузки"
            }));
            assert!(
                text.to_lowercase()
                    .contains(if language == Language::English {
                        "audio"
                    } else {
                        "аудио"
                    })
            );
            assert!(text.len() < 4096);
            let markup = serde_json::to_value(keyboard_values(
                QualityStage::Choosing,
                id,
                1,
                None,
                language,
            ))
            .unwrap();
            let values = markup["inline_keyboard"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|row| row.as_array().unwrap())
                .map(|button| button["callback_data"].as_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(values.len(), 11);
            assert!(values.iter().all(|value| callback(value).is_some()));
            for choice in [
                "best", "audio", "2160", "1440", "1080", "720", "480", "360", "240", "144",
                "cancel",
            ] {
                assert!(
                    values
                        .iter()
                        .any(|value| value.starts_with(&format!("ql:{choice}:")))
                );
            }
            let confirming = serde_json::to_value(keyboard_values(
                QualityStage::Confirming,
                id,
                2,
                Some(QualityChoice::parse("720").unwrap()),
                language,
            ))
            .unwrap();
            let retry = confirming["inline_keyboard"][0][0]["callback_data"]
                .as_str()
                .unwrap();
            assert_eq!(retry, callback_data("720", id, 2));
            let confirming_audio = serde_json::to_value(keyboard_values(
                QualityStage::Confirming,
                id,
                2,
                Some(parse_choice("audio").unwrap()),
                language,
            ))
            .unwrap();
            assert_eq!(
                confirming_audio["inline_keyboard"][0][0]["callback_data"],
                callback_data("audio", id, 2)
            );
            let audio_label = confirming_audio["inline_keyboard"][0][0]["text"]
                .as_str()
                .unwrap();
            assert_eq!(audio_label, translate(language, Key::QualityAudio, &[]));
            assert!(!audio_label.contains("p"));
            let rendered_audio =
                translate(language, Key::QualityConfirming, &[("choice", audio_label)]);
            assert!(rendered_audio.contains(audio_label));
            assert!(!rendered_audio.contains(if language == Language::English {
                "Up to"
            } else {
                "Не выше"
            }));
            let submitted = serde_json::to_value(keyboard_values(
                QualityStage::Submitted,
                id,
                3,
                Some(QualityChoice::parse("720").unwrap()),
                language,
            ))
            .unwrap();
            assert!(submitted["inline_keyboard"].as_array().unwrap().is_empty());
            let cancelled = serde_json::to_value(keyboard_values(
                QualityStage::Cancelled,
                id,
                4,
                Some(parse_choice("audio").unwrap()),
                language,
            ))
            .unwrap();
            assert!(cancelled["inline_keyboard"].as_array().unwrap().is_empty());
            assert!(callback_error(DownloadError::Conflict, language).contains(
                if language == Language::English {
                    "outdated"
                } else {
                    "устарела"
                }
            ));
        }
    }
}
