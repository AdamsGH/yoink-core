use crate::config::{ConfigError, invalid};
use std::{path::PathBuf, time::Duration};
pub fn from_lookup(
    read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
) -> Result<Option<yoink_user_session::Config>, ConfigError> {
    let origin = read("USER_SESSION_URL")?;
    let token_file = read("USER_SESSION_TOKEN_FILE")?;
    let (origin, token_file) = match (origin, token_file) {
        (None, None) => return Ok(None),
        (Some(u), Some(p)) => (u, p),
        _ => {
            return Err(invalid(
                "USER_SESSION_URL",
                "requires URL and TOKEN_FILE together",
            ));
        }
    };
    fn number(
        read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
        key: &str,
        default: usize,
        max: usize,
    ) -> Result<usize, ConfigError> {
        let n = read(key)?
            .map(|s| s.parse::<usize>())
            .transpose()
            .map_err(|_| invalid(key, "expected bounded positive integer"))?
            .unwrap_or(default);
        if n == 0 || n > max {
            return Err(invalid(key, "expected bounded positive integer"));
        }
        Ok(n)
    }
    let config = yoink_user_session::Config {
        origin: origin
            .parse()
            .map_err(|_| invalid("USER_SESSION_URL", "invalid trusted origin"))?,
        token_file: PathBuf::from(token_file),
        timeout: Duration::from_secs(number(read, "USER_SESSION_TIMEOUT_SECS", 30, 120)? as u64),
        token_timeout: Duration::from_millis(number(
            read,
            "USER_SESSION_TOKEN_TIMEOUT_MS",
            1000,
            10000,
        )? as u64),
        max_response_bytes: number(
            read,
            "USER_SESSION_RESPONSE_BYTES",
            4 * 1024 * 1024,
            16 * 1024 * 1024,
        )?,
        max_token_bytes: number(read, "USER_SESSION_TOKEN_BYTES", 4096, 16384)?,
        max_scan_pages: number(read, "USER_SESSION_SCAN_PAGES", 100, 100)?,
        max_scan_topics: number(read, "USER_SESSION_SCAN_TOPICS", 10000, 10000)?,
    };
    yoink_user_session::Client::new(config.clone()).map_err(|_| {
        invalid(
            "USER_SESSION_URL",
            "invalid origin, absolute token path or bounds",
        )
    })?;
    Ok(Some(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_session_typed_config_is_optional_and_redacted() {
        let mut read = |_: &str| Ok(None);
        assert!(from_lookup(&mut read).unwrap().is_none());
        let mut read = |k: &str| {
            Ok(match k {
                "USER_SESSION_URL" => Some("http://localhost/".into()),
                "USER_SESSION_TOKEN_FILE" => Some("/private/token".into()),
                _ => None,
            })
        };
        let c = from_lookup(&mut read).unwrap().unwrap();
        assert_eq!(c.timeout, Duration::from_secs(30));
        assert!(!format!("{c:?}").contains("localhost"));
        let mut read = |k: &str| {
            Ok(match k {
                "USER_SESSION_URL" => Some("http://user:secret@localhost/".into()),
                "USER_SESSION_TOKEN_FILE" => Some("/private/token".into()),
                _ => None,
            })
        };
        let e = from_lookup(&mut read).unwrap_err();
        assert!(!e.to_string().contains("secret"));
    }
}
