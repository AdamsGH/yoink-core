//! Native music product. Only provider libraries and MP3 processing cross the worker boundary.
mod audio;
mod card;
mod persistence;
mod service;
mod stats;
pub use audio::*;
pub use card::*;
pub use persistence::*;
use serde::{Deserialize, Serialize};
pub use service::*;
pub use stats::*;
use url::Url;
use yoink_worker_protocol::HttpSourceUrl;
pub use yoink_worker_protocol::{
    MusicArtistTrack, MusicCredentials, MusicMetadata, MusicProvider, MusicResult, MusicTask,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicLink {
    pub provider: MusicProvider,
    pub url: HttpSourceUrl,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub metadata: MusicMetadata,
    pub source: MusicLink,
    pub links: Vec<MusicLink>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Resolution {
    Track(Track),
    Artist {
        source: HttpSourceUrl,
        result: MusicResult,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MusicError {
    #[error("invalid or unsupported music link")]
    InvalidInput,
    #[error("music access denied")]
    Denied,
    #[error("music source unavailable")]
    SourceUnavailable,
    #[error("music resource limit")]
    ResourceLimit,
    #[error("music operation cancelled")]
    Cancelled,
    #[error("music persistence unavailable")]
    Persistence,
    #[error("audio delivery outcome unknown")]
    DeliveryUnknown,
    #[error("audio delivery rejected")]
    DeliveryRejected,
    #[error("music command conflicts with immutable replay")]
    Conflict,
}

pub fn parse_link(input: &str) -> Result<MusicLink, MusicError> {
    if input.len() > 8192 || input.chars().any(char::is_control) {
        return Err(MusicError::InvalidInput);
    }
    let mut url = Url::parse(input.trim_end_matches(['.', ',', '!', ')', ']', '>']))
        .map_err(|_| MusicError::InvalidInput)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(MusicError::InvalidInput);
    }
    let host = url.host_str().ok_or(MusicError::InvalidInput)?;
    let path = url.path();
    let has = |part: &str| path.split('/').any(|p| p == part);
    let provider = match host {
        "open.spotify.com" if has("track") || has("artist") || has("album") => {
            MusicProvider::Spotify
        }
        "spotify.link" | "tospotify.com" => MusicProvider::Spotify,
        "deezer.com" | "www.deezer.com" if has("track") || has("album") => MusicProvider::Deezer,
        "deezer.page.link" | "link.deezer.com" => MusicProvider::Deezer,
        "music.yandex.ru" | "music.yandex.com" | "music.yandex.by" | "music.yandex.kz"
            if has("track") =>
        {
            MusicProvider::Yandex
        }
        "music.youtube.com"
            if path == "/watch"
                && url
                    .query_pairs()
                    .any(|(k, v)| k == "v" && valid_video_id(&v)) =>
        {
            MusicProvider::Ytmusic
        }
        "youtube.com" | "www.youtube.com"
            if path == "/watch"
                && url
                    .query_pairs()
                    .any(|(k, v)| k == "v" && valid_video_id(&v)) =>
        {
            MusicProvider::Youtube
        }
        "youtu.be" if valid_video_id(path.trim_start_matches('/')) => MusicProvider::Youtube,
        "soundcloud.com" | "www.soundcloud.com"
            if path.split('/').filter(|s| !s.is_empty()).count() == 2 && !has("sets") =>
        {
            MusicProvider::Soundcloud
        }
        "music.apple.com" | "geo.music.apple.com" if has("album") || has("song") => {
            MusicProvider::AppleMusic
        }
        "tidal.com" | "www.tidal.com" | "listen.tidal.com" if has("track") || has("album") => {
            MusicProvider::Tidal
        }
        "qobuz.com" | "www.qobuz.com" if has("track") || has("album") => MusicProvider::Qobuz,
        h if h.ends_with(".bandcamp.com")
            && h.split('.').count() == 3
            && (has("track") || has("album")) =>
        {
            MusicProvider::Bandcamp
        }
        _ => return Err(MusicError::InvalidInput),
    };
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !k.starts_with("utm_") && !matches!(k.as_ref(), "si" | "context" | "nd"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    if !pairs.is_empty() {
        url.query_pairs_mut().extend_pairs(pairs);
    }
    url.set_fragment(None);
    Ok(MusicLink {
        provider,
        url: HttpSourceUrl::parse(url.as_str()).map_err(|_| MusicError::InvalidInput)?,
    })
}
fn valid_video_id(s: &str) -> bool {
    s.len() == 11
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub fn extract_links(text: &str, hidden: &[String], max: usize) -> Vec<MusicLink> {
    let mut result = Vec::new();
    for raw in text
        .match_indices("http")
        .filter_map(|(start, _)| {
            let tail = &text[start..];
            if !tail.starts_with("https://") && !tail.starts_with("http://") {
                return None;
            }
            let end = tail
                .find(|c: char| {
                    c.is_whitespace()
                        || matches!(
                            c,
                            '<' | '>' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}'
                        )
                })
                .unwrap_or(tail.len());
            Some(&tail[..end])
        })
        .chain(hidden.iter().map(String::as_str))
    {
        if let Ok(link) = parse_link(raw)
            && !result.contains(&link)
            && result.len() < max
        {
            result.push(link);
        }
    }
    result
}
