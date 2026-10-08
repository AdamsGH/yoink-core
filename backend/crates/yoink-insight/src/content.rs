use crate::{FutureResult, invalid};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use url::Url;
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Content {
    pub text: String,
    pub title: Option<String>,
    pub video_id: Option<String>,
    pub video_seconds: Option<i32>,
    pub via: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Citation {
    pub url: String,
    pub title: String,
    pub text: String,
}
/// Bounded content-worker input. Implementations must enforce the character cap
/// and ordered transcript languages, falling back to any available transcript.
#[derive(Clone, Copy)]
pub struct FetchOptions<'a> {
    pub transcript_only: bool,
    pub transcript_languages: &'a [String],
    pub max_chars: usize,
    pub github_token: Option<&'a str>,
}
impl std::fmt::Debug for FetchOptions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchOptions")
            .field("transcript_only", &self.transcript_only)
            .field("max_chars", &self.max_chars)
            .finish_non_exhaustive()
    }
}
/// Implement using the shared confined content-fetch worker. Do not implement by
/// fetching arbitrary URLs in the API process. Tokens are read-only GitHub tokens.
pub trait ContentSource: Send + Sync {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        options: FetchOptions<'a>,
        cancel: CancellationToken,
    ) -> FutureResult<'a, Content>;
    fn research<'a>(
        &'a self,
        query: &'a str,
        cancel: CancellationToken,
    ) -> FutureResult<'a, Vec<Citation>>;
}
pub fn normalize(raw: &str) -> Result<Url, crate::Error> {
    let raw = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("https://{raw}")
    };
    let url = Url::parse(&raw).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || raw.len() > 4096
    {
        return Err(invalid());
    }
    Ok(url)
}
pub fn video_id(url: &Url) -> Option<String> {
    yoink_worker_protocol::HttpSourceUrl::parse(url.as_str())
        .ok()
        .and_then(|url| yoink_worker_protocol::full_content_video_id(&url))
}

#[cfg(test)]
mod tests {
    #[test]
    fn youtube_id_matches_worker_query_and_path_semantics() {
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../worker/youtube_video_ids.json")).unwrap();
        for case in cases {
            let url = url::Url::parse(case["url"].as_str().unwrap()).unwrap();
            assert_eq!(super::video_id(&url).as_deref(), case["id"].as_str());
        }
    }
}
