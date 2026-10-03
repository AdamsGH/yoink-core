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
            offset: 0,
            pending: VecDeque::new(),
            stop_token,
            stop_flag,
            failed: false,
        }
    }

    async fn fetch(&self) -> Result<Vec<Value>, PollingError> {
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .timeout(self.timeout.saturating_mul(2))
            .json(&json!({
                "offset": self.offset,
                "limit": 100,
                "timeout": self.timeout.as_secs(),
                "allowed_updates": ["message", "callback_query", "my_chat_member", "chat_member"]
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
            next_offset = next_offset.max(i64::from(update.id.0) + 1);
            decoded.push_back(update);
        }
        // A failed batch leaves the previous offset intact. Committed observations
        // can replay after restart; membership ordering makes those replays harmless.
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
    async fn stop_does_not_contact_api() {
        let mut listener = listener(None);
        listener.stop_token().stop();
        assert!(listener.next().await.is_none());
    }
}
