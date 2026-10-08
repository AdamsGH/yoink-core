//! Experimental Telegram adapter with owner-private diagnostics and opt-in group media.

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
    types::{CallbackQuery, Message},
    update_listeners::UpdateListener,
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
use yoink_downloads::{DownloadError, DownloadService, TelegramSubmission};
use yoink_i18n::{CatalogError, Key, Language, embedded as catalog};
use yoink_identity::{IdentityError, IdentityStore, TrustedProfile};
use yoink_storage::{Admission, OperationStatus};

mod cookie;
mod cookie_upload;
mod cookie_url;
mod cut;
mod inline;
mod inline_search;
pub use inline_search::InlineSearchConfig;
mod lang;
mod media_command;
mod membership_update;
mod menus;
mod raw_polling;
pub use menus::MenuConfig;
pub mod inbox_runtime;
pub mod insight_runtime;
pub use insight_runtime::InsightRuntime;
#[cfg(feature = "test-fixtures")]
pub mod confirmation_hold;
mod insight_lifecycle;
mod plain_url;
mod progress;
mod proxy;
mod quality;
mod sender;
pub mod text;
pub use inbox_runtime::{InboxRuntime, InboxRuntimeConfig};
pub use text::TextDelivery;
mod settings;
mod subs;
mod uncache;
pub use cookie_upload::{CookieUploadConfig, CookieUploadConfigError};
pub use cookie_url::{CookieUrlConfig, CookieUrlConfigError};
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
    groups_enabled: bool,
    identity_enabled: bool,
}

/// Telegram long polling with owner-private controls and trusted private-user downloads.
pub struct TelegramAdapter {
    bot: Bot,
    owner_id: u64,
    bot_id: u64,
    bot_username: String,
    system: SystemService,
    downloads: Option<DownloadService>,
    poll_timeout: Duration,
    progress: ProgressConfig,
    groups_enabled: bool,
    identities: Option<Arc<IdentityStore>>,
    cookie_upload: Option<CookieUploadConfig>,
    cookie_url: Option<CookieUrlConfig>,
    inline_search: InlineSearchConfig,
    file_base: Url,
    polling_endpoint: Url,
    http_client: reqwest::Client,
    menu_config: MenuConfig,
    menus: Option<menus::Menus>,
    inbox: Option<InboxRuntime>,
    insight: Option<InsightRuntime>,
}

impl TelegramAdapter {
    /// Creates the bot and verifies Bot API readiness before returning.
    pub async fn prepare(
        token: String,
        api_url: Option<Url>,
        owner_id: u64,
        system: SystemService,
        poll_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self, TelegramError> {
        if owner_id == 0
            || owner_id > i64::MAX as u64
            || !(1..=50).contains(&poll_timeout.as_secs())
            || poll_timeout.subsec_nanos() != 0
            || !(1..=120).contains(&request_timeout.as_secs())
            || request_timeout.subsec_nanos() != 0
        {
            return Err(TelegramError::BotInitializationFailed);
        }

        catalog().map_err(|_| TelegramError::BotInitializationFailed)?;
        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(request_timeout)
            .build()
            .map_err(|_| TelegramError::BotInitializationFailed)?;
        let mut bot = Bot::with_client(token.clone(), http_client.clone());
        let api_origin = api_url
            .clone()
            .unwrap_or_else(|| Url::parse("https://api.telegram.org/").expect("static URL"));
        if let Some(api_url) = api_url {
            bot = bot.set_api_url(api_url);
        }

        let me = bot
            .get_me()
            .await
            .map_err(|_| TelegramError::ReadinessCheckFailed)?;
        if me.user.id.0 == 0 || me.user.id.0 > i64::MAX as u64 {
            return Err(TelegramError::BotInitializationFailed);
        }
        let bot_username = me
            .user
            .username
            .clone()
            .ok_or(TelegramError::BotInitializationFailed)?;

        let mut polling_endpoint = api_origin.clone();
        polling_endpoint.set_path(&format!("/bot{token}/getUpdates"));
        polling_endpoint.set_query(None);
        polling_endpoint.set_fragment(None);

        Ok(Self {
            bot,
            owner_id,
            bot_id: me.user.id.0,
            bot_username,
            system,
            downloads: None,
            poll_timeout,
            progress: ProgressConfig::default(),
            groups_enabled: false,
            identities: None,
            cookie_upload: None,
            cookie_url: None,
            inline_search: InlineSearchConfig::default(),
            file_base: cookie_upload::file_base(&api_origin, &token),
            polling_endpoint,
            http_client,
            menu_config: MenuConfig::default(),
            menus: None,
            inbox: None,
            insight: None,
        })
    }

    /// Shared text transport uses the bot identity verified by prepare/getMe.
    pub fn text_delivery(
        &self,
        ledger: Arc<yoink_storage::Store>,
        timeout: Duration,
    ) -> Result<TextDelivery, yoink_storage::StoreError> {
        TextDelivery::new(self.bot.clone(), self.bot_id as i64, ledger, timeout)
    }

    /// Inject the shared native Insight runtime before menu initialization.
    pub fn with_insight(mut self, runtime: InsightRuntime) -> Self {
        self.insight = Some(runtime);
        self
    }

    /// Installs Inbox commands only. The coordinator separately owns run().
    pub fn with_inbox(mut self, runtime: InboxRuntime) -> Self {
        self.inbox = Some(runtime);
        self
    }

    /// Enables the progressive video-download slice.
    pub fn with_downloads(mut self, service: DownloadService) -> Self {
        self.downloads = Some(service);
        self
    }

    pub fn with_cookie_upload(mut self, config: CookieUploadConfig) -> Self {
        self.cookie_upload = Some(config);
        self
    }

    pub fn with_cookie_url(mut self, config: CookieUrlConfig) -> Self {
        self.cookie_url = Some(config);
        self
    }

    /// Keeps owner-private access unchanged and permits policy-checked group commands.
    pub fn with_group_access(mut self, enabled: bool) -> Self {
        self.groups_enabled = enabled;
        self
    }

    /// Enables trusted private-user onboarding against the shared identity database.
    pub fn with_identity_store(mut self, identities: Arc<IdentityStore>) -> Self {
        self.identities = Some(identities);
        self
    }

    pub fn with_inline_search(mut self, config: InlineSearchConfig) -> Result<Self, TelegramError> {
        if !config.valid() {
            return Err(TelegramError::BotInitializationFailed);
        }
        self.inline_search = config;
        Ok(self)
    }

    pub fn with_progress(mut self, config: ProgressConfig) -> Self {
        self.progress = config;
        self
    }

    pub fn with_menu_config(mut self, config: MenuConfig) -> Result<Self, TelegramError> {
        if !config.valid() {
            return Err(TelegramError::BotInitializationFailed);
        }
        self.menu_config = config;
        Ok(self)
    }

    pub fn bot_username(&self) -> &str {
        &self.bot_username
    }

    /// Returns a clone-backed delivery implementation sharing this bot's client.
    pub fn delivery(&self) -> Arc<dyn yoink_downloads::Delivery> {
        Arc::new(TelegramDelivery {
            bot: self.bot.clone(),
            bot_id: i64::try_from(self.bot_id).unwrap_or_default(),
        })
    }

    /// Reconciles persisted menus before the composition root declares readiness.
    /// The caller must apply its startup deadline to this operation.
    pub async fn initialize_menus(&mut self) {
        let mut menus = menus::Menus::new(
            self.bot.clone(),
            self.identities.clone(),
            self.owner_id,
            i64::try_from(self.bot_id).expect("validated bot identity"),
            self.downloads.is_some(),
            self.groups_enabled,
            self.menu_config,
        )
        .with_inbox(self.inbox.is_some())
        .with_insight(self.insight.as_ref().map(InsightRuntime::service));
        menus.initialize().await;
        self.menus = Some(menus);
    }

    /// Polls until cancellation or the first polling failure.
    pub async fn run(mut self, shutdown: CancellationToken) -> Result<(), TelegramError> {
        let insight_stop = self
            .insight
            .as_ref()
            .map(InsightRuntime::shutdown_token)
            .unwrap_or_else(|| shutdown.child_token());
        let _insight_guard = insight_stop.clone().drop_guard();
        if shutdown.is_cancelled() {
            return Ok(());
        }

        if self.menus.is_none() {
            tokio::select! {
                _ = shutdown.cancelled() => return Ok(()),
                _ = self.initialize_menus() => {},
            }
        }
        let searches = inline_search::Searches::new(self.inline_search, shutdown.child_token());
        let _search_stop = searches.stop.clone().drop_guard();
        let menus = self.menus.take().expect("initialized menus").run();
        let observer = self.identities.clone().map(|identities| {
            let source_id = i64::try_from(self.bot_id).expect("validated bot identity");
            let observer: raw_polling::Observer = Arc::new(move |update| {
                let identities = identities.clone();
                Box::pin(async move {
                    let observation = update.into_observation(source_id);
                    identities
                        .observe_membership(&observation)
                        .await
                        .map(|_| ())
                        .map_err(|_| raw_polling::PollingError::MembershipCommit)
                })
            });
            observer
        });
        let mut listener = raw_polling::RawPolling::new(
            self.http_client.clone(),
            self.polling_endpoint,
            self.poll_timeout,
            i64::try_from(self.bot_id).expect("validated bot identity"),
            observer,
        );
        let stop = listener.stop_token();
        let failed = Arc::new(AtomicBool::new(false));
        let progress = progress::run(self.bot.clone(), self.downloads.clone(), self.progress);
        let handler = dptree::entry()
            .branch(Update::filter_message().endpoint(handle_message))
            .branch(Update::filter_callback_query().endpoint(handle_callback))
            .branch(Update::filter_inline_query().endpoint(inline::handle));
        let insight_enabled = self.insight.is_some();
        let mut dispatcher = Dispatcher::builder(self.bot.clone(), handler)
            .distribution_function(if insight_enabled {
                insight_distribution
            } else {
                chat_distribution
            })
            .dependencies(dptree::deps![
                TelegramIdentity {
                    owner_id: self.owner_id,
                    bot_id: self.bot_id,
                    groups_enabled: self.groups_enabled,
                    identity_enabled: self.identities.is_some(),
                },
                searches.clone(),
                self.bot_username,
                self.system,
                DownloadsDependency(self.downloads.clone()),
                MessageDependencies {
                    downloads: self.downloads,
                    upload: CookieUploadDependency {
                        config: self.cookie_upload,
                        base: self.file_base,
                        client: self.http_client
                    },
                    cookie_url: CookieUrlDependency(self.cookie_url),
                    identities: self.identities,
                    inbox: self.inbox,
                    insight: self.insight,
                }
            ])
            .default_handler(|_| async {})
            .error_handler(teloxide::error_handlers::IgnoringErrorHandlerSafe::new())
            .build();

        let error_handler = {
            let stop = stop.clone();
            let failed = failed.clone();
            let insight_stop = insight_stop.clone();
            Arc::new(move |error: raw_polling::PollingError| {
                tracing::error!(category = ?error, "Telegram polling stopped");
                insight_stop.cancel();
                failed.store(true, Ordering::SeqCst);
                stop.stop();
                async {}
            })
        };
        let dispatch = dispatcher.try_dispatch_with_listener(listener, error_handler);
        tokio::pin!(dispatch);

        tokio::select! {
            () = menus => unreachable!("menu loop runs until dropped"),
            () = progress => unreachable!("progress loop runs until dropped"),
            _ = shutdown.cancelled() => {
                insight_stop.cancel();
                stop.stop();
                (&mut dispatch)
                    .await
                    .map_err(|_| TelegramError::PollingFailed)?;
            }
            result = &mut dispatch => {
                result.map_err(|_| TelegramError::PollingFailed)?;
                if !shutdown.is_cancelled() {
                    searches.shutdown().await;
                    return Err(TelegramError::PollingFailed);
                }
            }
        }

        searches.shutdown().await;
        if failed.load(Ordering::SeqCst) {
            return Err(TelegramError::PollingFailed);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct DownloadsDependency(Option<DownloadService>);

#[derive(Clone)]
struct CookieUploadDependency {
    config: Option<CookieUploadConfig>,
    base: Url,
    client: reqwest::Client,
}

#[derive(Clone)]
struct CookieUrlDependency(Option<CookieUrlConfig>);

#[derive(Clone)]
struct MessageDependencies {
    downloads: Option<DownloadService>,
    upload: CookieUploadDependency,
    cookie_url: CookieUrlDependency,
    identities: Option<Arc<IdentityStore>>,
    inbox: Option<InboxRuntime>,
    insight: Option<InsightRuntime>,
}

#[derive(Clone, PartialEq)]
enum Command {
    Start,
    Help,
    Status,
    Settings(settings::Action),
    Nsfw(settings::Action),
    ArmMedia(DomainMediaMode),
    Quality(bool),
    Subs,
    Proxy(proxy::Action),
    Download(String),
    Media(media_command::ParsedMedia),
    Job(Uuid),
    Cancel(Uuid),
    Invalid,
    InvalidUrl,
    Uncache(Option<String>),
    Cookie(cookie::Action),
    CookieUrl(cookie_url::Action),
    Lang(lang::Action),
}

impl std::fmt::Debug for Command {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Media(_) => formatter.write_str("Media([REDACTED])"),
            Self::Download(_) => formatter.write_str("Download([REDACTED])"),
            Self::Start => formatter.write_str("Start"),
            Self::Help => formatter.write_str("Help"),
            Self::Status => formatter.write_str("Status"),
            Self::Settings(_) => formatter.write_str("Settings"),
            Self::ArmMedia(mode) => formatter.debug_tuple("ArmMedia").field(mode).finish(),
            Self::Nsfw(_) => formatter.write_str("Nsfw"),
            Self::Quality(_) => formatter.write_str("Quality"),
            Self::Subs => formatter.write_str("Subs"),
            Self::Proxy(_) => formatter.write_str("Proxy"),
            Self::Job(id) => formatter.debug_tuple("Job").field(id).finish(),
            Self::Cancel(id) => formatter.debug_tuple("Cancel").field(id).finish(),
            Self::Invalid => formatter.write_str("Invalid"),
            Self::InvalidUrl => formatter.write_str("InvalidUrl"),
            Self::Uncache(_) => formatter.write_str("Uncache([REDACTED])"),
            Self::Cookie(_) => formatter.write_str("Cookie([REDACTED])"),
            Self::CookieUrl(_) => formatter.write_str("CookieUrl([REDACTED])"),
            Self::Lang(_) => formatter.write_str("Lang"),
        }
    }
}

fn inline_help_command(text: &str, own_username: &str) -> bool {
    let mut words = text.split_whitespace();
    let Some(token) = words.next() else {
        return false;
    };
    let Some(command) = token.strip_prefix('/') else {
        return false;
    };
    let name = match command.split_once('@') {
        Some((name, suffix)) if suffix.eq_ignore_ascii_case(own_username) => name,
        Some(_) => return false,
        None => command,
    };
    name.eq_ignore_ascii_case("start")
        && words.next() == Some("search_help")
        && words.next().is_none()
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
    if name == "uncache" {
        return Some(Command::Uncache(uncache::parse(&args)));
    }
    if name == "cookie" {
        if let Some(action) = cookie_url::parse(&args) {
            return Some(Command::CookieUrl(action));
        }
        return Some(Command::Cookie(cookie::command(&args)));
    }
    if name == "lang" {
        return Some(Command::Lang(lang::command(&args)));
    }
    if name == "nsfw" {
        return Some(Command::Nsfw(settings::nsfw_command(&args)));
    }
    if name == "clean" {
        return Some(Command::Settings(settings::clean_command(&args)));
    }
    if name == "settings" || name == "format" {
        return Some(settings::command(&args).map_or(
            Command::Settings(settings::Action::Invalid),
            Command::Settings,
        ));
    }
    if let Some(field) = match name.as_str() {
        "split" => Some(yoink_downloads::SettingField::SplitSize),
        "keyboard" => Some(yoink_downloads::SettingField::Keyboard),
        "mediainfo" => Some(yoink_downloads::SettingField::MediaInfo),
        _ => None,
    } {
        let action = match args.as_slice() {
            [] => settings::Action::Field(field),
            [value] => {
                let value = match *value {
                    "on" => "true",
                    "off" => "false",
                    other => other,
                };
                yoink_downloads::SettingChange::new(field, value)
                    .map_or(settings::Action::Invalid, |change| {
                        settings::Action::Change(change, None)
                    })
            }
            _ => settings::Action::Invalid,
        };
        return Some(Command::Settings(action));
    }
    if name == "quality" {
        return Some(Command::Quality(args.is_empty()));
    }
    if name == "subs" && args.is_empty() {
        return Some(Command::Subs);
    }
    if name == "proxy" {
        return Some(Command::Proxy(proxy::command(&args)));
    }
    if args.is_empty() && matches!(name.as_str(), "audio" | "image" | "video") {
        return Some(Command::ArmMedia(match name.as_str() {
            "audio" => DomainMediaMode::Audio,
            "image" => DomainMediaMode::Image,
            _ => DomainMediaMode::Video,
        }));
    }
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

fn incoming_route(identity: TelegramIdentity, message: &Message) -> Option<(u64, DeliveryTarget)> {
    let user = message.from.as_ref()?;
    if user.is_bot || message.sender_chat.is_some() {
        return None;
    }
    if message.chat.is_private() {
        let matching_private =
            message.chat.id.0 > 0 && u64::try_from(message.chat.id.0).ok() == Some(user.id.0);
        if !matching_private
            || message.thread_id.is_some()
            || (!identity.identity_enabled && user.id.0 != identity.owner_id)
        {
            return None;
        }
    } else if !identity.groups_enabled
        || !(message.chat.is_group() || message.chat.is_supergroup())
        || message.chat.id.0 >= 0
    {
        return None;
    }
    let target = DeliveryTarget::new(
        i64::try_from(identity.bot_id).ok()?,
        message.chat.id.0,
        message.thread_id.map(|thread| i64::from(thread.0.0)),
        Some(message.id.0),
    )
    .ok()?;
    if target.chat_id() > 0 && u64::try_from(target.chat_id()).ok()? != user.id.0 {
        return None;
    }
    Some((user.id.0, target))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrivateAccess {
    Owner,
    User,
    Pending,
    Denied,
}

async fn private_access(
    identities: Option<&IdentityStore>,
    message: &Message,
    actor: u64,
    configured_owner: u64,
) -> PrivateAccess {
    let Some(identities) = identities else {
        return if actor == configured_owner {
            PrivateAccess::Owner
        } else {
            PrivateAccess::Denied
        };
    };
    match identities.current_profile(actor as i64).await {
        Ok(profile) => private_access_for_role(&profile.role.to_string(), actor, configured_owner),
        Err(IdentityError::Unauthorized) => {
            let Some(sender) = message.from.as_ref() else {
                return PrivateAccess::Denied;
            };
            let profile = TrustedProfile {
                user_id: actor as i64,
                username: sender.username.clone(),
                first_name: Some(sender.first_name.clone()),
                photo_url: None,
            };
            match identities.register(&profile).await {
                Ok(profile) => {
                    private_access_for_role(&profile.role.to_string(), actor, configured_owner)
                }
                Err(_) => PrivateAccess::Denied,
            }
        }
        Err(_) => PrivateAccess::Denied,
    }
}

fn private_access_for_role(role: &str, actor: u64, configured_owner: u64) -> PrivateAccess {
    if role == "banned" {
        PrivateAccess::Denied
    } else if role == "restricted" {
        PrivateAccess::Pending
    } else if actor == configured_owner {
        PrivateAccess::Owner
    } else if matches!(role, "user" | "moderator" | "admin" | "owner") {
        PrivateAccess::User
    } else {
        PrivateAccess::Denied
    }
}

fn owner_only_command(command: &Command, _text: &str) -> bool {
    matches!(command, Command::Status | Command::Uncache(_))
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
    let show_cut = enabled && matches!(&command, Command::Start | Command::Help);
    let mut text = match command {
        Command::Start | Command::Help if enabled => translate(language, Key::HelpEnabled, &[]),
        Command::Start | Command::Help => translate(language, Key::HelpBasic, &[]),
        Command::Status => format_status(system, language),
        Command::Invalid if enabled => translate(language, Key::UsageDownload, &[]),
        Command::Invalid | Command::InvalidUrl if !enabled => {
            translate(language, Key::DownloadsUnavailable, &[])
        }
        Command::InvalidUrl => translate(language, Key::PlainUrlUsage, &[]),
        Command::Invalid => translate(language, Key::UsageDownload, &[]),
        _ => String::new(),
    };
    if show_cut {
        text.push('\n');
        text.push_str(&translate(language, Key::ProxyUsage, &[]));
        text.push('\n');
        text.push_str(&translate(language, Key::CutUsage, &[]));
        text.push('\n');
        text.push_str(&translate(language, Key::PlainUrlUsage, &[]));
        text.push('\n');
        text.push_str(&translate(language, Key::UncacheUsage, &[]));
    }
    text
}

fn media_unavailable_response(language: Language) -> String {
    translate(language, Key::DownloadsUnavailable, &[])
}

fn format_status(system: &SystemService, language: Language) -> String {
    format_status_with_insight(system, language, false)
}
fn format_status_with_insight(system: &SystemService, language: Language, insight: bool) -> String {
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
        .filter(|module| {
            module.status == ModuleStatus::Planned && !(insight && module.id == "insight")
        })
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
        DownloadError::QuotaExceeded { retry_after_secs } => {
            return translate(
                language,
                Key::ErrorQuota,
                &[("seconds", &retry_after_secs.to_string())],
            );
        }
        DownloadError::Denied => Key::ErrorDenied,
        DownloadError::PersonalCookieCapacity => Key::ErrorPersonalCookieCapacity,
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

fn chat_distribution(update: &Update) -> Option<ChatId> {
    update.chat().map(|chat| chat.id)
}
fn insight_distribution(update: &Update) -> Option<ChatId> {
    if let teloxide::types::UpdateKind::Message(message) = &update.kind
        && message
            .text()
            .is_some_and(insight_runtime::concurrent_command)
    {
        return None;
    }
    chat_distribution(update)
}

async fn handle_message(
    bot: Bot,
    message: Message,
    identity: TelegramIdentity,
    bot_username: String,
    system: SystemService,
    dependencies: MessageDependencies,
) -> Result<(), Infallible> {
    // Keep ownership through late combined help sends and receipt work, not
    // merely through the Insight product future or active cancellation map.
    let _insight_handler = if let Some(insight) = &dependencies.insight {
        let Some(handler) = insight.admit_handler() else {
            return Ok(());
        };
        Some(handler)
    } else {
        None
    };
    let downloads = dependencies.downloads;
    let upload = dependencies.upload;
    let CookieUrlDependency(cookie_url_config) = dependencies.cookie_url;
    let bot_id = identity.bot_id;
    let Some((actor_id, target)) = incoming_route(identity, &message) else {
        return Ok(());
    };
    if !system.snapshot().ready {
        return Ok(());
    }
    if let Some(inbox) = &dependencies.inbox
        && inbox.handle(&message, &bot_username).await
    {
        return Ok(());
    }
    if let Some(insight) = &dependencies.insight
        && insight
            .handle(&message, actor_id as i64, &bot_username)
            .await
    {
        return Ok(());
    }
    let product_help = message.text().is_some_and(|text| {
        text.split_whitespace().count() == 1
            && matches!(
                parse_command(text, &bot_username),
                Some(Command::Start | Command::Help)
            )
    });
    if !message.chat.is_private()
        && downloads.is_none()
        && product_help
        && let Some(insight) = &dependencies.insight
    {
        let language =
            reply_language_with_identity(None, dependencies.identities.as_deref(), actor_id, None)
                .await;
        if insight
            .handle_help(&message, actor_id as i64, language)
            .await
        {
            return Ok(());
        }
    }
    let private = message.chat.is_private();
    // Inline-granted restricted users can read help without registering or
    // acquiring any normal-message media authority.
    if private
        && message
            .text()
            .is_some_and(|text| inline_help_command(text, &bot_username))
    {
        let language = reply_language_with_identity(
            downloads.as_ref(),
            dependencies.identities.as_deref(),
            actor_id,
            message
                .from
                .as_ref()
                .and_then(|user| user.language_code.as_deref()),
        )
        .await;
        let reply = if inline::authorized_actor(dependencies.identities.as_deref(), actor_id).await
        {
            inline::help(language)
        } else {
            translate(language, Key::ErrorDenied, &[])
        };
        let _ = bot.send_message(message.chat.id, reply).await;
        return Ok(());
    }
    let access = if private {
        private_access(
            dependencies.identities.as_deref(),
            &message,
            actor_id,
            identity.owner_id,
        )
        .await
    } else {
        PrivateAccess::User
    };
    if private {
        match access {
            PrivateAccess::Denied => {
                let fallback = message
                    .from
                    .as_ref()
                    .and_then(|user| user.language_code.as_deref());
                let language = reply_language_with_identity(
                    downloads.as_ref(),
                    dependencies.identities.as_deref(),
                    actor_id,
                    fallback,
                )
                .await;
                let _ = bot
                    .send_message(message.chat.id, translate(language, Key::ErrorDenied, &[]))
                    .await;
                return Ok(());
            }
            PrivateAccess::Pending => {
                let fallback = message
                    .from
                    .as_ref()
                    .and_then(|user| user.language_code.as_deref());
                let language = reply_language_with_identity(
                    downloads.as_ref(),
                    dependencies.identities.as_deref(),
                    actor_id,
                    fallback,
                )
                .await;
                if product_help
                    && let Some(insight) = &dependencies.insight
                    && insight
                        .handle_help(&message, actor_id as i64, language)
                        .await
                {
                    return Ok(());
                }
                let reply = translate(language, Key::RegistrationPending, &[]);
                let _ = bot.send_message(message.chat.id, reply).await;
                return Ok(());
            }
            PrivateAccess::Owner | PrivateAccess::User => {}
        }
    }
    let is_owner = access == PrivateAccess::Owner;

    if message.document().is_some() && target.chat_id() > 0 {
        cookie_upload::handle_document(
            &bot,
            &message,
            &bot_username,
            actor_id,
            target,
            cookie_upload::UploadDependencies {
                downloads: downloads.as_ref(),
                config: upload.config,
                base: &upload.base,
                client: &upload.client,
            },
        )
        .await;
        return Ok(());
    }

    let Some(text) = message.text() else {
        return Ok(());
    };
    let language = reply_language_with_identity(
        downloads.as_ref(),
        if private {
            dependencies.identities.as_deref()
        } else {
            None
        },
        actor_id,
        message
            .from
            .as_ref()
            .and_then(|user| user.language_code.as_deref()),
    )
    .await;
    let parsed_command = parse_command(text, &bot_username);
    if private
        && !is_owner
        && parsed_command
            .as_ref()
            .is_some_and(|command| owner_only_command(command, text))
    {
        return Ok(());
    }
    if cut::try_handle_message(
        &bot,
        &message,
        &bot_username,
        actor_id,
        target,
        downloads.as_ref(),
        language,
    )
    .await
    {
        return Ok(());
    }
    let plain_media = parsed_command.is_none();
    let command = match parsed_command {
        Some(command) => command,
        None => match plain_url::parse(&message, bot_id) {
            None => return Ok(()),
            Some(Err(())) => Command::InvalidUrl,
            Some(Ok(plain_url::Action::Download(media))) => Command::Media(media),
            Some(Ok(plain_url::Action::AskEnd { media, start })) => {
                // The parser only offers a dialog in a private chat. Its storage,
                // authorization and recovery are shared with explicit /cut.
                let reply = if let Some(service) = downloads.as_ref() {
                    match service
                        .cut_begin(actor_id, target, message.id.0, &media.url, Some(start))
                        .await
                    {
                        Ok(session) => {
                            cut::send_session(&bot, &message, &session, language).await;
                            return Ok(());
                        }
                        Err(error) => download_error_response(error, language).to_owned(),
                    }
                } else {
                    media_unavailable_response(language)
                };
                let _ = bot.send_message(message.chat.id, reply).await;
                return Ok(());
            }
        },
    };
    if private && !is_owner && owner_only_command(&command, text) {
        return Ok(());
    }

    if let Command::ArmMedia(mode) = command {
        let reply = if !private {
            translate(language, Key::UsageDownload, &[])
        } else if let Some(service) = downloads.as_ref() {
            match service
                .arm_media(actor_id, target, message.id.0, mode)
                .await
            {
                Ok(()) => {
                    let label = translate(
                        language,
                        match mode {
                            DomainMediaMode::Audio => Key::MenuAudio,
                            DomainMediaMode::Image => Key::MenuImage,
                            _ => Key::MenuVideo,
                        },
                        &[],
                    );
                    translate(language, Key::MediaArmed, &[("mode", &label)])
                }
                Err(error) => download_error_response(error, language).to_owned(),
            }
        } else {
            media_unavailable_response(language)
        };
        let _ = bot.send_message(message.chat.id, reply).await;
        return Ok(());
    }
    if let Command::Uncache(source) = command {
        if target.chat_id() > 0 {
            if let Some(service) = downloads.as_ref() {
                uncache::handle_command(
                    &bot, &message, service, actor_id, target, source, language,
                )
                .await;
            } else {
                let _ = bot
                    .send_message(message.chat.id, media_unavailable_response(language))
                    .await;
            }
        }
        return Ok(());
    }
    if let Command::Cookie(action) = command {
        if target.chat_id() > 0 {
            if let Some(service) = downloads.as_ref() {
                cookie::handle_command(&bot, &message, service, actor_id, target, action, language)
                    .await;
            } else {
                let _ = bot
                    .send_message(message.chat.id, media_unavailable_response(language))
                    .await;
            }
        }
        return Ok(());
    }
    if let Command::CookieUrl(action) = command {
        if target.chat_id() > 0 {
            cookie_url::handle(
                &bot,
                &message,
                cookie_url::ImportDependencies {
                    service: downloads.as_ref(),
                    config: cookie_url_config,
                    actor: actor_id,
                    target,
                    action,
                    language,
                },
            )
            .await;
        }
        return Ok(());
    }
    if let Command::Lang(action) = command {
        if target.chat_id() > 0 && message.thread_id.is_none() {
            lang::handle_command(
                &bot,
                &message,
                identity.bot_id,
                downloads.as_ref(),
                actor_id,
                action,
                language,
            )
            .await;
        }
        return Ok(());
    }
    if let Command::Settings(action) | Command::Nsfw(action) = command {
        if target.chat_id() > 0 || matches!(action, settings::Action::NsfwMark) {
            if let Some(service) = downloads.as_ref() {
                settings::handle_command(
                    &bot, &message, service, actor_id, target, action, language,
                )
                .await;
            } else {
                let _ = bot
                    .send_message(message.chat.id, media_unavailable_response(language))
                    .await;
            }
        }
        return Ok(());
    }
    if let Command::Quality(valid) = command {
        if target.chat_id() > 0 {
            quality::handle_command(
                &bot,
                &message,
                downloads.as_ref(),
                actor_id,
                target,
                valid,
                language,
            )
            .await;
        }
        return Ok(());
    }
    if let Command::Subs = command {
        if target.chat_id() > 0
            && target.chat_id() as u64 == actor_id
            && message.thread_id.is_none()
        {
            subs::handle_command(
                &bot,
                &message,
                downloads.as_ref(),
                actor_id,
                identity.bot_id,
                language,
            )
            .await;
        }
        return Ok(());
    }
    if let Command::Proxy(action) = command {
        if target.chat_id() > 0 {
            if let Some(service) = downloads.as_ref() {
                proxy::handle_command(&bot, &message, service, actor_id, target, action, language)
                    .await;
            } else {
                let _ = bot
                    .send_message(message.chat.id, media_unavailable_response(language))
                    .await;
            }
        }
        return Ok(());
    }
    if target.chat_id() < 0 {
        if matches!(command, Command::Status) {
            return Ok(());
        }
        let Some(service) = downloads.as_ref() else {
            return Ok(());
        };
        if service.authorize_chat(actor_id, target).await.is_err() {
            return Ok(());
        }
    }
    let mut admitted = None;
    let insight_help = matches!(command, Command::Start | Command::Help);
    let mut reply = match command {
        Command::ArmMedia(_)
        | Command::Settings(_)
        | Command::Nsfw(_)
        | Command::Quality(_)
        | Command::Subs
        | Command::Proxy(_)
        | Command::Uncache(_)
        | Command::Cookie(_)
        | Command::CookieUrl(_)
        | Command::Lang(_) => {
            unreachable!("private controls handled before media routing")
        }
        Command::Start | Command::Help if target.chat_id() < 0 => {
            translate(language, Key::UsageDownload, &[])
        }
        Command::Start | Command::Help | Command::Invalid | Command::InvalidUrl
            if private && !is_owner =>
        {
            translate(
                language,
                if downloads.is_some() {
                    Key::UserHelpEnabled
                } else {
                    Key::UserHelpBasic
                },
                &[],
            )
        }
        Command::Status if dependencies.insight.is_some() => {
            format_status_with_insight(&system, language, true)
        }
        Command::Start
        | Command::Help
        | Command::Status
        | Command::Invalid
        | Command::InvalidUrl => response(command, language, &system, downloads.is_some()),
        Command::Download(_) | Command::Media(_) | Command::Job(_) | Command::Cancel(_)
            if downloads.is_none() =>
        {
            media_unavailable_response(language)
        }
        Command::Download(url) => {
            let Some(service) = downloads.as_ref() else {
                return Ok(());
            };
            let request = {
                DownloadRequest::new(
                    actor_id,
                    &url,
                    DomainMediaMode::Video,
                    None,
                    DownloadParams::default(),
                )
                .and_then(|request| request.with_target(target))
                .map_err(|_| DownloadError::InvalidInput)
            };
            match request {
                Ok(request) => match service
                    .submit_telegram_media(
                        &request,
                        message.id.0,
                        DownloadPreferenceOverrides::default(),
                        false,
                    )
                    .await
                {
                    Ok(TelegramSubmission::Admitted(admission)) => {
                        admitted = Some(admission.operation_id);
                        admitted_response(admission, language)
                    }
                    Ok(TelegramSubmission::Quality(session)) => {
                        quality::send_session(
                            &bot, &message, service, actor_id, target, &session, language,
                        )
                        .await;
                        return Ok(());
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
            let mut params = DownloadParams::default().with_playlist(media.playlist);
            if matches!(
                media.mode,
                DomainMediaMode::Formats | DomainMediaMode::Links | DomainMediaMode::AudioLinks
            ) {
                params = params.with_send_as_file(true);
            }
            if media.link_height.is_some() {
                params = params
                    .with_max_height(media.link_height)
                    .expect("closed link quality parser");
            }
            if let Some(engine) = media.engine {
                params = params.with_engine(engine);
            }
            let request =
                DownloadRequest::new(actor_id, &media.url, media.mode, media.clip, params)
                    .and_then(|request| request.with_target(target))
                    .map_err(|_| DownloadError::InvalidInput);
            match request {
                Ok(request) => match service
                    .submit_telegram_media(&request, message.id.0, media.overrides, plain_media)
                    .await
                {
                    Ok(TelegramSubmission::Admitted(admission)) => {
                        admitted = Some(admission.operation_id);
                        admitted_response(admission, language)
                    }
                    Ok(TelegramSubmission::Quality(session)) => {
                        quality::send_session(
                            &bot, &message, service, actor_id, target, &session, language,
                        )
                        .await;
                        return Ok(());
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
            match service.status_in_chat(actor_id, id, target).await {
                Ok(Some(record)) => progress::render(&record, language),
                Ok(None) => unknown_operation(language),
                Err(error) => download_error_response(error, language).to_owned(),
            }
        }
        Command::Cancel(id) => {
            let Some(service) = downloads.as_ref() else {
                return Ok(());
            };
            match service.cancel_in_chat(actor_id, id, target).await {
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

    if target.chat_id() < 0 {
        let Some(service) = downloads.as_ref() else {
            return Ok(());
        };
        if service.authorize_chat(actor_id, target).await.is_err() {
            return Ok(());
        }
    }
    if insight_help && let Some(insight) = &dependencies.insight {
        let help = insight.help_for(&message, actor_id as i64, language).await;
        if !help.is_empty() {
            reply.push_str("\n\n");
            reply.push_str(&help);
        }
    }
    let mut send = bot.send_message(message.chat.id, reply);
    if let Some(thread) = message.thread_id {
        send = send.message_thread_id(thread);
    }
    match send.await {
        Ok(sent)
            if sent.chat.id == message.chat.id
                && sent.thread_id == message.thread_id
                && sent.id.0 > 0 =>
        {
            if let (Some(id), Some(service)) = (admitted, downloads.as_ref()) {
                let code = match language {
                    Language::English => "en",
                    Language::Russian => "ru",
                };
                if service
                    .register_progress_message(actor_id, id, sent.id.0, code)
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

async fn handle_callback(
    bot: Bot,
    query: CallbackQuery,
    identity: TelegramIdentity,
    downloads: DownloadsDependency,
) -> Result<(), Infallible> {
    if query
        .data
        .as_deref()
        .is_some_and(|data| data == "lang" || data.starts_with("lang:"))
    {
        lang::handle_callback(bot, query, identity, downloads).await
    } else if query
        .data
        .as_deref()
        .is_some_and(|data| data == "ql" || data.starts_with("ql:"))
    {
        quality::handle_callback(bot, query, identity, downloads).await
    } else if query
        .data
        .as_deref()
        .is_some_and(|data| data == "subs" || data.starts_with("subs:"))
    {
        subs::handle_callback(bot, query, identity, downloads).await
    } else if query
        .data
        .as_deref()
        .is_some_and(|data| data.starts_with("cut:"))
    {
        cut::handle_callback(bot, query, identity, downloads).await
    } else if query
        .data
        .as_deref()
        .is_some_and(|data| data.starts_with("proxy:"))
    {
        proxy::handle_callback(bot, query, identity, downloads).await
    } else if query
        .data
        .as_deref()
        .is_some_and(|data| data.starts_with("cookie:"))
    {
        cookie::handle_callback(bot, query, identity, downloads).await
    } else {
        settings::handle_callback(bot, query, identity, downloads).await
    }
}

async fn reply_language(
    service: Option<&DownloadService>,
    actor: u64,
    fallback: Option<&str>,
) -> Language {
    let preferred = if let Some(service) = service {
        service.preferred_language(actor).await.ok().flatten()
    } else {
        None
    };
    lang::resolve_language(preferred.as_deref(), fallback)
}

async fn reply_language_with_identity(
    service: Option<&DownloadService>,
    identities: Option<&IdentityStore>,
    actor: u64,
    fallback: Option<&str>,
) -> Language {
    let identity_preference = match (identities, i64::try_from(actor)) {
        (Some(store), Ok(user_id)) => store
            .language_preference(user_id)
            .await
            .ok()
            .flatten()
            .map(|preference| preference.code),
        _ => None,
    };
    let preferred = match identity_preference {
        Some(preferred) => Some(preferred),
        None => match service {
            Some(service) => service.preferred_language(actor).await.ok().flatten(),
            None => None,
        },
    };
    lang::resolve_language(preferred.as_deref(), fallback)
}

fn unknown_operation(language: Language) -> String {
    translate(language, Key::OperationNotFound, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoink_downloads::DeliveryError;
    use yoink_i18n::language_for;

    #[tokio::test]
    async fn bot_transport_deadline_covers_a_stalled_response_body() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (entered, observed) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1024\r\n\r\n{")
                .await
                .unwrap();
            let _ = entered.send(());
            std::future::pending::<()>().await;
        });
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            TelegramAdapter::prepare(
                "123456:test".into(),
                Some(Url::parse(&format!("http://{address}")).unwrap()),
                42,
                SystemService::new("test"),
                Duration::from_secs(1),
                Duration::from_secs(1),
            ),
        )
        .await;
        server.abort();
        let _ = server.await;
        assert!(
            observed.await.is_ok(),
            "request reached the held response body"
        );
        assert!(matches!(
            result,
            Ok(Err(TelegramError::ReadinessCheckFailed))
        ));
    }

    #[tokio::test]
    async fn persisted_identity_language_localizes_without_download_service() {
        use std::time::Duration;
        use yoink_db::{Database, raw_sql};

        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let database = Database::open(
            &directory.path().join("identity.db"),
            true,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        raw_sql(
            r#"CREATE TABLE users (id INTEGER PRIMARY KEY, role TEXT NOT NULL, language TEXT NOT NULL, updated_at INTEGER NOT NULL);
               INSERT INTO users VALUES (43, 'user', 'ru', 1), (44, 'restricted', 'ru', 1);"#,
        )
        .execute(&database)
        .await
        .unwrap();
        let identities = IdentityStore::new(database, None, false);

        for actor in [43, 44] {
            assert_eq!(
                reply_language_with_identity(None, Some(&identities), actor, Some("en")).await,
                Language::Russian,
                "stored Russian must override English Telegram metadata even when downloads are disabled"
            );
        }
        assert_eq!(
            reply_language_with_identity(None, None, 45, Some("en")).await,
            Language::English,
            "unregistered identity lookup must retain Telegram fallback"
        );
    }

    #[test]
    fn incoming_routes_require_opt_in_and_an_identified_user_in_the_original_chat() {
        let identity = TelegramIdentity {
            owner_id: 42,
            bot_id: 123456,
            groups_enabled: false,
            identity_enabled: false,
        };
        let value = serde_json::json!({
            "message_id": 10, "date": 0, "from": {"id": 42, "is_bot": false, "first_name": "Fixture"},
            "chat": {"id": 42, "type": "private"}, "text": "/video https://example.com/media"
        });
        let route = |identity, value: serde_json::Value| {
            incoming_route(identity, &serde_json::from_value(value).unwrap())
        };
        assert_eq!(route(identity, value.clone()).unwrap().0, 42);
        let mut group = value.clone();
        group["chat"] = serde_json::json!({"id": -10010, "type": "supergroup", "title": "Fixture", "is_forum": true});
        group["from"]["id"] = 43.into();
        group["message_thread_id"] = 18.into();
        assert!(route(identity, group.clone()).is_none());
        let enabled = TelegramIdentity {
            groups_enabled: true,
            identity_enabled: true,
            ..identity
        };
        let (actor, target) = route(enabled, group.clone()).unwrap();
        assert_eq!(actor, 43);
        assert_eq!(target.chat_id(), -10010);
        assert_eq!(target.thread_id(), Some(18));
        assert_eq!(target.reply_to_message_id(), Some(10));
        for (field, changed) in [
            ("sender_chat", group["chat"].clone()),
            (
                "from",
                serde_json::json!({"id": 43, "is_bot": true, "first_name": "Bot"}),
            ),
            ("from", serde_json::Value::Null),
            (
                "chat",
                serde_json::json!({"id": -10010, "type": "channel", "title": "Channel"}),
            ),
            ("message_id", 0.into()),
        ] {
            let mut denied = group.clone();
            denied[field] = changed;
            assert!(route(enabled, denied).is_none(), "{field}");
        }
        let mut wrong_private = value.clone();
        wrong_private["chat"]["id"] = 43.into();
        assert!(route(enabled, wrong_private).is_none());
        let mut nonowner = value.clone();
        nonowner["from"]["id"] = 43.into();
        nonowner["chat"]["id"] = 43.into();
        assert_eq!(route(enabled, nonowner.clone()).unwrap().0, 43);
        nonowner["from"]["is_bot"] = true.into();
        assert!(route(enabled, nonowner).is_none());
        let mut spoofed = value.clone();
        spoofed["sender_chat"] = serde_json::json!({"id": 42, "type": "channel", "title": "spoof"});
        assert!(route(enabled, spoofed).is_none());
        let mut private_topic = value;
        private_topic["message_thread_id"] = 18.into();
        assert!(route(enabled, private_topic).is_none());
    }

    #[test]
    fn trusted_private_access_maps_roles_without_elevating_sender_claims() {
        assert_eq!(private_access_for_role("user", 43, 42), PrivateAccess::User);
        assert_eq!(
            private_access_for_role("restricted", 43, 42),
            PrivateAccess::Pending
        );
        assert_eq!(
            private_access_for_role("banned", 43, 42),
            PrivateAccess::Denied
        );
        assert_eq!(
            private_access_for_role("admin", 42, 42),
            PrivateAccess::Owner
        );
        assert_eq!(
            private_access_for_role("owner", 43, 42),
            PrivateAccess::User
        );
    }

    #[test]
    fn ordinary_help_is_localized_and_keeps_system_controls_private() {
        for language in [Language::English, Language::Russian] {
            let enabled = translate(language, Key::UserHelpEnabled, &[]);
            let basic = translate(language, Key::UserHelpBasic, &[]);
            let pending = translate(language, Key::RegistrationPending, &[]);
            for text in [&enabled, &basic, &pending] {
                for forbidden in [
                    "/status",
                    "/uncache",
                    "/runtime",
                    "--height",
                    "--video-codec",
                ] {
                    assert!(!text.contains(forbidden), "{forbidden} leaked in {text}");
                }
            }
            assert!(enabled.contains("/video"));
            assert!(enabled.contains("/audio"));
            assert!(enabled.contains("/image"));
            for supported in [
                "/settings",
                "/format",
                "/quality",
                "/list",
                "/cut",
                "/playlist",
                "/subs",
                "/cookie",
                "/proxy",
            ] {
                assert!(enabled.contains(supported));
            }
            for text in [&basic, &pending] {
                for personal in ["/cookie", "/proxy"] {
                    assert!(!text.contains(personal));
                }
            }
            assert!(basic.contains("/start"));
        }
    }

    #[test]
    fn private_control_dispatch_keeps_only_operational_commands_owner_only() {
        let text_and_commands = [
            ("/status", Command::Status),
            ("/uncache https://example.test", Command::Uncache(None)),
        ];
        for (text, command) in text_and_commands {
            assert!(owner_only_command(&command, text), "{text}");
        }
        for text in [
            "/cookie",
            "/proxy",
            "/cookie https://example.test/cookies.txt",
            "/settings",
            "/format",
            "/quality",
            "/subs",
            "/cut",
            "/list https://example.test",
            "/playlist https://example.test",
            "/video https://example.test --quality 360",
        ] {
            assert!(!owner_only_command(
                &parse_command(text, "yoink_bot").unwrap(),
                text
            ));
        }
        assert!(!owner_only_command(
            &Command::Lang(lang::Action::Menu),
            "/lang"
        ));
        assert!(!owner_only_command(
            &Command::Media(
                media_command::parse("video", &["https://example.test"])
                    .unwrap()
                    .unwrap()
            ),
            "/video https://example.test"
        ));
        assert!(!owner_only_command(
            &Command::Download("https://example.test".into()),
            "/download https://example.test"
        ));
    }

    #[test]
    fn parses_supported_commands_with_strict_arguments_and_bot_suffix() {
        assert_eq!(parse_command("/start", "yoink_bot"), Some(Command::Start));
        assert_eq!(
            parse_command("/quality", "yoink_bot"),
            Some(Command::Quality(true))
        );
        assert_eq!(
            parse_command("/quality@YOINK_BOT", "yoink_bot"),
            Some(Command::Quality(true))
        );
        assert_eq!(
            parse_command("/quality extra", "yoink_bot"),
            Some(Command::Quality(false))
        );
        assert_eq!(parse_command("/quality@OtherBot", "yoink_bot"), None);
        assert_eq!(
            parse_command("/lang", "yoink_bot"),
            Some(Command::Lang(lang::Action::Menu))
        );
        assert_eq!(
            parse_command("/lang@YOINK_BOT", "yoink_bot"),
            Some(Command::Lang(lang::Action::Menu))
        );
        assert_eq!(
            parse_command("/lang extra", "yoink_bot"),
            Some(Command::Lang(lang::Action::Invalid))
        );
        assert_eq!(parse_command("/lang@OtherBot", "yoink_bot"), None);
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
        assert_eq!(
            parse_command("/settings", "yoink_bot"),
            Some(Command::Settings(settings::Action::Menu))
        );
        assert_eq!(parse_command("/subs", "yoink_bot"), Some(Command::Subs));
        assert_eq!(
            parse_command("/subs@YOINK_BOT", "yoink_bot"),
            Some(Command::Subs)
        );
        assert_eq!(
            parse_command("/format@YOINK_BOT", "yoink_bot"),
            Some(Command::Settings(settings::Action::Menu))
        );
        assert_eq!(
            parse_command("/proxy", "yoink_bot"),
            Some(Command::Proxy(proxy::Action::Menu))
        );
        assert_eq!(
            parse_command("/proxy@YOINK_BOT", "yoink_bot"),
            Some(Command::Proxy(proxy::Action::Menu))
        );
        assert_eq!(
            parse_command("/proxy secret", "yoink_bot"),
            Some(Command::Proxy(proxy::Action::Invalid))
        );
        assert_eq!(parse_command("/proxy@other_bot", "yoink_bot"), None);
        assert!(
            !format!(
                "{:?}",
                parse_command(
                    "/proxy http://private-user:private-password@example.test",
                    "yoink_bot"
                )
            )
            .contains("private-password")
        );
        assert_eq!(
            parse_command("/settings role owner", "yoink_bot"),
            Some(Command::Settings(settings::Action::Invalid))
        );
        assert_eq!(parse_command("/settings@other_bot", "yoink_bot"), None);
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
        let cut = parse_command(
            "/cut@YOINK_BOT https://example.test/watch 00:01 00:03",
            "yoink_bot",
        )
        .unwrap();
        assert!(
            matches!(cut, Command::Media(media) if media.clip.is_some_and(|clip| clip.start_seconds() == 1.0 && clip.end_seconds() == 3.0))
        );
        assert!(parse_command("/image https://example.test/photo", "yoink_bot")
            .is_some_and(|command| matches!(command, Command::Media(media) if media.engine == Some(Engine::Gallery))));
        assert!(parse_command("/playlist 3 https://example.test/list", "yoink_bot")
            .is_some_and(|command| matches!(command, Command::Media(media) if media.playlist.is_some_and(|range| range.start() == 1 && range.end() == Some(3)))));
        assert!(parse_command("/playlist https://example.test/list", "yoink_bot")
            .is_some_and(|command| matches!(command, Command::Media(media) if media.playlist.is_some_and(|range| range.start() == 1 && range.end() == Some(20)))));
        assert!(parse_command("/subs https://example.test/subtitles", "yoink_bot")
            .is_some_and(|command| matches!(command, Command::Media(media) if media.mode == DomainMediaMode::Subtitles)));
        assert!(parse_command("/list@yoink_bot https://example.test/video", "yoink_bot")
            .is_some_and(|command| matches!(command, Command::Media(media) if media.mode == DomainMediaMode::Formats && media.engine == Some(Engine::YtDlp))));
        for invalid in [
            "/list",
            "/list https://example.test/a extra",
            "/list --format https://example.test/a",
            "/list ftp://example.test/a",
            "/list https://user@example.test/a",
        ] {
            assert_eq!(parse_command(invalid, "yoink_bot"), Some(Command::Invalid));
        }
        assert_eq!(
            parse_command("/subs invalid", "yoink_bot"),
            Some(Command::Invalid)
        );
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
    fn owner_identity_cannot_override_blocking_or_unknown_roles() {
        assert_eq!(
            private_access_for_role("banned", 42, 42),
            PrivateAccess::Denied
        );
        assert_eq!(
            private_access_for_role("restricted", 42, 42),
            PrivateAccess::Pending
        );
        assert_eq!(
            private_access_for_role("unknown", 43, 42),
            PrivateAccess::Denied
        );
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
    fn quota_messages_are_actionable_localized_and_bounded() {
        for (lang, hint) in [
            (Language::English, "Wait 60 seconds"),
            (Language::Russian, "Подождите 60 сек."),
        ] {
            let text = download_error_response(
                DownloadError::QuotaExceeded {
                    retry_after_secs: 60,
                },
                lang,
            );
            assert!(text.contains(hint));
            assert!(text.len() < 512);
        }
        for lang in [Language::English, Language::Russian] {
            assert!(
                download_error_response(
                    DownloadError::QuotaExceeded {
                        retry_after_secs: u64::MAX
                    },
                    lang
                )
                .len()
                    < 512
            );
        }
    }

    #[test]
    fn every_download_error_has_english_and_russian_user_text() {
        let errors = [
            DownloadError::Denied,
            DownloadError::Capacity,
            DownloadError::PersonalCookieCapacity,
            DownloadError::QuotaExceeded {
                retry_after_secs: 60,
            },
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
    #[test]
    fn private_media_intent_commands_parse_without_breaking_immediate_urls() {
        for (name, mode) in [
            ("audio", DomainMediaMode::Audio),
            ("image", DomainMediaMode::Image),
            ("video", DomainMediaMode::Video),
        ] {
            assert_eq!(
                parse_command(&format!("/{name}"), "yoink"),
                Some(Command::ArmMedia(mode))
            );
            assert!(
                matches!(parse_command(&format!("/{name} https://example.com/media"), "yoink"), Some(Command::Media(media)) if media.mode == mode)
            );
            assert_eq!(parse_command(&format!("/{name}@other"), "yoink"), None);
        }
    }
}
