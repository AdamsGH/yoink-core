use std::time::Duration;

use teloxide::{ApiError, Bot, RequestError, prelude::*, types::MessageId};
use yoink_downloads::{DownloadError, DownloadService};
use yoink_i18n::{Key, Language};
use yoink_storage::{OperationRecord, OperationStatus, ProgressNotice, ProgressStage};

use crate::{TelegramError, translate};

#[derive(Clone, Copy, Debug)]
pub struct ProgressConfig {
    interval: Duration,
    edit_timeout: Duration,
}

impl Default for ProgressConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(2),
            edit_timeout: Duration::from_secs(10),
        }
    }
}

impl ProgressConfig {
    pub fn new(interval: Duration, edit_timeout: Duration) -> Result<Self, TelegramError> {
        if [interval, edit_timeout]
            .iter()
            .any(|value| !(1..=60).contains(&value.as_secs()) || value.subsec_nanos() != 0)
        {
            return Err(TelegramError::BotInitializationFailed);
        }
        Ok(Self {
            interval,
            edit_timeout,
        })
    }
}

pub(super) fn render(record: &OperationRecord, language: Language) -> String {
    let key = match record.status {
        OperationStatus::Failed => match record.content_failure.as_deref() {
            Some("nsfw_blocked") => Key::NsfwBlocked,
            Some("nsfw_policy_unavailable") => Key::NsfwPolicyUnavailable,
            _ => failure_key(record.failure_code.as_deref()),
        },
        OperationStatus::Queued if record.failure_code.is_some() => Key::ProgressRetry,
        OperationStatus::Queued => Key::ProgressQueued,
        OperationStatus::Running => match record.progress_stage {
            Some(ProgressStage::Processing) => Key::ProgressProcessing,
            Some(ProgressStage::WritingArtifact) => Key::ProgressWriting,
            _ => Key::ProgressFetching,
        },
        OperationStatus::Delivering => Key::ProgressUploading,
        OperationStatus::Succeeded => Key::ProgressComplete,
        OperationStatus::Cancelled => Key::ProgressCancelled,
        OperationStatus::DeliveryUnknown => Key::DeliveryUnconfirmed,
        OperationStatus::DeliveryPartial => Key::DeliveryPartialHint,
    };
    let mut text = translate(language, key, &[]).trim().to_owned();
    if record.status == OperationStatus::Running
        && let Some(percent) = record.progress_percent
    {
        text.push_str(&translate(
            language,
            Key::ProgressPercent,
            &[("percent", &percent.to_string())],
        ));
    }
    if let Some(total) = record.planned_artifact_count {
        text.push_str(&translate(
            language,
            Key::ProgressDelivered,
            &[
                ("delivered", &record.delivered_artifact_count.to_string()),
                ("total", &total.to_string()),
            ],
        ));
    }
    text
}

fn failure_key(code: Option<&str>) -> Key {
    match code {
        Some("unsupported_source") => Key::DownloadUnsupported,
        Some("source_unavailable") => Key::DownloadUnavailable,
        Some("rate_limited") => Key::DownloadRateLimited,
        Some("invalid_media") => Key::DownloadInvalid,
        Some("processing_failed") => Key::DownloadProcessing,
        Some("resource_limit") => Key::DownloadLimit,
        Some("cookie_required") => Key::DownloadCookie,
        Some("geo_blocked") => Key::DownloadGeo,
        Some("age_restricted") => Key::DownloadAge,
        Some("private_content") => Key::DownloadPrivate,
        Some("live_stream") => Key::DownloadLive,
        Some("no_media") => Key::DownloadNoMedia,
        Some("media_timeout") => Key::DownloadTimeout,
        Some("network_denied") => Key::DownloadNetworkDenied,
        Some("content_unavailable") => Key::DownloadRemoved,
        Some("source_blocked") => Key::DownloadBlocked,
        _ => Key::ErrorOperation,
    }
}

fn terminal(status: OperationStatus) -> bool {
    !matches!(
        status,
        OperationStatus::Queued | OperationStatus::Running | OperationStatus::Delivering
    )
}

#[derive(Debug, PartialEq, Eq)]
enum EditOutcome {
    Acknowledged,
    Stop,
    Retry(Duration),
}

fn classify(error: RequestError, interval: Duration) -> EditOutcome {
    match error {
        RequestError::Api(ApiError::MessageNotModified) => EditOutcome::Acknowledged,
        RequestError::RetryAfter(delay) if delay.duration() <= Duration::from_secs(86400) => {
            EditOutcome::Retry(delay.duration().max(interval))
        }
        RequestError::RetryAfter(_) => EditOutcome::Stop,
        RequestError::Api(ApiError::Unknown(_)) => EditOutcome::Retry(interval),
        RequestError::Api(_) => EditOutcome::Stop,
        _ => EditOutcome::Retry(interval),
    }
}

async fn advance(
    service: &DownloadService,
    notice: &ProgressNotice,
    text: Option<&str>,
    finished: bool,
    delay: Duration,
) {
    if service
        .advance_progress_notice(notice, text, finished, delay)
        .await
        .is_err()
    {
        tracing::warn!("Telegram progress checkpoint failed");
    }
}

async fn update(
    bot: &Bot,
    service: &DownloadService,
    notice: ProgressNotice,
    config: ProgressConfig,
) {
    let message = &notice.message;
    let record = match service.status(message.actor_id, message.operation_id).await {
        Ok(Some(record))
            if record.target.is_some_and(|target| {
                target.bot_id() == message.bot_id && target.chat_id() == message.chat_id
            }) =>
        {
            record
        }
        Ok(_) | Err(DownloadError::Denied) => {
            advance(service, &notice, None, true, config.interval).await;
            return;
        }
        Err(_) => {
            advance(service, &notice, None, false, config.interval).await;
            return;
        }
    };
    let text = render(&record, Language::from_code(Some(&message.language)));
    let finished = terminal(record.status);
    if notice.last_text.as_deref() == Some(text.as_str()) {
        advance(service, &notice, None, finished, config.interval).await;
        return;
    }
    let result = tokio::time::timeout(
        config.edit_timeout,
        bot.edit_message_text(
            ChatId(message.chat_id),
            MessageId(message.message_id),
            text.clone(),
        )
        .send(),
    )
    .await;
    let outcome = match result {
        Ok(Ok(_)) => EditOutcome::Acknowledged,
        Ok(Err(error)) => classify(error, config.interval),
        Err(_) => EditOutcome::Retry(config.interval),
    };
    match outcome {
        EditOutcome::Acknowledged => {
            advance(service, &notice, Some(&text), finished, config.interval).await
        }
        EditOutcome::Stop => advance(service, &notice, None, true, config.interval).await,
        EditOutcome::Retry(delay) => advance(service, &notice, None, false, delay).await,
    }
}

/// One edit at most per interval, with durable fair scheduling. Dropping the future cancels work.
/// Only known message IDs are edited, so retries after timeout or restart cannot duplicate media.
pub(super) async fn run(bot: Bot, service: Option<DownloadService>, config: ProgressConfig) {
    let Some(service) = service else {
        std::future::pending::<()>().await;
        return;
    };
    loop {
        tokio::time::sleep(config.interval).await;
        match service.due_progress_notice().await {
            Ok(Some(notice)) => update(&bot, &service, notice, config).await,
            Ok(None) => {}
            Err(_) => tracing::warn!("Telegram progress lookup failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn public_failure_notices_are_actionable_without_internal_operation_status() {
        for code in [
            "unsupported_source",
            "source_unavailable",
            "rate_limited",
            "invalid_media",
            "processing_failed",
            "resource_limit",
            "cookie_required",
            "geo_blocked",
            "age_restricted",
            "private_content",
            "live_stream",
            "no_media",
            "media_timeout",
            "network_denied",
            "content_unavailable",
            "source_blocked",
            "future_private_code",
        ] {
            let mut record = record(OperationStatus::Failed);
            record.failure_code = Some(code.into());
            record.planned_artifact_count = None;
            for lang in [Language::English, Language::Russian] {
                let text = render(&record, lang);
                assert!(!text.contains("Operation"));
                assert!(!text.contains("попытка"));
                assert!(!text.contains(code));
                assert!(text.len() > 30);
            }
        }
        let mut record = record(OperationStatus::Failed);
        record.failure_code = Some("cookie_required".into());
        assert!(render(&record, Language::Russian).contains("/cookie"));
        assert!(render(&record, Language::English).contains("/cookie"));
    }

    #[test]
    fn nsfw_failures_are_durable_classified_and_localized() {
        for (failure, en, ru) in [
            (
                "nsfw_blocked",
                "NSFW content is not allowed",
                "Контент NSFW запрещён",
            ),
            (
                "nsfw_policy_unavailable",
                "Content policy is unavailable",
                "Политика контента недоступна",
            ),
        ] {
            let mut record = record(OperationStatus::Failed);
            record.content_failure = Some(failure.into());
            assert!(render(&record, Language::English).contains(en));
            assert!(render(&record, Language::Russian).contains(ru));
        }
    }
    use super::*;

    fn record(status: OperationStatus) -> OperationRecord {
        OperationRecord {
            content_failure: None,
            failure_code: None,
            operation_id: uuid::Uuid::nil(),
            actor_id: 42,
            target: None,
            status,
            attempt: 1,
            max_attempts: 1,
            cancel_requested: false,
            cache_bypassed: false,
            available_at: std::time::UNIX_EPOCH.into(),
            delivered_message_id: None,
            planned_artifact_count: Some(11),
            delivered_artifact_count: 10,
            delivery_started_at: None,
            progress_percent: Some(25),
            progress_stage: Some(ProgressStage::Fetching),
            created_at: std::time::UNIX_EPOCH.into(),
            updated_at: std::time::UNIX_EPOCH.into(),
        }
    }

    #[test]
    fn progress_renders_localized_stage_percent_and_confirmed_counts() {
        let running = record(OperationStatus::Running);
        let english = render(&running, Language::English);
        assert!(english.contains("Downloading. 25%."));
        assert!(english.contains("Delivered files: 10/11."));
        let russian = render(&running, Language::Russian);
        assert!(russian.contains("Загрузка. 25%."));
        assert!(russian.contains("Доставлено файлов: 10/11."));
        let partial = render(&record(OperationStatus::DeliveryPartial), Language::English);
        assert!(partial.contains("Some files were already delivered"));
        assert!(!partial.contains("25%"));
        assert!(
            render(&record(OperationStatus::DeliveryUnknown), Language::Russian)
                .contains("Проверьте чат")
        );
    }

    #[test]
    fn terminal_states_stop_updates_and_configuration_is_bounded() {
        for status in [
            OperationStatus::Succeeded,
            OperationStatus::Failed,
            OperationStatus::Cancelled,
            OperationStatus::DeliveryPartial,
            OperationStatus::DeliveryUnknown,
        ] {
            assert!(terminal(status));
        }
        for status in [
            OperationStatus::Queued,
            OperationStatus::Running,
            OperationStatus::Delivering,
        ] {
            assert!(!terminal(status));
        }
        for invalid in [
            Duration::ZERO,
            Duration::from_millis(1500),
            Duration::from_secs(61),
        ] {
            assert!(ProgressConfig::new(invalid, Duration::from_secs(1)).is_err());
            assert!(ProgressConfig::new(Duration::from_secs(1), invalid).is_err());
        }
        assert!(ProgressConfig::new(Duration::from_secs(1), Duration::from_secs(60)).is_ok());
    }

    #[test]
    fn edit_retries_are_safe_but_deleted_messages_are_not_recreated() {
        let interval = Duration::from_secs(2);
        let wire_error: ApiError = serde_json::from_value(serde_json::json!("Bad Request: message is not modified: specified new message content and reply markup are exactly the same as a current content and reply markup of the message")).unwrap();
        assert_eq!(
            classify(RequestError::Api(wire_error), interval),
            EditOutcome::Acknowledged
        );
        assert_eq!(
            classify(RequestError::Api(ApiError::MessageNotModified), interval),
            EditOutcome::Acknowledged
        );
        assert_eq!(
            classify(RequestError::Api(ApiError::MessageToEditNotFound), interval),
            EditOutcome::Stop
        );
        assert_eq!(
            classify(RequestError::Api(ApiError::BotBlocked), interval),
            EditOutcome::Stop
        );
        assert_eq!(
            classify(
                RequestError::Api(ApiError::Unknown("Internal Server Error".into())),
                interval
            ),
            EditOutcome::Retry(interval)
        );
        for (seconds, expected) in [
            (1, EditOutcome::Retry(interval)),
            (3, EditOutcome::Retry(Duration::from_secs(3))),
            (86401, EditOutcome::Stop),
        ] {
            let delay = serde_json::from_value(serde_json::json!(seconds)).unwrap();
            assert_eq!(
                classify(RequestError::RetryAfter(delay), interval),
                expected
            );
        }
    }
}
