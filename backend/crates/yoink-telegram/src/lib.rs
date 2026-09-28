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
#[cfg(test)]
use yoink_domain::Engine;
use yoink_domain::{
    DeliveryTarget, DownloadParams, DownloadPreferenceOverrides, DownloadRequest,
    MediaMode as DomainMediaMode,
};
use yoink_downloads::{DownloadError, DownloadService};
use yoink_i18n::{CatalogError, Key, Language, embedded as catalog, language_for};
use yoink_storage::{Admission, OperationStatus};

mod media_command;
mod progress;
mod sender;
pub use progress::ProgressConfig;

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
    progress: ProgressConfig,
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

        catalog().map_err(|_| TelegramError::BotInitializationFailed)?;
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
            progress: ProgressConfig::default(),
        })
    }

    /// Enables the progressive video-download slice.
    pub fn with_downloads(mut self, service: DownloadService) -> Self {
        self.downloads = Some(service);
        self
    }

    pub fn with_progress(mut self, config: ProgressConfig) -> Self {
        self.progress = config;
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
        let progress = progress::run(self.bot.clone(), self.downloads.clone(), self.progress);
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
            () = progress => unreachable!("progress loop runs until dropped"),
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

#[derive(Clone, PartialEq)]
enum Command {
    Start,
    Help,
    Status,
    Download(String),
    Media(media_command::ParsedMedia),
    Job(Uuid),
    Cancel(Uuid),
    Invalid,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Media(_) => formatter.write_str("Media([REDACTED])"),
            Self::Download(_) => formatter.write_str("Download([REDACTED])"),
            Self::Start => formatter.write_str("Start"),
            Self::Help => formatter.write_str("Help"),
            Self::Status => formatter.write_str("Status"),
            Self::Job(id) => formatter.debug_tuple("Job").field(id).finish(),
            Self::Cancel(id) => formatter.debug_tuple("Cancel").field(id).finish(),
            Self::Invalid => formatter.write_str("Invalid"),
        }
    }
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

    let name = name.to_ascii_lowercase();
    let args = words.collect::<Vec<_>>();
    if let Some(parsed) = media_command::parse(&name, &args) {
        return Some(parsed.map_or(Command::Invalid, Command::Media));
    }

    match name.as_str() {
        "start" => {
            if args.is_empty() {
                Some(Command::Start)
            } else {
                Some(Command::Invalid)
            }
        }
        "help" => {
            if args.is_empty() {
                Some(Command::Help)
            } else {
                Some(Command::Invalid)
            }
        }
        "status" => {
            if args.is_empty() {
                Some(Command::Status)
            } else {
                Some(Command::Invalid)
            }
        }
        "download" => {
            let Some(url) = args.first().copied() else {
                return Some(Command::Invalid);
            };
            if args.len() != 1 || url.len() > 4096 || !is_http_url(url) {
                return Some(Command::Invalid);
            }
            Some(Command::Download(url.to_owned()))
        }
        "job" => {
            let Some(id) = args.first().copied() else {
                return Some(Command::Invalid);
            };
            if args.len() != 1 {
                return Some(Command::Invalid);
            }
            Some(
                Uuid::parse_str(id)
                    .ok()
                    .map_or(Command::Invalid, Command::Job),
            )
        }
        "cancel" => {
            let Some(id) = args.first().copied() else {
                return Some(Command::Invalid);
            };
            if args.len() != 1 {
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

fn translate(language: Language, key: Key, parameters: &[(&str, &str)]) -> String {
    match catalog()
        .map_err(CatalogError::clone)
        .and_then(|catalog| catalog.render(language, key, parameters))
    {
        Ok(text) => text,
        Err(error) => {
            let category = match error {
                CatalogError::Parse { .. } => "parse",
                CatalogError::InvalidShape { .. } => "shape",
                CatalogError::NonString { .. } => "non_string",
                CatalogError::EmptyTranslation { .. } => "empty_translation",
                CatalogError::MissingKey { .. } => "missing_key",
                CatalogError::ExtraKey { .. } => "extra_key",
                CatalogError::MalformedTemplate { .. } => "malformed_template",
                CatalogError::PlaceholderMismatch { .. } => "placeholder_mismatch",
                CatalogError::MissingParameter { .. } => "missing_parameter",
                CatalogError::ExtraParameter { .. } => "extra_parameter",
                CatalogError::DuplicateParameter { .. } => "duplicate_parameter",
            };
            tracing::warn!(key = key.as_str(), category, "Telegram translation failed");
            format!("[{}]", key.as_str())
        }
    }
}

fn response(command: Command, language: Language, system: &SystemService, enabled: bool) -> String {
    match command {
        Command::Start | Command::Help if enabled => translate(language, Key::HelpEnabled, &[]),
        Command::Start | Command::Help => translate(language, Key::HelpBasic, &[]),
        Command::Status => format_status(system, language),
        Command::Invalid if enabled => translate(language, Key::UsageDownload, &[]),
        Command::Invalid => translate(language, Key::DownloadsUnavailable, &[]),
        _ => String::new(),
    }
}

fn media_unavailable_response(language: Language) -> String {
    translate(language, Key::DownloadsUnavailable, &[])
}

fn format_status(system: &SystemService, language: Language) -> String {
    let snapshot = system.snapshot();
    let readiness = translate(
        language,
        if snapshot.ready {
            Key::ReadinessReady
        } else {
            Key::ReadinessNotReady
        },
        &[],
    );
    let planned = snapshot
        .modules
        .iter()
        .filter(|module| module.status == ModuleStatus::Planned)
        .map(|module| module.id)
        .collect::<Vec<_>>()
        .join(", ");
    translate(
        language,
        Key::Status,
        &[
            ("version", &snapshot.version),
            ("readiness", &readiness),
            ("modules", &planned),
        ],
    )
}

fn download_error_response(error: DownloadError, language: Language) -> String {
    let key = match error {
        DownloadError::Denied => Key::ErrorDenied,
        DownloadError::Capacity => Key::ErrorCapacity,
        DownloadError::Conflict => Key::ErrorConflict,
        DownloadError::InvalidInput => Key::ErrorInvalidInput,
        DownloadError::InvalidPreferences => Key::ErrorInvalidPreferences,
        DownloadError::InvalidCredentials => Key::ErrorInvalidCredentials,
        DownloadError::ProxyUnavailable => Key::ErrorProxyUnavailable,
        DownloadError::UnsupportedAuthentication => Key::ErrorUnsupportedAuthentication,
        DownloadError::ChangedCredentials => Key::ErrorChangedCredentials,
        DownloadError::InvalidConfig => Key::ErrorInvalidConfig,
        DownloadError::Database | DownloadError::Executor | DownloadError::Task => {
            Key::ErrorOperation
        }
    };
    translate(language, key, &[])
}

fn operation_status(status: OperationStatus) -> Key {
    match status {
        OperationStatus::Queued => Key::StatusQueued,
        OperationStatus::Running => Key::StatusRunning,
        OperationStatus::Delivering => Key::StatusDelivering,
        OperationStatus::DeliveryUnknown => Key::StatusDeliveryUnknown,
        OperationStatus::DeliveryPartial => Key::StatusDeliveryPartial,
        OperationStatus::Succeeded => Key::StatusSucceeded,
        OperationStatus::Failed => Key::StatusFailed,
        OperationStatus::Cancelled => Key::StatusCancelled,
    }
}

fn operation_response(
    id: Uuid,
    status: OperationStatus,
    attempt: u32,
    delivered_message_id: Option<i64>,
    language: Language,
) -> String {
    let id = id.to_string();
    let status_text = translate(language, operation_status(status), &[]);
    let mut reply = translate(
        language,
        Key::Operation,
        &[
            ("id", &id),
            ("status", &status_text),
            ("attempt", &attempt.to_string()),
        ],
    );
    if status == OperationStatus::DeliveryUnknown {
        reply.push_str(&translate(language, Key::DeliveryUnconfirmed, &[]));
    }
    if let Some(message_id) = delivered_message_id {
        reply.push_str(&translate(
            language,
            Key::DeliveredMessage,
            &[("id", &message_id.to_string())],
        ));
    }
    reply
}

fn admitted_response(admission: Admission, language: Language) -> String {
    let key = if admission.created {
        Key::OperationCreated
    } else {
        Key::ExistingOperation
    };
    translate(
        language,
        key,
        &[("id", &admission.operation_id.to_string())],
    )
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

    let mut admitted = None;
    let reply = match command {
        Command::Start | Command::Help | Command::Status | Command::Invalid => {
            response(command, language, &system, downloads.is_some())
        }
        Command::Download(_) | Command::Media(_) | Command::Job(_) | Command::Cancel(_)
            if downloads.is_none() =>
        {
            media_unavailable_response(language)
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
                Ok(request) => match service
                    .submit_preferred_request(
                        &request,
                        &key,
                        DownloadPreferenceOverrides::default(),
                    )
                    .await
                {
                    Ok(admission) => {
                        admitted = Some(admission.operation_id);
                        admitted_response(admission, language)
                    }
                    Err(error) => download_error_response(error, language).to_owned(),
                },
                Err(error) => download_error_response(error, language).to_owned(),
            }
        }
        Command::Media(media) => {
            let Some(service) = downloads.as_ref() else {
                return Ok(());
            };
            let key = format!("tg:{bot_id}:{}:{}", message.chat.id.0, message.id.0);
            let target =
                DeliveryTarget::new(bot_id as i64, message.chat.id.0, None, Some(message.id.0))
                    .map_err(|_| DownloadError::InvalidInput);
            let mut params = DownloadParams::default().with_playlist(media.playlist);
            if let Some(engine) = media.engine {
                params = params.with_engine(engine);
            }
            let request = target.and_then(|target| {
                DownloadRequest::new(owner_id, &media.url, media.mode, media.clip, params)
                    .map_err(|_| DownloadError::InvalidInput)?
                    .with_target(target)
                    .map_err(|_| DownloadError::InvalidInput)
            });
            match request {
                Ok(request) => match service
                    .submit_preferred_request(&request, &key, media.overrides)
                    .await
                {
                    Ok(admission) => {
                        admitted = Some(admission.operation_id);
                        admitted_response(admission, language)
                    }
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
                Ok(Some(record)) => progress::render(&record, language),
                Ok(None) => unknown_operation(language),
                Err(error) => download_error_response(error, language).to_owned(),
            }
        }
        Command::Cancel(id) => {
            let Some(service) = downloads.as_ref() else {
                return Ok(());
            };
            match service.cancel(owner_id, id).await {
                Ok(true) => translate(
                    language,
                    Key::CancellationRequested,
                    &[("id", &id.to_string())],
                ),
                Ok(false) => unknown_operation(language),
                Err(error) => download_error_response(error, language).to_owned(),
            }
        }
    };

    match bot.send_message(message.chat.id, reply).await {
        Ok(sent) if sent.chat.id == message.chat.id && sent.id.0 > 0 => {
            if let (Some(id), Some(service)) = (admitted, downloads.as_ref()) {
                let code = match language {
                    Language::English => "en",
                    Language::Russian => "ru",
                };
                if service
                    .register_progress_message(owner_id, id, sent.id.0, code)
                    .await
                    .is_err()
                {
                    tracing::warn!("Telegram progress registration failed");
                }
            }
        }
        _ => tracing::warn!("Telegram response delivery failed"),
    }
    Ok(())
}

fn unknown_operation(language: Language) -> String {
    translate(language, Key::OperationNotFound, &[])
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
            parse_command(
                "/download https://example.test/video --height 720",
                "yoink_bot"
            ),
            Some(Command::Invalid)
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
    fn media_commands_accept_suffixes_and_keep_urls_out_of_debug_output() {
        let parsed = parse_command(
            "/AUDIO@YOINK_BOT https://example.test/watch?token=secret 3 --audio-codec best",
            "yoink_bot",
        )
        .unwrap();
        assert!(matches!(parsed, Command::Media(_)));
        assert!(!format!("{parsed:?}").contains("secret"));
        assert!(
            parse_command("/cut https://example.test/watch 00:01 00:03", "yoink_bot").is_some()
        );
        assert!(parse_command("/image https://example.test/photo", "yoink_bot")
            .is_some_and(|command| matches!(command, Command::Media(media) if media.engine == Some(Engine::Gallery))));
        assert!(parse_command("/playlist 3 https://example.test/list", "yoink_bot")
            .is_some_and(|command| matches!(command, Command::Media(media) if media.playlist.is_some_and(|range| range.start() == 1 && range.end() == Some(3)))));
        assert!(parse_command("/playlist https://example.test/list", "yoink_bot")
            .is_some_and(|command| matches!(command, Command::Media(media) if media.playlist.is_some_and(|range| range.start() == 1 && range.end() == Some(20)))));
        assert_eq!(
            media_unavailable_response(Language::English),
            "Experimental downloads are unavailable."
        );
        assert_eq!(
            media_unavailable_response(Language::Russian),
            "Экспериментальные загрузки недоступны."
        );
        assert_eq!(
            parse_command("/video https://example.test/a extra extra", "yoink_bot"),
            Some(Command::Invalid)
        );
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
        assert!(matches!(language_for(Some("ru-RU")), Language::Russian));
        assert!(matches!(language_for(Some("en-US")), Language::English));
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
    fn every_download_error_has_english_and_russian_user_text() {
        let errors = [
            DownloadError::Denied,
            DownloadError::Capacity,
            DownloadError::Conflict,
            DownloadError::InvalidInput,
            DownloadError::InvalidPreferences,
            DownloadError::InvalidCredentials,
            DownloadError::ProxyUnavailable,
            DownloadError::UnsupportedAuthentication,
            DownloadError::ChangedCredentials,
            DownloadError::InvalidConfig,
            DownloadError::Database,
            DownloadError::Executor,
            DownloadError::Task,
        ];
        for error in errors {
            let en = download_error_response(error, Language::English);
            let ru = download_error_response(error, Language::Russian);
            assert!(!en.is_empty(), "English response missing for {error:?}");
            assert!(!ru.is_empty(), "Russian response missing for {error:?}");
            assert_ne!(en, ru, "locales unexpectedly match for {error:?}");
        }
        assert_eq!(
            download_error_response(DownloadError::ProxyUnavailable, Language::English),
            "Proxy is enabled, but no proxy is configured. Configure one or disable proxy use."
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
