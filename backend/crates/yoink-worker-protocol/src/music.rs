//! Confined provider-library tasks. Service orchestration remains native.
use crate::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicProvider {
    Spotify,
    Deezer,
    Yandex,
    Ytmusic,
    Soundcloud,
    AppleMusic,
    Bandcamp,
    Qobuz,
    Tidal,
    Youtube,
}
impl MusicProvider {
    pub const ALL: [Self; 10] = [
        Self::Spotify,
        Self::Deezer,
        Self::Yandex,
        Self::Ytmusic,
        Self::Soundcloud,
        Self::AppleMusic,
        Self::Bandcamp,
        Self::Qobuz,
        Self::Tidal,
        Self::Youtube,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Spotify => "spotify",
            Self::Deezer => "deezer",
            Self::Yandex => "yandex",
            Self::Ytmusic => "ytmusic",
            Self::Soundcloud => "soundcloud",
            Self::AppleMusic => "apple_music",
            Self::Bandcamp => "bandcamp",
            Self::Qobuz => "qobuz",
            Self::Tidal => "tidal",
            Self::Youtube => "youtube",
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MusicCredentials {
    pub spotify_client_id: Option<String>,
    pub spotify_client_secret: Option<String>,
    pub tidal_client_id: Option<String>,
    pub tidal_client_secret: Option<String>,
}
impl std::fmt::Debug for MusicCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MusicCredentials([REDACTED])")
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MusicTask {
    MusicParse {
        provider: MusicProvider,
        url: HttpSourceUrl,
    },
    MusicSearch {
        provider: MusicProvider,
        query: String,
        title: String,
        artist: String,
    },
    MusicArtist {
        url: HttpSourceUrl,
    },
    MusicAudio {
        url: HttpSourceUrl,
        title: String,
        artist: String,
        thumbnail_url: Option<HttpSourceUrl>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MusicRequest {
    pub version: u16,
    pub operation_id: Uuid,
    pub attempt: u32,
    pub task: MusicTask,
    pub proxy_url: Option<String>,
    pub credentials: MusicCredentials,
    pub request_timeout_ms: u32,
    pub response_max_bytes: u32,
    pub max_requests: u16,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MusicMetadata {
    pub title: String,
    pub artist: Option<String>,
    pub thumbnail_url: Option<HttpSourceUrl>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MusicArtistTrack {
    pub title: String,
    pub links: Vec<(MusicProvider, String, HttpSourceUrl)>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MusicResult {
    Metadata {
        metadata: MusicMetadata,
    },
    Link {
        url: Option<HttpSourceUrl>,
    },
    Artist {
        name: String,
        genres: Vec<String>,
        thumbnail_url: Option<HttpSourceUrl>,
        platform_links: Vec<(MusicProvider, String, HttpSourceUrl)>,
        top_tracks: Vec<MusicArtistTrack>,
    },
}
fn clean(s: &str, max: usize) -> bool {
    s.len() <= max && !s.chars().any(char::is_control)
}
impl MusicResult {
    pub fn valid(&self) -> bool {
        match self {
            Self::Metadata { metadata: m } => {
                !m.title.trim().is_empty()
                    && clean(&m.title, 1024)
                    && m.artist.as_ref().is_none_or(|s| clean(s, 1024))
            }
            Self::Link { .. } => true,
            Self::Artist {
                name,
                genres,
                platform_links,
                top_tracks,
                ..
            } => {
                !name.is_empty()
                    && clean(name, 1024)
                    && genres.len() <= 20
                    && genres.iter().all(|s| clean(s, 256))
                    && platform_links.len() <= 10
                    && top_tracks.len() <= 10
                    && top_tracks
                        .iter()
                        .all(|t| clean(&t.title, 1024) && t.links.len() <= 10)
            }
        }
    }
}
pub fn encode_music_request(r: &MusicRequest) -> Result<Vec<u8>, ProtocolError> {
    if r.version != PROTOCOL_VERSION
        || r.operation_id.is_nil()
        || r.attempt == 0
        || !(1..=120000).contains(&r.request_timeout_ms)
        || !(1024..=8_388_608).contains(&r.response_max_bytes)
        || !(1..=100).contains(&r.max_requests)
    {
        return Err(ProtocolError::InvalidRequest);
    }
    NetworkCredentials {
        proxy_url: r.proxy_url.clone(),
        cookie_jar: None,
        youtube_oauth: None,
    }
    .validate()?;
    for s in [
        &r.credentials.spotify_client_id,
        &r.credentials.spotify_client_secret,
        &r.credentials.tidal_client_id,
        &r.credentials.tidal_client_secret,
    ]
    .into_iter()
    .flatten()
    {
        if !clean(s, 4096) {
            return Err(ProtocolError::InvalidRequest);
        }
    }
    match &r.task {
        MusicTask::MusicSearch {
            query,
            title,
            artist,
            ..
        } if query.trim().is_empty()
            || !clean(query, 2048)
            || !clean(title, 1024)
            || !clean(artist, 1024) =>
        {
            return Err(ProtocolError::InvalidRequest);
        }
        MusicTask::MusicAudio { title, artist, .. }
            if title.trim().is_empty() || !clean(title, 1024) || !clean(artist, 1024) =>
        {
            return Err(ProtocolError::InvalidRequest);
        }
        _ => {}
    }
    encode(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn music_envelopes_are_closed_bounded_and_credentials_redacted() {
        let mut r = MusicRequest {
            version: PROTOCOL_VERSION,
            operation_id: Uuid::new_v4(),
            attempt: 1,
            task: MusicTask::MusicParse {
                provider: MusicProvider::Spotify,
                url: HttpSourceUrl::parse("https://open.spotify.com/track/abc").unwrap(),
            },
            proxy_url: None,
            credentials: MusicCredentials {
                spotify_client_secret: Some("fixture-secret".into()),
                ..Default::default()
            },
            request_timeout_ms: 1000,
            response_max_bytes: 1024,
            max_requests: 10,
        };
        let bytes = encode_music_request(&r).unwrap();
        let decoded: MusicRequest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            decoded.credentials.spotify_client_secret.as_deref(),
            Some("fixture-secret")
        );
        assert!(!format!("{:?}", r.credentials).contains("fixture-secret"));
        r.max_requests = 0;
        assert!(encode_music_request(&r).is_err());
        r.max_requests = 10;
        r.request_timeout_ms = 120001;
        assert!(encode_music_request(&r).is_err());
    }
    #[test]
    fn music_terminal_results_validate_metadata_and_artist_limits() {
        let good = MusicResult::Metadata {
            metadata: MusicMetadata {
                title: "Song".into(),
                artist: None,
                thumbnail_url: None,
            },
        };
        assert!(good.valid());
        let mut event = WorkerEvent {
            version: PROTOCOL_VERSION,
            operation_id: Uuid::new_v4(),
            attempt: 1,
            event: WorkerEventKind::MusicCompleted { result: good },
        };
        let bytes = encode(&event).unwrap();
        assert!(decode_event_frame(&bytes).is_ok());
        event.event = WorkerEventKind::MusicCompleted {
            result: MusicResult::Metadata {
                metadata: MusicMetadata {
                    title: "".into(),
                    artist: None,
                    thumbnail_url: None,
                },
            },
        };
        assert!(decode_event_frame(&encode(&event).unwrap()).is_err());
    }
}
