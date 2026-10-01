use std::convert::Infallible;

use teloxide::{
    prelude::*,
    types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message},
};
use uuid::Uuid;
use yoink_domain::{DeliveryTarget, MediaMode};
use yoink_downloads::{
    DownloadError, DownloadService, QualityChoice, QualitySession, QualityStage,
};
use yoink_i18n::{Key, Language};
use yoink_storage::Admission;

use crate::{
    DownloadsDependency, TelegramIdentity, admitted_response, cut, download_error_response,
    translate,
};

const HEIGHTS: &[&str] = &["2160", "1440", "1080", "720", "480", "360", "240", "144"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Callback {
    Select(QualityChoice, Uuid, i64),
    Cancel(Uuid, i64),
    Cut(Uuid, i64),
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
    } else if action == "cut" {
        Some(Callback::Cut(id, revision))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CutAvailability {
    Ineligible,
    Available,
    ActiveDraft,
    StaleLink,
    WatermarkBlocked,
}

fn cut_watermark_blocked(last_message_id: i32, origin_message_id: i32) -> bool {
    last_message_id >= origin_message_id
}

fn cut_link_current(
    quality_id: Uuid,
    quality_cut_id: Option<Uuid>,
    cut_id: Uuid,
    cut_quality_id: Option<Uuid>,
) -> bool {
    quality_cut_id == Some(cut_id) && cut_quality_id == Some(quality_id)
}

fn cut_availability_for(
    origin_message_id: i32,
    cut: Option<(i32, yoink_downloads::CutStage)>,
) -> CutAvailability {
    let Some((last, stage)) = cut else {
        return CutAvailability::Available;
    };
    if cut_watermark_blocked(last, origin_message_id) {
        return CutAvailability::WatermarkBlocked;
    }
    if matches!(
        stage,
        yoink_downloads::CutStage::Start
            | yoink_downloads::CutStage::End
            | yoink_downloads::CutStage::Ready
            | yoink_downloads::CutStage::Quality
            | yoink_downloads::CutStage::Confirming
    ) {
        CutAvailability::ActiveDraft
    } else {
        CutAvailability::Available
    }
}

async fn cut_availability(
    service: &DownloadService,
    actor: u64,
    target: DeliveryTarget,
    session: &QualitySession,
) -> Result<CutAvailability, DownloadError> {
    if session.stage == QualityStage::Cutting {
        let current = service.cut_current(actor, target).await?;
        return Ok(
            if current.is_some_and(|cut| {
                cut_link_current(
                    session.id,
                    session.cut_session_id,
                    cut.id,
                    cut.linked_quality_id(),
                )
            }) {
                CutAvailability::Available
            } else {
                CutAvailability::StaleLink
            },
        );
    }
    if session.stage != QualityStage::Choosing || !cut_eligible(session) {
        return Ok(CutAvailability::Ineligible);
    }
    let current = service.cut_current(actor, target).await?;
    Ok(cut_availability_for(
        session.origin_message_id,
        current.map(|cut| (cut.last_message_id, cut.stage)),
    ))
}

fn menu_state(
    session: &QualitySession,
    language: Language,
    availability: Result<CutAvailability, DownloadError>,
) -> (InlineKeyboardMarkup, Option<String>) {
    menu_state_values(
        session.stage,
        session.id,
        session.revision,
        session.selected,
        cut_eligible(session),
        language,
        availability,
    )
}

fn menu_state_values(
    stage: QualityStage,
    id: Uuid,
    revision: i64,
    selected: Option<QualityChoice>,
    eligible: bool,
    language: Language,
    availability: Result<CutAvailability, DownloadError>,
) -> (InlineKeyboardMarkup, Option<String>) {
    let (allow_cut, hint) = match availability {
        Ok(CutAvailability::Available) => (true, None),
        Ok(CutAvailability::ActiveDraft) => {
            (false, Some(translate(language, Key::QualityCutActive, &[])))
        }
        Ok(CutAvailability::StaleLink) => {
            (false, Some(translate(language, Key::QualityCutStale, &[])))
        }
        Ok(CutAvailability::WatermarkBlocked) => {
            (false, Some(translate(language, Key::QualityCutStale, &[])))
        }
        Ok(CutAvailability::Ineligible) => (eligible, None),
        Err(error) => (false, Some(callback_error(error, language))),
    };
    (
        keyboard_values(stage, id, revision, selected, allow_cut, language),
        hint,
    )
}

fn cut_eligible(session: &QualitySession) -> bool {
    cut_eligible_request(session.request())
}

fn cut_eligible_request(request: &yoink_domain::DownloadRequest) -> bool {
    request.media_mode() == MediaMode::Video
        && request.params().playlist().is_none()
        && request.params().clips().is_empty()
}

fn keyboard_values(
    stage: QualityStage,
    id: Uuid,
    revision: i64,
    selected: Option<QualityChoice>,
    cut_eligible: bool,
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
            if cut_eligible {
                rows.push(vec![InlineKeyboardButton::callback(
                    translate(language, Key::QualityCut, &[]),
                    callback_data("cut", id, revision),
                )]);
            }
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
        QualityStage::Cutting if cut_eligible => vec![vec![InlineKeyboardButton::callback(
            translate(language, Key::QualityResumeCut, &[]),
            callback_data("cut", id, revision),
        )]],
        QualityStage::Cutting => Vec::new(),
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
        QualityStage::Cutting => translate(language, Key::QualityCutting, &[]),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CutConflictGuidance {
    ActiveDraft,
    WatermarkBlocked,
    Generic,
}

fn classify_cut_conflict(
    action: Callback,
    requested_id: Uuid,
    quality: Option<(Uuid, QualityStage)>,
    quality_origin: i32,
    active_cut: Option<(yoink_downloads::CutStage, i32)>,
) -> CutConflictGuidance {
    if !matches!(action, Callback::Cut(id, _) if id == requested_id)
        || quality != Some((requested_id, QualityStage::Choosing))
    {
        return CutConflictGuidance::Generic;
    }
    let Some((stage, last_message_id)) = active_cut else {
        return CutConflictGuidance::Generic;
    };
    if cut_watermark_blocked(last_message_id, quality_origin) {
        return CutConflictGuidance::WatermarkBlocked;
    }
    if matches!(
        stage,
        yoink_downloads::CutStage::Start
            | yoink_downloads::CutStage::End
            | yoink_downloads::CutStage::Ready
            | yoink_downloads::CutStage::Quality
            | yoink_downloads::CutStage::Confirming
    ) {
        CutConflictGuidance::ActiveDraft
    } else {
        CutConflictGuidance::Generic
    }
}

pub(super) async fn send_session(
    bot: &Bot,
    message: &Message,
    service: &DownloadService,
    actor: u64,
    target: DeliveryTarget,
    session: &QualitySession,
    language: Language,
) {
    let availability = cut_availability(service, actor, target, session).await;
    let (markup, hint) = menu_state(session, language, availability);
    let mut text = render(session, language);
    if let Some(hint) = hint {
        text.push('\n');
        text.push_str(&hint);
    }
    let mut send = bot.send_message(message.chat.id, text);
    if matches!(
        session.stage,
        QualityStage::Choosing | QualityStage::Confirming | QualityStage::Cutting
    ) {
        send = send.reply_markup(markup);
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
        Ok(Some(session)) if session.stage == QualityStage::Cutting => {
            match service.cut_current(actor, target).await {
                Ok(Some(cut))
                    if cut_link_current(
                        session.id,
                        session.cut_session_id,
                        cut.id,
                        cut.linked_quality_id(),
                    ) =>
                {
                    cut::send_session(bot, message, &cut, language).await
                }
                Ok(_) => {
                    let _ = bot
                        .send_message(
                            message.chat.id,
                            translate(language, Key::QualityCutStale, &[]),
                        )
                        .await;
                }
                Err(error) => {
                    let _ = bot
                        .send_message(message.chat.id, download_error_response(error, language))
                        .await;
                }
            }
        }
        Ok(Some(session)) => {
            send_session(bot, message, service, actor, target, &session, language).await
        }
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
        Callback::Select(_, id, _) | Callback::Cancel(id, _) | Callback::Cut(id, _) => id,
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
        Callback::Cut(id, revision) => service
            .quality_cut(actor, target, id, revision)
            .await
            .map(|session| Outcome::Cut(Box::new(session))),
    };
    match result {
        Ok(Outcome::Updated(session)) => {
            let availability = cut_availability(&service, actor, target, &session).await;
            let (markup, hint) = menu_state(&session, language, availability);
            let mut text = render(&session, language);
            if let Some(hint) = hint {
                text.push('\n');
                text.push_str(&hint);
            }
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
            if let Ok(Some(session)) = service.quality_current(actor, target).await
                && session.id == session_id
            {
                let _ = bot
                    .edit_message_text(message.chat.id, message.id, render(&session, language))
                    .reply_markup(keyboard_values(
                        session.stage,
                        session.id,
                        session.revision,
                        session.selected,
                        false,
                        language,
                    ))
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
        Ok(Outcome::Cut(session)) => {
            let _ = bot.answer_callback_query(query.id).await;
            let _ = bot
                .edit_message_text(
                    message.chat.id,
                    message.id,
                    translate(language, Key::QualityCutting, &[]),
                )
                .reply_markup(InlineKeyboardMarkup::new(
                    Vec::<Vec<InlineKeyboardButton>>::new(),
                ))
                .await;
            cut::send_session(&bot, message, &session, language).await;
        }
        Err(error) => {
            let text = if matches!(error, DownloadError::Conflict)
                && matches!(action, Callback::Cut(_, _))
            {
                match service.quality_current(actor, target).await {
                    Ok(Some(current))
                        if current.id == session_id && current.stage == QualityStage::Choosing =>
                    {
                        match service.cut_current(actor, target).await {
                            Ok(Some(cut)) => match classify_cut_conflict(
                                action,
                                session_id,
                                Some((current.id, current.stage)),
                                current.origin_message_id,
                                Some((cut.stage, cut.last_message_id)),
                            ) {
                                CutConflictGuidance::WatermarkBlocked => {
                                    translate(language, Key::QualityCutStale, &[])
                                }
                                CutConflictGuidance::ActiveDraft => {
                                    translate(language, Key::QualityCutActive, &[])
                                }
                                CutConflictGuidance::Generic => callback_error(error, language),
                            },
                            Ok(None) => callback_error(error, language),
                            Err(cut_error) => callback_error(cut_error, language),
                        }
                    }
                    Ok(Some(current))
                        if current.id == session_id && current.stage == QualityStage::Cutting =>
                    {
                        translate(language, Key::QualityCutStale, &[])
                    }
                    Err(quality_error) => callback_error(quality_error, language),
                    _ => callback_error(error, language),
                }
            } else {
                callback_error(error, language)
            };
            answer(&bot, query.id, text).await;
            if let Ok(Some(session)) = service.quality_current(actor, target).await
                && session.id == session_id
            {
                let availability = cut_availability(&service, actor, target, &session).await;
                let (markup, hint) = menu_state(&session, language, availability);
                let mut text = render(&session, language);
                if let Some(hint) = hint {
                    text.push('\n');
                    text.push_str(&hint);
                }
                let _ = bot
                    .edit_message_text(message.chat.id, message.id, text)
                    .reply_markup(markup)
                    .await;
            }
        }
    }
    Ok(())
}

enum Outcome {
    Updated(Box<QualitySession>),
    Admitted(Admission),
    Cut(Box<yoink_downloads::CutSession>),
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
    use yoink_domain::{ClipRange, DownloadParams, DownloadRequest, MediaMode, PlaylistRange};

    fn request(
        mode: MediaMode,
        params: DownloadParams,
        seed: Option<ClipRange>,
    ) -> DownloadRequest {
        DownloadRequest::new(1, "https://example.com/video", mode, seed, params).unwrap()
    }

    #[test]
    fn cut_is_available_only_for_single_video_requests_without_clips() {
        let seed = ClipRange::new(1.0, 2.0).unwrap();
        assert!(cut_eligible_request(&request(
            MediaMode::Video,
            DownloadParams::default(),
            Some(seed),
        )));
        assert!(!cut_eligible_request(&request(
            MediaMode::Video,
            DownloadParams::default().with_playlist(Some(PlaylistRange::new(1, Some(2)).unwrap())),
            None,
        )));
        assert!(!cut_eligible_request(&request(
            MediaMode::Video,
            DownloadParams::default()
                .with_clips(vec![
                    ClipRange::new(1.0, 2.0).unwrap(),
                    ClipRange::new(3.0, 4.0).unwrap(),
                ])
                .unwrap(),
            None,
        )));
        assert!(!cut_eligible_request(&request(
            MediaMode::Audio,
            DownloadParams::default(),
            None
        )));
    }

    #[test]
    fn cut_availability_respects_watermark_and_terminal_stage() {
        use yoink_downloads::CutStage;
        assert_eq!(
            cut_availability_for(100, Some((99, CutStage::Ready))),
            CutAvailability::ActiveDraft
        );
        assert_eq!(
            cut_availability_for(100, Some((100, CutStage::Ready))),
            CutAvailability::WatermarkBlocked
        );
        assert_eq!(
            cut_availability_for(100, Some((101, CutStage::Confirming))),
            CutAvailability::WatermarkBlocked
        );
        assert_eq!(
            cut_availability_for(100, Some((101, CutStage::Cancelled))),
            CutAvailability::WatermarkBlocked
        );
        assert_eq!(
            cut_availability_for(100, Some((99, CutStage::Cancelled))),
            CutAvailability::Available
        );
    }

    #[test]
    fn active_draft_availability_hides_cut_and_shows_guidance_while_retaining_controls() {
        let id = Uuid::new_v4();
        for language in [Language::English, Language::Russian] {
            let (markup, hint) = menu_state_values(
                QualityStage::Choosing,
                id,
                1,
                None,
                true,
                language,
                Ok(CutAvailability::ActiveDraft),
            );
            let markup = serde_json::to_value(markup).unwrap();
            let actions = markup["inline_keyboard"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|row| row.as_array().unwrap())
                .filter_map(|button| button["callback_data"].as_str())
                .collect::<Vec<_>>();
            assert!(!actions.iter().any(|action| action.starts_with("ql:cut:")));
            assert!(actions.iter().any(|action| action.starts_with("ql:audio:")));
            assert!(
                actions
                    .iter()
                    .any(|action| action.starts_with("ql:cancel:"))
            );
            assert!(hint.unwrap().contains(if language == Language::English {
                "active clip"
            } else {
                "активн"
            }));

            let (markup, hint) = menu_state_values(
                QualityStage::Choosing,
                id,
                1,
                None,
                true,
                language,
                Ok(CutAvailability::WatermarkBlocked),
            );
            assert!(hint.unwrap().contains(if language == Language::English {
                "no longer"
            } else {
                "больше не"
            }));
            let cut_available = serde_json::to_value(markup).unwrap()["inline_keyboard"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|row| row.as_array().unwrap())
                .any(|button| {
                    button["callback_data"]
                        .as_str()
                        .is_some_and(|value| value.starts_with("ql:cut:"))
                });
            assert!(!cut_available);
        }
    }

    #[test]
    fn cutting_keyboard_requires_current_link_and_keeps_linked_terminal_resume() {
        let quality_id = Uuid::new_v4();
        let linked_cut_id = Uuid::new_v4();
        assert!(cut_link_current(
            quality_id,
            Some(linked_cut_id),
            linked_cut_id,
            Some(quality_id)
        ));
        assert!(!cut_link_current(
            quality_id,
            Some(quality_id),
            linked_cut_id,
            Some(quality_id)
        ));
        assert!(!cut_link_current(
            quality_id,
            None,
            linked_cut_id,
            Some(quality_id)
        ));
        assert!(!cut_link_current(
            quality_id,
            Some(linked_cut_id),
            linked_cut_id,
            None
        ));
        assert!(!cut_link_current(
            quality_id,
            Some(linked_cut_id),
            linked_cut_id,
            Some(Uuid::new_v4())
        ));
        for language in [Language::English, Language::Russian] {
            let linked =
                keyboard_values(QualityStage::Cutting, quality_id, 2, None, true, language);
            assert_eq!(
                serde_json::to_value(linked).unwrap()["inline_keyboard"][0][0]["callback_data"],
                callback_data("cut", quality_id, 2),
                "cancelled linked rows remain resumable without original_request"
            );
            let stale =
                keyboard_values(QualityStage::Cutting, quality_id, 2, None, false, language);
            assert!(
                serde_json::to_value(stale).unwrap()["inline_keyboard"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn cut_conflict_prioritizes_watermark_over_active_draft_guidance() {
        let id = Uuid::new_v4();
        let choosing = Some((id, QualityStage::Choosing));
        for stage in [
            yoink_downloads::CutStage::Start,
            yoink_downloads::CutStage::End,
            yoink_downloads::CutStage::Ready,
            yoink_downloads::CutStage::Quality,
            yoink_downloads::CutStage::Confirming,
        ] {
            assert_eq!(
                classify_cut_conflict(Callback::Cut(id, 1), id, choosing, 100, Some((stage, 99))),
                CutConflictGuidance::ActiveDraft
            );
            for last in [100, 101] {
                assert_eq!(
                    classify_cut_conflict(
                        Callback::Cut(id, 1),
                        id,
                        choosing,
                        100,
                        Some((stage, last))
                    ),
                    CutConflictGuidance::WatermarkBlocked
                );
            }
        }
        for stage in [
            yoink_downloads::CutStage::Cancelled,
            yoink_downloads::CutStage::Submitted,
        ] {
            assert_eq!(
                classify_cut_conflict(Callback::Cut(id, 1), id, choosing, 100, Some((stage, 99))),
                CutConflictGuidance::Generic
            );
            assert_eq!(
                classify_cut_conflict(Callback::Cut(id, 1), id, choosing, 100, Some((stage, 101))),
                CutConflictGuidance::WatermarkBlocked
            );
        }
        assert_eq!(
            classify_cut_conflict(
                Callback::Cancel(id, 1),
                id,
                choosing,
                100,
                Some((yoink_downloads::CutStage::Ready, 99))
            ),
            CutConflictGuidance::Generic
        );
        assert_eq!(
            classify_cut_conflict(
                Callback::Cut(id, 1),
                id,
                Some((id, QualityStage::Submitted)),
                100,
                Some((yoink_downloads::CutStage::Ready, 99))
            ),
            CutConflictGuidance::Generic
        );
        assert_eq!(
            classify_cut_conflict(
                Callback::Cut(Uuid::new_v4(), 1),
                id,
                choosing,
                100,
                Some((yoink_downloads::CutStage::Ready, 99))
            ),
            CutConflictGuidance::Generic
        );
    }

    #[test]
    fn ineligible_keyboard_keeps_audio_and_height_choices_without_cut() {
        let id = Uuid::new_v4();
        for language in [Language::English, Language::Russian] {
            let markup = serde_json::to_value(keyboard_values(
                QualityStage::Choosing,
                id,
                1,
                None,
                false,
                language,
            ))
            .unwrap();
            let buttons = markup["inline_keyboard"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|row| row.as_array().unwrap())
                .collect::<Vec<_>>();
            let callbacks = buttons
                .iter()
                .map(|button| button["callback_data"].as_str().unwrap())
                .collect::<Vec<_>>();
            assert!(callbacks.iter().any(|data| data.starts_with("ql:audio:")));
            assert!(callbacks.iter().any(|data| data.starts_with("ql:720:")));
            assert!(callbacks.iter().any(|data| data.starts_with("ql:cancel:")));
            assert!(!callbacks.iter().any(|data| data.starts_with("ql:cut:")));
        }
    }

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
        let cut = callback_data("cut", id, 9);
        assert!(cut.len() <= 64);
        assert_eq!(callback(&cut), Some(Callback::Cut(id, 9)));
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
                true,
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
            assert_eq!(values.len(), 12);
            assert!(values.iter().all(|value| callback(value).is_some()));
            for choice in [
                "best", "audio", "2160", "1440", "1080", "720", "480", "360", "240", "144",
                "cancel", "cut",
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
                true,
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
                true,
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
                true,
                language,
            ))
            .unwrap();
            assert!(submitted["inline_keyboard"].as_array().unwrap().is_empty());
            let cancelled = serde_json::to_value(keyboard_values(
                QualityStage::Cancelled,
                id,
                4,
                Some(parse_choice("audio").unwrap()),
                true,
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
            let cutting = serde_json::to_value(keyboard_values(
                QualityStage::Cutting,
                id,
                5,
                None,
                true,
                language,
            ))
            .unwrap();
            let cutting_buttons = cutting["inline_keyboard"][0].as_array().unwrap();
            assert_eq!(cutting_buttons.len(), 1);
            assert_eq!(
                cutting_buttons[0]["callback_data"],
                callback_data("cut", id, 5)
            );
            assert_eq!(
                cutting_buttons[0]["text"],
                translate(language, Key::QualityResumeCut, &[])
            );
            let choosing = serde_json::to_value(keyboard_values(
                QualityStage::Choosing,
                id,
                1,
                None,
                true,
                language,
            ))
            .unwrap();
            let cut_button = choosing["inline_keyboard"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|row| row.as_array().unwrap())
                .find(|button| {
                    button["callback_data"]
                        .as_str()
                        .unwrap()
                        .starts_with("ql:cut:")
                })
                .unwrap();
            assert_eq!(
                cut_button["text"],
                translate(language, Key::QualityCut, &[])
            );
        }
    }
}
