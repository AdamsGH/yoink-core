//! Coordinator-owned Inbox notification and reconciliation adapter. No detached tasks.
use crate::{text::TextDelivery, translate};
use std::{sync::Arc, time::Duration};
use teloxide::types::Message;
use tokio_util::sync::CancellationToken;
use yoink_access::{AccessPolicy, AccessScope, Role};
use yoink_db::{Executor, query};
use yoink_i18n::{Key, Language};
use yoink_identity::IdentityStore;
use yoink_inbox::{
    Error, Result,
    contracts::{Authorization, Call},
    notifications::{Outcome, RouteAuthority},
    store::Store,
    telegram::ReplyTarget,
};
use yoink_storage::{TextIdentity, TextOutcome};

/// Dispatch/recovery work bounds. Coordinator config remains independently owned.
#[derive(Clone, Copy, Debug)]
pub struct InboxRuntimeConfig {
    pub notification_lease: Duration,
    pub recovery_batch: usize,
    pub dispatch_batch: usize,
}
impl Default for InboxRuntimeConfig {
    fn default() -> Self {
        Self {
            notification_lease: Duration::from_secs(120),
            recovery_batch: 64,
            dispatch_batch: 1,
        }
    }
}
#[derive(Clone)]
pub struct InboxRuntime {
    store: Store,
    identities: Arc<IdentityStore>,
    delivery: TextDelivery,
    groups: bool,
    config: InboxRuntimeConfig,
    #[cfg(test)]
    hooks: Arc<TestHooks>,
}
struct Auth(Arc<IdentityStore>, Option<(i64, Option<i64>)>);
impl Authorization for Auth {
    fn require<'a>(&'a self, user: i64, feature: &'a str) -> Call<'a, ()> {
        Box::pin(async move {
            let (plugin, feature) = feature.split_once(':').ok_or(Error::Forbidden)?;
            if plugin != "inbox" {
                return Err(Error::Forbidden);
            }
            let s = self
                .0
                .snapshot(user, self.1)
                .await
                .map_err(|_| Error::External)?
                .ok_or(Error::Forbidden)?;
            let p = AccessPolicy {
                min_role: Role::Restricted,
                scopes: vec![AccessScope::Private, AccessScope::Group],
                check_group_enabled: true,
                check_thread_policy: true,
            };
            if s.authorize(&p, Some((plugin, feature))).decision.allowed {
                Ok(())
            } else {
                Err(Error::Forbidden)
            }
        })
    }
    fn require_admin(&self, _: i64) -> Call<'_, ()> {
        Box::pin(async { Err(Error::Forbidden) })
    }
}
struct Route<'a> {
    runtime: &'a InboxRuntime,
    identity: TextIdentity,
}
impl RouteAuthority for Route<'_> {
    fn require<'a>(&'a self, user: i64, target: ReplyTarget) -> Call<'a, ()> {
        Box::pin(async move {
            if user != self.identity.actor
                || target.chat_id != self.identity.chat
                || target.message_id != i64::from(self.identity.message)
            {
                return Err(Error::Forbidden);
            }
            self.runtime.authorize(&self.identity, "ingest").await
        })
    }
}
fn external(_: yoink_storage::StoreError) -> Error {
    Error::External
}
impl InboxRuntime {
    /// Construct after Bot API getMe and schema migration. Supply the same owning
    /// database and IdentityStore as HTTP/media. Runtime never creates tasks.
    pub fn new(
        store: Store,
        identities: Arc<IdentityStore>,
        delivery: TextDelivery,
        groups: bool,
    ) -> Self {
        Self {
            store,
            identities,
            delivery,
            groups,
            config: InboxRuntimeConfig::default(),
            #[cfg(test)]
            hooks: Arc::new(TestHooks::default()),
        }
    }
    fn now(&self) -> i64 {
        #[cfg(test)]
        if let Some(clock) = &self.hooks.clock {
            return clock.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        chrono::Utc::now().timestamp_micros()
    }
    /// The connection/transaction belongs here, not to the HTTP ingestion wrapper.
    /// Serializing lookup, ingestion and preparation makes the original creator
    /// result win even when identical Telegram updates race.
    async fn prepare_save(
        &self,
        token: &str,
        mut i: TextIdentity,
        url: &str,
        language: Language,
    ) -> Result<TextIdentity> {
        if self.store.db.path() != self.delivery.ledger().database().path() {
            return Err(Error::Invalid);
        }
        #[cfg(test)]
        if let Some(barrier) = &self.hooks.before_preparation {
            barrier.wait().await;
        }
        let c = self.store.db.connection().await?;
        c.batch("BEGIN IMMEDIATE").await?;
        let result=async {
            if let Some(effect)=yoink_storage::Store::text_effect_on(&*c,token).await.map_err(external)? {
                let old=effect.identity.ok_or(Error::External)?;
                if !same_command(&old,&i) { return Err(Error::External); }
                return Ok(old);
            }
            let saved=Store::ingest_on(&*c,i.actor,url,"bot",self.now()).await?;
            i.item=Some(saved.item_id);
            i.payload["bind_item"]=saved.created.into();
            i.payload["save_result"]=serde_json::json!({"item_id":saved.item_id,"normalized_url":saved.normalized_url,"created":saved.created});
            i.text=translate(language,if saved.created {Key::InboxSaved} else {Key::InboxDuplicate},&[]);
            yoink_storage::Store::prepare_text_on(&*c,token,&i).await.map_err(external)?;
            Ok(i)
        }.await;
        match result {
            Ok(i) => {
                c.batch("COMMIT").await?;
                Ok(i)
            }
            Err(e) => {
                let _ = c.batch("ROLLBACK").await;
                Err(e)
            }
        }
    }
    /// Lease must strictly exceed the entire text request deadline. Server drain
    /// should cover dispatch_batch * request timeout plus bounded DB persistence.
    pub fn with_config(mut self, config: InboxRuntimeConfig) -> Result<Self> {
        if config.notification_lease <= self.delivery.timeout()
            || config.notification_lease.as_micros() == 0
            || config.notification_lease > Duration::from_secs(3600)
            || !(1..=256).contains(&config.recovery_batch)
            || !(1..=16).contains(&config.dispatch_batch)
        {
            return Err(Error::Invalid);
        }
        self.config = config;
        Ok(self)
    }
    async fn authorize(&self, i: &TextIdentity, feature: &str) -> Result<()> {
        if i.bot != self.delivery.bot_id()
            || !i.valid()
            || (i.chat > 0 && (i.chat != i.actor || i.topic.is_some()))
            || (i.chat < 0 && !self.groups)
        {
            return Err(Error::Forbidden);
        }
        if i.chat < 0
            && !self
                .identities
                .membership_eligible(i.bot, i.chat, i.actor)
                .await
                .map_err(|_| Error::External)?
        {
            return Err(Error::Forbidden);
        }
        let s = self
            .identities
            .snapshot(
                i.actor,
                (i.chat < 0).then_some((i.chat, i.topic.map(i64::from))),
            )
            .await
            .map_err(|_| Error::External)?
            .ok_or(Error::Forbidden)?;
        let p = AccessPolicy {
            min_role: Role::Restricted,
            scopes: vec![AccessScope::Private, AccessScope::Group],
            check_group_enabled: true,
            check_thread_policy: true,
        };
        if s.authorize(&p, Some(("inbox", feature))).decision.allowed {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }
    /// Recover confirmed initial receipts without sends or current grants. Never
    /// bind an incoming user message or infer delivery from absence.
    pub async fn reconcile_bindings(&self) -> Result<()> {
        if self.store.db.path() != self.delivery.ledger().database().path() {
            return Err(Error::Invalid);
        }
        for e in self
            .delivery
            .ledger()
            .unbound_text_receipts_page(self.delivery.bot_id(), self.config.recovery_batch)
            .await
            .map_err(external)?
        {
            let Some(i) = e.identity else { continue };
            if i.bot != self.delivery.bot_id() || i.edit {
                continue;
            }
            let (Some(item), TextOutcome::Confirmed(receipt)) = (i.item, e.outcome) else {
                continue;
            };
            let c = self.store.db.connection().await?;
            c.batch("BEGIN IMMEDIATE").await?;
            let result = async {
                let changed = query("UPDATE inbox_items SET tg_chat_id=?,tg_reply_message_id=? WHERE id=? AND user_id=? AND source='bot' AND ((tg_chat_id IS NULL AND tg_reply_message_id IS NULL) OR (tg_chat_id=? AND tg_reply_message_id=?))")
                    .bind(i.chat).bind(receipt).bind(item).bind(i.actor).bind(i.chat).bind(receipt).execute(&*c).await?.rows_affected();
                if changed == 1 {
                    query("UPDATE inbox_notifications SET chat_id=?,message_id=?,state='ready' WHERE item_id=? AND user_id=? AND state='waiting'").bind(i.chat).bind(receipt).bind(item).bind(i.actor).execute(&*c).await?;
                }
                // First binding wins. Conflicting receipts are never retargeted.
                query("UPDATE runtime_text_effects SET bound=1 WHERE token=? AND state='confirmed'").bind(&e.token).execute(&*c).await?;
                Ok::<_,Error>(())
            }.await;
            match result {
                Ok(()) => c.batch("COMMIT").await?,
                Err(e) => {
                    let _ = c.batch("ROLLBACK").await;
                    return Err(e);
                }
            }
        }
        Ok(())
    }
    async fn provenance(
        &self,
        user: i64,
        item: Option<i64>,
        chat: i64,
        message: i64,
    ) -> Result<TextIdentity> {
        let rows = query("SELECT identity FROM runtime_text_effects WHERE state='confirmed' AND receipt=? AND bound=1 AND json_extract(identity,'$.bot')=? AND json_extract(identity,'$.chat')=? AND json_extract(identity,'$.actor')=? AND json_extract(identity,'$.item') IS ? AND json_extract(identity,'$.edit')=0 ORDER BY token LIMIT 1").bind(message).bind(self.delivery.bot_id()).bind(chat).bind(user).bind(item).fetch_all(&self.store.db).await?;
        for row in rows {
            let raw: String = row.try_get("identity")?;
            let i: TextIdentity = serde_json::from_str(&raw).map_err(|_| Error::External)?;
            if !i.edit
                && i.bot == self.delivery.bot_id()
                && i.chat == chat
                && i.actor == user
                && i.item == item
            {
                return Ok(TextIdentity {
                    message: i32::try_from(message).map_err(|_| Error::Invalid)?,
                    edit: true,
                    ..i
                });
            }
        }
        Err(Error::Forbidden)
    }
    /// One centrally owned tick: recovery precedes new admissions. Existing
    /// unknown edits block later edits through Inbox's sequence fencing.
    pub async fn tick(&self) -> Result<()> {
        self.tick_stopping(None).await
    }
    async fn tick_stopping(&self, stop: Option<&CancellationToken>) -> Result<()> {
        self.reconcile_bindings().await?;
        let now = self.now();
        let rows = query("SELECT n.user_id,n.id,n.send_token FROM inbox_notifications n LEFT JOIN runtime_text_effects e ON e.token=n.send_token WHERE n.state IN ('sending','unknown') AND n.send_token IS NOT NULL AND (e.token IS NULL OR e.state IN ('confirmed','sealed')) ORDER BY n.sequence LIMIT ?").bind(self.config.recovery_batch as i64).fetch_all(&self.store.db).await?;
        for row in rows {
            let user: i64 = row.try_get("user_id")?;
            let id: String = row.try_get("id")?;
            let token: String = row.try_get("send_token")?;
            let effect = self
                .delivery
                .ledger()
                .seal_text_absence(&token)
                .await
                .map_err(external)?;
            let outcome = match effect.outcome {
                TextOutcome::Confirmed(receipt) => {
                    let n = self.store.notification(user, &id).await?;
                    let i = effect.identity.ok_or(Error::External)?;
                    if !i.edit
                        || i.bot != self.delivery.bot_id()
                        || i.actor != user
                        || i.item != n.item_id
                        || Some(i.chat) != n.chat_id
                        || Some(i64::from(i.message)) != n.message_id
                        || i.message != receipt
                        || i.payload != n.payload
                    {
                        return Err(Error::External);
                    }
                    Outcome::Confirmed {
                        receipt: receipt.to_string(),
                    }
                }
                TextOutcome::Sealed => Outcome::DefinitelyNotSent { retry_at: now },
                TextOutcome::Unknown => Outcome::Unknown,
                TextOutcome::Prepared => return Err(Error::External),
            };
            self.store
                .notification_outcome(user, &id, &token, outcome, now)
                .await?;
        }
        for _ in 0..self.config.dispatch_batch {
            if stop.is_some_and(|s| s.is_cancelled()) {
                break;
            }
            self.dispatch_one().await?;
        }
        Ok(())
    }
    async fn dispatch_one(&self) -> Result<()> {
        let now = self.now();
        let Some(n) = self
            .store
            .claim_notification(now, self.config.notification_lease.as_micros() as i64)
            .await?
        else {
            return Ok(());
        };
        let Some(chat) = n.chat_id else {
            return Err(Error::Invalid);
        };
        let Some(message) = n.message_id else {
            return Err(Error::Invalid);
        };
        let mut i = match self.provenance(n.user_id, n.item_id, chat, message).await {
            Ok(i) => i,
            Err(Error::Forbidden) => {
                self.store.cancel_notification_claim(&n, self.now()).await?;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let lang = self.language(n.user_id).await;
        i.payload = n.payload.clone();
        let categories = n.payload["categories"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .map(|v| format!("#{}", v.replace(' ', "_")))
            .collect::<Vec<_>>()
            .join(" ");
        i.text = bounded(&format!(
            "{}\n{}\n{}\n{}\n{}",
            translate(lang, Key::InboxCompleted, &[]),
            n.payload["title"].as_str().unwrap_or(""),
            n.payload["summary"].as_str().unwrap_or(""),
            n.payload["url"].as_str().unwrap_or(""),
            categories
        ));
        let route = Route {
            runtime: self,
            identity: i.clone(),
        };
        let auth = Auth(
            self.identities.clone(),
            (i.chat < 0).then_some((i.chat, i.topic.map(i64::from))),
        );
        let token = match self
            .store
            .begin_notification_send(&auth, &route, &n, || self.now())
            .await
        {
            Ok(token) => token,
            Err(Error::Forbidden) => {
                self.store.cancel_notification_claim(&n, self.now()).await?;
                return Ok(());
            }
            Err(Error::Uncertain) => return Ok(()), // stale fenced claim, no attempt
            Err(e) => return Err(e),
        };
        #[cfg(test)]
        pause(&self.hooks.after_admission).await;
        // The exact Inbox send_token is the ledger key, with no prefix or wrapping.
        let outcome = match self.authorize(&i, "ingest").await {
            Ok(()) => self.delivery.dispatch(&token, &i).await.map_err(external)?,
            Err(Error::Forbidden) => {
                // Inbox already says sending: absence must be atomically sealed
                // before it can be called definitely-not-sent. A racing attempt
                // remains Unknown and continues to fence later edits.
                self.delivery
                    .ledger()
                    .seal_text_absence(&token)
                    .await
                    .map_err(external)?
                    .outcome
            }
            Err(e) => return Err(e),
        };
        let outcome_now = self.now();
        let outcome = match outcome {
            TextOutcome::Prepared => return Err(Error::External),
            TextOutcome::Confirmed(id) => Outcome::Confirmed {
                receipt: id.to_string(),
            },
            TextOutcome::Unknown => Outcome::Unknown,
            TextOutcome::Sealed => Outcome::DefinitelyNotSent {
                retry_at: outcome_now,
            },
        };
        self.store
            .notification_outcome(n.user_id, &n.id, &token, outcome, outcome_now)
            .await?;
        Ok(())
    }
    /// Await this future in the composition root's task set. Cancellation stops
    /// admission; an admitted request is allowed its bounded timeout to persist
    /// the result. Never abort until the coordinator's outer drain deadline.
    pub async fn run(self, stop: CancellationToken, interval: Duration) -> Result<()> {
        if interval.is_zero() || interval > Duration::from_secs(60) {
            return Err(Error::Invalid);
        }
        loop {
            if stop.is_cancelled() {
                return Ok(());
            }
            match self.tick_stopping(Some(&stop)).await {
                Ok(()) | Err(Error::Forbidden | Error::Uncertain) => {}
                Err(e) => return Err(e),
            }
            tokio::select! { _ = stop.cancelled() => return Ok(()), _ = tokio::time::sleep(interval) => {} }
        }
    }
    async fn language(&self, user: i64) -> Language {
        self.identities
            .snapshot(user, None)
            .await
            .ok()
            .flatten()
            .map(|s| Language::from_code(Some(&s.language)))
            .unwrap_or(Language::English)
    }
    /// Returns false for unrelated commands. Fresh authorization occurs before
    /// ingestion and again before text dispatch; menus are not authority.
    pub(crate) async fn handle(&self, message: &Message, username: &str) -> bool {
        let Some(text) = message.text() else {
            return false;
        };
        let mut parts = text.splitn(2, char::is_whitespace);
        let head = parts.next().unwrap_or("");
        let Some(command) = head.strip_prefix('/') else {
            return false;
        };
        let (command, addressed) = command
            .split_once('@')
            .map_or((command, None), |(c, b)| (c, Some(b)));
        if addressed.is_some_and(|b| !b.eq_ignore_ascii_case(username))
            || !["save", "inbox", "stars", "stars_sync"].contains(&command)
        {
            return false;
        }
        let Some(actor) = message
            .from
            .as_ref()
            .filter(|u| !u.is_bot && message.sender_chat.is_none())
        else {
            return true;
        };
        let Ok(actor) = i64::try_from(actor.id.0) else {
            return true;
        };
        let mut i = TextIdentity {
            bot: self.delivery.bot_id(),
            chat: message.chat.id.0,
            topic: message.thread_id.map(|v| v.0.0),
            message: message.id.0,
            actor,
            item: None,
            edit: false,
            payload: serde_json::json!({"incoming":text,"reply":reply_source(message)}),
            text: "pending".into(),
        };
        let command_token = format!(
            "inbox-command:{}:{}:{}:{}",
            i.bot,
            i.chat,
            i.topic.unwrap_or(0),
            i.message
        );
        // Recover the immutable original response before re-running application
        // mutations or re-rendering a changed feed/locale. No grants are needed
        // merely to record an already confirmed effect.
        match self.delivery.ledger().text_effect(&command_token).await {
            Ok(Some(effect)) => {
                let same = effect
                    .identity
                    .as_ref()
                    .is_some_and(|old| same_command(old, &i));
                if same {
                    if effect.outcome == TextOutcome::Prepared {
                        let old = effect.identity.expect("checked above");
                        let feature = if command.starts_with("stars") {
                            "gh_sync"
                        } else {
                            "ingest"
                        };
                        if self.authorize(&old, feature).await.is_ok()
                            && self.delivery.dispatch(&command_token, &old).await.is_err()
                        {
                            tracing::warn!(
                                category = "inbox_dispatch",
                                "Prepared Inbox command not completed"
                            );
                        }
                    }
                    let _ = self.reconcile_bindings().await;
                } else {
                    tracing::warn!(
                        category = "inbox_identity_conflict",
                        "Inbox command identity conflicts"
                    );
                }
                return true;
            }
            Ok(None) => {}
            Err(_) => {
                tracing::warn!(
                    category = "inbox_ledger",
                    "Inbox command ledger unavailable"
                );
                return true;
            }
        }
        let feature = if command.starts_with("stars") {
            "gh_sync"
        } else {
            "ingest"
        };
        let result = async {
            self.authorize(&i, feature).await?;
            let language = self.language(actor).await;
            let auth = Auth(
                self.identities.clone(),
                (i.chat < 0).then_some((i.chat, i.topic.map(i64::from))),
            );
            let args = parts.next().unwrap_or("").trim();
            i.text = match command {
                "save" => {
                    let reply = reply_source(message);
                    if let Some(url) = yoink_inbox::telegram::extract_url(args, reply.as_deref()) {
                        if yoink_inbox::normalize::normalize_url(&url).is_err() {
                            translate(language, Key::InboxSaveFailed, &[])
                        } else {
                            auth.require(actor, "inbox:ingest").await?;
                            i = self
                                .prepare_save(&command_token, i.clone(), &url, language)
                                .await?;
                            #[cfg(test)]
                            pause(&self.hooks.after_preparation).await;
                            i.text.clone()
                        }
                    } else {
                        translate(language, Key::InboxSaveUsage, &[])
                    }
                }

                "stars_sync" => {
                    yoink_inbox::telegram::stars_sync(&self.store, &auth, actor, self.now())
                        .await?;
                    translate(language, Key::InboxSyncQueued, &[])
                }
                "inbox" | "stars" => {
                    let v = if command == "inbox" {
                        yoink_inbox::telegram::inbox(&self.store, &auth, actor, 10).await?
                    } else {
                        auth.require(actor, "inbox:gh_sync").await?;
                        self.store
                            .stars(
                                actor,
                                &yoink_inbox::stars::StarFilters {
                                    limit: Some(10),
                                    ..Default::default()
                                },
                            )
                            .await?
                    };
                    let lines: Vec<_> = v["items"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|v| {
                            format!(
                                "{}\n{}",
                                v["title"]
                                    .as_str()
                                    .or_else(|| v["full_name"].as_str())
                                    .unwrap_or(""),
                                v["url"]
                                    .as_str()
                                    .or_else(|| v["html_url"].as_str())
                                    .unwrap_or("")
                            )
                        })
                        .collect();
                    if lines.is_empty() {
                        translate(
                            language,
                            if command == "stars" {
                                Key::InboxStarsEmpty
                            } else {
                                Key::InboxEmpty
                            },
                            &[],
                        )
                    } else {
                        bounded(&lines.join("\n\n"))
                    }
                }
                _ => return Err(Error::Invalid),
            };
            self.authorize(&i, feature).await?;
            self.delivery
                .dispatch(&command_token, &i)
                .await
                .map_err(external)?;
            self.reconcile_bindings().await
        }
        .await;
        if matches!(result, Err(Error::Forbidden)) && i.chat == i.actor && i.topic.is_none() {
            // Existing private denial feedback is not execution authority. It is
            // deliberately item-less and can never become a completion target.
            i.item = None;
            i.text = translate(self.language(actor).await, Key::ErrorDenied, &[]);
            let token = format!("inbox-denial:{}:{}:{}", i.bot, i.chat, i.message);
            let _ = self.delivery.dispatch(&token, &i).await;
        }
        if result.is_err() {
            tracing::warn!(category = "inbox_dispatch", "Inbox command not completed");
        }
        true
    }
}
fn same_command(old: &TextIdentity, new: &TextIdentity) -> bool {
    old.bot == new.bot
        && old.chat == new.chat
        && old.topic == new.topic
        && old.message == new.message
        && old.actor == new.actor
        && !old.edit
        && !new.edit
        && old.payload["incoming"] == new.payload["incoming"]
        && old.payload["reply"] == new.payload["reply"]
}
#[cfg(test)]
type Pause = tokio::sync::Mutex<
    Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
>;
#[cfg(test)]
#[derive(Default)]
struct TestHooks {
    after_admission: Pause,
    after_preparation: Pause,
    before_preparation: Option<Arc<tokio::sync::Barrier>>,
    clock: Option<Arc<std::sync::atomic::AtomicI64>>,
}
#[cfg(test)]
async fn pause(hook: &Pause) {
    let gate = hook.lock().await.take();
    if let Some((entered, resume)) = gate {
        let _ = entered.send(());
        let _ = resume.await;
    }
}
fn reply_source(message: &Message) -> Option<String> {
    let reply = message.reply_to_message()?;
    for entity in reply
        .entities()
        .or_else(|| reply.caption_entities())
        .into_iter()
        .flatten()
    {
        if let teloxide::types::MessageEntityKind::TextLink { url } = &entity.kind {
            return Some(url.to_string());
        }
    }
    reply.text().or_else(|| reply.caption()).map(str::to_owned)
}
fn bounded(text: &str) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|c| {
            used += c.len_utf16();
            used <= 4000
        })
        .collect()
}

#[cfg(test)]
#[path = "inbox_runtime_tests.rs"]
mod tests;
