//! Transient YouTube TV credentials. Never a cookie jar or a queue/API payload.
use serde::{Deserialize, Serialize};
use std::fmt;
pub const OAUTH_CONTENT_PREFIX: &str = "__oauth2__";
pub const CREDENTIAL_KIND: &str = "yttv_oauth";
pub const MAX_OAUTH_CONTENT_BYTES: usize = 64 * 1024;
pub const MAX_OAUTH_TOKEN_BYTES: usize = 16_384;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YoutubeOAuth {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: String,
}
impl fmt::Debug for YoutubeOAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("YoutubeOAuth([REDACTED])")
    }
}
impl YoutubeOAuth {
    pub fn decode_content(content: &str) -> Result<Self, crate::ProtocolError> {
        if content.len() > MAX_OAUTH_CONTENT_BYTES {
            return Err(crate::ProtocolError::InvalidRequest);
        }
        let raw = content
            .strip_prefix(OAUTH_CONTENT_PREFIX)
            .ok_or(crate::ProtocolError::InvalidRequest)?;
        let value: Self =
            serde_json::from_str(raw).map_err(|_| crate::ProtocolError::InvalidRequest)?;
        value.validate()?;
        Ok(value)
    }
    pub fn encode_content(&self) -> Result<String, crate::ProtocolError> {
        self.validate()?;
        let encoded = format!(
            "{}{}",
            OAUTH_CONTENT_PREFIX,
            serde_json::to_string(self).map_err(|_| crate::ProtocolError::InvalidRequest)?
        );
        if encoded.len() > MAX_OAUTH_CONTENT_BYTES {
            return Err(crate::ProtocolError::InvalidRequest);
        }
        Ok(encoded)
    }

    pub fn validate(&self) -> Result<(), crate::ProtocolError> {
        if [&self.access_token, &self.refresh_token]
            .into_iter()
            .any(|s| {
                s.is_empty() || s.len() > MAX_OAUTH_TOKEN_BYTES || s.chars().any(char::is_control)
            })
            || self.expires_at.is_empty()
            || self.expires_at.len() > 64
            || self.expires_at.chars().any(char::is_control)
        {
            return Err(crate::ProtocolError::InvalidRequest);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_oauth_content_roundtrips_with_exact_bounds_and_redacted_debug() {
        let tokens = YoutubeOAuth {
            access_token: "synthetic-secret".into(),
            refresh_token: "r".repeat(MAX_OAUTH_TOKEN_BYTES),
            expires_at: "2099-01-01T00:00:00Z".into(),
        };
        let encoded = tokens.encode_content().unwrap();
        assert_eq!(YoutubeOAuth::decode_content(&encoded).unwrap(), tokens);
        assert!(!format!("{tokens:?}").contains("synthetic-secret"));
        let mut bad = tokens.clone();
        bad.refresh_token.push('r');
        assert!(bad.validate().is_err());
        assert!(
            YoutubeOAuth::decode_content(&format!(
                "{}{}",
                OAUTH_CONTENT_PREFIX,
                " ".repeat(MAX_OAUTH_CONTENT_BYTES)
            ))
            .is_err()
        );
        assert!(YoutubeOAuth::decode_content("__oauth2__{}").is_err());
        assert!(YoutubeOAuth::decode_content("# Netscape HTTP Cookie File").is_err());
    }
}
