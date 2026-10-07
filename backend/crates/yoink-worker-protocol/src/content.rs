//! Anonymous confined primitives. No cookies, authorization or GitHub credentials.
use crate::*;
use std::collections::BTreeMap;
pub const MAX_CONTENT_BYTES: usize = 128 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentStage {
    ContentGet {
        url: HttpSourceUrl,
        headers: BTreeMap<String, String>,
    },
    ContentReadability {
        html: String,
        url: HttpSourceUrl,
    },
    ContentTrafilatura {
        html: String,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentRequest {
    pub version: u16,
    pub operation_id: Uuid,
    pub attempt: u32,
    pub task: ContentStage,
    pub proxy_url: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentResult {
    pub text: String,
    pub title: Option<String>,
}
impl ContentResult {
    pub fn valid(&self) -> bool {
        self.text.len() <= MAX_CONTENT_BYTES && self.title.as_ref().is_none_or(|s| s.len() <= 2000)
    }
}
pub fn content_headers_valid(headers: &BTreeMap<String, String>) -> bool {
    headers.iter().all(|(k, v)| {
        matches!(
            (k.as_str(), v.as_str()),
            ("Accept", "text/markdown") | ("X-No-Cache", "true") | ("X-Return-Format", "markdown")
        )
    })
}
pub fn encode_content_request(r: &ContentRequest) -> Result<Vec<u8>, ProtocolError> {
    if r.version != PROTOCOL_VERSION || r.operation_id.is_nil() || r.attempt == 0 {
        return Err(ProtocolError::InvalidRequest);
    }
    NetworkCredentials {
        proxy_url: r.proxy_url.clone(),
        cookie_jar: None,
        youtube_oauth: None,
    }
    .validate()?;
    match &r.task {
        ContentStage::ContentGet { url, headers } => {
            if url.as_url().as_str().len() > 8192 || !content_headers_valid(headers) {
                return Err(ProtocolError::InvalidRequest);
            }
        }
        ContentStage::ContentReadability { html, url } => {
            if html.len() > MAX_CONTENT_BYTES || url.as_url().as_str().len() > 8192 {
                return Err(ProtocolError::InvalidRequest);
            }
        }
        ContentStage::ContentTrafilatura { html } => {
            if html.len() > MAX_CONTENT_BYTES {
                return Err(ProtocolError::InvalidRequest);
            }
        }
    }
    encode(r)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headers_and_result_bounds() {
        assert!(!content_headers_valid(&BTreeMap::from([(
            "Authorization".into(),
            "secret".into()
        )])));
        assert!(
            !ContentResult {
                text: "x".repeat(MAX_CONTENT_BYTES + 1),
                title: None
            }
            .valid()
        );
    }
}
