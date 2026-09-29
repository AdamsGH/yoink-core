use std::convert::Infallible;

use teloxide::{
    prelude::*,
    types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message},
};
use uuid::Uuid;
use yoink_domain::DeliveryTarget;
use yoink_downloads::{
    CutSession, CutStage, CutSubmission, DownloadError, DownloadService, QualityChoice,
};
use yoink_i18n::{Key, Language};

use crate::{
    DownloadsDependency, TelegramIdentity, admitted_response, download_error_response,
    media_command, translate,
};

#[derive(Debug, Clone, PartialEq)]
enum Callback {
    Go(Uuid, i64),
    Quality(QualityChoice, Uuid, i64),
    Edit(Uuid, i64),
    Cancel(Uuid, i64),
}

#[derive(Debug, Clone, PartialEq)]
enum CallbackError {
    Syntax,
}

fn callback(data: &str) -> Result<Callback, CallbackError> {
    if data.len() > 64 {
        return Err(CallbackError::Syntax);
    }
    let mut fields = data.split(':');
    let (Some("cut"), Some(action), Some(id), Some(revision), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return Err(CallbackError::Syntax);
    };
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CallbackError::Syntax);
    }
    let id = Uuid::parse_str(id).map_err(|_| CallbackError::Syntax)?;
    let revision = revision.parse::<i64>().map_err(|_| CallbackError::Syntax)?;
    if revision <= 0 {
        return Err(CallbackError::Syntax);
    }
    match action {
        "go" => Ok(Callback::Go(id, revision)),
        "qbest" => Ok(Callback::Quality(QualityChoice::Best, id, revision)),
        "q144" => Ok(Callback::Quality(QualityChoice::Height(144), id, revision)),
        "q240" => Ok(Callback::Quality(QualityChoice::Height(240), id, revision)),
        "q360" => Ok(Callback::Quality(QualityChoice::Height(360), id, revision)),
        "q480" => Ok(Callback::Quality(QualityChoice::Height(480), id, revision)),
        "q720" => Ok(Callback::Quality(QualityChoice::Height(720), id, revision)),
        "q1080" => Ok(Callback::Quality(QualityChoice::Height(1080), id, revision)),
        "q1440" => Ok(Callback::Quality(QualityChoice::Height(1440), id, revision)),
        "q2160" => Ok(Callback::Quality(QualityChoice::Height(2160), id, revision)),
        "edit" => Ok(Callback::Edit(id, revision)),
        "cancel" => Ok(Callback::Cancel(id, revision)),
        _ => Err(CallbackError::Syntax),
    }
}

fn encode_values(action: &str, id: Uuid, revision: i64) -> String {
    format!("cut:{action}:{}:{revision}", id.simple())
}

fn keyboard_values(
    stage: CutStage,
    id: Uuid,
    revision: i64,
    language: Language,
) -> InlineKeyboardMarkup {
    let rows = match stage {
        CutStage::Ready => vec![vec![
            InlineKeyboardButton::callback(
                translate(language, Key::CutGo, &[]),
                encode_values("go", id, revision),
            ),
            InlineKeyboardButton::callback(
                translate(language, Key::CutEdit, &[]),
                encode_values("edit", id, revision),
            ),
            InlineKeyboardButton::callback(
                translate(language, Key::CutCancel, &[]),
                encode_values("cancel", id, revision),
            ),
        ]],
        CutStage::Confirming => vec![vec![InlineKeyboardButton::callback(
            translate(language, Key::CutGo, &[]),
            encode_values("go", id, revision),
        )]],
        CutStage::Quality => {
            let mut buttons = vec![InlineKeyboardButton::callback(
                translate(language, Key::QualityBest, &[]),
                format!("cut:qbest:{}:{revision}", id.simple()),
            )];
            for height in [144, 240, 360, 480, 720, 1080, 1440, 2160] {
                buttons.push(InlineKeyboardButton::callback(
                    translate(
                        language,
                        Key::QualityHeight,
                        &[("height", &height.to_string())],
                    ),
                    format!("cut:q{height}:{}:{revision}", id.simple()),
                ));
            }
            let mut rows = buttons
                .chunks(3)
                .map(|row| row.to_vec())
                .collect::<Vec<_>>();
            rows.push(vec![
                InlineKeyboardButton::callback(
                    translate(language, Key::CutEdit, &[]),
                    encode_values("edit", id, revision),
                ),
                InlineKeyboardButton::callback(
                    translate(language, Key::CutCancel, &[]),
                    encode_values("cancel", id, revision),
                ),
            ]);
            rows
        }
        _ => Vec::new(),
    };
    InlineKeyboardMarkup::new(rows)
}

fn render(session: &CutSession, language: Language) -> (String, InlineKeyboardMarkup) {
    let (mut text, mut keyboard) = render_values(
        session.stage,
        session.start,
        session.end,
        session.operation_id,
        session.id,
        session.revision,
        language,
    );
    if session.stage == CutStage::Confirming
        && let Some(choice) = session.selected
    {
        let (label, selected_keyboard) =
            selected_confirmation_keyboard(choice, session.id, session.revision, language);
        text.push('\n');
        text.push_str(&label);
        keyboard = selected_keyboard;
    }
    (text, keyboard)
}

fn selected_confirmation_keyboard(
    choice: QualityChoice,
    id: Uuid,
    revision: i64,
    language: Language,
) -> (String, InlineKeyboardMarkup) {
    let (action, label) = match choice {
        QualityChoice::Best => (
            "qbest".to_owned(),
            translate(language, Key::QualityBest, &[]),
        ),
        QualityChoice::Height(height) => (
            format!("q{height}"),
            translate(
                language,
                Key::QualityHeight,
                &[("height", &height.to_string())],
            ),
        ),
    };
    let callback = encode_values(&action, id, revision);
    (
        label.clone(),
        InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(label, callback)]]),
    )
}

fn render_values(
    stage: CutStage,
    start: Option<f64>,
    end: Option<f64>,
    operation_id: Option<Uuid>,
    id: Uuid,
    revision: i64,
    language: Language,
) -> (String, InlineKeyboardMarkup) {
    let tail = match stage {
        CutStage::Start => translate(language, Key::CutAskStart, &[]),
        CutStage::End => translate(language, Key::CutAskEnd, &[]),
        CutStage::Ready => translate(
            language,
            Key::CutSummary,
            &[("start", &time_display(start)), ("end", &time_display(end))],
        ),
        CutStage::Quality => translate(language, Key::QualityTitle, &[]),
        CutStage::Confirming => translate(language, Key::CutConfirming, &[]),
        CutStage::Submitted => translate(
            language,
            Key::CutSubmitted,
            &[(
                "id",
                &operation_id.map(|id| id.to_string()).unwrap_or_default(),
            )],
        ),
        CutStage::Cancelled => translate(language, Key::CutCancelled, &[]),
    };
    (tail, keyboard_values(stage, id, revision, language))
}

fn time_display(value: Option<f64>) -> String {
    value
        .map(|v| {
            format!("{v:.3}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_owned()
        })
        .unwrap_or_else(|| "0".to_owned())
}

fn invalid_hint(language: Language) -> String {
    translate(language, Key::CutTimeHint, &[])
}

#[derive(PartialEq)]
enum InteractiveForm<'a> {
    Resume,
    Begin(&'a str, Option<f64>),
}

impl std::fmt::Debug for InteractiveForm<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resume => formatter.write_str("Resume"),
            Self::Begin(_, start) => formatter
                .debug_tuple("Begin")
                .field(&"[REDACTED]")
                .field(start)
                .finish(),
        }
    }
}

fn interactive_form<'a>(args: &[&'a str]) -> Option<Result<InteractiveForm<'a>, ()>> {
    if args.len() > 2 || args.iter().any(|arg| arg.starts_with("--")) {
        return None;
    }
    Some(match args {
        [] => Ok(InteractiveForm::Resume),
        [url] => Ok(InteractiveForm::Begin(url, None)),
        [url, start] => {
            media_command::parse_time(start).map(|time| InteractiveForm::Begin(url, Some(time)))
        }
        _ => unreachable!(),
    })
}

fn err_key(error: &DownloadError, language: Language) -> String {
    match error {
        DownloadError::Conflict => translate(language, Key::CutStale, &[]),
        DownloadError::InvalidInput => translate(language, Key::CutInvalidTime, &[]),
        _ => download_error_response(*error, language),
    }
}

pub(super) async fn try_handle_message(
    bot: &Bot,
    message: &Message,
    own_username: &str,
    actor: u64,
    target: DeliveryTarget,
    service: Option<&DownloadService>,
    language: Language,
) -> bool {
    if target.chat_id() <= 0 {
        return false;
    }
    let Some(text) = message.text().map(str::trim) else {
        return false;
    };
    let words = text.split_whitespace().collect::<Vec<_>>();
    if words.first().is_some_and(|first| first.starts_with('/')) {
        let token = match words[0].split_once('@') {
            Some((command, suffix)) if suffix.eq_ignore_ascii_case(own_username) => command,
            Some(_) => return false,
            None => words[0],
        };
        if token.eq_ignore_ascii_case("/cancel") && words.len() == 1 {
            let Some(service) = service else {
                let _ = bot
                    .send_message(
                        message.chat.id,
                        translate(language, Key::DownloadsUnavailable, &[]),
                    )
                    .await;
                return true;
            };
            match service.cut_current(actor, target).await {
                Ok(Some(session))
                    if !matches!(session.stage, CutStage::Cancelled | CutStage::Submitted) =>
                {
                    match service
                        .cut_cancel(
                            actor,
                            target,
                            session.id,
                            session.revision,
                            Some(message.id.0),
                        )
                        .await
                    {
                        Ok(updated) => send_session(bot, message, &updated, language).await,
                        Err(error) => {
                            let _ = bot
                                .send_message(message.chat.id, err_key(&error, language))
                                .await;
                        }
                    }
                }
                Ok(_) => {
                    let _ = bot
                        .send_message(
                            message.chat.id,
                            translate(language, Key::CutNothingToCancel, &[]),
                        )
                        .await;
                }
                Err(error) => {
                    let _ = bot
                        .send_message(message.chat.id, err_key(&error, language))
                        .await;
                }
            }
            return true;
        }
        if token.eq_ignore_ascii_case("/cut") {
            let args = &words[1..];
            let Some(form) = interactive_form(args) else {
                return false;
            };
            let form = match form {
                Ok(form) => form,
                Err(()) => {
                    let _ = bot
                        .send_message(message.chat.id, invalid_hint(language))
                        .await;
                    return true;
                }
            };
            let Some(service) = service else {
                let _ = bot
                    .send_message(
                        message.chat.id,
                        translate(language, Key::DownloadsUnavailable, &[]),
                    )
                    .await;
                return true;
            };
            let result = match form {
                InteractiveForm::Resume => match service.cut_current(actor, target).await {
                    Ok(Some(session)) if session.stage != CutStage::Cancelled => {
                        send_session(bot, message, &session, language).await;
                        return true;
                    }
                    Ok(_) => {
                        let _ = bot
                            .send_message(message.chat.id, translate(language, Key::CutUsage, &[]))
                            .await;
                        return true;
                    }
                    Err(error) => {
                        let _ = bot
                            .send_message(message.chat.id, err_key(&error, language))
                            .await;
                        return true;
                    }
                },
                InteractiveForm::Begin(url, start) => {
                    service
                        .cut_begin(actor, target, message.id.0, url, start)
                        .await
                }
            };
            match result {
                Ok(session) => send_session(bot, message, &session, language).await,
                Err(error) => {
                    let _ = bot
                        .send_message(message.chat.id, err_key(&error, language))
                        .await;
                }
            }
            return true;
        }
        return false;
    }
    let Some(service) = service else {
        return false;
    };
    let Ok(Some(session)) = service.cut_current(actor, target).await else {
        return false;
    };
    if text.eq_ignore_ascii_case("cancel") || text.to_lowercase() == "отмена" {
        if matches!(
            session.stage,
            CutStage::Start | CutStage::End | CutStage::Ready | CutStage::Quality
        ) {
            match service
                .cut_cancel(
                    actor,
                    target,
                    session.id,
                    session.revision,
                    Some(message.id.0),
                )
                .await
            {
                Ok(updated) => send_session(bot, message, &updated, language).await,
                Err(error) => {
                    let _ = bot
                        .send_message(message.chat.id, err_key(&error, language))
                        .await;
                }
            }
            return true;
        }
        return false;
    }
    if !matches!(session.stage, CutStage::Start | CutStage::End) {
        return false;
    }
    let numeric = !text.contains(':') && is_numeric(text);
    let value = match media_command::parse_time(text) {
        Ok(value) => value,
        Err(_) => {
            let _ = bot
                .send_message(message.chat.id, invalid_hint(language))
                .await;
            return true;
        }
    };
    match service
        .cut_input(actor, target, message.id.0, value, numeric)
        .await
    {
        Ok(Some(updated)) => send_session(bot, message, &updated, language).await,
        Ok(None) => {}
        Err(error) => {
            let _ = bot
                .send_message(message.chat.id, err_key(&error, language))
                .await;
        }
    }
    true
}

fn is_numeric(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

pub(super) async fn send_session(
    bot: &Bot,
    source: &Message,
    session: &CutSession,
    language: Language,
) {
    let (text, markup) = render(session, language);
    let send = bot.send_message(source.chat.id, text);
    if matches!(
        session.stage,
        CutStage::Ready | CutStage::Quality | CutStage::Confirming
    ) {
        let _ = send.reply_markup(markup).await;
    } else {
        let _ = send.await;
    }
}

pub(super) async fn handle_callback(
    bot: Bot,
    query: CallbackQuery,
    identity: TelegramIdentity,
    DownloadsDependency(downloads): DownloadsDependency,
) -> Result<(), Infallible> {
    let Some(data) = query.data.as_deref().filter(|d| d.starts_with("cut:")) else {
        return Ok(());
    };
    let language = crate::reply_language(
        downloads.as_ref(),
        query.from.id.0,
        query.from.language_code.as_deref(),
    )
    .await;
    let Some(message) = query.message.as_ref().and_then(|m| m.regular_message()) else {
        answer_alert(&bot, query.id, translate(language, Key::ErrorDenied, &[])).await;
        return Ok(());
    };
    let valid_owner = !query.from.is_bot
        && query.from.id.0 == identity.owner_id
        && message.chat.is_private()
        && message.chat.id.0 > 0
        && message.chat.id.0 as u64 == query.from.id.0
        && message.thread_id.is_none()
        && message.sender_chat.is_none()
        && message
            .from
            .as_ref()
            .is_some_and(|u| u.is_bot && u.id.0 == identity.bot_id);
    if !valid_owner {
        answer_alert(&bot, query.id, translate(language, Key::ErrorDenied, &[])).await;
        return Ok(());
    }
    let Some(service) = downloads else {
        answer_alert(
            &bot,
            query.id,
            translate(language, Key::DownloadsUnavailable, &[]),
        )
        .await;
        return Ok(());
    };
    let action = match callback(data) {
        Ok(action) => action,
        Err(_) => {
            answer_alert(&bot, query.id, translate(language, Key::CutStale, &[])).await;
            return Ok(());
        }
    };
    let target = match DeliveryTarget::new(identity.bot_id as i64, message.chat.id.0, None, None) {
        Ok(target) => target,
        Err(_) => {
            let _ = bot.answer_callback_query(query.id).await;
            return Ok(());
        }
    };
    let actor = query.from.id.0;
    enum Outcome {
        Updated(CutSession),
        Admitted(yoink_storage::Admission),
    }
    let operation =
        match action {
            Callback::Edit(id, revision) => service
                .cut_edit(actor, target, id, revision)
                .await
                .map(Outcome::Updated),
            Callback::Cancel(id, revision) => service
                .cut_cancel(actor, target, id, revision, None)
                .await
                .map(Outcome::Updated),
            Callback::Go(id, revision) => service
                .cut_submit(actor, target, id, revision)
                .await
                .map(|submission| match submission {
                    CutSubmission::Admitted(admission) => Outcome::Admitted(admission),
                    CutSubmission::Quality(session) => Outcome::Updated(session),
                }),
            Callback::Quality(choice, id, revision) => match service
                .cut_choose(actor, target, id, revision, choice)
                .await
            {
                Ok(session) => service
                    .cut_confirm(actor, target, session.id, session.revision)
                    .await
                    .map(Outcome::Admitted),
                Err(error) => Err(error),
            },
        };
    match operation {
        Ok(Outcome::Updated(session)) => {
            let (text, markup) = render(&session, language);
            let _ = bot.answer_callback_query(query.id).await;
            let _ = bot
                .edit_message_text(message.chat.id, message.id, text)
                .reply_markup(markup)
                .await;
        }
        Ok(Outcome::Admitted(admission)) => {
            let operation_id = admission.operation_id;
            let text = admitted_response(admission, language);
            let _ = bot.answer_callback_query(query.id).await;
            match bot.send_message(message.chat.id, text).await {
                Ok(sent)
                    if sent.chat.id == message.chat.id
                        && sent.id.0 > 0
                        && sent.thread_id.is_none() =>
                {
                    let code = if language == Language::Russian {
                        "ru"
                    } else {
                        "en"
                    };
                    if service
                        .register_progress_message(actor, operation_id, sent.id.0, code)
                        .await
                        .is_err()
                    {
                        tracing::warn!("Telegram progress registration failed");
                    }
                }
                _ => tracing::warn!("Telegram cut admission response delivery failed"),
            }
        }
        Err(error) => {
            let alert = match error {
                DownloadError::Conflict => translate(language, Key::CutStale, &[]),
                DownloadError::InvalidInput => translate(language, Key::CutInvalidTime, &[]),
                DownloadError::Database | DownloadError::Executor | DownloadError::Task => {
                    translate(language, Key::CutRetry, &[])
                }
                other => download_error_response(other, language),
            };
            answer_alert(&bot, query.id, alert).await;
        }
    }
    Ok(())
}

async fn answer_alert(bot: &Bot, id: teloxide::types::CallbackQueryId, text: String) {
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
    fn interactive_forms_leave_inline_cut_syntax_for_the_existing_parser() {
        assert_eq!(interactive_form(&[]), Some(Ok(InteractiveForm::Resume)));
        let sensitive =
            interactive_form(&["https://example.test/?credential=source-secret-marker"]);
        assert!(!format!("{sensitive:?}").contains("source-secret-marker"));
        assert_eq!(
            interactive_form(&["https://example.test/v"]),
            Some(Ok(InteractiveForm::Begin("https://example.test/v", None)))
        );
        assert_eq!(
            interactive_form(&["https://example.test/v", "01:30"]),
            Some(Ok(InteractiveForm::Begin(
                "https://example.test/v",
                Some(90.0)
            )))
        );
        assert_eq!(
            interactive_form(&["https://example.test/v", "not-a-time"]),
            Some(Err(()))
        );
        assert_eq!(
            interactive_form(&["https://example.test/v", "10", "20"]),
            None
        );
        assert_eq!(
            interactive_form(&["https://example.test/v", "--height", "720"]),
            None
        );
    }

    #[test]
    fn callbacks_round_trip_are_bounded_and_closed() {
        let id = Uuid::parse_str("38e4f36a14da4f7b8d1d12d2191d8db1").unwrap();
        for action in [
            "go", "edit", "cancel", "qbest", "q144", "q240", "q360", "q480", "q720", "q1080",
            "q1440", "q2160",
        ] {
            let data = encode_values(action, id, 7);
            assert!(data.len() <= 64);
            assert_eq!(
                callback(&data).unwrap(),
                match action {
                    "go" => Callback::Go(id, 7),
                    "edit" => Callback::Edit(id, 7),
                    "cancel" => Callback::Cancel(id, 7),
                    "qbest" => Callback::Quality(QualityChoice::Best, id, 7),
                    height => Callback::Quality(
                        QualityChoice::Height(height[1..].parse().unwrap()),
                        id,
                        7
                    ),
                }
            );
        }
        let too_long = format!("cut:go:{}:1", "a".repeat(65));
        for invalid in [
            "cut:go:uuid:1",
            "cut:go:38e4f36a14da4f7b8d1d12d2191d8db1:-1",
            "cut:go:38e4f36a14da4f7b8d1d12d2191d8db1:1:extra",
            "cut:q999:38e4f36a14da4f7b8d1d12d2191d8db1:1",
            "cut:q720:38e4f36a14da4f7b8d1d12d2191d8db1:0",
            "prefs:menu",
            too_long.as_str(),
        ] {
            assert!(callback(invalid).is_err());
        }
    }

    #[test]
    fn chooser_has_localized_choices_and_selected_confirming_replays_same_callback() {
        let id = Uuid::parse_str("38e4f36a14da4f7b8d1d12d2191d8db1").unwrap();
        for language in [Language::English, Language::Russian] {
            let markup = keyboard_values(CutStage::Quality, id, 11, language);
            let value = serde_json::to_value(markup).unwrap();
            let rows = value["inline_keyboard"].as_array().unwrap();
            let callbacks = rows
                .iter()
                .flat_map(|row| row.as_array().unwrap())
                .map(|button| button["callback_data"].as_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(callbacks.len(), 11);
            for data in callbacks {
                assert!(data.len() <= 64);
                assert!(matches!(
                    callback(data),
                    Ok(Callback::Quality(_, _, 11))
                        | Ok(Callback::Edit(_, 11))
                        | Ok(Callback::Cancel(_, 11))
                ));
            }
            let (label, selected) =
                selected_confirmation_keyboard(QualityChoice::Height(720), id, 12, language);
            let row = serde_json::to_value(selected).unwrap();
            let buttons = row["inline_keyboard"][0].as_array().unwrap();
            assert_eq!(buttons.len(), 1);
            assert_eq!(buttons[0]["text"], label);
            let data = buttons[0]["callback_data"].as_str().unwrap();
            assert!(data.starts_with("cut:q720:"));
            assert_eq!(
                callback(data).unwrap(),
                Callback::Quality(QualityChoice::Height(720), id, 12)
            );
        }
    }

    #[test]
    fn render_is_localized_redacts_source_and_confirming_exposes_only_go() {
        let id = Uuid::parse_str("38e4f36a14da4f7b8d1d12d2191d8db1").unwrap();
        for language in [Language::English, Language::Russian] {
            let (text, markup) =
                render_values(CutStage::Ready, Some(5.0), Some(9.0), None, id, 7, language);
            assert!(text.contains(if language == Language::English {
                "Clip summary"
            } else {
                "Сводка фрагмента"
            }));
            let buttons = serde_json::to_value(markup).unwrap();
            assert_eq!(buttons["inline_keyboard"][0].as_array().unwrap().len(), 3);
            let (text, markup) = render_values(
                CutStage::Confirming,
                Some(5.0),
                Some(9.0),
                None,
                id,
                7,
                language,
            );
            assert!(text.contains(if language == Language::English {
                "Clip summary"
            } else {
                "Сводка фрагмента"
            }));
            let data = serde_json::to_value(markup).unwrap();
            let row = data["inline_keyboard"][0].as_array().unwrap();
            assert_eq!(row.len(), 1);
            assert!(
                row[0]["callback_data"]
                    .as_str()
                    .unwrap()
                    .starts_with("cut:go:")
            );
        }
    }
}
