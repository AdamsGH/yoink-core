use std::{collections::VecDeque, pin::Pin, sync::Arc, time::Duration};

use futures::{Stream, future::BoxFuture, stream};
use serde_json::{Value, json};
use teloxide::{
    stop::{StopFlag, StopToken, mk_stop_token},
    types::Update,
    update_listeners::{AsUpdateStream, UpdateListener},
};
use url::Url;

use crate::membership_update::{self, MembershipUpdate};

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PollingError {
    Transport,
    InvalidResponse,
    InvalidMembership,
    MembershipCommit,
    ProductCommit,
}

pub(crate) type Observer =
    Arc<dyn Fn(MembershipUpdate) -> BoxFuture<'static, Result<(), PollingError>> + Send + Sync>;

/// Preserve raw tag fields and commit membership before the next offset acknowledges them.
pub(crate) struct RawPolling {
    client: reqwest::Client,
    endpoint: Url,
    timeout: Duration,
    bot_id: i64,
    observer: Option<Observer>,
    update_observer: Option<crate::UpdateObserver>,
    offset: i64,
    pending: VecDeque<Update>,
    stop_token: StopToken,
    stop_flag: StopFlag,
    failed: bool,
}

impl RawPolling {
    pub(crate) fn new(
        client: reqwest::Client,
        endpoint: Url,
        timeout: Duration,
        bot_id: i64,
        observer: Option<Observer>,
    ) -> Self {
        let (stop_token, stop_flag) = mk_stop_token();
        Self {
            client,
            endpoint,
            timeout,
            bot_id,
            observer,
            update_observer: None,
            offset: 0,
            pending: VecDeque::new(),
            stop_token,
            stop_flag,
            failed: false,
        }
    }

    pub(crate) fn with_update_observer(mut self, observer: Option<crate::UpdateObserver>) -> Self {
        self.update_observer = observer;
        self
    }

    async fn fetch(&self) -> Result<Vec<Value>, PollingError> {
        let mut allowed = vec![
            "message",
            "callback_query",
            "inline_query",
            "my_chat_member",
            "chat_member",
        ];
        if self.update_observer.is_some() {
            allowed.extend([
                "edited_message",
                "message_reaction",
                "message_reaction_count",
            ]);
        }
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .timeout(self.timeout.saturating_mul(2))
            .json(&json!({
                "offset": self.offset,
                "limit": 100,
                "timeout": self.timeout.as_secs(),
                "allowed_updates": allowed
            }))
            .send()
            .await
            .map_err(|_| PollingError::Transport)?;
        if !response.status().is_success() {
            return Err(PollingError::Transport);
        }
        if response
            .content_length()
            .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
        {
            return Err(PollingError::InvalidResponse);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| PollingError::Transport)?
        {
            if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                return Err(PollingError::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| PollingError::InvalidResponse)?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(PollingError::Transport);
        }
        let updates = value
            .get("result")
            .and_then(Value::as_array)
            .ok_or(PollingError::InvalidResponse)?;
        if updates.len() > 100 {
            return Err(PollingError::InvalidResponse);
        }
        Ok(updates.clone())
    }

    async fn ingest(&mut self, updates: Vec<Value>) -> Result<(), PollingError> {
        let mut decoded = VecDeque::new();
        let mut next_offset = self.offset;
        for value in updates {
            if self.stop_flag.is_stopped() {
                return Ok(());
            }
            // Teloxide captures raw JSON during decoding; from_value silently
            // turns otherwise valid updates into UpdateKind::Error.
            let encoded = serde_json::to_vec(&value).map_err(|_| PollingError::InvalidResponse)?;
            let update: Update =
                serde_json::from_slice(&encoded).map_err(|_| PollingError::InvalidResponse)?;
            if let Some(observer) = &self.observer
                && let Some(observation) = membership_update::parse(&value, self.bot_id)
                    .map_err(|_| PollingError::InvalidMembership)?
            {
                observer(observation).await?;
            }
            if let Some(observer) = &self.update_observer {
                observer(value.clone())
                    .await
                    .map_err(|_| PollingError::ProductCommit)?;
            }
            next_offset = next_offset.max(i64::from(update.id.0) + 1);
            // Malformed inline payloads decode to Error; do not dispatch them.
            // Actor validation is repeated in the handler before identity lookup.
            let valid_inline = match &update.kind {
                teloxide::types::UpdateKind::InlineQuery(query) => {
                    !query.from.is_bot
                        && query.from.id.0 > 0
                        && query.from.id.0 <= i64::MAX as u64
                        && !query.id.0.is_empty()
                        && query.id.0.len() <= 256
                }
                teloxide::types::UpdateKind::Error(_) if value.get("inline_query").is_some() => {
                    false
                }
                _ => true,
            };
            if valid_inline {
                decoded.push_back(update);
            }
        }
        // A failed batch leaves the previous offset intact. Committed observations
        // can replay after restart; membership ordering makes those replays harmless.
        if self.stop_flag.is_stopped() {
            return Ok(());
        }
        self.offset = next_offset;
        self.pending = decoded;
        Ok(())
    }

    async fn next(&mut self) -> Option<Result<Update, PollingError>> {
        loop {
            if self.failed || self.stop_flag.is_stopped() {
                return None;
            }
            if let Some(update) = self.pending.pop_front() {
                return Some(Ok(update));
            }
            let stop = self.stop_flag.clone();
            let fetched = tokio::select! {
                () = stop => return None,
                result = self.fetch() => result,
            };
            let result = match fetched {
                Ok(updates) => self.ingest(updates).await,
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                self.failed = true;
                return Some(Err(error));
            }
        }
    }
}

impl<'a> AsUpdateStream<'a> for RawPolling {
    type StreamErr = PollingError;
    type Stream = Pin<Box<dyn Stream<Item = Result<Update, PollingError>> + Send + 'a>>;

    fn as_stream(&'a mut self) -> Self::Stream {
        Box::pin(stream::unfold(self, |listener| async move {
            listener.next().await.map(|item| (item, listener))
        }))
    }
}

impl UpdateListener for RawPolling {
    type Err = PollingError;

    fn stop_token(&mut self) -> StopToken {
        self.stop_token.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn listener(observer: Option<Observer>) -> RawPolling {
        RawPolling::new(
            reqwest::Client::new(),
            Url::parse("http://127.0.0.1:1/botfixture/getUpdates").unwrap(),
            Duration::from_secs(1),
            7,
            observer,
        )
    }

    fn membership(id: u32) -> Value {
        json!({
            "update_id": id,
            "chat_member": {
                "chat": {"id": -77, "type": "supergroup", "title": "Group"},
                "date": 1700000000,
                "from": {"id": 999, "is_bot": false, "first_name": "Actor"},
                "old_chat_member": {"user": {"id": 123, "is_bot": false, "first_name": "Subject"}, "status": "left"},
                "new_chat_member": {
                    "user": {"id": 123, "is_bot": false, "first_name": "Subject"},
                    "status": "member",
                    "tag": "Download"
                }
            }
        })
    }

    #[tokio::test]
    async fn commits_raw_tags_before_advancing_offset() {
        let calls = Arc::new(AtomicUsize::new(0));
        let received = calls.clone();
        let observer: Observer = Arc::new(move |update| {
            let received = received.clone();
            Box::pin(async move {
                assert_eq!(update.tag.as_deref(), Some("Download"));
                received.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });
        let mut listener = listener(Some(observer));
        listener.ingest(vec![membership(42)]).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(listener.offset, 43);
        assert_eq!(listener.pending.len(), 1);
        assert!(matches!(
            listener.pending[0].kind,
            teloxide::types::UpdateKind::ChatMember(_)
        ));
    }

    #[tokio::test]
    async fn commit_failure_does_not_acknowledge_or_dispatch_partial_batch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let received = calls.clone();
        let observer: Observer = Arc::new(move |_| {
            let received = received.clone();
            Box::pin(async move {
                if received.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(())
                } else {
                    Err(PollingError::MembershipCommit)
                }
            })
        });
        let mut listener = listener(Some(observer));
        listener.offset = 10;
        assert_eq!(
            listener.ingest(vec![membership(42), membership(43)]).await,
            Err(PollingError::MembershipCommit)
        );
        assert_eq!(listener.offset, 10);
        assert!(listener.pending.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn no_identity_store_preserves_ordinary_typed_dispatch() {
        let mut listener = listener(None);
        listener.ingest(vec![membership(42)]).await.unwrap();
        assert_eq!(listener.offset, 43);
        assert_eq!(listener.pending.len(), 1);
    }

    #[tokio::test]
    async fn own_inline_selection_preserves_real_private_group_topic_and_url_intent() {
        let source = "https://example.com/video";
        let text = format!("{source} 10 20");
        let identity = crate::TelegramIdentity {
            owner_id: 42,
            bot_id: 7,
            groups_enabled: true,
            identity_enabled: true,
        };
        for (chat, thread) in [
            (
                json!({"id": 43, "type": "private", "first_name": "User"}),
                None,
            ),
            (
                json!({"id": -77, "type": "supergroup", "title": "Group", "is_forum": true}),
                Some(18),
            ),
        ] {
            let mut value = json!({"update_id": 60, "message": {
                "message_id": 10, "date": 1700000000, "chat": chat,
                "from": {"id": 43, "is_bot": false, "first_name": "User"},
                "via_bot": {"id": 7, "is_bot": true, "first_name": "Bot"},
                "text": text, "entities": [{"type": "url", "offset": 0, "length": source.len()}]
            }});
            if let Some(thread) = thread {
                value["message"]["message_thread_id"] = json!(thread);
            }
            let mut listener = listener(None);
            listener.ingest(vec![value]).await.unwrap();
            let teloxide::types::UpdateKind::Message(message) = &listener.pending[0].kind else {
                panic!("message not decoded");
            };
            let (actor, target) = crate::incoming_route(identity, message).unwrap();
            assert_eq!(actor, 43);
            assert_eq!(target.chat_id(), message.chat.id.0);
            assert_eq!(target.thread_id(), thread);
            let Some(Ok(crate::plain_url::Action::Download(media))) =
                crate::plain_url::parse(message, 7)
            else {
                panic!("selection not parsed");
            };
            assert_eq!(media.url, source);
            assert_eq!(media.clip.unwrap().end_seconds(), 30.0);
        }
    }

    #[tokio::test]
    async fn raw_inline_decodes_real_actor_without_destination_and_skips_bad_actors() {
        let query = json!({"update_id": 50, "inline_query": {
            "id": "fixture-query", "from": {"id": 43, "is_bot": false, "first_name": "User"},
            "query": "https://example.com/a", "offset": "", "chat_type": "supergroup"
        }});
        let mut listener = listener(None);
        listener.ingest(vec![query.clone()]).await.unwrap();
        assert!(
            matches!(&listener.pending[0].kind, teloxide::types::UpdateKind::InlineQuery(q) if q.from.id.0 == 43 && q.query == "https://example.com/a")
        );
        for actor in [
            json!({"id": 43, "is_bot": true, "first_name": "Bot"}),
            json!({"id": 0, "is_bot": false, "first_name": "Bad"}),
            Value::Null,
        ] {
            let mut bad = query.clone();
            bad["update_id"] = 51.into();
            bad["inline_query"]["from"] = actor;
            listener.ingest(vec![bad]).await.unwrap();
            assert!(listener.pending.is_empty());
            assert_eq!(listener.offset, 52);
        }
    }

    #[tokio::test]
    async fn product_commit_failure_does_not_acknowledge_the_batch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let received = calls.clone();
        let observer: crate::UpdateObserver = Arc::new(move |value| {
            let received = received.clone();
            Box::pin(async move {
                assert_eq!(value["update_id"], 42);
                assert_eq!(value["chat_member"]["new_chat_member"]["tag"], "Download");
                received.fetch_add(1, Ordering::SeqCst);
                Err(crate::TelegramError::PollingFailed)
            })
        });
        let mut listener = listener(None).with_update_observer(Some(observer));
        assert_eq!(
            listener.ingest(vec![membership(42)]).await,
            Err(PollingError::ProductCommit)
        );
        assert_eq!(listener.offset, 0);
        assert!(listener.pending.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        listener.update_observer = Some(Arc::new(|_| Box::pin(async { Ok(()) })));
        listener.ingest(vec![membership(42)]).await.unwrap();
        assert_eq!(listener.offset, 43);
        assert_eq!(listener.pending.len(), 1);
    }

    #[tokio::test]
    async fn stopping_during_a_product_observation_does_not_acknowledge_the_partial_batch() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut listener = listener(None);
        let stop = listener.stop_token();
        let (signal, wait, observed) = (entered.clone(), release.clone(), calls.clone());
        listener.update_observer = Some(Arc::new(move |_| {
            let (signal, wait, observed) = (signal.clone(), wait.clone(), observed.clone());
            Box::pin(async move {
                if observed.fetch_add(1, Ordering::SeqCst) != 0 {
                    return Err(crate::TelegramError::PollingFailed);
                }
                signal.notify_one();
                wait.notified().await;
                Ok(())
            })
        }));
        let task = tokio::spawn(async move {
            let result = listener.ingest(vec![membership(42), membership(43)]).await;
            (listener, result)
        });
        entered.notified().await;
        stop.stop();
        release.notify_one();
        let (listener, result) = task.await.unwrap();
        assert!(result.is_ok());
        assert!(!listener.failed);
        assert_eq!(listener.offset, 0);
        assert!(listener.pending.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stop_does_not_contact_api() {
        let mut listener = listener(None);
        listener.stop_token().stop();
        assert!(listener.next().await.is_none());
    }
}
