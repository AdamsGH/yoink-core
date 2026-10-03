use crate::config::{ConfigError, invalid};

#[derive(Debug)]
pub struct AuthHttpConfig {
    pub crypto: yoink_auth::AuthConfig,
    pub owner_id: i64,
    pub dev_enabled: bool,
}

impl AuthHttpConfig {
    pub fn from_lookup(
        read: &mut impl FnMut(&str) -> Result<Option<String>, ConfigError>,
        has_database: bool,
    ) -> Result<Option<Self>, ConfigError> {
        let enabled = boolean(read("AUTH_ENABLED")?, "AUTH_ENABLED")?;
        let dev_enabled = boolean(read("DEV_AUTH_ENABLED")?, "DEV_AUTH_ENABLED")?;
        if !enabled {
            if dev_enabled {
                return Err(invalid("DEV_AUTH_ENABLED", "requires AUTH_ENABLED=true"));
            }
            return Ok(None);
        }
        if !has_database {
            return Err(invalid("AUTH_ENABLED", "requires DATABASE_PATH"));
        }
        let bot_token = read("TELEGRAM_TOKEN")?
            .ok_or_else(|| invalid("TELEGRAM_TOKEN", "required when authentication is enabled"))?;
        let jwt_secret = read("API_SECRET_KEY")?
            .ok_or_else(|| invalid("API_SECRET_KEY", "required when authentication is enabled"))?;
        let owner_id = read("OWNER_ID")?
            .ok_or_else(|| invalid("OWNER_ID", "required when authentication is enabled"))?
            .parse::<i64>()
            .map_err(|_| invalid("OWNER_ID", "expected a positive Telegram user ID"))?;
        if owner_id <= 0 {
            return Err(invalid("OWNER_ID", "expected a positive Telegram user ID"));
        }
        let max_age = number(
            read("AUTH_INIT_DATA_MAX_AGE_SECS")?,
            "AUTH_INIT_DATA_MAX_AGE_SECS",
            86400,
            1,
            86400,
        )?;
        let skew = number(
            read("AUTH_FUTURE_SKEW_SECS")?,
            "AUTH_FUTURE_SKEW_SECS",
            30,
            0,
            300,
        )?;
        let minutes = number(
            read("API_TOKEN_EXPIRE_MINUTES")?,
            "API_TOKEN_EXPIRE_MINUTES",
            1440,
            1,
            10080,
        )?;
        let crypto =
            yoink_auth::AuthConfig::new(bot_token, jwt_secret, max_age, skew, minutes * 60)
                .map_err(|_| {
                    invalid(
                        "AUTH_ENABLED",
                        "invalid authentication credentials or bounds",
                    )
                })?;
        Ok(Some(Self {
            crypto,
            owner_id,
            dev_enabled,
        }))
    }
}

fn boolean(value: Option<String>, key: &str) -> Result<bool, ConfigError> {
    match value.as_deref().unwrap_or("false") {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(invalid(key, "expected true or false")),
    }
}

fn number(
    value: Option<String>,
    key: &str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, ConfigError> {
    let value = value
        .map(|value| value.parse::<u64>())
        .transpose()
        .map_err(|_| invalid(key, "expected an integer within the documented bounds"))?
        .unwrap_or(default);
    if !(min..=max).contains(&value) {
        return Err(invalid(
            key,
            "expected an integer within the documented bounds",
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(
        values: &[(&str, &str)],
        database: bool,
    ) -> Result<Option<AuthHttpConfig>, ConfigError> {
        AuthHttpConfig::from_lookup(
            &mut |key| {
                Ok(values
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| value.to_string()))
            },
            database,
        )
    }
    const BASE: &[(&str, &str)] = &[
        ("AUTH_ENABLED", "true"),
        ("TELEGRAM_TOKEN", "fixture-bot-secret"),
        ("API_SECRET_KEY", "fixture-jwt-secret-at-least-32bytes"),
        ("OWNER_ID", "42"),
    ];
    #[test]
    fn offline_by_default_and_dev_requires_auth() {
        assert!(config(&[], false).unwrap().is_none());
        assert!(config(&[("DEV_AUTH_ENABLED", "true")], true).is_err());
        assert!(config(BASE, false).is_err());
    }
    #[test]
    fn enabled_is_explicit_and_secrets_are_redacted() {
        let config = config(BASE, true).unwrap().unwrap();
        assert!(!config.dev_enabled);
        assert_eq!(config.owner_id, 42);
        let debug = format!("{config:?}");
        assert!(!debug.contains("fixture-bot-secret"));
        assert!(!debug.contains("fixture-jwt-secret"));
    }
    #[test]
    fn invalid_credentials_and_bounds_do_not_expose_input() {
        for (key, value) in [
            ("AUTH_ENABLED", "yes"),
            ("DEV_AUTH_ENABLED", "1"),
            ("OWNER_ID", "0"),
            ("TELEGRAM_TOKEN", " secret "),
            ("API_SECRET_KEY", ""),
            ("AUTH_INIT_DATA_MAX_AGE_SECS", "86401"),
            ("AUTH_FUTURE_SKEW_SECS", "301"),
            ("API_TOKEN_EXPIRE_MINUTES", "10081"),
        ] {
            let mut values = BASE.to_vec();
            values.retain(|(name, _)| *name != key);
            values.push((key, value));
            let error = config(&values, true).unwrap_err();
            assert!(!format!("{error:?}").contains("fixture-"));
        }
    }
}
