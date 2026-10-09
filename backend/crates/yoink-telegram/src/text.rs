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
    #[cfg(feature = "test-fixtures")]
    confirmation_hold: crate::confirmation_hold::ConfirmationHold,
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
            #[cfg(feature = "test-fixtures")]
            confirmation_hold: Default::default(),
        })
    }
    /// Shared by delivery clones. Coordinator retains this before adapter construction.
    #[cfg(feature = "test-fixtures")]
    pub fn confirmation_hold(&self) -> crate::confirmation_hold::ConfirmationHold {
        self.confirmation_hold.clone()
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
        reject_delivery_metadata(identity)?;
        self.dispatch_entities(token, identity, None, false).await
    }
    /// Accept only the b/pre markup and named escapes emitted by native Stats.
    /// Convert to Telegram entities so receipt echo is exact plain text, not markup.
    pub async fn dispatch_html(
        &self,
        token: &str,
        identity: &TextIdentity,
    ) -> Result<TextOutcome, StoreError> {
        let (identity, entities) = Self::html_identity(identity)?;
        self.dispatch_entities(token, &identity, Some(entities), false)
            .await
    }
    pub(crate) fn html_identity(
        identity: &TextIdentity,
    ) -> Result<(TextIdentity, serde_json::Value), StoreError> {
        reject_delivery_metadata(identity)?;
        if !identity.valid() {
            return Err(StoreError::InvalidInput);
        }
        let (plain, entities) = stats_html(&identity.text)?;
        let binding = serde_json::json!({"_yoink_text_delivery":{"format":"stats-html-v2","markup":identity.text,"plain":plain,"entities":entities},"application":identity.payload});
        let mut identity = identity.clone();
        identity.text = plain;
        identity.payload = binding;
        if !identity.valid() {
            return Err(StoreError::InvalidInput);
        }
        Ok((identity, entities))
    }
    /// Stats has already atomically reauthorized and admitted the bound identity.
    pub(crate) async fn send_admitted_html(
        &self,
        token: &str,
        identity: &TextIdentity,
    ) -> Result<TextOutcome, StoreError> {
        let (identity, entities) = Self::html_identity(identity)?;
        self.dispatch_entities(token, &identity, Some(entities), true)
            .await
    }
    async fn dispatch_entities(
        &self,
        token: &str,
        identity: &TextIdentity,
        entities: Option<serde_json::Value>,
        admitted: bool,
    ) -> Result<TextOutcome, StoreError> {
        if identity.bot != self.bot_id {
            return Err(StoreError::Conflict);
        }
        if !admitted && !self.ledger.begin_text(token, identity).await? {
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
            if let Some(entities) = entities {
                payload["entities"] = entities;
            }
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
        #[cfg(feature = "test-fixtures")]
        self.confirmation_hold.enter(token).await;
        self.ledger.confirm_text(token, message.id.0).await?;
        Ok(TextOutcome::Confirmed(message.id.0))
    }
}

fn reject_delivery_metadata(identity: &TextIdentity) -> Result<(), StoreError> {
    // Plain prepared identities stay byte-for-byte compatible. Application
    // payloads may not claim delivery-owned transport metadata in either mode.
    if identity.payload.get("_yoink_text_delivery").is_some()
        || identity.payload["format"] == "stats-html-v1"
    {
        return Err(StoreError::Conflict);
    }
    Ok(())
}

fn stats_html(markup: &str) -> Result<(String, serde_json::Value), StoreError> {
    let mut rest = markup;
    let mut plain = String::new();
    let mut offset = 0;
    let mut stack = Vec::new();
    let mut entities = Vec::new();
    while !rest.is_empty() {
        if rest.starts_with('<') {
            let end = rest.find('>').ok_or(StoreError::InvalidInput)?;
            let tag = &rest[1..end];
            match tag {
                "b" | "pre" => {
                    // Telegram pre entities cannot overlap any formatting. Native
                    // Stats emits separate heading/body entities, never nesting.
                    if !stack.is_empty() {
                        return Err(StoreError::InvalidInput);
                    }
                    stack.push((tag, offset));
                }
                "/b" | "/pre" => {
                    let (open, start) = stack.pop().ok_or(StoreError::InvalidInput)?;
                    if open != &tag[1..] || offset == start {
                        return Err(StoreError::InvalidInput);
                    }
                    entities.push(serde_json::json!({"type":if open == "b" {"bold"} else {"pre"},"offset":start,"length":offset-start}));
                }
                _ => return Err(StoreError::InvalidInput),
            }
            rest = &rest[end + 1..];
        } else {
            let c = if rest.starts_with('&') {
                let end = rest.find(';').ok_or(StoreError::InvalidInput)?;
                let c = match &rest[..=end] {
                    "&amp;" => '&',
                    "&lt;" => '<',
                    "&gt;" => '>',
                    "&quot;" => '"',
                    _ => return Err(StoreError::InvalidInput),
                };
                rest = &rest[end + 1..];
                c
            } else {
                let c = rest.chars().next().ok_or(StoreError::InvalidInput)?;
                rest = &rest[c.len_utf8()..];
                c
            };
            plain.push(c);
            offset += c.len_utf16();
            if offset > 4096 {
                return Err(StoreError::InvalidInput);
            }
        }
    }
    if !stack.is_empty() || plain.is_empty() {
        return Err(StoreError::InvalidInput);
    }
    Ok((plain, entities.into()))
}
