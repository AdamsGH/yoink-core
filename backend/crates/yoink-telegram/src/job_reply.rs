use teloxide::{
    prelude::*,
    types::{Message, ReplyParameters},
};
use yoink_domain::DeliveryTarget;
use yoink_downloads::DownloadService;
use yoink_i18n::Key;

use crate::{download_error_response, progress, reply_language, translate, unknown_operation};

fn action(text: &str, username: &str) -> Option<bool> {
    let mut words = text.split_whitespace();
    let command = words.next()?.strip_prefix('/')?;
    if words.next().is_some() {
        return None;
    }
    let name = match command.split_once('@') {
        Some((name, suffix)) if suffix.eq_ignore_ascii_case(username) => name,
        Some(_) => return None,
        None => command,
    };
    match name.to_ascii_lowercase().as_str() {
        "job" => Some(false),
        "cancel" => Some(true),
        _ => None,
    }
}

pub(super) async fn try_handle(
    bot: &Bot,
    message: &Message,
    username: &str,
    service: Option<&DownloadService>,
    actor: u64,
    target: DeliveryTarget,
) -> bool {
    let Some(cancel) = message.text().and_then(|text| action(text, username)) else {
        return false;
    };
    let Some(reply) = message.reply_to_message() else {
        return false;
    };
    let Some(service) = service else {
        return false;
    };
    let language = reply_language(
        Some(service),
        actor,
        message
            .from
            .as_ref()
            .and_then(|user| user.language_code.as_deref()),
    )
    .await;
    let text = match service.operation_by_reply(actor, target, reply.id.0).await {
        Ok(Some(record)) if cancel => match service
            .cancel_in_chat(actor, record.operation_id, target)
            .await
        {
            Ok(true) => translate(language, Key::CancellationRequested, &[]),
            Ok(false) => unknown_operation(language),
            Err(error) => download_error_response(error, language).to_owned(),
        },
        Ok(Some(record)) => progress::render(&record, language),
        Ok(None) => return false,
        Err(error) => {
            if target.chat_id() < 0 {
                return true;
            }
            download_error_response(error, language).to_owned()
        }
    };
    let mut send = bot
        .send_message(message.chat.id, text)
        .reply_parameters(ReplyParameters::new(message.id));
    if let Some(thread) = message.thread_id {
        send = send.message_thread_id(thread);
    }
    if send.await.is_err() {
        tracing::warn!("Telegram download reply delivery failed");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_actions_are_id_free_and_bot_scoped() {
        assert_eq!(action("/job", "yoink"), Some(false));
        assert_eq!(action("/cancel@Yoink", "yoink"), Some(true));
        for text in ["/cancel@other", "/job ID", "/cancel extra", "job"] {
            assert_eq!(action(text, "yoink"), None);
        }
    }
}
