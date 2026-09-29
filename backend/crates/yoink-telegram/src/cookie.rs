use std::convert::Infallible;
use teloxide::{
    prelude::*,
    types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message},
};
use yoink_domain::DeliveryTarget;
use yoink_downloads::{CookieDomain, DownloadService, PersonalCookiePage, PersonalCookieStatus};
use yoink_i18n::{Key, Language};

use crate::{DownloadsDependency, TelegramIdentity, download_error_response, translate};

const PAGE_SIZE: usize = 10;

#[derive(Clone, PartialEq)]
pub(super) enum Action {
    List(Option<i64>),
    Status(CookieDomain),
    Delete(Option<CookieDomain>),
    Invalid,
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::List(_) => "List",
            Self::Status(_) => "Status",
            Self::Delete(_) => "Delete",
            Self::Invalid => "Invalid",
        })
    }
}

pub(super) fn command(args: &[&str]) -> Action {
    match args {
        [] => Action::List(None),
        [name] if name.eq_ignore_ascii_case("clear") => Action::Delete(None),
        [name, domain] if name.eq_ignore_ascii_case("del") => {
            CookieDomain::parse(domain).map_or(Action::Invalid, |d| Action::Delete(Some(d)))
        }
        [name]
            if ["del", "token", "upload"]
                .iter()
                .any(|reserved| name.eq_ignore_ascii_case(reserved)) =>
        {
            Action::Invalid
        }
        [domain] => CookieDomain::parse(domain).map_or(Action::Invalid, Action::Status),
        _ => Action::Invalid,
    }
}

fn cursor(data: &str) -> Option<i64> {
    let raw = data.strip_prefix("cookie:page:")?;
    if raw.is_empty()
        || raw.len() > 20
        || (raw.starts_with('-') && raw.len() == 1)
        || !raw
            .strip_prefix('-')
            .unwrap_or(raw)
            .bytes()
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let value = raw.parse::<i64>().ok()?;
    (value.to_string() == raw).then_some(value)
}

fn render(page: &PersonalCookiePage, language: Language) -> (String, InlineKeyboardMarkup) {
    let domains = page
        .entries
        .iter()
        .take(PAGE_SIZE)
        .map(|entry| {
            entry.domain.as_ref().map_or_else(
                || translate(language, Key::CookieInvalidEntry, &[]),
                |d| d.as_str().to_owned(),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let rows = page
        .next_after
        .map(|after| {
            vec![InlineKeyboardButton::callback(
                translate(language, Key::CookieNext, &[]),
                format!("cookie:page:{after}"),
            )]
        })
        .into_iter()
        .collect::<Vec<_>>();
    let text = if page.entries.is_empty() {
        translate(language, Key::CookieEmpty, &[])
    } else {
        translate(language, Key::CookieList, &[("domains", &domains)])
    };
    (text, InlineKeyboardMarkup::new(rows))
}

fn status_text(domain: &CookieDomain, status: PersonalCookieStatus, language: Language) -> String {
    match status {
        PersonalCookieStatus::Stored { entries } => translate(
            language,
            Key::CookieStored,
            &[("domain", domain.as_str()), ("count", &entries.to_string())],
        ),
        PersonalCookieStatus::Missing => {
            translate(language, Key::CookieMissing, &[("domain", domain.as_str())])
        }
        PersonalCookieStatus::Invalid => {
            translate(language, Key::CookieInvalid, &[("domain", domain.as_str())])
        }
    }
}

async fn apply(
    service: &DownloadService,
    actor: u64,
    target: DeliveryTarget,
    action: Action,
    message_id: i32,
    language: Language,
) -> Result<(String, InlineKeyboardMarkup), String> {
    match action {
        Action::Invalid => Err(translate(language, Key::CookieUsage, &[])),
        Action::List(after) => {
            let page = service
                .personal_cookies(actor, target, after)
                .await
                .map_err(|e| download_error_response(e, language))?;
            Ok(render(&page, language))
        }
        Action::Status(domain) => {
            let status = service
                .personal_cookie_status(actor, target, &domain)
                .await
                .map_err(|e| download_error_response(e, language))?;
            Ok((
                status_text(&domain, status, language),
                InlineKeyboardMarkup::default(),
            ))
        }
        Action::Delete(domain) => {
            match service
                .remove_personal_cookies(actor, target, message_id, domain.as_ref())
                .await
            {
                Ok(Some(count)) => Ok((
                    translate(
                        language,
                        Key::CookieRemoved,
                        &[("count", &count.to_string())],
                    ),
                    InlineKeyboardMarkup::default(),
                )),
                Ok(None) => Ok((
                    translate(language, Key::CookieProcessed, &[]),
                    InlineKeyboardMarkup::default(),
                )),
                Err(e) => Err(download_error_response(e, language)),
            }
        }
    }
}

pub(super) async fn handle_command(
    bot: &Bot,
    message: &Message,
    service: &DownloadService,
    actor: u64,
    target: DeliveryTarget,
    action: Action,
    language: Language,
) {
    match apply(service, actor, target, action, message.id.0, language).await {
        Ok((text, keyboard)) => {
            let _ = bot
                .send_message(message.chat.id, text)
                .reply_markup(keyboard)
                .await;
        }
        Err(text) => {
            let _ = bot.send_message(message.chat.id, text).await;
        }
    }
}

pub(super) async fn handle_callback(
    bot: Bot,
    query: CallbackQuery,
    identity: TelegramIdentity,
    DownloadsDependency(downloads): DownloadsDependency,
) -> Result<(), Infallible> {
    let Some(data) = query.data.as_deref() else {
        return Ok(());
    };
    if !data.starts_with("cookie:") {
        return Ok(());
    }
    let Some(message) = query.message.as_ref().and_then(|m| m.regular_message()) else {
        let _ = bot.answer_callback_query(query.id).await;
        return Ok(());
    };
    if query.from.is_bot
        || query.from.id.0 != identity.owner_id
        || !message.chat.is_private()
        || message.chat.id.0 <= 0
        || message.chat.id.0 as u64 != query.from.id.0
        || message.thread_id.is_some()
        || message.sender_chat.is_some()
        || !message
            .from
            .as_ref()
            .is_some_and(|u| u.is_bot && u.id.0 == identity.bot_id)
    {
        let _ = bot.answer_callback_query(query.id).await;
        return Ok(());
    }
    let language = yoink_i18n::language_for(query.from.language_code.as_deref());
    let Some(service) = downloads.as_ref() else {
        let _ = bot
            .answer_callback_query(query.id)
            .text(translate(language, Key::DownloadsUnavailable, &[]))
            .show_alert(true)
            .await;
        return Ok(());
    };
    let Some(after) = cursor(data) else {
        let _ = bot
            .answer_callback_query(query.id)
            .text(translate(language, Key::CookieStale, &[]))
            .show_alert(true)
            .await;
        return Ok(());
    };
    let Ok(target) = DeliveryTarget::new(identity.bot_id as i64, message.chat.id.0, None, None)
    else {
        let _ = bot.answer_callback_query(query.id).await;
        return Ok(());
    };
    match apply(
        service,
        query.from.id.0,
        target,
        Action::List(Some(after)),
        message.id.0,
        language,
    )
    .await
    {
        Ok((text, keyboard)) => {
            let _ = bot.answer_callback_query(query.id).await;
            let _ = bot
                .edit_message_text(message.chat.id, message.id, text)
                .reply_markup(keyboard)
                .await;
        }
        Err(text) => {
            let _ = bot
                .answer_callback_query(query.id)
                .text(text)
                .show_alert(true)
                .await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_command_suffix_cursor_and_redaction() {
        let cmd = command(&["https://example.com/?token=secret"]);
        assert_eq!(format!("{cmd:?}"), "Invalid");
        assert!(cursor("cookie:page:-42").is_some());
        for bad in [
            "cookie:page:+1",
            "cookie:page:01",
            "cookie:page:1:x",
            "cookie:page:99999999999999999999",
        ] {
            assert!(cursor(bad).is_none());
        }
        assert!(crate::parse_command("/cookie@other_bot", "own_bot").is_none());
        assert!(matches!(
            crate::parse_command("/cookie@OWN_BOT", "own_bot"),
            Some(crate::Command::Cookie(_))
        ));
        assert!(matches!(command(&["clear", "secret"]), Action::Invalid));
        for input in [
            vec!["token"],
            vec!["TOKEN"],
            vec!["del"],
            vec!["del", "example.com", "secret"],
            vec!["upload"],
        ] {
            assert!(matches!(command(&input), Action::Invalid));
        }
        assert!(matches!(command(&["CLEAR"]), Action::Delete(None)));
        assert!(matches!(
            command(&["DEL", "example.com"]),
            Action::Delete(Some(_))
        ));
    }

    #[test]
    fn statuses_render_with_exact_catalog_parameters_in_both_languages() {
        let domain = CookieDomain::parse("example.com").unwrap();
        for language in [Language::English, Language::Russian] {
            for status in [
                PersonalCookieStatus::Missing,
                PersonalCookieStatus::Invalid,
                PersonalCookieStatus::Stored { entries: 2 },
            ] {
                let text = status_text(&domain, status, language);
                assert!(text.contains("example.com"));
                assert!(!text.contains("[telegram."));
                if matches!(status, PersonalCookieStatus::Stored { .. }) {
                    assert!(text.contains('2'));
                }
            }
        }
    }

    #[test]
    fn list_render_caps_safe_entries_and_hides_invalid_value() {
        let page = PersonalCookiePage {
            entries: (0..12)
                .map(|n| yoink_downloads::PersonalCookieEntry {
                    domain: if n == 0 {
                        None
                    } else {
                        CookieDomain::parse(&format!("host{n}.example")).ok()
                    },
                })
                .collect(),
            next_after: Some(12),
        };
        let (text, keyboard) = render(&page, Language::English);
        assert!(text.contains("personal cookie"));
        assert_eq!(
            keyboard.inline_keyboard.iter().map(Vec::len).sum::<usize>(),
            1
        );
        assert!(text.contains("Invalid stored domain"));
        assert!(!text.contains("host10.example"));
        assert!(!text.contains("secret"));
        for language in [Language::English, Language::Russian] {
            let (empty, keyboard) = render(
                &PersonalCookiePage {
                    entries: vec![],
                    next_after: None,
                },
                language,
            );
            assert!(!empty.is_empty());
            assert!(keyboard.inline_keyboard.is_empty());
        }
    }
}
