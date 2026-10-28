use std::{collections::HashSet, future::Future, task::Poll, time::Duration};

use teloxide::{
    ApiError, Bot, RequestError,
    payloads::{
        SendAudioSetters, SendChatActionSetters, SendDocumentSetters, SendMediaGroupSetters,
        SendMessageSetters, SendPhotoSetters, SendVideoSetters,
    },
    prelude::{ChatId, Requester},
    types::{
        ChatAction, InputFile, InputMedia, InputMediaAudio, InputMediaDocument, InputMediaPhoto,
        InputMediaVideo, Message, MessageId, ParseMode, ReplyParameters, ThreadId,
    },
};
use tokio_util::sync::CancellationToken;
use yoink_domain::{CachedMediaKind, DeliveryTarget};
use yoink_downloads::{
    DeliveredMedia, DeliveryArtifact, DeliveryError, DeliveryFuture, DeliveryPresentation,
};

use crate::media_caption::MediaCaption;

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

fn input_media(
    kind: MediaKind,
    artifact: DeliveryArtifact,
    spoiler: bool,
    caption: Option<&MediaCaption>,
) -> InputMedia {
    let file = input_file(artifact);
    macro_rules! configure {
        ($media:expr) => {{
            let mut media = $media;
            if let Some(caption) = caption {
                media.caption = Some(caption.html.clone());
                media.parse_mode = Some(ParseMode::Html);
            }
            media
        }};
    }
    match kind {
        MediaKind::Photo => {
            let mut media = configure!(InputMediaPhoto::new(file));
            media.has_spoiler = spoiler;
            media.show_caption_above_media = caption.is_some_and(|caption| caption.above);
            InputMedia::Photo(media)
        }
        MediaKind::Video => {
            let mut media = configure!(InputMediaVideo::new(file));
            media.has_spoiler = spoiler;
            media.show_caption_above_media = caption.is_some_and(|caption| caption.above);
            InputMedia::Video(media)
        }
        MediaKind::Audio => InputMedia::Audio(configure!(InputMediaAudio::new(file))),
        MediaKind::Document => InputMedia::Document(configure!(InputMediaDocument::new(file))),
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

fn validate_destination(
    message: &Message,
    expected_chat: i64,
    expected_thread: Option<ThreadId>,
) -> Result<(), DeliveryError> {
    if message.id.0 <= 0
        || message.chat.id.0 != expected_chat
        || message.thread_id != expected_thread
    {
        return Err(DeliveryError::Unknown);
    }
    Ok(())
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
        validate_destination(&message, expected_chat, expected_thread)?;
        if !ids.insert(message_id) {
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
    spoiler: bool,
    caption: Option<&MediaCaption>,
) -> Result<Message, RequestError> {
    let chat = ChatId(target.chat_id());
    let file = input_file(artifact);
    let reply = media_reply(target);
    macro_rules! configure {
        ($request:expr) => {{
            let mut request = $request;
            if let Some(caption) = caption {
                request = request
                    .caption(caption.html.clone())
                    .parse_mode(ParseMode::Html);
            }
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
        MediaKind::Photo => configure!(
            bot.send_photo(chat, file)
                .has_spoiler(spoiler)
                .show_caption_above_media(caption.is_some_and(|caption| caption.above))
        ),
        MediaKind::Video => configure!(
            bot.send_video(chat, file)
                .has_spoiler(spoiler)
                .show_caption_above_media(caption.is_some_and(|caption| caption.above))
        ),
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
    spoiler: bool,
    caption: Option<&MediaCaption>,
) -> Result<Vec<Message>, RequestError> {
    let media: Vec<InputMedia> = kinds
        .iter()
        .copied()
        .zip(artifacts)
        .enumerate()
        .map(|(index, (kind, artifact))| {
            input_media(
                kind,
                artifact,
                spoiler,
                if index == 0 { caption } else { None },
            )
        })
        .collect();
    let mut request = bot.send_media_group(ChatId(target.chat_id()), media);
    if let Some(thread) = thread {
        request = request.message_thread_id(thread);
    }
    if let Some(reply) = media_reply(target) {
        request = request.reply_parameters(reply);
    }
    request.await
}

fn media_reply(target: DeliveryTarget) -> Option<ReplyParameters> {
    // Private input messages are deleted after delivery. Keep their IDs only for cleanup.
    target
        .reply_to_message_id()
        .filter(|_| target.chat_id() < 0)
        .map(|id| ReplyParameters::new(MessageId(id)))
}

async fn guard_upload_start<F>(
    upload: F,
    cancel: &CancellationToken,
) -> Result<Vec<DeliveredMedia>, DeliveryError>
where
    F: Future<Output = Result<Vec<DeliveredMedia>, DeliveryError>>,
{
    tokio::pin!(upload);
    let mut started = false;
    std::future::poll_fn(|cx| {
        if !started {
            if cancel.is_cancelled() {
                return Poll::Ready(Err(DeliveryError::Rejected));
            }
            // The first poll commits the send. Later cancellation cannot discard its receipt.
            started = true;
        }
        upload.as_mut().poll(cx)
    })
    .await
}

async fn upload_action(
    bot: &Bot,
    target: DeliveryTarget,
    kind: MediaKind,
    thread: Option<ThreadId>,
) {
    let action = match kind {
        MediaKind::Photo => ChatAction::UploadPhoto,
        MediaKind::Video => ChatAction::UploadVideo,
        MediaKind::Audio => ChatAction::UploadVoice,
        MediaKind::Document => ChatAction::UploadDocument,
    };
    let mut request = bot.send_chat_action(ChatId(target.chat_id()), action);
    if let Some(thread) = thread {
        request = request.message_thread_id(thread);
    }
    let _ = tokio::time::timeout(Duration::from_secs(4), request).await;
}

async fn upload_activity(
    bot: &Bot,
    target: DeliveryTarget,
    kind: MediaKind,
    thread: Option<ThreadId>,
) {
    // Telegram expires chat actions after five seconds. The upload future owns this loop.
    let refresh = Duration::from_secs(4);
    let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + refresh, refresh);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticks.tick().await;
        upload_action(bot, target, kind, thread).await;
    }
}

impl yoink_downloads::Delivery for TelegramDelivery {
    fn deliver_diagnostic(
        &self,
        target: DeliveryTarget,
        report: yoink_worker_protocol::MediaInfoReport,
    ) -> yoink_downloads::DiagnosticFuture<'_> {
        let bot = self.bot.clone();
        Box::pin(async move {
            if target.chat_id() > 0 && target.thread_id().is_none() && report.valid() {
                let _ = bot
                    .send_message(ChatId(target.chat_id()), report.render())
                    .await;
            }
        })
    }
    fn bot_id(&self) -> i64 {
        self.bot_id
    }

    fn deliver(
        &self,
        target: DeliveryTarget,
        send_as_file: bool,
        artifacts: Vec<DeliveryArtifact>,
    ) -> DeliveryFuture<'_> {
        self.deliver_with_policy(target, send_as_file, false, artifacts)
    }
    fn deliver_with_policy(
        &self,
        target: DeliveryTarget,
        send_as_file: bool,
        spoiler: bool,
        artifacts: Vec<DeliveryArtifact>,
    ) -> DeliveryFuture<'_> {
        self.deliver_media(
            target,
            send_as_file,
            spoiler,
            artifacts,
            None,
            CancellationToken::new(),
        )
    }

    fn deliver_with_presentation(
        &self,
        target: DeliveryTarget,
        send_as_file: bool,
        spoiler: bool,
        presentation: Option<DeliveryPresentation>,
        activity_cancel: CancellationToken,
        artifacts: Vec<DeliveryArtifact>,
    ) -> DeliveryFuture<'_> {
        self.deliver_media(
            target,
            send_as_file,
            spoiler,
            artifacts,
            presentation,
            activity_cancel,
        )
    }
}

impl TelegramDelivery {
    fn deliver_media(
        &self,
        target: DeliveryTarget,
        send_as_file: bool,
        spoiler: bool,
        artifacts: Vec<DeliveryArtifact>,
        presentation: Option<DeliveryPresentation>,
        activity_cancel: CancellationToken,
    ) -> DeliveryFuture<'_> {
        let bot = self.bot.clone();
        Box::pin(async move {
            // Validate the complete unit before moving any file handle or making an API call.
            if artifacts.is_empty() || artifacts.len() > 10 {
                return Err(DeliveryError::Rejected);
            }
            if artifacts.len() == 1
                && matches!(&artifacts[0], DeliveryArtifact::Upload(a) if a.filename == "links.json" && a.media_type == "application/json")
            {
                if target.chat_id() <= 0 || target.thread_id().is_some() || spoiler {
                    return Err(DeliveryError::Rejected);
                }
                let DeliveryArtifact::Upload(upload) = artifacts.into_iter().next().unwrap() else {
                    unreachable!()
                };
                let mut file = tokio::fs::File::from_std(upload.into_file());
                let mut bytes = Vec::new();
                tokio::io::AsyncReadExt::read_to_end(
                    &mut tokio::io::AsyncReadExt::take(
                        &mut file,
                        yoink_worker_protocol::MAX_LINK_REPORT_BYTES as u64 + 1,
                    ),
                    &mut bytes,
                )
                .await
                .map_err(|_| DeliveryError::Rejected)?;
                let links = yoink_worker_protocol::StreamLinks::decode(&bytes)
                    .map_err(|_| DeliveryError::Rejected)?;
                let mut lines = Vec::new();
                if let Some(url) = links.video_url {
                    lines.push(format!("Video stream\n{url}"));
                }
                if let Some(url) = links.audio_url {
                    lines.push(format!("Audio stream\n{url}"));
                }
                let message = bot
                    .send_message(ChatId(target.chat_id()), lines.join("\n\n"))
                    .link_preview_options(teloxide::types::LinkPreviewOptions {
                        is_disabled: true,
                        url: None,
                        prefer_small_media: false,
                        prefer_large_media: false,
                        show_above_text: false,
                    })
                    .await
                    .map_err(|error| classify_delivery_error_for_origin(&error, false))?;
                if !message.chat.is_private() {
                    return Err(DeliveryError::Unknown);
                }
                validate_destination(&message, target.chat_id(), None)?;
                return Ok(vec![DeliveredMedia {
                    message_id: i64::from(message.id.0),
                    file_id: None,
                }]);
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
            if spoiler && target.chat_id() < 0 {
                return Err(DeliveryError::Rejected);
            }
            let thread = thread_id(target)?;
            let caption = presentation
                .as_ref()
                .and_then(|presentation| MediaCaption::new(target, presentation, &artifacts));
            let kind = if kinds.contains(&MediaKind::Video) {
                MediaKind::Video
            } else {
                kinds[0]
            };
            let upload = async {
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
                        spoiler,
                        caption.as_ref(),
                    )
                    .await
                    .map_err(|error| classify_delivery_error_for_origin(&error, is_cached))?;
                    let receipts =
                        validate_messages(vec![message], &kinds, target.chat_id(), thread)?;
                    Ok(receipts)
                } else {
                    let messages = send_group(
                        &bot,
                        target,
                        &kinds,
                        artifacts,
                        thread,
                        spoiler,
                        caption.as_ref(),
                    )
                    .await
                    .map_err(|error| classify_delivery_error_for_origin(&error, is_cached))?;
                    validate_messages(messages, &kinds, target.chat_id(), thread)
                }
            };
            // Announce before polling the upload, including sends that finish in one poll.
            tokio::select! {
                biased;
                _ = activity_cancel.cancelled() => return Err(DeliveryError::Rejected),
                _ = upload_action(&bot, target, kind, thread) => {}
            }
            let upload = guard_upload_start(upload, &activity_cancel);
            let activity = async {
                tokio::select! {
                    biased;
                    _ = activity_cancel.cancelled() => {}
                    _ = upload_activity(&bot, target, kind, thread) => {}
                }
                // Stop chat actions without abandoning a possibly accepted upload receipt.
                std::future::pending::<()>().await;
            };
            tokio::select! {
                biased;
                result = upload => result,
                _ = activity => Err(DeliveryError::Unknown),
            }
        })
    }
}

#[cfg(test)]
mod upload_start_tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_before_the_first_poll_cannot_start_a_send() {
        let cancel = CancellationToken::new();
        let upload = guard_upload_start(
            async { panic!("cancelled send must not be polled") },
            &cancel,
        );
        cancel.cancel();
        assert!(matches!(upload.await, Err(DeliveryError::Rejected)));
    }

    #[tokio::test]
    async fn cancellation_after_the_first_poll_preserves_the_receipt() {
        let cancel = CancellationToken::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let upload = guard_upload_start(async { rx.await.unwrap() }, &cancel);
        tokio::pin!(upload);
        assert!(matches!(futures::poll!(&mut upload), Poll::Pending));
        cancel.cancel();
        tx.send(Ok(vec![DeliveredMedia {
            message_id: 1001,
            file_id: None,
        }]))
        .unwrap();
        assert_eq!(upload.await.unwrap()[0].message_id, 1001);
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
    #[tokio::test]
    async fn media_wire_omits_private_replies_but_preserves_group_replies_and_topics() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        for (chat, topic) in [(43, None), (-43, None), (-43, Some(8))] {
            for grouped in [false, true] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let server = tokio::spawn(async move {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut bytes = Vec::new();
                    let mut chunk = [0; 4096];
                    let (header_end, length) = loop {
                        let n = stream.read(&mut chunk).await.unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&chunk[..n]);
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&bytes[..end]);
                            let length = headers
                                .lines()
                                .find_map(|line| {
                                    let (key, value) = line.split_once(':')?;
                                    key.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().unwrap())
                                })
                                .unwrap();
                            break (end + 4, length);
                        }
                    };
                    while bytes.len() < header_end + length {
                        let n = stream.read(&mut chunk).await.unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&chunk[..n]);
                    }
                    let body = String::from_utf8(bytes[header_end..].to_vec()).unwrap();
                    let response =
                        r#"{"ok":false,"error_code":400,"description":"Bad Request: fixture"}"#;
                    stream.write_all(format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                        response.len()
                    ).as_bytes()).await.unwrap();
                    body
                });
                let bot = Bot::with_client(
                    "123:fixture",
                    reqwest::Client::builder().no_proxy().build().unwrap(),
                )
                .set_api_url(url::Url::parse(&format!("http://{address}/")).unwrap());
                let target = DeliveryTarget::new(123, chat, topic, Some(10)).unwrap();
                let thread = thread_id(target).unwrap();
                let result = if grouped {
                    send_group(
                        &bot,
                        target,
                        &[MediaKind::Photo; 2],
                        vec![
                            cached(CachedMediaKind::Photo, "one", 1),
                            cached(CachedMediaKind::Photo, "two", 1),
                        ],
                        thread,
                        false,
                        None,
                    )
                    .await
                    .map(|_| ())
                } else {
                    send_single(
                        &bot,
                        target,
                        MediaKind::Photo,
                        cached(CachedMediaKind::Photo, "one", 1),
                        thread,
                        false,
                        None,
                    )
                    .await
                    .map(|_| ())
                };
                assert!(result.is_err());
                let body = server.await.unwrap();
                assert_eq!(body.contains("reply_parameters"), chat < 0, "{body}");
                assert_eq!(
                    body.contains("message_thread_id"),
                    topic.is_some(),
                    "{body}"
                );
                if chat < 0 {
                    assert!(body.contains(r#""message_id":10"#), "{body}");
                }
                assert_eq!(target.reply_to_message_id(), Some(10));
            }
        }
    }

    #[test]
    fn nsfw_spoiler_applies_only_to_supported_photo_video_input_media() {
        for kind in [
            MediaKind::Photo,
            MediaKind::Video,
            MediaKind::Audio,
            MediaKind::Document,
        ] {
            let cached = yoink_domain::CachedMedia::new(
                match kind {
                    MediaKind::Photo => CachedMediaKind::Photo,
                    MediaKind::Video => CachedMediaKind::Video,
                    MediaKind::Audio => CachedMediaKind::Audio,
                    MediaKind::Document => CachedMediaKind::Document,
                },
                "opaque_id".into(),
                1024,
            )
            .unwrap();
            let value = serde_json::to_value(input_media(
                kind,
                DeliveryArtifact::Cached(cached),
                true,
                None,
            ))
            .unwrap();
            if matches!(kind, MediaKind::Photo | MediaKind::Video) {
                assert_eq!(value["has_spoiler"], true);
            } else {
                assert!(value.get("has_spoiler").is_none());
            }
        }
    }
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

#[cfg(test)]
mod direct_links_wire_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use yoink_downloads::Delivery;
    #[tokio::test]
    async fn links_are_private_plain_messages_not_uploads_or_format_documents() {
        link_response(serde_json::json!({"message_id":77,"date":0,"chat":{"id":43,"type":"private"},"text":"Streams"}), true).await;
    }
    #[tokio::test]
    async fn links_wrong_chat_thread_or_nonpositive_id_are_unknown() {
        for result in [
            serde_json::json!({"message_id":77,"date":0,"chat":{"id":44,"type":"private"},"text":"Streams"}),
            serde_json::json!({"message_id":77,"date":0,"chat":{"id":43,"type":"private"},"message_thread_id":8,"text":"Streams"}),
            serde_json::json!({"message_id":0,"date":0,"chat":{"id":43,"type":"private"},"text":"Streams"}),
            serde_json::json!({"message_id":77,"date":0,"chat":{"id":43,"type":"group","title":"Wrong kind"},"text":"Streams"}),
        ] {
            link_response(result, false).await;
        }
    }
    #[tokio::test]
    async fn accepted_links_without_a_valid_receipt_are_unknown() {
        link_wire(None, false).await;
        link_response(serde_json::json!({"message_id":77,"date":0}), false).await;
    }
    async fn link_response(result: serde_json::Value, success: bool) {
        link_wire(Some(result), success).await;
    }
    async fn link_wire(result: Option<serde_json::Value>, success: bool) {
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let root = temp.path().join("worker");
        std::fs::create_dir(&root).unwrap();
        let executor = yoink_executor::Executor::new(yoink_executor::ExecutorConfig {
            program: "/usr/bin/env".into(),
            args: vec![
                "node".into(),
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../yoink-downloads/tests/remaining_worker.mjs"
                )
                .into(),
            ],
            workspace_root: root,
            timeout: std::time::Duration::from_secs(15),
            processing_timeout: std::time::Duration::from_secs(10),
            socket_timeout: std::time::Duration::from_secs(3),
            max_artifact_bytes: 4096,
            max_workspace_bytes: 8192,
            max_output_bytes: 8192,
            max_artifacts: 1,
            allowed_proxy_origins: vec![],
            allowed_private_origins: vec![],
        })
        .unwrap();
        let request = yoink_worker_protocol::WorkerRequest {
            version: yoink_worker_protocol::PROTOCOL_VERSION,
            operation_id: uuid::Uuid::new_v4(),
            attempt: 1,
            source_url: yoink_worker_protocol::HttpSourceUrl::parse("https://example.com/source")
                .unwrap(),
            task: yoink_worker_protocol::TaskKind::Download {
                media_mode: yoink_worker_protocol::MediaMode::Links,
            },
            options: Default::default(),
            network: None,
        };
        let artifacts = executor
            .execute(&request, tokio_util::sync::CancellationToken::new())
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            let header_end = loop {
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break end + 4;
                }
                assert!(bytes.len() < 8192);
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            assert!(headers.to_ascii_lowercase().contains("/sendmessage "));
            let length = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            while bytes.len() < header_end + length {
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let body: serde_json::Value =
                serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
            if let Some(result) = result {
                let response = serde_json::json!({"ok":true,"result":result}).to_string();
                let payload = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(),
                    response
                );
                stream.write_all(payload.as_bytes()).await.unwrap();
            }
            body
        });
        let bot = Bot::with_client(
            "123:fixture",
            reqwest::Client::builder().no_proxy().build().unwrap(),
        )
        .set_api_url(url::Url::parse(&format!("http://{address}/")).unwrap());
        let delivery = TelegramDelivery { bot, bot_id: 123 };
        let receipts = delivery
            .deliver(
                DeliveryTarget::new(123, 43, None, None).unwrap(),
                true,
                artifacts
                    .into_iter()
                    .map(DeliveryArtifact::Upload)
                    .collect(),
            )
            .await;
        if success {
            let receipts = receipts.unwrap();
            assert_eq!(receipts[0].message_id, 77);
            assert!(receipts[0].file_id.is_none());
        } else {
            assert!(matches!(receipts, Err(DeliveryError::Unknown)));
        }
        let body = server.await.unwrap();
        assert_eq!(body["chat_id"], 43);
        assert!(body["text"].as_str().unwrap().contains("signature=fixture"));
        assert_eq!(body["link_preview_options"]["is_disabled"], true);
        assert!(body.get("parse_mode").is_none());
    }
}
