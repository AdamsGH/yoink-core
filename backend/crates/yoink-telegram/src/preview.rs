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
pub(crate) struct DraftResult {
    pub outcome: DraftOutcome,
    pub reason: &'static str,
    pub http_status: Option<u16>,
    pub api_error_code: Option<i64>,
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
    ) -> DraftResult {
        if chat <= 0 || id <= 0 || text.text().encode_utf16().count() > 4096 {
            return DraftResult {
                outcome: DraftOutcome::Unavailable,
                reason: "invalid_draft",
                http_status: None,
                api_error_code: None,
            };
        }
        let mut body =
            json!({"chat_id":chat,"draft_id":id,"text":text.text(),"entities":text.entities()});
        if let Some(topic) = topic {
            body["message_thread_id"] = topic.into();
        }
        let (status, reply) = match self.request("sendMessageDraft", body).await {
            Ok(reply) => reply,
            Err(reason) => {
                return DraftResult {
                    outcome: DraftOutcome::Unavailable,
                    reason,
                    http_status: None,
                    api_error_code: None,
                };
            }
        };
        let outcome = match &reply {
            reply if reply["ok"] == true && reply["result"] == true => DraftOutcome::Sent,
            reply if reply["error_code"] == 404 => DraftOutcome::Unsupported,
            reply
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
        };
        DraftResult {
            outcome,
            reason: match outcome {
                DraftOutcome::Sent => "sent",
                DraftOutcome::Unsupported => "unsupported",
                DraftOutcome::Unavailable => "api_error",
            },
            http_status: Some(status),
            api_error_code: reply["error_code"].as_i64(),
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
    async fn request(&self, method: &str, body: Value) -> Result<(u16, Value), &'static str> {
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
                .map_err(|_| "transport_error")?;
            let status = response.status().as_u16();
            if response
                .content_length()
                .is_some_and(|n| n > MAX_TEXT_RESPONSE_BYTES as u64)
            {
                return Err("response_too_large");
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| "transport_error")? {
                if bytes
                    .len()
                    .checked_add(chunk.len())
                    .ok_or("response_too_large")?
                    > MAX_TEXT_RESPONSE_BYTES
                {
                    return Err("response_too_large");
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok((
                status,
                serde_json::from_slice(&bytes).map_err(|_| "invalid_response")?,
            ))
        };
        tokio::time::timeout(self.timeout, request)
            .await
            .map_err(|_| "timeout")?
    }
}
