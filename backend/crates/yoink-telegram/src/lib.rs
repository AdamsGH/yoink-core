//! Owner-only experimental Telegram adapter. No business operations are exposed.

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
use yoink_application::{ModuleStatus, SystemService};

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

/// Owner-only diagnostics over Telegram long polling.
pub struct TelegramAdapter {
    bot: Bot,
    owner_id: u64,
    bot_username: String,
    system: SystemService,
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
            bot_username,
            system,
            poll_timeout,
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
            .dependencies(dptree::deps![self.owner_id, self.bot_username, self.system])
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Start,
    Help,
    Status,
}

fn parse_command(text: &str, own_username: &str) -> Option<Command> {
    let token = text.split_whitespace().next()?;
    let command = token.strip_prefix('/')?;
    let name = match command.split_once('@') {
        Some((name, suffix)) if suffix.eq_ignore_ascii_case(own_username) => name,
        Some(_) => return None,
        None => command,
    };
    match name.to_ascii_lowercase().as_str() {
        "start" => Some(Command::Start),
        "help" => Some(Command::Help),
        "status" => Some(Command::Status),
        _ => None,
    }
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

fn response(command: Command, language: Language, system: &SystemService) -> String {
    match (language, command) {
        (Language::English, Command::Start | Command::Help) => {
            "Available diagnostics: /start, /help, /status.".to_owned()
        }
        (Language::Russian, Command::Start | Command::Help) => {
            "Доступна диагностика: /start, /help, /status.".to_owned()
        }
        (Language::English, Command::Status) => format_status(system, Language::English),
        (Language::Russian, Command::Status) => format_status(system, Language::Russian),
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

async fn handle_message(
    bot: Bot,
    message: Message,
    owner_id: u64,
    bot_username: String,
    system: SystemService,
) -> Result<(), Infallible> {
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
    let text = response(command, language, &system);

    if bot.send_message(message.chat.id, text).await.is_err() {
        tracing::warn!("Telegram diagnostic response delivery failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_commands_and_only_own_suffix() {
        assert_eq!(parse_command("/start", "yoink_bot"), Some(Command::Start));
        assert_eq!(
            parse_command("/HELP@yoink_bot", "yoink_bot"),
            Some(Command::Help)
        );
        assert_eq!(parse_command("/status@OtherBot", "yoink_bot"), None);
        assert_eq!(
            parse_command("/status@yoink_bot extra", "yoink_bot"),
            Some(Command::Status)
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
    fn errors_expose_only_safe_fixed_categories() {
        let error = TelegramError::ReadinessCheckFailed;
        assert_eq!(error.to_string(), "Telegram readiness check failed");
        assert_eq!(format!("{error:?}"), "ReadinessCheckFailed");
    }
}
