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
    Add(Uuid, i64),
    EditSegment(usize, Uuid, i64),
    DeleteSegment(usize, Uuid, i64),
    Back(Uuid, i64),
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
        "add" => Ok(Callback::Add(id, revision)),
        "back" => Ok(Callback::Back(id, revision)),
        _ if action.starts_with('e') => parse_segment_action(&action[1..])
            .map(|index| Callback::EditSegment(index, id, revision))
            .ok_or(CallbackError::Syntax),
        _ if action.starts_with('d') => parse_segment_action(&action[1..])
            .map(|index| Callback::DeleteSegment(index, id, revision))
            .ok_or(CallbackError::Syntax),
        "cancel" => Ok(Callback::Cancel(id, revision)),
        _ => Err(CallbackError::Syntax),
    }
}

fn parse_segment_action(value: &str) -> Option<usize> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let index = value.parse::<usize>().ok()?;
    (index < 20).then_some(index)
}

fn encode_values(action: &str, id: Uuid, revision: i64) -> String {
    format!("cut:{action}:{}:{revision}", id.simple())
}

fn keyboard_values(
    stage: CutStage,
    clips: &[yoink_domain::ClipRange],
    id: Uuid,
    revision: i64,
    language: Language,
) -> InlineKeyboardMarkup {
    let rows = match stage {
        CutStage::Ready if !clips.is_empty() => {
            let mut rows = Vec::new();
            if clips.len() < 20 {
                rows.push(vec![InlineKeyboardButton::callback(
                    translate(language, Key::CutAdd, &[]),
                    encode_values("add", id, revision),
                )]);
            }
            for index in 0..clips.len() {
                rows.push(vec![
                    InlineKeyboardButton::callback(
                        translate(
                            language,
                            Key::CutChangeSegment,
                            &[("index", &(index + 1).to_string())],
                        ),
                        encode_values(
                            &if clips.len() == 1 {
                                "edit".to_owned()
                            } else {
                                format!("e{index}")
                            },
                            id,
                            revision,
                        ),
                    ),
                    InlineKeyboardButton::callback(
                        translate(
                            language,
                            Key::CutDeleteSegment,
                            &[("index", &(index + 1).to_string())],
                        ),
                        encode_values(&format!("d{index}"), id, revision),
                    ),
                ]);
            }
            rows.push(vec![
                InlineKeyboardButton::callback(
                    translate(language, Key::CutDownload, &[]),
                    encode_values("go", id, revision),
                ),
                InlineKeyboardButton::callback(
                    translate(language, Key::CutCancel, &[]),
                    encode_values("cancel", id, revision),
                ),
            ]);
            rows
        }
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
            if !clips.is_empty() {
                if clips.len() < 20 {
                    rows.push(vec![InlineKeyboardButton::callback(
                        translate(language, Key::CutAdd, &[]),
                        encode_values("add", id, revision),
                    )]);
                }
                for index in 0..clips.len() {
                    rows.push(vec![
                        InlineKeyboardButton::callback(
                            translate(
                                language,
                                Key::CutChangeSegment,
                                &[("index", &(index + 1).to_string())],
                            ),
                            encode_values(
                                &if clips.len() == 1 {
                                    "edit".to_owned()
                                } else {
                                    format!("e{index}")
                                },
                                id,
                                revision,
                            ),
                        ),
                        InlineKeyboardButton::callback(
                            translate(
                                language,
                                Key::CutDeleteSegment,
                                &[("index", &(index + 1).to_string())],
                            ),
                            encode_values(&format!("d{index}"), id, revision),
                        ),
                    ]);
                }
                rows.push(vec![InlineKeyboardButton::callback(
                    translate(language, Key::CutCancel, &[]),
                    encode_values("cancel", id, revision),
                )]);
            } else {
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
            }
            rows
        }
        CutStage::Start | CutStage::End if !clips.is_empty() => vec![vec![
            InlineKeyboardButton::callback(
                translate(language, Key::CutBack, &[]),
                encode_values("back", id, revision),
            ),
            InlineKeyboardButton::callback(
                translate(language, Key::CutCancel, &[]),
                encode_values("cancel", id, revision),
            ),
        ]],
        _ => Vec::new(),
    };
    InlineKeyboardMarkup::new(rows)
}

fn render(session: &CutSession, language: Language) -> (String, InlineKeyboardMarkup) {
    let (mut text, _) = render_values(
        session.stage,
        session.start,
        session.end,
        session.operation_id,
        session.id,
        session.revision,
        language,
    );
    if (!session.clips.is_empty()
        && matches!(
            session.stage,
            CutStage::Ready | CutStage::Quality | CutStage::Confirming
        ))
        || (!session.clips.is_empty() && matches!(session.stage, CutStage::Start | CutStage::End))
    {
        text = translate(
            language,
            Key::CutRanges,
            &[("ranges", &ranges_display(&session.clips))],
        );
        if matches!(session.stage, CutStage::Start | CutStage::End) {
            let key = if session.stage == CutStage::Start {
                Key::CutAskStart
            } else {
                Key::CutAskEnd
            };
            text = format!("{}\n{text}", translate(language, key, &[]));
        }
        if session.stage == CutStage::Quality {
            text.push('\n');
            text.push_str(&translate(language, Key::CutQualityTitle, &[]));
        }
        if session.stage == CutStage::Confirming {
            text.push('\n');
            text.push_str(&translate(language, Key::CutConfirming, &[]));
        }
    }
    let mut keyboard = keyboard_values(
        session.stage,
        &session.clips,
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
        QualityChoice::Audio => {
            return (
                translate(language, Key::CutStale, &[]),
                InlineKeyboardMarkup::new(Vec::<Vec<InlineKeyboardButton>>::new()),
            );
        }
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

fn ranges_display(clips: &[yoink_domain::ClipRange]) -> String {
    clips
        .iter()
        .enumerate()
        .map(|(index, clip)| {
            format!(
                "{}. {}-{}",
                index + 1,
                time_display(Some(clip.start_seconds())),
                time_display(Some(clip.end_seconds()))
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        CutStage::Quality => translate(language, Key::CutQualityTitle, &[]),
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
    (tail, keyboard_values(stage, &[], id, revision, language))
}

fn time_display(value: Option<f64>) -> String {
    value
        .map(|v| {
            if v >= 1_000_000_000.0 || (v > 0.0 && v < 0.001) {
                format!("{v:.3e}")
            } else {
                format!("{v:.3}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_owned()
            }
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
    ) || (matches!(session.stage, CutStage::Start | CutStage::End) && !session.clips.is_empty())
    {
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
        Updated(Box<CutSession>),
        Admitted(yoink_storage::Admission),
    }
    let operation =
        match action {
            Callback::Edit(id, revision) => service
                .cut_edit(actor, target, id, revision)
                .await
                .map(|session| Outcome::Updated(Box::new(session))),
            Callback::Add(id, revision) => service
                .cut_add(actor, target, id, revision)
                .await
                .map(|session| Outcome::Updated(Box::new(session))),
            Callback::EditSegment(index, id, revision) => service
                .cut_edit_segment(actor, target, id, revision, index)
                .await
                .map(|session| Outcome::Updated(Box::new(session))),
            Callback::DeleteSegment(index, id, revision) => service
                .cut_delete_segment(actor, target, id, revision, index)
                .await
                .map(|session| Outcome::Updated(Box::new(session))),
            Callback::Back(id, revision) => service
                .cut_back(actor, target, id, revision)
                .await
                .map(|session| Outcome::Updated(Box::new(session))),
            Callback::Cancel(id, revision) => service
                .cut_cancel(actor, target, id, revision, None)
                .await
                .map(|session| Outcome::Updated(Box::new(session))),
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
            "go", "edit", "cancel", "add", "e0", "d19", "back", "qbest", "q144", "q240", "q360",
            "q480", "q720", "q1080", "q1440", "q2160",
        ] {
            let data = encode_values(action, id, 7);
            assert!(data.len() <= 64);
            assert_eq!(
                callback(&data).unwrap(),
                match action {
                    "go" => Callback::Go(id, 7),
                    "edit" => Callback::Edit(id, 7),
                    "add" => Callback::Add(id, 7),
                    "e0" => Callback::EditSegment(0, id, 7),
                    "d19" => Callback::DeleteSegment(19, id, 7),
                    "back" => Callback::Back(id, 7),
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
            "cut:e20:38e4f36a14da4f7b8d1d12d2191d8db1:1",
            "cut:d01:38e4f36a14da4f7b8d1d12d2191d8db1:1",
            "cut:e:38e4f36a14da4f7b8d1d12d2191d8db1:1",
            "cut:d-1:38e4f36a14da4f7b8d1d12d2191d8db1:1",
            "prefs:menu",
            too_long.as_str(),
        ] {
            assert!(callback(invalid).is_err());
        }
    }

    #[test]
    fn multi_range_display_and_keyboard_are_bounded_and_ordered_in_both_locales() {
        let id = Uuid::parse_str("38e4f36a14da4f7b8d1d12d2191d8db1").unwrap();
        let mut clips = (0..20)
            .map(|index| {
                yoink_domain::ClipRange::new(index as f64 * 2.0, index as f64 * 2.0 + 1.0).unwrap()
            })
            .collect::<Vec<_>>();
        clips[19] = yoink_domain::ClipRange::new(1.0e300, f64::MAX).unwrap();
        let shown = ranges_display(&clips);
        assert!(shown.contains("20. 1.000e300-1.798e308"));
        for language in [Language::English, Language::Russian] {
            let summary = translate(language, Key::CutRanges, &[("ranges", &shown)]);
            assert!(summary.len() < 4096);
            let markup =
                serde_json::to_value(keyboard_values(CutStage::Ready, &clips, id, 9, language))
                    .unwrap();
            let callbacks = markup["inline_keyboard"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|row| row.as_array().unwrap())
                .map(|button| button["callback_data"].as_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(callbacks.len(), 42);
            assert!(!callbacks.iter().any(|data| data.contains(":add:")));
            assert_eq!(callbacks[0], format!("cut:e0:{}:9", id.simple()));
            assert_eq!(callbacks[1], format!("cut:d0:{}:9", id.simple()));
            assert_eq!(callbacks[38], format!("cut:e19:{}:9", id.simple()));
            assert_eq!(callbacks[39], format!("cut:d19:{}:9", id.simple()));
        }
    }

    #[test]
    fn singleton_ready_and_quality_expose_add_delete_and_legacy_edit() {
        let id = Uuid::new_v4();
        let clips = [yoink_domain::ClipRange::new(0.1, 0.3).unwrap()];
        for language in [Language::English, Language::Russian] {
            for stage in [CutStage::Ready, CutStage::Quality] {
                let markup =
                    serde_json::to_value(keyboard_values(stage, &clips, id, 7, language)).unwrap();
                let actions = markup["inline_keyboard"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|row| row.as_array().unwrap())
                    .map(|button| button["callback_data"].as_str().unwrap())
                    .collect::<Vec<_>>();
                for action in ["add", "edit", "d0", "cancel"] {
                    assert!(actions.contains(&encode_values(action, id, 7).as_str()));
                }
            }
        }
    }

    #[test]
    fn chooser_has_localized_choices_and_selected_confirming_replays_same_callback() {
        let id = Uuid::parse_str("38e4f36a14da4f7b8d1d12d2191d8db1").unwrap();
        for language in [Language::English, Language::Russian] {
            let markup = keyboard_values(CutStage::Quality, &[], id, 11, language);
            let value = serde_json::to_value(markup).unwrap();
            let rows = value["inline_keyboard"].as_array().unwrap();
            let callbacks = rows
                .iter()
                .flat_map(|row| row.as_array().unwrap())
                .map(|button| button["callback_data"].as_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(callbacks.len(), 11);
            let (title, _) = render_values(
                CutStage::Quality,
                Some(1.0),
                Some(2.0),
                None,
                id,
                11,
                language,
            );
            assert!(!title.to_lowercase().contains("audio"));
            assert!(!title.to_lowercase().contains("аудио"));
            assert!(callback(&encode_values("qaudio", id, 11)).is_err());
            let (_, invalid_selected) =
                selected_confirmation_keyboard(QualityChoice::Audio, id, 12, language);
            assert!(
                serde_json::to_value(invalid_selected).unwrap()["inline_keyboard"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            for data in callbacks {
                assert!(data.len() <= 64);
                assert!(matches!(
                    callback(data),
                    Ok(Callback::Quality(_, _, 11))
                        | Ok(Callback::Edit(_, 11))
                        | Ok(Callback::Add(_, 11))
                        | Ok(Callback::EditSegment(_, _, 11))
                        | Ok(Callback::DeleteSegment(_, _, 11))
                        | Ok(Callback::Back(_, 11))
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
