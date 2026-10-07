//! Visible Telegram URL entities only. Hidden links and music cards are not downloads.
use teloxide::types::{Message, MessageEntityKind};
use url::Url;
use yoink_domain::{ClipRange, DownloadPreferenceOverrides, MediaMode};

use super::media_command::{ParsedMedia, parse_time, query_start};

#[derive(Debug, PartialEq)]
pub(super) enum Action {
    Download(ParsedMedia),
    AskEnd { media: ParsedMedia, start: f64 },
}

pub(super) fn parse(message: &Message, bot_id: u64) -> Option<Result<Action, ()>> {
    let text = message.text()?;
    if text.trim_start().starts_with('/')
        || message
            .via_bot
            .as_ref()
            .is_some_and(|bot| !bot.is_bot || bot.id.0 != bot_id)
    {
        return None;
    }
    // Ordinary group chatter is never an implicit request to download.
    if !message.chat.is_private()
        && !message
            .via_bot
            .as_ref()
            .is_some_and(|bot| bot.id.0 == bot_id)
        && !message.reply_to_message().is_some_and(|reply| {
            reply
                .from
                .as_ref()
                .is_some_and(|bot| bot.is_bot && bot.id.0 == bot_id)
        })
    {
        return None;
    }
    let entities = message.entities()?;
    if entities
        .iter()
        .any(|entity| matches!(entity.kind, MessageEntityKind::BotCommand))
    {
        return None;
    }
    let mut urls = entities
        .iter()
        .filter(|entity| matches!(entity.kind, MessageEntityKind::Url));
    let entity = urls.next()?;
    if urls.next().is_some() || text.len() > 16_384 {
        return Some(Err(()));
    }
    let result = (|| {
        let end = entity.offset.checked_add(entity.length).ok_or(())?;
        let start_byte = utf16_boundary(text, entity.offset).ok_or(())?;
        let end_byte = utf16_boundary(text, end).ok_or(())?;
        let source = text.get(start_byte..end_byte).ok_or(())?;
        let url = Url::parse(source).map_err(|_| ())?;
        if source.len() > 4096
            || source.chars().any(|c| c.is_whitespace() || c.is_control())
            || !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(());
        }
        if reserved_music_host(url.host_str().ok_or(())?) {
            return Ok(None);
        }
        let start_hint = query_start(source)?;
        let times = text[end_byte..].split_whitespace().collect::<Vec<_>>();
        let mut media = ParsedMedia {
            link_height: None,
            url: source.to_owned(),
            mode: MediaMode::Video,
            playlist: None,
            clip: None,
            engine: None,
            overrides: DownloadPreferenceOverrides::default(),
        };
        let range = match times.as_slice() {
            [] => {
                return Ok(Some(match start_hint {
                    Some(start) if message.chat.is_private() => Action::AskEnd { media, start },
                    Some(_) => return Err(()), // Interactive drafts are private-only.
                    None => Action::Download(media),
                }));
            }
            [end] => (start_hint.ok_or(())?, *end),
            [start, end] => (parse_time(start)?, *end),
            _ => return Err(()),
        };
        let value = parse_time(range.1)?;
        let end = if range.1.contains(':') {
            value
        } else {
            range.0 + value
        };
        media.clip = Some(ClipRange::new(range.0, end).map_err(|_| ())?);
        Ok(Some(Action::Download(media)))
    })();
    match result {
        Ok(action) => action.map(Ok),
        Err(()) => Some(Err(())),
    }
}

// Entity offsets count UTF-16 code units, not UTF-8 bytes or Unicode scalar values.
// A boundary in the middle of a surrogate pair is invalid, never rounded or sliced.
fn utf16_boundary(text: &str, offset: usize) -> Option<usize> {
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        if units == offset {
            return Some(index);
        }
        units += ch.len_utf16();
        if units > offset {
            return None;
        }
    }
    (units == offset).then_some(text.len())
}

pub(super) fn reserved_music_host(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    // These providers belong to yoink-music, not implicit video downloads.
    const HOSTS: &[&str] = &[
        "music.yandex.com",
        "music.yandex.ru",
        "music.yandex.by",
        "music.yandex.kz",
        "spotify.com",
        "tospotify.com",
        "spotify.link",
        "deezer.com",
        "deezer.page.link",
        "link.deezer.com",
        "music.apple.com",
        "music.youtube.com",
        "tidal.com",
        "soundcloud.com",
        "soundcloud.app.goo.gl",
        "bandcamp.com",
        "qobuz.com",
    ];
    HOSTS.iter().any(|base| {
        host == *base
            || host
                .strip_suffix(base)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

#[cfg(test)]
#[path = "plain_url_tests.rs"]
mod tests;
