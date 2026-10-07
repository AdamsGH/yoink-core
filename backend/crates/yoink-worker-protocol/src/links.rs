//! Credential-bearing source stream output. Never a format report or cache entry.
use crate::{HttpSourceUrl, ProtocolError};
use serde::{Deserialize, Serialize};

pub const MAX_LINK_REPORT_BYTES: usize = 4096;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamLinks {
    pub video_url: Option<String>,
    pub audio_url: Option<String>,
}
impl std::fmt::Debug for StreamLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StreamLinks([REDACTED])")
    }
}
impl StreamLinks {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.video_url.is_none() && self.audio_url.is_none() {
            return Err(ProtocolError::InvalidRequest);
        }
        for value in [&self.video_url, &self.audio_url].into_iter().flatten() {
            if value.len() > 1800
                || value.is_empty()
                || !value.is_ascii()
                || value.bytes().any(|b| b <= 32 || b == 127)
            {
                return Err(ProtocolError::InvalidRequest);
            }
            HttpSourceUrl::parse(value)?;
        }
        Ok(())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_LINK_REPORT_BYTES {
            return Err(ProtocolError::InvalidRequest);
        }
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| ProtocolError::InvalidRequest)?;
        value.validate()?;
        Ok(value)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_sensitive_output_is_not_a_format_report() {
        let value = StreamLinks::decode(
            br#"{"video_url":"https://cdn.example/video?signature=private","audio_url":null}"#,
        )
        .unwrap();
        assert!(!format!("{value:?}").contains("signature"));
        for bad in [
            br#"{"video_url":null,"audio_url":null}"#.as_slice(),
            br#"{"video_url":"file:///tmp/x","audio_url":null}"#,
            br#"{"video_url":"https://user:secret@example.com/x","audio_url":null}"#,
            b"Yoink source formats v1\n",
        ] {
            assert!(StreamLinks::decode(bad).is_err());
        }
    }
}
