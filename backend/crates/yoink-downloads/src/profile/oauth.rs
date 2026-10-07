//! Imported legacy OAuth decoding, shared with credential-management integration.
use super::ProfileError;
use chrono::{DateTime, Utc};
use yoink_worker_protocol::youtube_auth::YoutubeOAuth;

pub use yoink_worker_protocol::youtube_auth::OAUTH_CONTENT_PREFIX as CONTENT_PREFIX;
pub use yoink_worker_protocol::youtube_auth::{
    CREDENTIAL_KIND, MAX_OAUTH_CONTENT_BYTES, MAX_OAUTH_TOKEN_BYTES,
};

pub fn decode(content: &str) -> Result<YoutubeOAuth, ProfileError> {
    let tokens = YoutubeOAuth::decode_content(content).map_err(|_| ProfileError::InvalidCookie)?;
    DateTime::parse_from_rfc3339(&tokens.expires_at).map_err(|_| ProfileError::InvalidCookie)?;
    Ok(tokens)
}

pub fn require_fresh(tokens: &YoutubeOAuth, now: DateTime<Utc>) -> Result<(), ProfileError> {
    let expiry = DateTime::parse_from_rfc3339(&tokens.expires_at)
        .map_err(|_| ProfileError::InvalidCookie)?;
    if expiry <= now + chrono::Duration::seconds(30) {
        // Management must CAS-refresh and readmit; executors never mutate frozen rows.
        return Err(ProfileError::StaleReference);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oauth_import_is_typed_bounded_and_redacted() {
        let value = decode("__oauth2__{\"access_token\":\"secret-access\",\"refresh_token\":\"secret-refresh\",\"expires_at\":\"2099-01-01T00:00:00+00:00\"}").unwrap();
        assert!(!format!("{value:?}").contains("secret"));
        assert!(require_fresh(&value, Utc::now()).is_ok());
        for bad in [
            "# Netscape HTTP Cookie File",
            "__oauth2__{}",
            "__oauth2__{\"access_token\":\"a\",\"refresh_token\":\"b\",\"expires_at\":\"bad\"}",
        ] {
            assert!(decode(bad).is_err());
        }
    }
}
