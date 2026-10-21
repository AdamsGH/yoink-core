use crate::*;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardEntity {
    pub offset: usize,
    pub length: usize,
    pub url: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MusicCard {
    pub title: String,
    pub text: String,
    pub entities: Vec<CardEntity>,
    pub thumbnail_url: Option<String>,
}
pub fn provider_name(p: MusicProvider) -> &'static str {
    match p {
        MusicProvider::Spotify => "Spotify",
        MusicProvider::Deezer => "Deezer",
        MusicProvider::Yandex => "Yandex Music",
        MusicProvider::Ytmusic => "YouTube Music",
        MusicProvider::Soundcloud => "SoundCloud",
        MusicProvider::AppleMusic => "Apple Music",
        MusicProvider::Bandcamp => "Bandcamp",
        MusicProvider::Qobuz => "Qobuz",
        MusicProvider::Tidal => "Tidal",
        MusicProvider::Youtube => "YouTube",
    }
}
impl MusicCard {
    /// Audio metadata is carried by title/performer, not repeated in the 1024-unit caption.
    pub fn audio_caption(user: i64, display_name: &str) -> Self {
        let mut card = Self {
            title: String::new(),
            text: String::new(),
            entities: Vec::new(),
            thumbnail_url: None,
        };
        card.add_requester(user, display_name);
        card
    }
    pub fn from_resolution(resolution: &Resolution) -> Self {
        let (title, thumbnail, links, tracks) = match resolution {
            Resolution::Track(t) => (
                t.metadata
                    .artist
                    .as_ref()
                    .filter(|a| !a.is_empty())
                    .map(|a| format!("{a} - {}", t.metadata.title))
                    .unwrap_or_else(|| t.metadata.title.clone()),
                t.metadata
                    .thumbnail_url
                    .as_ref()
                    .map(|u| u.as_url().to_string()),
                t.links.clone(),
                Vec::new(),
            ),
            Resolution::Artist {
                result:
                    MusicResult::Artist {
                        name,
                        thumbnail_url,
                        platform_links,
                        top_tracks,
                        ..
                    },
                ..
            } => (
                name.clone(),
                thumbnail_url.as_ref().map(|u| u.as_url().to_string()),
                platform_links
                    .iter()
                    .map(|(p, _, u)| MusicLink {
                        provider: *p,
                        url: u.clone(),
                    })
                    .collect(),
                top_tracks.clone(),
            ),
            _ => (String::new(), None, Vec::new(), Vec::new()),
        };
        let mut card = Self {
            text: title.clone(),
            title: title.chars().take(128).collect(),
            entities: Vec::new(),
            thumbnail_url: thumbnail,
        };
        if let Resolution::Artist {
            result: MusicResult::Artist { genres, .. },
            ..
        } = resolution
            && !genres.is_empty()
        {
            card.text.push('\n');
            card.text.push_str(
                &genres
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        for (index, link) in links.into_iter().enumerate() {
            card.append_link(
                provider_name(link.provider),
                &link.url,
                if index == 0 { "\n" } else { " | " },
            );
        }
        for track in tracks.into_iter().take(5) {
            let additional = 2
                + track.title.encode_utf16().count()
                + track
                    .links
                    .iter()
                    .map(|(p, _, _)| 3 + provider_name(*p).len())
                    .sum::<usize>();
            if card.text.encode_utf16().count() + additional > 3800 {
                break;
            }
            card.text.push_str("\n\n");
            card.text.push_str(&track.title);
            for (index, (p, _, u)) in track.links.into_iter().enumerate() {
                card.append_link(provider_name(p), &u, if index == 0 { "\n" } else { " | " });
            }
        }
        card
    }
    fn append_link(
        &mut self,
        name: &str,
        url: &yoink_worker_protocol::HttpSourceUrl,
        separator: &str,
    ) {
        self.text.push_str(separator);
        let offset = self.text.encode_utf16().count();
        self.text.push_str(name);
        self.entities.push(CardEntity {
            offset,
            length: name.encode_utf16().count(),
            url: url.as_url().to_string(),
        });
    }
    pub fn add_requester(&mut self, user: i64, display_name: &str) {
        if user <= 0 {
            return;
        }
        if self.text.encode_utf16().count() > 3800 {
            return;
        }
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        let offset = self.text.encode_utf16().count();
        let name: String = display_name
            .chars()
            .filter(|c| !c.is_control())
            .take(128)
            .collect();
        self.text.push_str(&name);
        self.entities.push(CardEntity {
            offset,
            length: name.encode_utf16().count(),
            url: format!("tg://user?id={user}"),
        });
    }
}
pub fn error_message(error: MusicError, language: &str) -> &'static str {
    use yoink_i18n::Language::{English, Russian};
    match (yoink_i18n::Language::from_code(Some(language)), error) {
        (Russian, MusicError::Denied) => "Нет доступа к музыке.",
        (Russian, MusicError::InvalidInput) => "Вставьте поддерживаемую музыкальную ссылку.",
        (Russian, MusicError::DeliveryUnknown) => {
            "Результат отправки аудио неизвестен. Повторная отправка отключена."
        }
        (Russian, _) => "Не удалось получить музыку. Попробуйте позже.",
        (English, MusicError::Denied) => "Music access denied.",
        (English, MusicError::InvalidInput) => "Paste a supported music link.",
        (English, MusicError::DeliveryUnknown) => {
            "Audio delivery is uncertain. Automatic resend is disabled."
        }
        _ => "Could not fetch music. Try again later.",
    }
}
