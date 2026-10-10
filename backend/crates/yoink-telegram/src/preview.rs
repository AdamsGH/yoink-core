//! Bounded, best-effort Bot API effects that cannot confirm a durable response.
use crate::{markdown::FormattedText, text::MAX_TEXT_RESPONSE_BYTES};
use serde_json::{Value, json};
use std::time::Duration;
use teloxide::Bot;

#[derive(Clone)]
pub(crate) struct PreviewTransport {
    bot: Bot,
    timeout: Duration,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DraftOutcome {
    Sent,
    Unsupported,
    Unavailable,
}
impl PreviewTransport {
    pub(crate) fn new(bot: Bot, timeout: Duration) -> Self {
        Self { bot, timeout }
    }
    pub(crate) async fn draft(
        &self,
        chat: i64,
        topic: Option<i32>,
        id: i32,
        text: &FormattedText,
    ) -> DraftOutcome {
        if chat <= 0 || id <= 0 || text.text().encode_utf16().count() > 4096 {
            return DraftOutcome::Unavailable;
        }
        let mut body =
            json!({"chat_id":chat,"draft_id":id,"text":text.text(),"entities":text.entities()});
        if let Some(topic) = topic {
            body["message_thread_id"] = topic.into();
        }
        match self.request("sendMessageDraft", body).await {
            Some(reply) if reply["ok"] == true && reply["result"] == true => DraftOutcome::Sent,
            Some(reply) if reply["error_code"] == 404 => DraftOutcome::Unsupported,
            Some(reply)
                if reply["error_code"] == 400
                    && reply["description"].as_str().is_some_and(|s| {
                        let s = s.to_ascii_lowercase();
                        s.contains("not supported")
                            || s.contains("not available")
                            || s.contains("method not found")
                    }) =>
            {
                DraftOutcome::Unsupported
            }
            _ => DraftOutcome::Unavailable,
        }
    }
    pub(crate) async fn typing(&self, chat: i64, topic: Option<i32>) {
        let mut body = json!({"chat_id":chat,"action":"typing"});
        if let Some(topic) = topic {
            body["message_thread_id"] = topic.into();
        }
        let _ = self.request("sendChatAction", body).await;
    }
    pub(crate) async fn remove(&self, chat: i64, message: i32) {
        let _ = self
            .request(
                "deleteMessage",
                json!({"chat_id":chat,"message_id":message}),
            )
            .await;
    }
    async fn request(&self, method: &str, body: Value) -> Option<Value> {
        let request = async {
            let mut endpoint = self.bot.api_url();
            endpoint.set_path(&format!("/bot{}/{method}", self.bot.token()));
            endpoint.set_query(None);
            endpoint.set_fragment(None);
            let mut response = self
                .bot
                .client()
                .post(endpoint)
                .json(&body)
                .send()
                .await
                .ok()?;
            if response
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
            serde_json::from_slice(&bytes).ok()
        };
        tokio::time::timeout(self.timeout, request)
            .await
            .ok()
            .flatten()
    }
}
