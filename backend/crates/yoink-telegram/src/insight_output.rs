//! Streaming presentation. Durable sends remain behind the runtime's authority checks.
use super::InsightRuntime;
use crate::{markdown::FormattedText, preview::DraftOutcome};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use yoink_insight::{
    Error, FutureResult,
    provider::{DeltaSink, Phase},
};
use yoink_storage::{TextIdentity, TextOutcome};

pub(super) struct State {
    pub(super) text: String,
    pub(super) sent: Vec<(i32, String)>,
    pub(super) byok_final: bool,
    revision: usize,
    last: Instant,
    typing_at: Instant,
    source: String,
    alias: Option<String>,
    phase: Option<Phase>,
    drafts: bool,
    fenced: bool,
    previewed_bytes: usize,
}
impl State {
    pub(super) fn new(private: bool) -> Self {
        Self {
            text: String::new(),
            sent: vec![],
            byok_final: false,
            revision: 0,
            last: Instant::now() - Duration::from_millis(1500),
            typing_at: Instant::now() - Duration::from_secs(4),
            source: String::new(),
            alias: None,
            phase: None,
            drafts: private,
            fenced: false,
            previewed_bytes: 0,
        }
    }
}
pub(super) struct Draft<'a> {
    pub(super) runtime: &'a InsightRuntime,
    pub(super) identity: TextIdentity,
    pub(super) command: &'a str,
    pub(super) cancel: CancellationToken,
    pub(super) base: String,
    pub(super) id: Uuid,
    pub(super) russian: bool,
    pub(super) state: tokio::sync::Mutex<State>,
}
impl Draft<'_> {
    pub(super) async fn flush(&self, complete: bool) -> Result<(), Error> {
        self.flush_status(complete, false).await
    }
    pub(super) async fn flush_status(&self, complete: bool, failed: bool) -> Result<(), Error> {
        self.flush_with_cancel(complete, failed, &self.cancel).await
    }
    async fn flush_with_cancel(
        &self,
        complete: bool,
        failed: bool,
        authority: &CancellationToken,
    ) -> Result<(), Error> {
        let mut s = self.state.lock().await;
        self.runtime
            .require(&self.identity, self.command, authority)
            .await?;
        let label = header(self.command, s.alias.as_deref(), self.russian);
        let source = (!s.source.is_empty()).then_some((label, s.source.as_str()));
        let body = if complete || failed || !s.text.is_empty() {
            s.text.clone()
        } else {
            format!("{}\n/cancel {}", phase_text(s.phase, self.russian), self.id)
        };
        let rendered = FormattedText::markdown(source, &body)
            .map_err(|_| Error::Code("response_too_large"))?;
        if !complete && !failed && s.drafts {
            if !s.fenced {
                // Ephemeral previews have no receipt. Fence replay using the
                // existing definite-absence tombstone, not a fictitious send.
                let token = format!("{}:0:0", self.base);
                let marker = self
                    .runtime
                    .delivery
                    .ledger()
                    .seal_text_absence(&token)
                    .await
                    .map_err(|_| Error::Database)?;
                if marker.outcome != TextOutcome::Sealed {
                    return Err(Error::Code("outbound_unknown"));
                }
                s.fenced = true;
                s.revision += 1;
            }
            let page = rendered
                .pages(4000)
                .pop()
                .ok_or(Error::Code("empty_content"))?;
            match self
                .runtime
                .delivery
                .preview_transport()
                .draft(
                    self.identity.chat,
                    self.identity.topic,
                    self.identity.message,
                    &page,
                )
                .await
            {
                DraftOutcome::Unsupported => s.drafts = false,
                DraftOutcome::Sent | DraftOutcome::Unavailable => {
                    s.previewed_bytes = s.text.len();
                    s.last = Instant::now();
                    return Ok(());
                }
            }
        }
        let mut chunks = rendered.pages(if complete || failed { 4000 } else { 3800 });
        if chunks.is_empty() {
            return Err(Error::Code("empty_content"));
        }
        if !complete && !failed && !s.text.is_empty() {
            let prefix = if self.russian {
                "Незавершённый ответ\n\n"
            } else {
                "Partial response\n\n"
            };
            chunks = chunks
                .into_iter()
                .map(|chunk| chunk.with_prefix(prefix))
                .collect();
        }
        if failed {
            chunks.resize(s.sent.len().max(chunks.len()), chunks[0].clone());
        }
        if complete && self.identity.chat < 0 && chunks.len() < s.sent.len() {
            let retired = FormattedText::markdown(
                None,
                if self.russian {
                    "Ответ продолжается выше."
                } else {
                    "Response continues above."
                },
            )
            .map_err(|_| Error::Code("response_too_large"))?;
            chunks.resize(s.sent.len(), retired);
        }
        let private_final = complete && self.identity.chat > 0;
        let old = s.sent.clone();
        let mut final_receipts = Vec::new();
        for (index, chunk) in chunks.into_iter().enumerate() {
            let signature =
                serde_json::json!({"text":chunk.text(),"entities":chunk.entities()}).to_string();
            if !private_final && s.sent.get(index).is_some_and(|(_, old)| old == &signature) {
                continue;
            }
            self.runtime
                .require(&self.identity, self.command, authority)
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
            i.text = chunk.text().into();
            if !private_final && let Some((receipt, _)) = s.sent.get(index) {
                i.message = *receipt;
                i.edit = true;
            }
            let token = format!("{}:{index}:{}", self.base, s.revision);
            let outcome = self
                .runtime
                .delivery
                .dispatch_formatted(&token, &i, &chunk)
                .await
                .map_err(|_| Error::Database)?;
            s.revision += 1;
            match outcome {
                TextOutcome::Confirmed(receipt) => {
                    if private_final {
                        final_receipts.push((receipt, signature));
                    } else if index < s.sent.len() {
                        s.sent[index] = (receipt, signature);
                    } else {
                        s.sent.push((receipt, signature));
                    }
                }
                _ => {
                    self.cancel.cancel();
                    return Err(Error::Code("outbound_unknown"));
                }
            }
        }
        if private_final {
            s.sent = final_receipts;
            for (message, _) in old {
                self.runtime
                    .require(&self.identity, self.command, authority)
                    .await?;
                self.runtime
                    .delivery
                    .preview_transport()
                    .remove(self.identity.chat, message)
                    .await;
            }
        }
        s.previewed_bytes = s.text.len();
        s.last = Instant::now();
        Ok(())
    }
    pub(super) async fn cancelled(&self) -> Result<(), Error> {
        self.state.lock().await.text = if self.russian {
            "Отменено."
        } else {
            "Cancelled."
        }
        .into();
        self.flush_with_cancel(false, true, &self.runtime.stop)
            .await
    }
    pub(super) async fn heartbeat(&self) -> Result<(), Error> {
        // Do not queue a heartbeat behind a status or delta writer.
        if self.state.try_lock().is_err() {
            return Ok(());
        }
        self.runtime
            .require(&self.identity, self.command, &self.cancel)
            .await?;
        let Ok(mut s) = self.state.try_lock() else {
            return Ok(());
        };
        if s.typing_at.elapsed() >= Duration::from_secs(4) {
            s.typing_at = Instant::now();
            drop(s);
            self.runtime
                .delivery
                .preview_transport()
                .typing(self.identity.chat, self.identity.topic)
                .await;
            let Ok(state) = self.state.try_lock() else {
                return Ok(());
            };
            s = state;
        }
        let refresh = (!s.text.is_empty()
            && s.text.len() != s.previewed_bytes
            && s.last.elapsed() >= Duration::from_millis(1500))
            || (s.drafts && s.phase.is_some() && s.last.elapsed() >= Duration::from_secs(15));
        drop(s);
        if refresh {
            self.flush(false).await?;
        }
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
            let flush = s.previewed_bytes == 0 || s.last.elapsed() >= Duration::from_millis(1500);
            drop(s);
            if flush {
                self.flush(false).await?;
            }
            Ok(())
        })
    }
    fn phase<'a>(
        &'a self,
        phase: Phase,
        source: &'a str,
        alias: Option<&'a str>,
    ) -> FutureResult<'a, ()> {
        Box::pin(async move {
            self.runtime
                .require(&self.identity, self.command, &self.cancel)
                .await?;
            let mut s = self.state.lock().await;
            s.phase = Some(phase);
            s.source = source.into();
            s.alias = alias.map(str::to_owned);
            drop(s);
            if phase != Phase::Cached {
                self.flush(false).await?;
            }
            Ok(())
        })
    }
}
fn header(command: &str, alias: Option<&str>, russian: bool) -> &'static str {
    match (command, alias, russian) {
        ("about", _, true) => "О видео",
        ("about", _, false) => "Video overview",
        (_, Some("nobullshit" | "noshit"), true) => "🔗 Без хуйни",
        (_, Some("nobullshit" | "noshit"), false) => "🔗 No BS",
        (_, Some("tale"), true) => "🔗 Сказ",
        (_, Some("tale"), false) => "🔗 The Tale",
        (_, _, true) => "🔗 Краткое содержание",
        (_, _, false) => "🔗 Summary",
    }
}
fn phase_text(phase: Option<Phase>, russian: bool) -> &'static str {
    match (phase, russian) {
        (Some(Phase::Fetching), true) => "Получаю содержимое источника.",
        (Some(Phase::Fetching), false) => "Reading the source.",
        (Some(Phase::Researching), true) => "Ищу независимые источники.",
        (Some(Phase::Researching), false) => "Searching independent sources.",
        (Some(Phase::Generating), true) => "Генерирую ответ.",
        (Some(Phase::Generating), false) => "Generating the response.",
        (_, true) => "Обрабатываю запрос.",
        (_, false) => "Processing the request.",
    }
}
