//! Correlated full fetch; independent of credential-free anonymous stages.
use crate::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FullContentOptions {
    pub transcript_only: bool,
    pub transcript_languages: Vec<String>,
    pub max_chars: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_token: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FullContentRequest {
    pub version: u16,
    pub operation_id: Uuid,
    pub attempt: u32,
    pub task: FullContentTask,
    pub url: HttpSourceUrl,
    pub options: FullContentOptions,
    pub timeout_secs: f64,
    pub max_response_bytes: usize,
    pub proxy_url: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FullContentTask {
    FullContentFetch,
}
impl fmt::Debug for FullContentRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FullContentRequest([REDACTED])")
    }
}
impl fmt::Debug for FullContentOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FullContentOptions([REDACTED])")
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FullContentResult {
    pub text: String,
    pub title: Option<String>,
    pub video_id: Option<String>,
    pub video_seconds: Option<i32>,
    pub via: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum FullContentError {
    #[error("no_content")]
    NoContent,
    #[error("no_transcript")]
    NoTranscript,
    #[error("content_network_denied")]
    ContentNetworkDenied,
    #[error("content_timeout")]
    ContentTimeout,
    #[error("content_too_large")]
    ContentTooLarge,
    #[error("content_http_error")]
    ContentHttpError,
    #[error("content_redirect_limit")]
    ContentRedirectLimit,
    #[error("invalid_request")]
    InvalidRequest,
    #[error("content_invalid_response")]
    ContentInvalidResponse,
}
pub fn full_content_video_id(url: &HttpSourceUrl) -> Option<String> {
    let u = url.as_url();
    let id = match u.host_str()? {
        "youtu.be" => u
            .path()
            .trim_start_matches('/')
            .split('/')
            .next()?
            .to_owned(),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            if let Some((_, v)) = u.query_pairs().find(|(k, v)| k == "v" && !v.is_empty()) {
                v.into_owned()
            } else {
                let mut p = u.path().trim_matches('/').split('/');
                if !matches!(p.next()?, "shorts" | "embed" | "v") {
                    return None;
                }
                let id = p.next()?;
                if p.next().is_some() {
                    return None;
                }
                id.to_owned()
            }
        }
        _ => return None,
    };
    (id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')))
    .then_some(id)
}
pub fn full_content_token_relevant(url: &HttpSourceUrl) -> bool {
    url.as_url().scheme() == "https"
        && matches!(
            url.as_url().host_str(),
            Some("github.com" | "api.github.com")
        )
        && url.as_url().port_or_known_default() == Some(443)
}
impl FullContentResult {
    pub fn valid(&self) -> bool {
        !self.text.trim().is_empty()
            && self.text.chars().count() <= 200_000
            && self.title.as_ref().is_none_or(|t| t.chars().count() <= 500)
            && self.video_seconds.is_none_or(|s| s >= 0)
            && self.video_id.as_ref().is_none_or(|id| {
                id.len() == 11
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            })
            && self.via.len() <= 100
            && (matches!(
                self.via.as_str(),
                "jina"
                    | "curl.md"
                    | "readability"
                    | "html-main"
                    | "trafilatura"
                    | "raw-html"
                    | "llms.txt"
                    | "llms.txt+readability"
                    | "llms.txt+html-main"
                    | "llms.txt+trafilatura"
                    | "llms.txt+raw-html"
                    | "github:raw"
                    | "github-api:api"
                    | "github-api:repo"
                    | "github-api:file"
                    | "github-api:tree"
                    | "github-api:issues"
                    | "github-api:pull"
                    | "github-api:pulls"
                    | "github-api:commits"
                    | "github-api:commit"
                    | "github-api:releases"
                    | "youtube-description"
            ) || self
                .via
                .strip_prefix("youtube-transcript:")
                .is_some_and(|s| {
                    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '-')
                }))
    }
    pub fn valid_for(&self, r: &FullContentRequest) -> bool {
        self.valid()
            && self.text.chars().count() <= r.options.max_chars
            && self.video_id == full_content_video_id(&r.url)
            && (self.video_id.is_some() || self.video_seconds.is_none())
            && self.video_id.is_some() == self.via.starts_with("youtube-")
            && (!r.options.transcript_only || self.via.starts_with("youtube-transcript:"))
    }
}
pub fn encode_full_content_request(r: &FullContentRequest) -> Result<Vec<u8>, ProtocolError> {
    let o = &r.options;
    if r.version != PROTOCOL_VERSION
        || r.operation_id.is_nil()
        || r.attempt == 0
        || r.url.as_url().as_str().len() > 4096
        || !(1..=200_000).contains(&o.max_chars)
        || o.transcript_languages.len() > 32
        || o.transcript_languages.iter().any(|s| {
            !(1..=32).contains(&s.chars().count())
                || !s.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
        || !r.timeout_secs.is_finite()
        || r.timeout_secs <= 0.0
        || r.timeout_secs > 300.0
        || !(1024..=8_388_608).contains(&r.max_response_bytes)
        || o.github_token.as_ref().is_some_and(|t| {
            !full_content_token_relevant(&r.url)
                || t.is_empty()
                || t.len() > 4096
                || !t.bytes().all(|b| (33..=126).contains(&b))
        })
    {
        return Err(ProtocolError::InvalidRequest);
    }
    NetworkCredentials {
        proxy_url: r.proxy_url.clone(),
        cookie_jar: None,
        youtube_oauth: None,
    }
    .validate()?;
    encode(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> FullContentRequest {
        FullContentRequest {
            version: 2,
            operation_id: Uuid::new_v4(),
            attempt: 1,
            task: FullContentTask::FullContentFetch,
            url: HttpSourceUrl::parse("https://github.com/owner/repo").unwrap(),
            options: FullContentOptions {
                transcript_only: false,
                transcript_languages: vec!["fr".into(), "en".into()],
                max_chars: 200_000,
                github_token: Some("readonly-secret".into()),
            },
            timeout_secs: 300.0,
            max_response_bytes: 8_388_608,
            proxy_url: Some("https://proxy-user:proxy-secret@proxy.example".into()),
        }
    }
    #[test]
    fn youtube_id_matches_worker_query_and_path_semantics() {
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../worker/youtube_video_ids.json")).unwrap();
        for case in cases {
            let url = HttpSourceUrl::parse(case["url"].as_str().unwrap()).unwrap();
            assert_eq!(full_content_video_id(&url).as_deref(), case["id"].as_str());
        }
    }

    #[test]
    fn full_content_bounds_redaction_and_credential_scope() {
        let mut r = request();
        assert!(encode_full_content_request(&r).is_ok());
        assert!(!format!("{r:?} {:?}", r.options).contains("secret"));
        let encoded = encode_full_content_request(&r).unwrap();
        let decoded: FullContentRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.options.transcript_languages, vec!["fr", "en"]);
        for source in [
            "https://raw.githubusercontent.com/o/r/f",
            "https://r.jina.ai/https://github.com/o/r",
            "http://api.github.com/a",
            "https://api.github.com:444/a",
            "https://provider.example/a",
        ] {
            r.url = HttpSourceUrl::parse(source).unwrap();
            assert!(encode_full_content_request(&r).is_err());
        }
        r = request();
        r.options.transcript_languages = vec!["en".into(); 33];
        assert!(encode_full_content_request(&r).is_err());
        r = request();
        r.options.transcript_languages = vec!["en.*".into()];
        assert!(encode_full_content_request(&r).is_err());
        r = request();
        r.options.max_chars = 200001;
        assert!(encode_full_content_request(&r).is_err());
        r = request();
        r.version = 1;
        assert!(encode_full_content_request(&r).is_err());
    }
    #[test]
    fn full_result_char_cap_video_correspondence_and_known_provenance() {
        let mut r = request();
        r.options.max_chars = 3;
        let mut result = FullContentResult {
            text: "😀éx".into(),
            title: None,
            video_id: None,
            video_seconds: None,
            via: "github-api:repo".into(),
        };
        assert!(result.valid_for(&r));
        result.text.push('x');
        assert!(!result.valid_for(&r));
        result.text.pop();
        result.via = "made-up-stub".into();
        assert!(!result.valid_for(&r));
        result.via = "html-main".into();
        result.video_id = Some("abcdefghijk".into());
        assert!(!result.valid_for(&r));
        r.url = HttpSourceUrl::parse("https://youtu.be/abcdefghijk").unwrap();
        result.via = "youtube-transcript:en".into();
        assert!(result.valid_for(&r));
        result.video_seconds = Some(-1);
        assert!(!result.valid_for(&r));
    }
}
