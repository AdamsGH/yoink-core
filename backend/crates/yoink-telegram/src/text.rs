//! Reusable durable plain-text sends and edits. No retry wrapper is used.
use std::{sync::Arc, time::Duration};
use teloxide::{Bot, types::Message};
use yoink_storage::{Store, StoreError, TextIdentity, TextOutcome};

/// Bounded JSON receipt, including echoed text and reply metadata.
pub const MAX_TEXT_RESPONSE_BYTES: usize = 64 * 1024;
#[derive(Clone)]
pub struct TextDelivery {
    bot: Bot,
    bot_id: i64,
    ledger: Arc<Store>,
    timeout: Duration,
}
impl TextDelivery {
    /// Timeout bounds connection, body reading and decoding together; valid
    /// range is (0,60s]. Responses over 64KiB are ambiguous, never retried.
    pub fn new(
        bot: Bot,
        bot_id: i64,
        ledger: Arc<Store>,
        timeout: Duration,
    ) -> Result<Self, StoreError> {
        if bot_id <= 0 || timeout.is_zero() || timeout > Duration::from_secs(60) {
            return Err(StoreError::InvalidInput);
        }
        Ok(Self {
            bot,
            bot_id,
            ledger,
            timeout,
        })
    }
    pub fn ledger(&self) -> &Arc<Store> {
        &self.ledger
    }
    pub fn bot_id(&self) -> i64 {
        self.bot_id
    }
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
    /// Caller must refresh grants and destination authority immediately before
    /// invoking this method. It never retries an already admitted identity.
    pub async fn dispatch(
        &self,
        token: &str,
        identity: &TextIdentity,
    ) -> Result<TextOutcome, StoreError> {
        if identity.bot != self.bot_id {
            return Err(StoreError::Conflict);
        }
        if !self.ledger.begin_text(token, identity).await? {
            return Ok(self
                .ledger
                .text_effect(token)
                .await?
                .ok_or(StoreError::Database)?
                .outcome);
        }
        let request = async {
            let method = if identity.edit {
                "editMessageText"
            } else {
                "sendMessage"
            };
            let mut endpoint = self.bot.api_url();
            endpoint.set_path(&format!("/bot{}/{method}", self.bot.token()));
            endpoint.set_query(None);
            endpoint.set_fragment(None);
            let mut payload = serde_json::json!({"chat_id":identity.chat,"text":identity.text,"link_preview_options":{"is_disabled":true}});
            if identity.edit {
                payload["message_id"] = identity.message.into();
            } else {
                payload["reply_parameters"] = serde_json::json!({"message_id":identity.message});
                if let Some(topic) = identity.topic {
                    payload["message_thread_id"] = topic.into();
                }
            }
            let mut response = self
                .bot
                .client()
                .post(endpoint)
                .json(&payload)
                .send()
                .await
                .ok()?;
            if !response.status().is_success()
                || response
                    .content_length()
                    .is_some_and(|n| n > MAX_TEXT_RESPONSE_BYTES as u64)
            {
                return None;
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.ok()? {
                if bytes.len().checked_add(chunk.len())? > MAX_TEXT_RESPONSE_BYTES {
                    return None;
                }
                bytes.extend_from_slice(&chunk);
            }
            let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            if value["ok"] != true {
                return None;
            }
            serde_json::from_value::<Message>(value["result"].clone()).ok()
        };
        let message = match tokio::time::timeout(self.timeout, request).await {
            Ok(Some(message)) => message,
            _ => {
                self.ledger.unknown_text(token).await?;
                return Ok(TextOutcome::Unknown);
            }
        };
        // Sender proof is mandatory, not merely a plausible chat/message ID.
        let trusted = message.id.0 > 0
            && ((identity.chat > 0 && message.chat.is_private())
                || (identity.chat < 0
                    && (message.chat.is_group() || message.chat.is_supergroup())))
            && (identity.edit || message.id.0 != identity.message)
            && message.chat.id.0 == identity.chat
            && message.thread_id.map(|v| v.0.0) == identity.topic
            && message
                .from
                .as_ref()
                .is_some_and(|u| u.is_bot && u.id.0 == self.bot_id as u64)
            && message.text() == Some(identity.text.as_str())
            && (!identity.edit || message.id.0 == identity.message);
        if !trusted {
            self.ledger.unknown_text(token).await?;
            return Ok(TextOutcome::Unknown);
        }
        self.ledger.confirm_text(token, message.id.0).await?;
        Ok(TextOutcome::Confirmed(message.id.0))
    }
}
