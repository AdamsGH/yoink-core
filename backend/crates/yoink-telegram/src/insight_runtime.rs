//! Shared Insight command bridge. No tasks, credentials, or authorization caches.
use crate::text::TextDelivery;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use teloxide::types::Message;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use yoink_access::{AccessPolicy, AccessScope, Role};
use yoink_identity::IdentityStore;
use yoink_insight::{Error, FutureResult, Insight, provider::DeltaSink};
use yoink_storage::{TextIdentity, TextOutcome};

#[derive(Clone)]
pub struct InsightRuntime {
    service: Insight,
    identities: Arc<IdentityStore>,
    delivery: TextDelivery,
    groups: bool,
    stop: CancellationToken,
    active: Arc<Mutex<HashMap<Uuid, Work>>>,
    handlers: crate::insight_lifecycle::Handlers,
}
struct Work {
    actor: i64,
    message: i32,
    chat: i64,
    topic: Option<i32>,
    cancel: CancellationToken,
}
impl InsightRuntime {
    /// Use the same identity store, ledger and Bot API identity as media/Inbox.
    /// No task is spawned. Adapter run binds lifetime to its own shutdown token.
    pub fn new(
        service: Insight,
        identities: Arc<IdentityStore>,
        delivery: TextDelivery,
        groups: bool,
    ) -> Self {
        Self {
            service,
            identities,
            delivery,
            groups,
            stop: CancellationToken::new(),
            active: Arc::new(Mutex::new(HashMap::new())),
            handlers: crate::insight_lifecycle::Handlers::new(),
        }
    }
    pub(crate) fn service(&self) -> Insight {
        self.service.clone()
    }
    pub(crate) fn shutdown_token(&self) -> CancellationToken {
        self.stop.clone()
    }
    /// Permanently reject new handlers and cancel active product work.
    pub fn cancel_all(&self) {
        self.handlers.close();
        self.stop.cancel();
    }
    /// Call after adapter drain/abort, before Insight shutdown and DB close.
    /// Concurrent calls are idempotent; cancelling a waiter does not reopen admission.
    pub async fn drain_handlers(&self) {
        self.cancel_all();
        self.handlers.drain().await;
    }
    pub(crate) fn admit_handler(&self) -> Option<crate::insight_lifecycle::Handler> {
        self.handlers.admit()
    }
    async fn require(
        &self,
        i: &TextIdentity,
        command: &str,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        if cancel.is_cancelled() || self.stop.is_cancelled() {
            return Err(Error::Code("cancelled"));
        }
        if self.service.db.path() != self.delivery.ledger().database().path() {
            return Err(Error::Database);
        }
        if i.bot != self.delivery.bot_id()
            || !i.valid()
            || (i.chat > 0 && (i.chat != i.actor || i.topic.is_some()))
            || (i.chat < 0 && !self.groups)
        {
            return Err(Error::Code("access_denied"));
        }
        if i.chat < 0
            && !self
                .identities
                .membership_eligible(i.bot, i.chat, i.actor)
                .await?
        {
            return Err(Error::Code("access_denied"));
        }
        let s = self
            .identities
            .snapshot(
                i.actor,
                (i.chat < 0).then_some((i.chat, i.topic.map(i64::from))),
            )
            .await?
            .ok_or(Error::Code("access_denied"))?;
        let policy = AccessPolicy {
            min_role: Role::Restricted,
            scopes: vec![AccessScope::Private, AccessScope::Group],
            check_group_enabled: true,
            check_thread_policy: true,
        };
        if !s.authorize(&policy, None).decision.allowed {
            return Err(Error::Code("access_denied"));
        }
        // Product policy includes fresh BYOK toggle/readiness and current grants.
        let settings = self.service.settings(i.actor).await?;
        let field = if command == "tldr" {
            "has_tldr_access"
        } else {
            "has_gemini_access"
        };
        if settings[field] != true {
            return Err(Error::Code("access_denied"));
        }
        if cancel.is_cancelled() || self.stop.is_cancelled() {
            return Err(Error::Code("cancelled"));
        }
        Ok(())
    }
    /// Builtin help advertises only freshly executable product commands.
    pub(crate) async fn help_for(
        &self,
        message: &Message,
        actor: i64,
        language: yoink_i18n::Language,
    ) -> String {
        let Some(_handler) = self.admit_handler() else {
            return String::new();
        };
        let i = TextIdentity {
            bot: self.delivery.bot_id(),
            actor,
            chat: message.chat.id.0,
            topic: message.thread_id.map(|t| t.0.0),
            message: message.id.0,
            item: None,
            edit: false,
            payload: serde_json::json!({}),
            text: "help".into(),
        };
        let russian = language == yoink_i18n::Language::Russian;
        let mut lines = Vec::new();
        if self.require(&i, "summary", &self.stop).await.is_ok() {
            lines.push(if russian {
                "/about, /summary <ссылка YouTube> — о видео и краткое содержание"
            } else {
                "/about, /summary <YouTube URL> — video overview and summary"
            });
        }
        if self.require(&i, "tldr", &self.stop).await.is_ok() {
            lines.push(if russian {
                "/tldr <ссылка> [псевдоним] [вопрос] — краткое содержание"
            } else {
                "/tldr <URL> [alias] [focus] — summarize a URL"
            });
        }
        if lines.is_empty() {
            return String::new();
        }
        lines.push(if russian {
            "/cancel <id> — отменить запрос Insight"
        } else {
            "/cancel <id> — cancel an Insight request"
        });
        format!("Insight\n{}", lines.join("\n"))
    }

    /// Help for an Insight-only admitted subject, without granting media access.
    pub(crate) async fn handle_help(
        &self,
        message: &Message,
        actor: i64,
        language: yoink_i18n::Language,
    ) -> bool {
        let Some(_handler) = self.admit_handler() else {
            return true;
        };
        let text = self.help_for(message, actor, language).await;
        if text.is_empty() {
            return false;
        }
        let i = TextIdentity {
            bot: self.delivery.bot_id(),
            actor,
            chat: message.chat.id.0,
            topic: message.thread_id.map(|t| t.0.0),
            message: message.id.0,
            item: None,
            edit: false,
            payload: serde_json::json!({"incoming":message.text()}),
            text,
        };
        if self.require(&i, "summary", &self.stop).await.is_err()
            && self.require(&i, "tldr", &self.stop).await.is_err()
        {
            return false;
        }
        let token = format!(
            "insight-help:{}:{}:{}:{}",
            i.bot,
            i.chat,
            i.topic.unwrap_or(0),
            i.message
        );
        let _ = self.delivery.dispatch(&token, &i).await;
        true
    }

    /// Called only after incoming_route established an observed actor/destination.
    pub(crate) async fn handle(&self, message: &Message, actor: i64, username: &str) -> bool {
        let Some(_handler) = self.admit_handler() else {
            return true;
        };
        let Some(text) = message.text() else {
            return false;
        };
        let Some(command) = command(text, username) else {
            return false;
        };
        let topic = message.thread_id.map(|t| t.0.0);
        if command == "cancel" {
            if text.split_whitespace().count() != 2 {
                return false;
            }
            let Some(id) = text
                .split_whitespace()
                .nth(1)
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                return false;
            };
            let active = self.active.lock().expect("work registry");
            if let Some(work) = active.get(&id)
                && work.actor == actor
                && work.chat == message.chat.id.0
                && work.topic == topic
            {
                work.cancel.cancel();
                return true;
            }
            return false;
        }
        let i = TextIdentity {
            bot: self.delivery.bot_id(),
            actor,
            chat: message.chat.id.0,
            topic,
            message: message.id.0,
            item: None,
            edit: false,
            payload: serde_json::json!({"incoming":text}),
            text: "pending".into(),
        };
        let base = format!(
            "insight:{}:{}:{}:{}",
            i.bot,
            i.chat,
            topic.unwrap_or(0),
            i.message
        );
        // Any existing admission (including Unknown) fences replay, not just success.
        let initial_token = format!("{base}:0:0");
        match self.delivery.ledger().text_effect(&initial_token).await {
            Ok(None) => {}
            Ok(Some(_)) => {
                #[cfg(feature = "test-fixtures")]
                self.delivery
                    .confirmation_hold()
                    .record_replay(&initial_token);
                return true;
            }
            Err(_) => return true,
        }
        let cancel = self.stop.child_token();
        if self.require(&i, command, &cancel).await.is_err() {
            return true;
        }
        let settings = match self.service.settings(actor).await {
            Ok(s) => s,
            Err(_) => return true,
        };
        let russian = settings["lang"] == "ru";
        let id = Uuid::new_v4();
        {
            let mut active = self.active.lock().expect("work registry");
            if active.len() >= 32
                || active
                    .values()
                    .any(|w| w.chat == i.chat && w.message == i.message)
            {
                return true;
            }
            active.insert(
                id,
                Work {
                    actor,
                    message: i.message,
                    chat: i.chat,
                    topic,
                    cancel: cancel.clone(),
                },
            );
        }
        let _registration = Registration(self.active.clone(), id);
        let sink = Draft {
            runtime: self,
            identity: i,
            command,
            cancel: cancel.clone(),
            base,
            id,
            russian,
            state: tokio::sync::Mutex::new(State {
                text: String::new(),
                sent: vec![],
                byok_final: false,
                revision: 0,
                last: Instant::now() - Duration::from_millis(1500),
            }),
        };
        let execution = yoink_insight::telegram::execute_stream(
            &self.service,
            actor,
            text,
            &sink,
            cancel.clone(),
        );
        tokio::pin!(execution);
        let thinking = tokio::time::sleep(Duration::from_secs(8));
        tokio::pin!(thinking);
        let mut thinking_shown = false;
        let result = loop {
            tokio::select! {
                result = &mut execution => break result,
                _ = &mut thinking, if !thinking_shown => {
                    thinking_shown = true;
                    let empty = sink.state.try_lock().is_ok_and(|s| s.sent.is_empty());
                    if empty && sink.flush(false).await.is_err() { cancel.cancel(); }
                }
            }
        };
        match result {
            Ok(summary) => {
                if summary.result.len() > 1024 * 1024 {
                    return true;
                }
                {
                    let mut state = sink.state.lock().await;
                    state.text = summary.result;
                    state.byok_final = summary.route == "byok";
                }
                let _ = sink.flush(true).await;
            }
            Err(error) => {
                // Partial drafts are always visibly incomplete. Only mark an error
                // while current authority remains; revocation/cancel emits nothing.
                if self.require(&sink.identity, command, &cancel).await.is_ok() {
                    sink.state.lock().await.text = error_text(&error, russian).into();
                    let _ = sink.flush(false).await;
                }
            }
        }
        true
    }
}
/// Insight work and cancellation must not queue behind the same chat's worker.
pub(crate) fn concurrent_command(text: &str) -> bool {
    text.split_whitespace()
        .next()
        .and_then(|s| s.strip_prefix('/'))
        .map(|s| s.split('@').next().unwrap_or(""))
        .is_some_and(|s| matches!(s, "about" | "summary" | "tldr" | "cancel"))
}
fn command<'a>(text: &'a str, username: &str) -> Option<&'a str> {
    let head = text.split_whitespace().next()?.strip_prefix('/')?;
    let (c, bot) = head
        .split_once('@')
        .map_or((head, None), |(c, b)| (c, Some(b)));
    if bot.is_some_and(|b| !b.eq_ignore_ascii_case(username)) {
        return None;
    }
    matches!(c, "about" | "summary" | "tldr" | "cancel").then_some(c)
}
struct Registration(Arc<Mutex<HashMap<Uuid, Work>>>, Uuid);
impl Drop for Registration {
    fn drop(&mut self) {
        self.0.lock().expect("work registry").remove(&self.1);
    }
}
struct State {
    text: String,
    sent: Vec<(i32, String)>,
    byok_final: bool,
    revision: usize,
    last: Instant,
}
struct Draft<'a> {
    runtime: &'a InsightRuntime,
    identity: TextIdentity,
    command: &'a str,
    cancel: CancellationToken,
    base: String,
    id: Uuid,
    russian: bool,
    state: tokio::sync::Mutex<State>,
}
impl Draft<'_> {
    async fn flush(&self, complete: bool) -> Result<(), Error> {
        let mut s = self.state.lock().await;
        let label = match (self.russian, complete) {
            (true, true) => "Готово",
            (true, false) => "Незавершённый ответ",
            (false, true) => "Complete",
            (false, false) => "Incomplete answer",
        };
        // Reserve header space inside the same UTF-16 bound used by product messages.
        let header = format!("{label} · {}\n/cancel {}\n\n", self.command, self.id);
        let chunks = chunks_with_header(&header, &s.text);
        for (index, text) in chunks.into_iter().enumerate() {
            if s.sent.get(index).is_some_and(|(_, old)| old == &text) {
                continue;
            }
            self.runtime
                .require(&self.identity, self.command, &self.cancel)
                .await?;
            if complete && s.byok_final {
                let byok = self.runtime.service.byok_view(self.identity.actor).await?;
                if byok["enabled"] != true
                    || byok["api_key_set"] != true
                    || byok["tested_at"].is_null()
                    || !byok["test_error"].is_null()
                {
                    return Err(Error::Code("access_denied"));
                }
            }
            let mut i = self.identity.clone();
            i.text = text.clone();
            if let Some((receipt, _)) = s.sent.get(index) {
                i.message = *receipt;
                i.edit = true;
            }
            let token = format!("{}:{index}:{}", self.base, s.revision);
            let outcome = self
                .runtime
                .delivery
                .dispatch(&token, &i)
                .await
                .map_err(|_| Error::Database)?;
            s.revision += 1;
            match outcome {
                TextOutcome::Confirmed(receipt) => {
                    if index < s.sent.len() {
                        s.sent[index] = (receipt, text);
                    } else {
                        s.sent.push((receipt, text));
                    }
                }
                _ => {
                    self.cancel.cancel();
                    return Err(Error::Code("outbound_unknown"));
                }
            }
        }
        s.last = Instant::now();
        Ok(())
    }
}
impl DeltaSink for Draft<'_> {
    fn delta<'a>(&'a self, text: &'a str) -> FutureResult<'a, ()> {
        Box::pin(async move {
            self.runtime
                .require(&self.identity, self.command, &self.cancel)
                .await?;
            let mut s = self.state.lock().await;
            if s.text.len().saturating_add(text.len()) > 1024 * 1024 {
                return Err(Error::Code("response_too_large"));
            }
            s.text.push_str(text);
            let flush = s.last.elapsed() >= Duration::from_millis(1500);
            drop(s);
            if flush {
                self.flush(false).await?;
            }
            Ok(())
        })
    }
}

fn chunks_with_header(header: &str, body: &str) -> Vec<String> {
    let budget = 3800.min(4000 - header.encode_utf16().count());
    let mut chunks = Vec::new();
    let mut chunk = header.to_owned();
    let mut used = 0;
    for c in body.chars() {
        if used + c.len_utf16() > budget {
            chunks.push(chunk);
            chunk = header.to_owned();
            used = 0;
        }
        chunk.push(c);
        used += c.len_utf16();
    }
    chunks.push(chunk);
    chunks
}
fn error_text(error: &Error, russian: bool) -> &'static str {
    match (error, russian) {
        (Error::Code("invalid_request"), false) => {
            "Usage: /about or /summary <YouTube URL>; /tldr <URL> [alias] [focus]."
        }
        (Error::Code("invalid_request"), true) => {
            "Использование: /about или /summary <ссылка YouTube>; /tldr <ссылка> [псевдоним] [вопрос]."
        }
        (Error::Code("quota_exceeded"), false) => "Daily request limit reached.",
        (Error::Code("quota_exceeded"), true) => "Достигнут дневной лимит запросов.",
        (
            Error::Code("content_unavailable" | "gemini_not_configured" | "streaming_unavailable"),
            false,
        ) => "Insight is not available. Contact the administrator.",
        (
            Error::Code("content_unavailable" | "gemini_not_configured" | "streaming_unavailable"),
            true,
        ) => "Insight недоступен. Обратитесь к администратору.",
        (_, false) => "Request could not be completed.",
        (_, true) => "Не удалось завершить запрос.",
    }
}

#[cfg(test)]
#[path = "insight_runtime_tests.rs"]
mod tests;
