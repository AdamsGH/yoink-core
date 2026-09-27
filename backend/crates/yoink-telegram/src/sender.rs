use std::collections::HashSet;

use teloxide::{
    ApiError, Bot, RequestError,
    payloads::{
        SendAudioSetters, SendDocumentSetters, SendMediaGroupSetters, SendPhotoSetters,
        SendVideoSetters,
    },
    prelude::{ChatId, Requester},
    types::{
        InputFile, InputMedia, InputMediaAudio, InputMediaDocument, InputMediaPhoto,
        InputMediaVideo, Message, MessageId, ReplyParameters, ThreadId,
    },
};
use yoink_domain::DeliveryTarget;
use yoink_downloads::{DeliveredMedia, DeliveryError, DeliveryFuture};
use yoink_executor::ValidatedArtifact;

#[derive(Clone)]
pub(super) struct TelegramDelivery {
    pub(super) bot: Bot,
    pub(super) bot_id: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MediaKind {
    Photo,
    Video,
    Audio,
    Document,
}

fn media_kind(media_type: &str, send_as_file: bool) -> MediaKind {
    if send_as_file {
        return MediaKind::Document;
    }
    match media_type {
        "image/jpeg" | "image/png" => MediaKind::Photo,
        "video/mp4" => MediaKind::Video,
        "audio/mpeg" | "audio/mp4" => MediaKind::Audio,
        _ => MediaKind::Document,
    }
}

fn compatible(left: MediaKind, right: MediaKind) -> bool {
    matches!(
        (left, right),
        (
            MediaKind::Photo | MediaKind::Video,
            MediaKind::Photo | MediaKind::Video
        ) | (MediaKind::Audio, MediaKind::Audio)
            | (MediaKind::Document, MediaKind::Document)
    )
}

fn preflight(media_types: &[&str], send_as_file: bool) -> Result<Vec<MediaKind>, DeliveryError> {
    if media_types.is_empty() || media_types.len() > 10 {
        return Err(DeliveryError::Rejected);
    }
    let kinds = media_types
        .iter()
        .map(|media_type| media_kind(media_type, send_as_file))
        .collect::<Vec<_>>();
    if kinds
        .iter()
        .skip(1)
        .any(|kind| !compatible(kinds[0], *kind))
    {
        return Err(DeliveryError::Rejected);
    }
    Ok(kinds)
}

fn thread_id(target: DeliveryTarget) -> Result<Option<ThreadId>, DeliveryError> {
    target
        .thread_id()
        .map(|id| {
            i32::try_from(id)
                .map(MessageId)
                .map(ThreadId)
                .map_err(|_| DeliveryError::Rejected)
        })
        .transpose()
}

fn input_media(kind: MediaKind, artifact: ValidatedArtifact) -> InputMedia {
    let filename = artifact.filename.clone();
    let file = InputFile::read(tokio::fs::File::from_std(artifact.into_file())).file_name(filename);
    match kind {
        MediaKind::Photo => InputMedia::Photo(InputMediaPhoto::new(file)),
        MediaKind::Video => InputMedia::Video(InputMediaVideo::new(file)),
        MediaKind::Audio => InputMedia::Audio(InputMediaAudio::new(file)),
        MediaKind::Document => InputMedia::Document(InputMediaDocument::new(file)),
    }
}

fn media_file_id(message: &Message, kind: MediaKind) -> Option<String> {
    match kind {
        MediaKind::Photo => message
            .photo()
            .and_then(|photos| photos.last())
            .map(|photo| photo.file.id.0.clone()),
        MediaKind::Video => message.video().map(|video| video.file.id.0.clone()),
        MediaKind::Audio => message.audio().map(|audio| audio.file.id.0.clone()),
        MediaKind::Document => message
            .document()
            .map(|document| document.file.id.0.clone()),
    }
}

fn validate_messages(
    messages: Vec<Message>,
    kinds: &[MediaKind],
    expected_chat: i64,
    expected_thread: Option<ThreadId>,
) -> Result<Vec<DeliveredMedia>, DeliveryError> {
    if messages.len() != kinds.len() {
        return Err(DeliveryError::Unknown);
    }
    let mut ids = HashSet::with_capacity(messages.len());
    let mut receipts = Vec::with_capacity(messages.len());
    for (message, kind) in messages.into_iter().zip(kinds.iter().copied()) {
        let message_id = i64::from(message.id.0);
        let actual_thread = message.thread_id;
        if message_id <= 0
            || !ids.insert(message_id)
            || message.chat.id.0 != expected_chat
            || actual_thread != expected_thread
        {
            return Err(DeliveryError::Unknown);
        }
        let file_id = media_file_id(&message, kind).ok_or(DeliveryError::Unknown)?;
        if file_id.is_empty() {
            return Err(DeliveryError::Unknown);
        }
        receipts.push(DeliveredMedia {
            message_id,
            file_id: Some(file_id),
        });
    }
    Ok(receipts)
}

async fn send_single(
    bot: &Bot,
    target: DeliveryTarget,
    kind: MediaKind,
    artifact: ValidatedArtifact,
    thread: Option<ThreadId>,
) -> Result<Message, RequestError> {
    let chat = ChatId(target.chat_id());
    let filename = artifact.filename.clone();
    let file = InputFile::read(tokio::fs::File::from_std(artifact.into_file())).file_name(filename);
    let reply = target
        .reply_to_message_id()
        .map(|id| ReplyParameters::new(MessageId(id)));
    macro_rules! configure {
        ($request:expr) => {{
            let mut request = $request;
            if let Some(thread) = thread {
                request = request.message_thread_id(thread);
            }
            if let Some(reply) = reply {
                request = request.reply_parameters(reply);
            }
            request.await
        }};
    }
    match kind {
        MediaKind::Photo => configure!(bot.send_photo(chat, file)),
        MediaKind::Video => configure!(bot.send_video(chat, file)),
        MediaKind::Audio => configure!(bot.send_audio(chat, file)),
        MediaKind::Document => configure!(bot.send_document(chat, file)),
    }
}

async fn send_group(
    bot: &Bot,
    target: DeliveryTarget,
    kinds: &[MediaKind],
    artifacts: Vec<ValidatedArtifact>,
    thread: Option<ThreadId>,
) -> Result<Vec<Message>, RequestError> {
    let media: Vec<InputMedia> = kinds
        .iter()
        .copied()
        .zip(artifacts)
        .map(|(kind, artifact)| input_media(kind, artifact))
        .collect();
    let mut request = bot.send_media_group(ChatId(target.chat_id()), media);
    if let Some(thread) = thread {
        request = request.message_thread_id(thread);
    }
    if let Some(reply) = target
        .reply_to_message_id()
        .map(|id| ReplyParameters::new(MessageId(id)))
    {
        request = request.reply_parameters(reply);
    }
    request.await
}

impl yoink_downloads::Delivery for TelegramDelivery {
    fn bot_id(&self) -> i64 {
        self.bot_id
    }

    fn deliver(
        &self,
        target: DeliveryTarget,
        send_as_file: bool,
        artifacts: Vec<ValidatedArtifact>,
    ) -> DeliveryFuture<'_> {
        let bot = self.bot.clone();
        Box::pin(async move {
            // Validate the complete journal unit before moving any file handle or making an API call.
            let media_types = artifacts
                .iter()
                .map(|artifact| artifact.media_type.as_str())
                .collect::<Vec<_>>();
            let kinds = preflight(&media_types, send_as_file)?;
            let thread = thread_id(target)?;
            if artifacts.len() == 1 {
                let message = send_single(
                    &bot,
                    target,
                    kinds[0],
                    artifacts
                        .into_iter()
                        .next()
                        .ok_or(DeliveryError::Rejected)?,
                    thread,
                )
                .await
                .map_err(|error| classify_delivery_error(&error))?;
                validate_messages(vec![message], &kinds, target.chat_id(), thread)
            } else {
                let messages = send_group(&bot, target, &kinds, artifacts, thread)
                    .await
                    .map_err(|error| classify_delivery_error(&error))?;
                validate_messages(messages, &kinds, target.chat_id(), thread)
            }
        })
    }
}

pub(super) fn classify_delivery_error(error: &RequestError) -> DeliveryError {
    // The API wrapper does not preserve HTTP status here. An unrecognized API
    // response may follow a processed upload, so do not infer non-delivery.
    match error {
        RequestError::Api(
            ApiError::BotBlocked
            | ApiError::ChatNotFound
            | ApiError::UserDeactivated
            | ApiError::RequestEntityTooLarge
            | ApiError::InvalidToken,
        ) => DeliveryError::Rejected,
        RequestError::RetryAfter(delay) => DeliveryError::RetryAfter(delay.duration()),
        _ => DeliveryError::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_rejects_empty_over_limit_and_incompatible_groups() {
        assert!(preflight(&[], false).is_err());
        let eleven = vec!["image/jpeg"; 11];
        assert!(preflight(&eleven, false).is_err());
        let mixed = ["image/jpeg", "audio/mpeg"];
        assert!(preflight(&mixed, false).is_err());
        assert_eq!(
            preflight(&mixed, true).unwrap(),
            vec![MediaKind::Document; 2]
        );
        let visual = ["image/jpeg", "video/mp4"];
        assert_eq!(
            preflight(&visual, false).unwrap(),
            vec![MediaKind::Photo, MediaKind::Video]
        );
    }

    fn photo_message(id: i32, chat: i64, thread: Option<i32>) -> Message {
        let thread = thread
            .map(|id| format!(r#","message_thread_id":{id}"#))
            .unwrap_or_default();
        let fixture = format!(
            r#"{{"message_id":{id},"date":0,"chat":{{"id":{chat},"type":"private"}}{thread},"photo":[{{"file_id":"fixture-file-id","file_unique_id":"fixture-unique-id","width":1,"height":1}}]}}"#
        );
        serde_json::from_str(&fixture).expect("safe Telegram response fixture")
    }

    #[test]
    fn response_fixtures_validate_count_ids_chat_thread_and_media() {
        let kinds = [MediaKind::Photo];
        let ok = validate_messages(vec![photo_message(1, 7, None)], &kinds, 7, None).unwrap();
        assert_eq!(ok[0].message_id, 1);
        assert_eq!(ok[0].file_id.as_deref(), Some("fixture-file-id"));
        assert_eq!(
            validate_messages(Vec::new(), &kinds, 7, None),
            Err(DeliveryError::Unknown)
        );
        assert_eq!(
            validate_messages(vec![photo_message(0, 7, None)], &kinds, 7, None),
            Err(DeliveryError::Unknown)
        );
        assert_eq!(
            validate_messages(vec![photo_message(1, 8, None)], &kinds, 7, None),
            Err(DeliveryError::Unknown)
        );
        assert_eq!(
            validate_messages(
                vec![photo_message(1, 7, Some(4))],
                &kinds,
                7,
                Some(ThreadId(MessageId(3)))
            ),
            Err(DeliveryError::Unknown)
        );
        assert_eq!(
            validate_messages(
                vec![photo_message(1, 7, None), photo_message(1, 7, None)],
                &[MediaKind::Photo; 2],
                7,
                None
            ),
            Err(DeliveryError::Unknown)
        );
        let no_photo = serde_json::from_str::<Message>(
            r#"{"message_id":2,"date":0,"chat":{"id":7,"type":"private"}}"#,
        )
        .expect("safe incomplete response fixture");
        assert_eq!(
            validate_messages(vec![no_photo], &kinds, 7, None),
            Err(DeliveryError::Unknown)
        );
    }

    #[test]
    fn classifies_rate_limit_as_retryable_without_changing_permanent_or_unknown() {
        use teloxide::{ApiError, RequestError};
        let delay = teloxide::types::Seconds::from_seconds(2);
        assert_eq!(
            classify_delivery_error(&RequestError::RetryAfter(delay)),
            DeliveryError::RetryAfter(delay.duration())
        );
        for api in [
            ApiError::BotBlocked,
            ApiError::ChatNotFound,
            ApiError::UserDeactivated,
            ApiError::RequestEntityTooLarge,
            ApiError::InvalidToken,
        ] {
            assert_eq!(
                classify_delivery_error(&RequestError::Api(api)),
                DeliveryError::Rejected
            );
        }
        for text in [
            "Internal Server Error",
            "Bad Request: unrecognized fixture failure",
        ] {
            assert_eq!(
                classify_delivery_error(&RequestError::Api(ApiError::Unknown(text.into()))),
                DeliveryError::Unknown
            );
        }
    }
}
