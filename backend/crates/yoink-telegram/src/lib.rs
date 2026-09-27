//! Owner-only experimental Telegram adapter with opt-in video download commands.

use sender::TelegramDelivery;
#[cfg(test)]
use sender::classify_delivery_error;
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use teloxide::{
    Bot,
    prelude::*,
    types::Message,
    update_listeners::{Polling, UpdateListener},
};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;
use yoink_application::{ModuleStatus, SystemService};
use yoink_domain::{DeliveryTarget, DownloadParams, DownloadRequest, MediaMode as DomainMediaMode};
use yoink_downloads::{DownloadError, DownloadService};
use yoink_storage::{Admission, OperationStatus};

mod sender;

/// Safe, fixed error categories. Underlying Telegram errors are deliberately discarded.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TelegramError {
    BotInitializationFailed,
    ReadinessCheckFailed,
    PollingFailed,
}

impl std::fmt::Debug for TelegramError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::BotInitializationFailed => "BotInitializationFailed",
            Self::ReadinessCheckFailed => "ReadinessCheckFailed",
            Self::PollingFailed => "PollingFailed",
        })
    }
}

impl std::fmt::Display for TelegramError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::BotInitializationFailed => "Telegram bot initialization failed",
            Self::ReadinessCheckFailed => "Telegram readiness check failed",
            Self::PollingFailed => "Telegram polling failed",
        })
    }
}

impl std::error::Error for TelegramError {}

#[derive(Clone, Copy)]
struct TelegramIdentity {
    owner_id: u64,
    bot_id: u64,
}

/// Owner-only diagnostics and opt-in media delivery over Telegram long polling.
pub struct TelegramAdapter {
    bot: Bot,
    owner_id: u64,
    bot_id: u64,
    bot_username: String,
    system: SystemService,
    downloads: Option<DownloadService>,
    poll_timeout: Duration,
}

impl TelegramAdapter {
    /// Creates the bot and verifies Bot API readiness before returning.
    pub async fn prepare(
        token: String,
        api_url: Option<Url>,
        owner_id: u64,
        system: SystemService,
        poll_timeout: Duration,
    ) -> Result<Self, TelegramError> {
        if owner_id == 0
            || owner_id > i64::MAX as u64
            || !(1..=50).contains(&poll_timeout.as_secs())
            || poll_timeout.subsec_nanos() != 0
        {
            return Err(TelegramError::BotInitializationFailed);
        }

        let mut bot = Bot::new(token);
        if let Some(api_url) = api_url {
            bot = bot.set_api_url(api_url);
        }

        let me = bot
            .get_me()
            .await
            .map_err(|_| TelegramError::ReadinessCheckFailed)?;
        let bot_username = me
            .user
            .username
            .clone()
            .ok_or(TelegramError::BotInitializationFailed)?;

        Ok(Self {
            bot,
            owner_id,
            bot_id: me.user.id.0,
            bot_username,
            system,
            downloads: None,
            poll_timeout,
        })
    }

    /// Enables the progressive video-download slice.
    pub fn with_downloads(mut self, service: DownloadService) -> Self {
        self.downloads = Some(service);
        self
    }

    /// Returns a clone-backed delivery implementation sharing this bot's client.
    pub fn delivery(&self) -> Arc<dyn yoink_downloads::Delivery> {
        Arc::new(TelegramDelivery {
            bot: self.bot.clone(),
            bot_id: i64::try_from(self.bot_id).unwrap_or_default(),
        })
    }

    /// Polls until cancellation or the first polling failure.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), TelegramError> {
        if shutdown.is_cancelled() {
            return Ok(());
        }

        let mut listener = Polling::builder(self.bot.clone())
            .timeout(self.poll_timeout)
            .build();
        let stop = listener.stop_token();
        let failed = Arc::new(AtomicBool::new(false));
        let handler = Update::filter_message().endpoint(handle_message);
        let mut dispatcher = Dispatcher::builder(self.bot.clone(), handler)
            .dependencies(dptree::deps![
                TelegramIdentity {
                    owner_id: self.owner_id,
                    bot_id: self.bot_id
                },
                self.bot_username,
                self.system,
                DownloadsDependency(self.downloads)
            ])
            .default_handler(|_| async {})
            .error_handler(teloxide::error_handlers::IgnoringErrorHandlerSafe::new())
            .build();

        let error_handler = {
            let stop = stop.clone();
            let failed = failed.clone();
            Arc::new(move |_error: teloxide::RequestError| {
                failed.store(true, Ordering::SeqCst);
                stop.stop();
                async {}
            })
        };
        let dispatch = dispatcher.try_dispatch_with_listener(listener, error_handler);
        tokio::pin!(dispatch);

        tokio::select! {
            _ = shutdown.cancelled() => {
                stop.stop();
                (&mut dispatch)
                    .await
                    .map_err(|_| TelegramError::PollingFailed)?;
            }
            result = &mut dispatch => {
                result.map_err(|_| TelegramError::PollingFailed)?;
                if !shutdown.is_cancelled() {
                    return Err(TelegramError::PollingFailed);
                }
            }
        }

        if failed.load(Ordering::SeqCst) {
            return Err(TelegramError::PollingFailed);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct DownloadsDependency(Option<DownloadService>);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Command {
    Start,
    Help,
    Status,
    Download(String),
    Job(Uuid),
    Cancel(Uuid),
    Invalid,
}

fn parse_command(text: &str, own_username: &str) -> Option<Command> {
    let mut words = text.split_whitespace();
    let token = words.next()?;
    let command = token.strip_prefix('/')?;
    let name = match command.split_once('@') {
        Some((name, suffix)) if suffix.eq_ignore_ascii_case(own_username) => name,
        Some(_) => return None,
        None => command,
    };

    match name.to_ascii_lowercase().as_str() {
        "start" => {
            if words.next().is_none() {
                Some(Command::Start)
            } else {
                Some(Command::Invalid)
            }
        }
        "help" => {
            if words.next().is_none() {
                Some(Command::Help)
            } else {
                Some(Command::Invalid)
            }
        }
        "status" => {
            if words.next().is_none() {
                Some(Command::Status)
            } else {
                Some(Command::Invalid)
            }
        }
        "download" => {
            let Some(url) = words.next() else {
                return Some(Command::Invalid);
            };
            if words.next().is_some() || url.len() > 4096 || !is_http_url(url) {
                return Some(Command::Invalid);
            }
            Some(Command::Download(url.to_owned()))
        }
        "job" => {
            let Some(id) = words.next() else {
                return Some(Command::Invalid);
            };
            if words.next().is_some() {
                return Some(Command::Invalid);
            }
            Some(
                Uuid::parse_str(id)
                    .ok()
                    .map_or(Command::Invalid, Command::Job),
            )
        }
        "cancel" => {
            let Some(id) = words.next() else {
                return Some(Command::Invalid);
            };
            if words.next().is_some() {
                return Some(Command::Invalid);
            }
            Some(
                Uuid::parse_str(id)
                    .ok()
                    .map_or(Command::Invalid, Command::Cancel),
            )
        }
        _ => None,
    }
}

fn is_http_url(value: &str) -> bool {
    Url::parse(value)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
}

fn is_owner_private(owner_id: u64, message: &Message) -> bool {
    has_owner_private_access(
        owner_id,
        message.chat.is_private(),
        message.from.as_ref().map(|user| user.id.0),
    )
}

fn has_owner_private_access(owner_id: u64, chat_is_private: bool, sender_id: Option<u64>) -> bool {
    chat_is_private && sender_id == Some(owner_id)
}

#[derive(Clone, Copy)]
enum Language {
    English,
    Russian,
}

fn language_for(code: Option<&str>) -> Language {
    if code.is_some_and(|code| code.to_ascii_lowercase().starts_with("ru")) {
        Language::Russian
    } else {
        Language::English
    }
}

fn response(command: Command, language: Language, system: &SystemService, enabled: bool) -> String {
    match (language, command) {
        (Language::English, Command::Start | Command::Help) if enabled => {
            "Available diagnostics: /start, /help, /status. Experimental video only: /download <http(s) URL>, /job <id>, /cancel <id>.".to_owned()
        }
        (Language::Russian, Command::Start | Command::Help) if enabled => {
            "Доступна диагностика: /start, /help, /status. Экспериментально, только видео: /download <http(s) URL>, /job <id>, /cancel <id>.".to_owned()
        }
        (Language::English, Command::Start | Command::Help) => {
            "Available diagnostics: /start, /help, /status.".to_owned()
        }
        (Language::Russian, Command::Start | Command::Help) => {
            "Доступна диагностика: /start, /help, /status.".to_owned()
        }
        (Language::English, Command::Status) => format_status(system, Language::English),
        (Language::Russian, Command::Status) => format_status(system, Language::Russian),
        (Language::English, Command::Invalid) if enabled => {
            "Usage: /download <http(s) URL>, /job <id>, /cancel <id>.".to_owned()
        }
        (Language::Russian, Command::Invalid) if enabled => {
            "Формат: /download <http(s) URL>, /job <id>, /cancel <id>.".to_owned()
        }
        (Language::English, Command::Invalid) => "Experimental downloads are unavailable.".to_owned(),
        (Language::Russian, Command::Invalid) => "Экспериментальные загрузки недоступны.".to_owned(),
        _ => String::new(),
    }
}

fn format_status(system: &SystemService, language: Language) -> String {
    let snapshot = system.snapshot();
    let readiness = if snapshot.ready {
        match language {
            Language::English => "ready",
            Language::Russian => "готов",
        }
    } else {
        match language {
            Language::English => "not ready",
            Language::Russian => "не готов",
        }
    };
    let planned = snapshot
        .modules
        .iter()
        .filter(|module| module.status == ModuleStatus::Planned)
        .map(|module| module.id)
        .collect::<Vec<_>>()
        .join(", ");

    match language {
        Language::English => format!(
            "Yoink {}: {}. Business modules are planned, not available: {}.",
            snapshot.version, readiness, planned
        ),
        Language::Russian => format!(
            "Yoink {}: {}. Бизнес-модули запланированы и пока недоступны: {}.",
            snapshot.version, readiness, planned
        ),
    }
}

fn download_error_response(error: DownloadError, language: Language) -> &'static str {
    match (language, error) {
        (Language::English, DownloadError::Denied) => "This operation is not allowed.",
        (Language::Russian, DownloadError::Denied) => "Эта операция запрещена.",
        (Language::English, DownloadError::Capacity) => {
            "The download queue is full. Try again later."
        }
        (Language::Russian, DownloadError::Capacity) => {
            "Очередь загрузок заполнена. Попробуйте позже."
        }
        (Language::English, DownloadError::Conflict) => {
            "This request conflicts with an existing operation."
        }
        (Language::Russian, DownloadError::Conflict) => {
            "Этот запрос конфликтует с существующей операцией."
        }
        (Language::English, DownloadError::InvalidInput) => "The request is invalid.",
        (Language::Russian, DownloadError::InvalidInput) => "Некорректный запрос.",
        (Language::English, DownloadError::InvalidConfig) => {
            "Downloads are temporarily unavailable."
        }
        (Language::Russian, DownloadError::InvalidConfig) => "Загрузки временно недоступны.",
        (Language::English, DownloadError::Database) => "The operation could not be completed.",
        (Language::Russian, DownloadError::Database) => "Не удалось выполнить операцию.",
        (Language::English, DownloadError::Executor) => "The operation could not be completed.",
        (Language::Russian, DownloadError::Executor) => "Не удалось выполнить операцию.",
        (Language::English, DownloadError::Task) => "The operation could not be completed.",
        (Language::Russian, DownloadError::Task) => "Не удалось выполнить операцию.",
    }
}

fn operation_status(status: OperationStatus, language: Language) -> &'static str {
    match (language, status) {
        (Language::English, OperationStatus::Queued) => "queued",
        (Language::Russian, OperationStatus::Queued) => "в очереди",
        (Language::English, OperationStatus::Running) => "running",
        (Language::Russian, OperationStatus::Running) => "выполняется",
        (Language::English, OperationStatus::Delivering) => "delivering",
        (Language::Russian, OperationStatus::Delivering) => "отправляется",
        (Language::English, OperationStatus::DeliveryUnknown) => "delivery could not be confirmed",
        (Language::Russian, OperationStatus::DeliveryUnknown) => "не удалось подтвердить доставку",
        (Language::English, OperationStatus::DeliveryPartial) => "partially delivered",
        (Language::Russian, OperationStatus::DeliveryPartial) => "доставлено частично",
        (Language::English, OperationStatus::Succeeded) => "succeeded",
        (Language::Russian, OperationStatus::Succeeded) => "завершено",
        (Language::English, OperationStatus::Failed) => "failed",
        (Language::Russian, OperationStatus::Failed) => "ошибка",
        (Language::English, OperationStatus::Cancelled) => "cancelled",
        (Language::Russian, OperationStatus::Cancelled) => "отменено",
    }
}

fn operation_response(
    id: Uuid,
    status: OperationStatus,
    attempt: u32,
    delivered_message_id: Option<i64>,
    language: Language,
) -> String {
    let mut reply = match language {
        Language::English => format!(
            "Operation {id}: {} (attempt {attempt}).",
            operation_status(status, language)
        ),
        Language::Russian => format!(
            "Операция {id}: {} (попытка {attempt}).",
            operation_status(status, language)
        ),
    };
    if status == OperationStatus::DeliveryUnknown {
        reply.push_str(match language {
            Language::English => {
                " Delivery could not be confirmed. Check the chat before submitting again."
            }
            Language::Russian => {
                " Не удалось подтвердить доставку. Проверьте чат перед повторной отправкой."
            }
        });
    }
    if let Some(message_id) = delivered_message_id {
        reply.push_str(&match language {
            Language::English => format!(" Delivered message: {message_id}."),
            Language::Russian => format!(" Сообщение доставлено: {message_id}."),
        });
    }
    reply
}

fn admitted_response(admission: Admission, language: Language) -> String {
    let replay = match (language, !admission.created) {
        (Language::English, true) => " Existing operation",
        (Language::Russian, true) => " Существующая операция",
        (Language::English, false) => " Operation created",
        (Language::Russian, false) => " Операция создана",
    };
    format!("{replay}: {}.", admission.operation_id)
}

async fn handle_message(
    bot: Bot,
    message: Message,
    identity: TelegramIdentity,
    bot_username: String,
    system: SystemService,
    DownloadsDependency(downloads): DownloadsDependency,
) -> Result<(), Infallible> {
    let TelegramIdentity { owner_id, bot_id } = identity;
    // This authorization check is deliberately the first boundary for all commands.
    if !is_owner_private(owner_id, &message) {
        return Ok(());
    }

    let Some(text) = message.text() else {
        return Ok(());
    };
    let Some(command) = parse_command(text, &bot_username) else {
        return Ok(());
    };
    let language = language_for(
        message
            .from
            .as_ref()
            .and_then(|user| user.language_code.as_deref()),
    );

    let reply = match command {
        Command::Start | Command::Help | Command::Status | Command::Invalid => {
            response(command, language, &system, downloads.is_some())
        }
        Command::Download(_) | Command::Job(_) | Command::Cancel(_) if downloads.is_none() => {
            match language {
                Language::English => "Experimental downloads are unavailable.",
                Language::Russian => "Экспериментальные загрузки недоступны.",
            }
            .to_owned()
        }
        Command::Download(url) => {
            let Some(service) = downloads.as_ref() else {
                return Ok(());
            };
            let key = format!("tg:{bot_id}:{}:{}", message.chat.id.0, message.id.0);
            let target =
                DeliveryTarget::new(bot_id as i64, message.chat.id.0, None, Some(message.id.0))
                    .map_err(|_| DownloadError::InvalidInput);
            let request = target.and_then(|target| {
                DownloadRequest::new(
                    owner_id,
                    &url,
                    DomainMediaMode::Video,
                    None,
                    DownloadParams::default(),
                )
                .map_err(|_| DownloadError::InvalidInput)?
                .with_target(target)
                .map_err(|_| DownloadError::InvalidInput)
            });
            match request {
                Ok(request) => match service.submit_request(&request, &key).await {
                    Ok(admission) => admitted_response(admission, language),
                    Err(error) => download_error_response(error, language).to_owned(),
                },
                Err(error) => download_error_response(error, language).to_owned(),
            }
        }
        Command::Job(id) => {
            let Some(service) = downloads.as_ref() else {
                return Ok(());
            };
            match service.status(owner_id, id).await {
                Ok(Some(record)) => operation_response(
                    id,
                    record.status,
                    record.attempt,
                    record.delivered_message_id,
                    language,
                ),
                Ok(None) => unknown_operation(language),
                Err(error) => download_error_response(error, language).to_owned(),
            }
        }
        Command::Cancel(id) => {
            let Some(service) = downloads.as_ref() else {
                return Ok(());
            };
            match service.cancel(owner_id, id).await {
                Ok(true) => match language {
                    Language::English => format!("Cancellation requested for operation {id}."),
                    Language::Russian => format!("Запрошена отмена операции {id}."),
                },
                Ok(false) => unknown_operation(language),
                Err(error) => download_error_response(error, language).to_owned(),
            }
        }
    };

    if bot.send_message(message.chat.id, reply).await.is_err() {
        tracing::warn!("Telegram response delivery failed");
    }
    Ok(())
}

fn unknown_operation(language: Language) -> String {
    match language {
        Language::English => "Operation not found.",
        Language::Russian => "Операция не найдена.",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoink_downloads::DeliveryError;

    #[test]
    fn parses_supported_commands_with_strict_arguments_and_bot_suffix() {
        assert_eq!(parse_command("/start", "yoink_bot"), Some(Command::Start));
        assert_eq!(
            parse_command("/HELP@yoink_bot", "yoink_bot"),
            Some(Command::Help)
        );
        assert_eq!(parse_command("/status@OtherBot", "yoink_bot"), None);
        assert_eq!(
            parse_command("/status@yoink_bot extra", "yoink_bot"),
            Some(Command::Invalid)
        );
        assert_eq!(
            parse_command("/download https://example.test/video", "yoink_bot"),
            Some(Command::Download("https://example.test/video".into()))
        );
        assert_eq!(
            parse_command("/download https://example.test/a b", "yoink_bot"),
            Some(Command::Invalid)
        );
        assert_eq!(
            parse_command("/download https://example.test", "yoink_bot"),
            Some(Command::Download("https://example.test".into()))
        );
        assert_eq!(
            parse_command("/download ftp://example.test/a", "yoink_bot"),
            Some(Command::Invalid)
        );
        let overlong_url = format!("https://e/{}", "x".repeat(4087));
        assert_eq!(overlong_url.len(), 4097);
        assert_eq!(
            parse_command(&format!("/download {overlong_url}"), "yoink_bot"),
            Some(Command::Invalid)
        );
        let url = format!("https://e/{}", "x".repeat(4086));
        assert_eq!(url.len(), 4096);
        assert_eq!(
            parse_command(&format!("/download {url}"), "yoink_bot"),
            Some(Command::Download(url))
        );
        assert_eq!(
            parse_command("/job 38e4f36a-14da-4f7b-8d1d-12d2191d8db1", "yoink_bot"),
            Some(Command::Job(
                Uuid::parse_str("38e4f36a-14da-4f7b-8d1d-12d2191d8db1").unwrap()
            ))
        );
        assert_eq!(
            parse_command(
                "/cancel 38e4f36a-14da-4f7b-8d1d-12d2191d8db1 trailing",
                "yoink_bot"
            ),
            Some(Command::Invalid)
        );
        assert_eq!(parse_command("status", "yoink_bot"), None);
        assert_eq!(parse_command("/settings", "yoink_bot"), None);
    }

    #[test]
    fn access_requires_owner_and_private_chat() {
        assert!(has_owner_private_access(7, true, Some(7)));
        assert!(!has_owner_private_access(7, false, Some(7)));
        assert!(!has_owner_private_access(7, true, Some(8)));
        assert!(!has_owner_private_access(7, true, None));
    }

    #[test]
    fn maps_russian_language_codes_and_defaults_to_english() {
        assert!(matches!(language_for(Some("ru")), Language::Russian));
        assert!(matches!(language_for(Some("RU-ru")), Language::Russian));
        assert!(matches!(language_for(Some("en")), Language::English));
        assert!(matches!(language_for(None), Language::English));
    }

    #[test]
    fn status_mapping_reports_readiness_and_planned_modules() {
        let system = SystemService::new("test-version");
        let english = format_status(&system, Language::English);
        assert!(english.contains("not ready"));
        assert!(english.contains("downloads, inbox, insight, music, stats"));
        assert!(english.contains("planned, not available"));

        system.set_ready(true);
        let russian = format_status(&system, Language::Russian);
        assert!(russian.contains("готов"));
        assert!(russian.contains("запланированы и пока недоступны"));
    }

    #[test]
    fn delivery_unknown_and_error_replies_are_sanitized_and_localized() {
        let id = Uuid::nil();
        let en = operation_response(
            id,
            OperationStatus::DeliveryUnknown,
            2,
            None,
            Language::English,
        );
        assert!(
            en.contains("Delivery could not be confirmed. Check the chat before submitting again.")
        );
        assert!(!en.contains("url"));
        let ru = operation_response(
            id,
            OperationStatus::DeliveryUnknown,
            2,
            None,
            Language::Russian,
        );
        assert!(ru.contains("Не удалось подтвердить доставку."));
        assert_eq!(
            download_error_response(DownloadError::Database, Language::English),
            "The operation could not be completed."
        );
        assert_eq!(
            download_error_response(DownloadError::Database, Language::Russian),
            "Не удалось выполнить операцию."
        );
        assert_eq!(
            download_error_response(DownloadError::Denied, Language::English),
            "This operation is not allowed."
        );
        assert_eq!(
            download_error_response(DownloadError::Capacity, Language::English),
            "The download queue is full. Try again later."
        );
        assert_eq!(
            download_error_response(DownloadError::Conflict, Language::Russian),
            "Этот запрос конфликтует с существующей операцией."
        );
        assert_eq!(
            download_error_response(DownloadError::InvalidInput, Language::Russian),
            "Некорректный запрос."
        );
        assert_eq!(
            download_error_response(DownloadError::InvalidConfig, Language::English),
            "Downloads are temporarily unavailable."
        );
        assert_eq!(
            download_error_response(DownloadError::Executor, Language::English),
            "The operation could not be completed."
        );
        assert_eq!(
            download_error_response(DownloadError::Task, Language::Russian),
            "Не удалось выполнить операцию."
        );
    }

    #[test]
    fn errors_expose_only_safe_fixed_categories() {
        let error = TelegramError::ReadinessCheckFailed;
        assert_eq!(error.to_string(), "Telegram readiness check failed");
        assert_eq!(format!("{error:?}"), "ReadinessCheckFailed");
    }

    #[test]
    fn only_definitive_api_rejections_prove_non_delivery() {
        use teloxide::{ApiError, RequestError};
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
