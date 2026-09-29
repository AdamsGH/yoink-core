//! Private actor-scoped cache invalidation. Never include source URLs in replies.
use teloxide::{prelude::*, types::Message};
use url::Url;
use yoink_domain::DeliveryTarget;
use yoink_downloads::DownloadService;
use yoink_i18n::{Key, Language};

use super::{download_error_response, translate};

pub(super) fn parse(args: &[&str]) -> Option<String> {
    let [source] = args else { return None };
    if source.len() > 4096 || source.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let parsed = Url::parse(source).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    Some((*source).to_owned())
}

pub(super) async fn handle_command(
    bot: &Bot,
    message: &Message,
    service: &DownloadService,
    actor: u64,
    target: DeliveryTarget,
    source: Option<String>,
    language: Language,
) {
    let reply = if let Some(source) = source {
        match service
            .clear_media_cache(actor, target, message.id.0, &source)
            .await
        {
            Ok(Some(0)) => translate(language, Key::UncacheEmpty, &[]),
            Ok(Some(count)) => {
                translate(language, Key::UncacheDone, &[("count", &count.to_string())])
            }
            Ok(None) => translate(language, Key::UncacheStale, &[]),
            Err(error) => download_error_response(error, language),
        }
    } else {
        translate(language, Key::UncacheUsage, &[])
    };
    let _ = bot.send_message(message.chat.id, reply).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, parse_command};

    #[test]
    fn grammar_is_closed_and_debug_hides_url() {
        let source = "https://example.test/path?token=private-marker";
        assert_eq!(parse(&[source]), Some(source.to_owned()));
        let command = parse_command(&format!("/uncache@OWN_BOT {source}"), "own_bot").unwrap();
        assert!(matches!(command, Command::Uncache(Some(_))));
        assert!(!format!("{command:?}").contains("private-marker"));
        assert!(parse_command(&format!("/uncache@other_bot {source}"), "own_bot").is_none());
        assert!(parse_command("/reload_cache", "own_bot").is_none());
        for args in [
            vec![],
            vec![source, "all"],
            vec!["file:///etc/passwd"],
            vec!["https://name:password@example.test/"],
            vec!["https://example.test/\nvalue"],
        ] {
            assert_eq!(parse(&args), None);
        }
        assert_eq!(
            parse(&[&format!("https://example.test/{}", "a".repeat(4096))]),
            None
        );
        assert!(matches!(
            parse_command("/uncache", "own_bot"),
            Some(Command::Uncache(None))
        ));
        assert!(matches!(
            parse_command("/uncache all", "own_bot"),
            Some(Command::Uncache(None))
        ));
    }
}
