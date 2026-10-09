//! Native Stats composition seams. The parent owns polling, jobs and database lifetime.
use crate::{TextDelivery, UpdateObserver, insight_lifecycle::Handlers};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use teloxide::Bot;
use tokio_util::sync::CancellationToken;
use turso::transaction::TransactionBehavior;
use yoink_db::Transaction;
use yoink_i18n::Language;
use yoink_stats::{
    Error, MemberAccess, MembershipFuture, MembershipProvider, NameFuture, Stats, UserName,
    UsernameRefreshSchedule,
};
use yoink_storage::{Store, StoreError, TextIdentity, TextOutcome};

#[derive(Clone, Copy, Debug)]
pub struct LookupBudget {
    pub timeout: Duration,
    pub response_bytes: usize,
}

/// Uses the parent's already configured native TLS/proxy client and Bot API URL.
#[derive(Clone)]
pub struct BotMembershipProvider {
    bot: Bot,
    budget: LookupBudget,
}
impl BotMembershipProvider {
    pub fn new(bot: Bot, budget: LookupBudget) -> Result<Self, Error> {
        if budget.timeout.is_zero() || budget.response_bytes == 0 {
            return Err(Error::Invalid);
        }
        Ok(Self { bot, budget })
    }
    async fn request(&self, method: &str, payload: Value) -> Result<Option<Value>, Error> {
        let request = async {
            let mut endpoint = self.bot.api_url();
            endpoint.set_path(&format!("/bot{}/{method}", self.bot.token()));
            endpoint.set_query(None);
            endpoint.set_fragment(None);
            let mut response = self
                .bot
                .client()
                .post(endpoint)
                .json(&payload)
                .send()
                .await
                .map_err(|_| Error::Unavailable)?;
            if response
                .content_length()
                .is_some_and(|n| n > self.budget.response_bytes as u64)
            {
                return Err(Error::Unavailable);
            }
            let status = response.status();
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
                if chunk.len() > self.budget.response_bytes.saturating_sub(bytes.len()) {
                    return Err(Error::Unavailable);
                }
                bytes.extend_from_slice(&chunk);
            }
            let body: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Unavailable)?;
            if body["ok"] == false && matches!(body["error_code"].as_u64(), Some(400 | 403)) {
                return Ok(None);
            }
            if !status.is_success() || body["ok"] != true || !body["result"].is_object() {
                return Err(Error::Unavailable);
            }
            Ok(Some(body["result"].clone()))
        };
        tokio::time::timeout(self.budget.timeout, request)
            .await
            .map_err(|_| Error::Unavailable)?
    }
}
impl MembershipProvider for BotMembershipProvider {
    fn lookup(&self, chat_id: i64, user_id: i64) -> MembershipFuture<'_> {
        Box::pin(async move {
            if chat_id >= 0 || user_id <= 0 {
                return Err(Error::Invalid);
            }
            let Some(value) = self
                .request(
                    "getChatMember",
                    json!({"chat_id":chat_id,"user_id":user_id}),
                )
                .await?
            else {
                return Ok(MemberAccess::default());
            };
            if value["user"]["id"].as_i64() != Some(user_id) {
                return Err(Error::Unavailable);
            }
            let (member, administrator) = match value["status"].as_str() {
                Some("creator" | "administrator") => (true, true),
                Some("member") => (true, false),
                Some("restricted") => (
                    value["is_member"].as_bool().ok_or(Error::Unavailable)?,
                    false,
                ),
                Some("left" | "kicked") => (false, false),
                _ => return Err(Error::Unavailable),
            };
            Ok(MemberAccess {
                member,
                administrator,
            })
        })
    }
    fn lookup_name(&self, chat_id: Option<i64>, user_id: i64) -> NameFuture<'_> {
        Box::pin(async move {
            if user_id <= 0 || chat_id.is_some_and(|c| c >= 0) {
                return Err(Error::Invalid);
            }
            let (method, payload) = match chat_id {
                Some(chat) => ("getChatMember", json!({"chat_id":chat,"user_id":user_id})),
                None => ("getChat", json!({"chat_id":user_id})),
            };
            let Some(value) = self.request(method, payload).await? else {
                return Ok(None);
            };
            let profile = if chat_id.is_some() {
                &value["user"]
            } else {
                &value
            };
            if profile["id"].as_i64() != Some(user_id)
                || profile["is_bot"] == true
                || profile["is_deleted"] == true
                || (chat_id.is_none() && profile["type"] != "private")
            {
                return Ok(None);
            }
            let username = name_part(&profile["username"]);
            if username.is_none()
                && profile["first_name"] == "Deleted Account"
                && name_part(&profile["last_name"]).is_none()
            {
                return Ok(None);
            }
            let display_name = [
                name_part(&profile["first_name"]),
                name_part(&profile["last_name"]),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
            if username.is_none() && display_name.is_empty() {
                return Ok(None);
            }
            Ok(Some(UserName {
                user_id,
                username,
                display_name: (!display_name.is_empty()).then_some(display_name),
            }))
        })
    }
}
fn name_part(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| !s.chars().any(char::is_control))
        .map(str::to_owned)
}

/// Values must come from the dispatcher's trusted Telegram message, not command arguments.
#[derive(Clone, Copy, Debug)]
pub struct CommandTarget {
    pub actor: i64,
    pub chat: i64,
    pub bot: i64,
    pub message: i32,
    pub topic: Option<i32>,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("stats runtime is closing")]
    Closing,
    #[error(transparent)]
    Stats(#[from] Error),
    #[error(transparent)]
    Delivery(#[from] yoink_storage::StoreError),
}

#[derive(Clone)]
pub struct StatsRuntime {
    stats: Stats,
    delivery: TextDelivery,
    handlers: Handlers,
    stop: CancellationToken,
    #[cfg(test)]
    transaction_pause: Option<Arc<TransactionPause>>,
}
impl StatsRuntime {
    /// Construct after migrations and getMe, using the same owning Stats as HTTP.
    pub fn new(stats: Stats, delivery: TextDelivery) -> Self {
        Self {
            stats,
            delivery,
            handlers: Handlers::new(),
            stop: CancellationToken::new(),
            #[cfg(test)]
            transaction_pause: None,
        }
    }
    pub async fn observe(&self, raw: Value) -> Result<(), RuntimeError> {
        let _guard = self.handlers.admit().ok_or(RuntimeError::Closing)?;
        self.stats
            .observe_raw_update(self.delivery.bot_id(), raw)
            .await?;
        Ok(())
    }
    /// Errors cross the existing pre-acknowledgment seam; no update is swallowed.
    pub fn observer(&self) -> UpdateObserver {
        let runtime = self.clone();
        Arc::new(move |raw| {
            let runtime = runtime.clone();
            Box::pin(async move {
                runtime.observe(raw).await.map_err(|error| {
                    tracing::error!(%error, "Stats observation failed");
                    crate::TelegramError::PollingFailed
                })
            })
        })
    }
    /// The dispatcher supplies a stable command token, shared across update replay.
    pub async fn command(
        &self,
        token: &str,
        target: CommandTarget,
        args: &[String],
        language: Language,
    ) -> Result<Vec<TextOutcome>, RuntimeError> {
        let _guard = self.handlers.admit().ok_or(RuntimeError::Closing)?;
        if target.bot != self.delivery.bot_id()
            || target.actor <= 0
            || target.chat == 0
            || target.message <= 0
            || target.topic.is_some_and(|t| t <= 0)
            || (target.chat > 0 && (target.chat != target.actor || target.topic.is_some()))
        {
            return Err(Error::Invalid.into());
        }
        let request = command_request(target, args, language);
        let result = self
            .command_snapshot(token, target, args, language, &request)
            .await?;
        let mut outcomes = Vec::new();
        for (part, text) in result.chunks.iter().enumerate() {
            let identity =
                command_identity(target, text.clone(), json!({"request":request,"part":part}));
            let chunk_token = format!("{token}:{part}");
            let (bound, _) = TextDelivery::html_identity(&identity)?;
            let ledger = self.delivery.ledger();
            let effect = ledger
                .text_effect(&chunk_token)
                .await?
                .ok_or(StoreError::InvalidStoredData)?;
            if effect.identity.as_ref() != Some(&bound) {
                return Err(StoreError::Conflict.into());
            }
            let outcome = if effect.outcome != TextOutcome::Prepared {
                effect.outcome
            } else {
                let access = self
                    .stats
                    .command_preflight(target.actor, result.source_group)
                    .await?;
                let mut c = ledger.database().connection().await.map_err(Error::from)?;
                let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
                    .await
                    .map_err(|_| Error::Database)?;
                let admission: Result<bool, RuntimeError> = async {
                    let current = Store::text_effect_on(&*tx, &chunk_token)
                        .await?
                        .ok_or(StoreError::InvalidStoredData)?;
                    if current.identity.as_ref() != Some(&bound) {
                        return Err(StoreError::Conflict.into());
                    }
                    if current.outcome != TextOutcome::Prepared {
                        return Ok(false);
                    }
                    self.stats
                        .command_authority_on(&tx, target.actor, result.source_group, access)
                        .await?;
                    let admitted = Store::begin_text_on(&*tx, &chunk_token, &bound).await?;
                    #[cfg(test)]
                    self.pause_transaction(TransactionPhase::Admission).await;
                    Ok(admitted)
                }
                .await;
                let admitted = match admission {
                    Ok(value) => {
                        tx.commit().await.map_err(|_| Error::Database)?;
                        value
                    }
                    Err(error) => {
                        let _ = tx.rollback().await;
                        return Err(error);
                    }
                };
                drop(c);
                if admitted {
                    self.delivery
                        .send_admitted_html(&chunk_token, &identity)
                        .await?
                } else {
                    ledger
                        .text_effect(&chunk_token)
                        .await?
                        .ok_or(StoreError::InvalidStoredData)?
                        .outcome
                }
            };
            let confirmed = matches!(outcome, TextOutcome::Confirmed(_));
            outcomes.push(outcome);
            if !confirmed {
                break;
            }
        }
        Ok(outcomes)
    }
    async fn command_snapshot(
        &self,
        token: &str,
        target: CommandTarget,
        args: &[String],
        language: Language,
        request: &Value,
    ) -> Result<yoink_stats::commands::CommandResult, RuntimeError> {
        let ledger = self.delivery.ledger();
        if let Some(effect) = ledger.text_effect(token).await? {
            return read_command_snapshot(effect, request);
        }
        let result = self
            .stats
            .command_result(target.actor, target.chat, args, language)
            .await?;
        if result.chunks.is_empty() {
            return Err(Error::Invalid.into());
        }
        // Validate the entire result and every transport identity before persisting
        // any intent. The existing 64KiB payload limit bounds the whole snapshot.
        let mut prepared = Vec::new();
        for (part, text) in result.chunks.iter().enumerate() {
            let chunk_token = format!("{token}:{part}");
            if chunk_token.len() > 256 {
                return Err(StoreError::InvalidInput.into());
            }
            let identity =
                command_identity(target, text.clone(), json!({"request":request,"part":part}));
            prepared.push((chunk_token, TextDelivery::html_identity(&identity)?.0));
        }
        let access = self
            .stats
            .command_preflight(target.actor, result.source_group)
            .await?;
        let mut c = ledger.database().connection().await.map_err(Error::from)?;
        let tx = Transaction::new(&mut c, TransactionBehavior::Immediate)
            .await
            .map_err(|_| Error::Database)?;
        let preparation: Result<yoink_stats::commands::CommandResult, RuntimeError> = async {
            if let Some(effect) = Store::text_effect_on(&*tx, token).await? {
                return read_command_snapshot(effect, request);
            }
            let authority = self
                .stats
                .command_authority_on(&tx, target.actor, result.source_group, access)
                .await?;
            let manifest = command_identity(
                target,
                "Stats command snapshot".into(),
                json!({"request":request,"result":result,"authorization":authority}),
            );
            if !manifest.valid() {
                return Err(StoreError::InvalidInput.into());
            }
            Store::prepare_text_on(&*tx, token, &manifest).await?;
            #[cfg(test)]
            self.pause_transaction(TransactionPhase::Snapshot).await;
            for (chunk_token, identity) in prepared {
                Store::prepare_text_on(&*tx, &chunk_token, &identity).await?;
            }
            Ok(result)
        }
        .await;
        match preparation {
            Ok(result) => {
                tx.commit().await.map_err(|_| Error::Database)?;
                Ok(result)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }
    #[cfg(test)]
    async fn pause_transaction(&self, phase: TransactionPhase) {
        if let Some(pause) = &self.transaction_pause
            && pause.phase == phase
        {
            pause.entered.notify_one();
            pause
                .release
                .acquire()
                .await
                .expect("test release")
                .forget();
        }
    }
    /// Parent places this owned future in its JoinSet. Missed ticks do not catch up.
    /// Cancellation stops admission/waits, not transactions already being drained.
    pub async fn background(
        &self,
        schedule: UsernameRefreshSchedule,
        search_interval: Duration,
    ) -> Result<(), RuntimeError> {
        if schedule.interval.is_zero()
            || schedule.lookup_timeout.is_zero()
            || schedule.lookup_timeout > Duration::from_secs(300)
            || search_interval.is_zero()
        {
            return Err(Error::Invalid.into());
        }
        let mut names = tokio::time::interval_at(
            tokio::time::Instant::now() + schedule.first_delay,
            schedule.interval,
        );
        let mut search = tokio::time::interval_at(
            tokio::time::Instant::now() + search_interval,
            search_interval,
        );
        names.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        search.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = self.stop.cancelled() => return Ok(()),
                _ = names.tick() => {
                    let Some(_guard) = self.handlers.admit() else { return Ok(()); };
                    self.stats.refresh_usernames(schedule.lookup_timeout).await?;
                }
                _ = search.tick() => {
                    let Some(_guard) = self.handlers.admit() else { return Ok(()); };
                    self.stats.refresh().await?;
                }
            }
        }
    }
    pub async fn recover_search(&self) -> Result<(), RuntimeError> {
        let _guard = self.handlers.admit().ok_or(RuntimeError::Closing)?;
        self.stats.recover_search().await?;
        Ok(())
    }
    pub async fn recover_search_at(&self, path: impl AsRef<Path>) -> Result<(), RuntimeError> {
        let _guard = self.handlers.admit().ok_or(RuntimeError::Closing)?;
        self.stats.recover_search_at(path).await?;
        Ok(())
    }
    pub fn stop_admission(&self) {
        self.handlers.close();
        self.stop.cancel();
    }
    /// Parent must also stop/drain direct HTTP Stats calls before this, and close
    /// the database only afterwards. Already admitted work completes before flush.
    pub async fn shutdown(&self) -> Result<(), RuntimeError> {
        self.stop_admission();
        self.handlers.drain().await;
        self.stats.shutdown().await?;
        Ok(())
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum TransactionPhase {
    Admission,
    Snapshot,
}
#[cfg(test)]
struct TransactionPause {
    phase: TransactionPhase,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}

fn command_identity(target: CommandTarget, text: String, payload: Value) -> TextIdentity {
    TextIdentity {
        bot: target.bot,
        chat: target.chat,
        topic: target.topic,
        message: target.message,
        actor: target.actor,
        item: None,
        edit: false,
        text,
        payload,
    }
}
fn command_request(target: CommandTarget, args: &[String], language: Language) -> Value {
    json!({"version":"stats-command-v1","actor":target.actor,"chat":target.chat,
        "bot":target.bot,"message":target.message,"topic":target.topic,"args":args,
        "language":match language {Language::English=>"en",Language::Russian=>"ru"}})
}
fn read_command_snapshot(
    effect: yoink_storage::TextEffect,
    request: &Value,
) -> Result<yoink_stats::commands::CommandResult, RuntimeError> {
    let identity = effect.identity.ok_or(StoreError::Conflict)?;
    if effect.outcome != TextOutcome::Prepared
        || identity.text != "Stats command snapshot"
        || identity.payload["request"] != *request
    {
        return Err(StoreError::Conflict.into());
    }
    serde_json::from_value(identity.payload["result"].clone())
        .map_err(|_| StoreError::InvalidStoredData.into())
}

#[cfg(test)]
#[path = "stats_runtime_tests.rs"]
mod tests;
