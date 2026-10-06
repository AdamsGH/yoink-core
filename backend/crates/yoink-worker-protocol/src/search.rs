//! Metadata-only search is a distinct envelope, never an HTTP source request.
use crate::{MAX_FRAME_BYTES, NetworkCredentials, PROTOCOL_VERSION, ProtocolError};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_SEARCH_QUERY_CHARS: usize = 256;
pub const MAX_SEARCH_QUERY_BYTES: usize = 1024;
pub const MAX_SEARCH_RESULTS: usize = 8;
pub const MAX_SEARCH_FRAME_BYTES: usize = 32 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    pub version: u16,
    pub operation_id: Uuid,
    pub attempt: u32,
    pub task: SearchTask,
    #[serde(default)]
    pub network: Option<SearchNetwork>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SearchTask {
    YoutubeSearch { query: String },
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchNetwork {
    pub proxy_url: Option<String>,
}
impl std::fmt::Debug for SearchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SearchRequest([REDACTED])")
    }
}
impl SearchRequest {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let SearchTask::YoutubeSearch { query } = &self.task;
        let count = query.chars().count();
        if self.version != PROTOCOL_VERSION
            || self.operation_id.is_nil()
            || self.attempt == 0
            || !(2..=MAX_SEARCH_QUERY_CHARS).contains(&count)
            || query.len() > MAX_SEARCH_QUERY_BYTES
            || query.trim() != query
            || query.chars().any(unsafe_display_char)
        {
            return Err(ProtocolError::InvalidSearchRequest);
        }
        if let Some(network) = &self.network {
            NetworkCredentials {
                proxy_url: network.proxy_url.clone(),
                cookie_jar: None,
            }
            .validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchEntry {
    pub url: String,
    pub title: String,
    pub channel: String,
    pub duration_seconds: Option<u32>,
    pub view_count: Option<u64>,
}
impl std::fmt::Debug for SearchEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SearchEntry([REDACTED])")
    }
}
pub fn unsafe_display_char(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
}
impl SearchEntry {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let Some(id) = self.url.strip_prefix("https://www.youtube.com/watch?v=") else {
            return Err(ProtocolError::InvalidEvent);
        };
        if id.len() != 11
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || self.title.chars().count() > 120
            || self.channel.chars().count() > 100
            || self
                .title
                .chars()
                .chain(self.channel.chars())
                .any(unsafe_display_char)
            || self.duration_seconds.is_some_and(|v| v > 31_536_000)
            || self.view_count.is_some_and(|v| v > 1_000_000_000_000_000)
        {
            return Err(ProtocolError::InvalidEvent);
        }
        Ok(())
    }
}
pub fn validate_entries(entries: &[SearchEntry]) -> Result<(), ProtocolError> {
    if entries.len() > MAX_SEARCH_RESULTS {
        return Err(ProtocolError::InvalidEvent);
    }
    let mut urls = std::collections::HashSet::new();
    for entry in entries {
        entry.validate()?;
        if !urls.insert(&entry.url) {
            return Err(ProtocolError::InvalidEvent);
        }
    }
    Ok(())
}
pub fn encode_search_request(request: &SearchRequest) -> Result<Vec<u8>, ProtocolError> {
    request.validate()?;
    let frame = super::encode(request)?;
    if frame.len() > MAX_SEARCH_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    Ok(frame)
}
pub fn decode_search_request(frame: &[u8]) -> Result<SearchRequest, ProtocolError> {
    if frame.len() > MAX_SEARCH_FRAME_BYTES.min(MAX_FRAME_BYTES) {
        return Err(ProtocolError::FrameTooLarge);
    }
    let request: SearchRequest =
        serde_json::from_slice(frame).map_err(|_| ProtocolError::InvalidSearchRequest)?;
    request.validate()?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_envelope_is_explicit_bounded_and_has_no_source_or_cookies() {
        let mut request = SearchRequest {
            version: PROTOCOL_VERSION,
            operation_id: Uuid::new_v4(),
            attempt: 1,
            task: SearchTask::YoutubeSearch {
                query: "unicode кот".into(),
            },
            network: None,
        };
        let frame = encode_search_request(&request).unwrap();
        assert_eq!(decode_search_request(&frame).unwrap(), request);
        assert!(crate::decode_request_frame(&frame).is_err());
        let mut value: serde_json::Value = serde_json::from_slice(&frame).unwrap();
        value["source_url"] = "https://example.com".into();
        assert!(decode_search_request(&serde_json::to_vec(&value).unwrap()).is_err());
        value.as_object_mut().unwrap().remove("source_url");
        value["network"] = serde_json::json!({"cookie_jar": "secret"});
        assert!(decode_search_request(&serde_json::to_vec(&value).unwrap()).is_err());
        request.task = SearchTask::YoutubeSearch {
            query: "x".repeat(257),
        };
        assert!(encode_search_request(&request).is_err());
    }
    #[test]
    fn result_validation_rejects_untrusted_urls_duplicates_and_oversize() {
        let entry = SearchEntry {
            url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            title: "Кот".into(),
            channel: "Channel".into(),
            duration_seconds: Some(1),
            view_count: Some(42),
        };
        assert!(validate_entries(&[]).is_ok());
        assert!(validate_entries(std::slice::from_ref(&entry)).is_ok());
        assert!(validate_entries(&[entry.clone(), entry.clone()]).is_err());
        for url in [
            "https://evil.com/watch?v=abcdefghijk",
            "https://www.youtube.com/watch?v=abcdefghijk&token=secret",
            "https://www.youtube.com/watch?v=short",
        ] {
            let mut bad = entry.clone();
            bad.url = url.into();
            assert!(bad.validate().is_err());
        }
        let mut bad = entry;
        bad.title = "x".repeat(121);
        assert!(bad.validate().is_err());
    }
}
