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
use yoink_domain::{CachedMediaKind, DeliveryTarget};
use yoink_downloads::{DeliveredMedia, DeliveryArtifact, DeliveryError, DeliveryFuture};

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

fn cached_kind(kind: CachedMediaKind) -> MediaKind {
    match kind {
        CachedMediaKind::Photo => MediaKind::Photo,
        CachedMediaKind::Video => MediaKind::Video,
        CachedMediaKind::Audio => MediaKind::Audio,
        CachedMediaKind::Document => MediaKind::Document,
    }
}

fn preflight_cached(
    artifacts: &[DeliveryArtifact],
    send_as_file: bool,
) -> Result<Vec<MediaKind>, DeliveryError> {
    if artifacts.is_empty()
        || artifacts.len() > 10
        || artifacts
            .iter()
            .any(|artifact| !matches!(artifact, DeliveryArtifact::Cached(_)))
    {
        return Err(DeliveryError::Rejected);
    }
    let kinds = artifacts
        .iter()
        .map(|artifact| match artifact {
            DeliveryArtifact::Cached(cached) => cached_kind(cached.kind()),
            DeliveryArtifact::Upload(_) => unreachable!(),
        })
        .collect::<Vec<_>>();
    if (send_as_file && kinds.iter().any(|kind| *kind != MediaKind::Document))
        || kinds
            .iter()
            .skip(1)
            .any(|kind| !compatible(kinds[0], *kind))
    {
        return Err(DeliveryError::Rejected);
    }
    Ok(kinds)
}

fn input_file(artifact: DeliveryArtifact) -> InputFile {
    match artifact {
        DeliveryArtifact::Upload(artifact) => {
            let filename = artifact.filename.clone();
            InputFile::read(tokio::fs::File::from_std(artifact.into_file())).file_name(filename)
        }
        DeliveryArtifact::Cached(cached) => {
            InputFile::file_id(teloxide::types::FileId(cached.file_id().to_owned()))
        }
    }
}

fn input_media(kind: MediaKind, artifact: DeliveryArtifact) -> InputMedia {
    let file = input_file(artifact);
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
    artifact: DeliveryArtifact,
    thread: Option<ThreadId>,
) -> Result<Message, RequestError> {
    let chat = ChatId(target.chat_id());
    let file = input_file(artifact);
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
    artifacts: Vec<DeliveryArtifact>,
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
        artifacts: Vec<DeliveryArtifact>,
    ) -> DeliveryFuture<'_> {
        let bot = self.bot.clone();
        Box::pin(async move {
            // Validate the complete unit before moving any file handle or making an API call.
            if artifacts.is_empty() || artifacts.len() > 10 {
                return Err(DeliveryError::Rejected);
            }
            let is_cached = matches!(artifacts.first(), Some(DeliveryArtifact::Cached(_)));
            if artifacts
                .iter()
                .any(|artifact| matches!(artifact, DeliveryArtifact::Cached(_)) != is_cached)
            {
                return Err(DeliveryError::Rejected);
            }
            let kinds = if is_cached {
                preflight_cached(&artifacts, send_as_file)?
            } else {
                let media_types = artifacts
                    .iter()
                    .map(|artifact| match artifact {
                        DeliveryArtifact::Upload(upload) => upload.media_type.as_str(),
                        DeliveryArtifact::Cached(_) => unreachable!(),
                    })
                    .collect::<Vec<_>>();
                preflight(&media_types, send_as_file)?
            };
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
                .map_err(|error| classify_delivery_error_for_origin(&error, is_cached))?;
                validate_messages(vec![message], &kinds, target.chat_id(), thread)
            } else {
                let messages = send_group(&bot, target, &kinds, artifacts, thread)
                    .await
                    .map_err(|error| classify_delivery_error_for_origin(&error, is_cached))?;
                validate_messages(messages, &kinds, target.chat_id(), thread)
            }
        })
    }
}

#[cfg(test)]
pub(super) fn classify_delivery_error(error: &RequestError) -> DeliveryError {
    classify_delivery_error_for_origin(error, false)
}

fn classify_delivery_error_for_origin(error: &RequestError, cached: bool) -> DeliveryError {
    // The API wrapper does not preserve HTTP status here. An unrecognized API
    // response may follow a processed upload, so do not infer non-delivery.
    match error {
        RequestError::Api(
            ApiError::WrongFileId | ApiError::WrongFileIdOrUrl | ApiError::FileIdInvalid,
        ) if cached => DeliveryError::InvalidFileId,
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

    fn cached(kind: CachedMediaKind, id: &str, size: u64) -> DeliveryArtifact {
        DeliveryArtifact::Cached(yoink_domain::CachedMedia::new(kind, id.to_owned(), size).unwrap())
    }

    #[test]
    fn cached_files_select_file_id_input_and_keep_the_id_out_of_debug() {
        let artifact = cached(CachedMediaKind::Video, "cached-test-id", 321);
        assert!(!format!("{artifact:?}").contains("cached-test-id"));
        let input = input_file(artifact);
        let serialized = serde_json::to_string(&input).unwrap();
        assert!(serialized.contains("cached-test-id"));
    }

    #[test]
    fn cached_kinds_grouping_limits_and_send_as_file_validation() {
        let visual = [
            cached(CachedMediaKind::Photo, "photo-id", 1),
            cached(CachedMediaKind::Video, "video-id", 2),
        ];
        assert_eq!(
            preflight_cached(&visual, false).unwrap(),
            [MediaKind::Photo, MediaKind::Video]
        );
        for (kind, expected) in [
            (CachedMediaKind::Audio, MediaKind::Audio),
            (CachedMediaKind::Document, MediaKind::Document),
        ] {
            assert_eq!(
                preflight_cached(&[cached(kind, "same-kind-id", 3)], false).unwrap(),
                [expected]
            );
            assert_eq!(
                preflight_cached(
                    &[
                        cached(kind, "first-same-kind-id", 3),
                        cached(kind, "second-same-kind-id", 4)
                    ],
                    false,
                )
                .unwrap(),
                [expected, expected]
            );
        }
        assert!(preflight_cached(&visual, true).is_err());
        assert_eq!(
            preflight_cached(&[cached(CachedMediaKind::Document, "document-id", 4)], true).unwrap(),
            [MediaKind::Document]
        );
        assert!(
            preflight_cached(
                &[
                    cached(CachedMediaKind::Audio, "audio-id", 1),
                    cached(CachedMediaKind::Document, "doc-id", 1),
                ],
                false
            )
            .is_err()
        );
        let ten = (0..10)
            .map(|index| cached(CachedMediaKind::Photo, &format!("photo-{index}"), 1))
            .collect::<Vec<_>>();
        assert_eq!(preflight_cached(&ten, false).unwrap().len(), 10);
        let eleven = (0..11)
            .map(|index| cached(CachedMediaKind::Photo, &format!("photo-{index}"), 1))
            .collect::<Vec<_>>();
        assert_eq!(
            preflight_cached(&eleven, false),
            Err(DeliveryError::Rejected)
        );
    }

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
        for api in [
            ApiError::WrongFileId,
            ApiError::WrongFileIdOrUrl,
            ApiError::FileIdInvalid,
        ] {
            let error = RequestError::Api(api);
            assert_eq!(
                classify_delivery_error_for_origin(&error, true),
                DeliveryError::InvalidFileId
            );
            assert_eq!(
                classify_delivery_error_for_origin(&error, false),
                DeliveryError::Unknown
            );
        }
        for text in [
            "Internal Server Error",
            "Bad Request: unrecognized fixture failure",
        ] {
            let error = RequestError::Api(ApiError::Unknown(text.into()));
            assert_eq!(classify_delivery_error(&error), DeliveryError::Unknown);
            assert_eq!(
                classify_delivery_error_for_origin(&error, true),
                DeliveryError::Unknown
            );
        }
    }
}
