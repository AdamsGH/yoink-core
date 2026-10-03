use chrono::{DateTime, Utc};
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_access::Role;
use yoink_db::query;

use crate::{IdentityError, IdentityStore};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedProfile {
    pub user_id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub photo_url: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserProfile {
    pub id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub photo_url: Option<String>,
    pub role: Role,
    pub language: String,
    pub theme: String,
    pub ban_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserSettings {
    pub user_id: i64,
    pub language: String,
    pub theme: String,
}

impl IdentityStore {
    pub async fn register(&self, profile: &TrustedProfile) -> Result<UserProfile, IdentityError> {
        if profile.user_id <= 0 || self.owner_id.is_some_and(|id| id <= 0) {
            return Err(IdentityError::InvalidId);
        }
        validate_profile(profile)?;
        let mut connection = self.database.connection().await?;
        let transaction = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| IdentityError::Database)?;
        let result = async {
            let old = query(include_str!("profile_read.sql"))
                .bind(profile.user_id)
                .fetch_optional(&*transaction)
                .await?;
            if let Some(row) = old {
                ensure_row_allowed(&row)?;
                let existing = decode_profile(row)?;
                if existing.updated_at.timestamp_micros() == i64::MAX {
                    return Err(IdentityError::LanguageRevisionOverflow);
                }
                let row = query(include_str!("profile_update.sql"))
                    .bind(profile.username.as_deref())
                    .bind(profile.first_name.as_deref())
                    .bind(profile.photo_url.as_deref())
                    .bind(profile.user_id)
                    .fetch_optional(&*transaction)
                    .await?
                    .ok_or(IdentityError::LanguageRevisionOverflow)?;
                let mut profile = decode_profile(row)?;
                if self.owner_id == Some(profile.id) {
                    profile.role = Role::Owner;
                }
                return Ok(profile);
            }
            let policy_row = query(include_str!("registration_policy.sql"))
                .fetch_optional(&*transaction)
                .await?;
            let policy = policy_row
                .map(|row| {
                    row.try_get::<Option<String>, _>(0)
                        .map_err(|_| IdentityError::Database)
                })
                .transpose()?;
            let role = match policy {
                None => Role::User,
                Some(Some(value)) if value == "open" => Role::User,
                Some(Some(value)) if value == "approved_only" => Role::Restricted,
                _ => return Err(IdentityError::InvalidPolicy),
            };
            let role = if self.owner_id == Some(profile.user_id) {
                Role::Owner
            } else {
                role
            };
            query(include_str!("profile_insert.sql"))
                .bind(profile.user_id)
                .bind(profile.username.as_deref())
                .bind(profile.first_name.as_deref())
                .bind(profile.photo_url.as_deref())
                .bind(role.to_string())
                .execute(&*transaction)
                .await?;
            let row = query(include_str!("profile_read.sql"))
                .bind(profile.user_id)
                .fetch_optional(&*transaction)
                .await?
                .ok_or(IdentityError::Database)?;
            decode_profile(row)
        }
        .await;
        finish(transaction, result).await
    }

    pub async fn current_profile(&self, user_id: i64) -> Result<UserProfile, IdentityError> {
        self.authorized_profile_or_settings(user_id, false, None, None)
            .await
            .map(|(p, _)| p)
    }

    pub async fn settings(&self, user_id: i64) -> Result<UserSettings, IdentityError> {
        self.authorized_profile_or_settings(user_id, true, None, None)
            .await
            .map(|(_, s)| s)
    }

    pub async fn update_settings(
        &self,
        user_id: i64,
        language: Option<&str>,
        theme: Option<&str>,
    ) -> Result<UserSettings, IdentityError> {
        if language.is_some_and(|v| !matches!(v, "en" | "ru")) {
            return Err(IdentityError::InvalidLanguage);
        }
        if theme.is_some_and(|v| !matches!(v, "latte" | "frappe" | "macchiato" | "mocha")) {
            return Err(IdentityError::InvalidTheme);
        }
        self.authorized_profile_or_settings(user_id, true, language, theme)
            .await
            .map(|(_, s)| s)
    }

    async fn authorized_profile_or_settings(
        &self,
        user_id: i64,
        with_settings: bool,
        language: Option<&str>,
        theme: Option<&str>,
    ) -> Result<(UserProfile, UserSettings), IdentityError> {
        if user_id <= 0 || self.owner_id.is_some_and(|id| id <= 0) {
            return Err(IdentityError::InvalidId);
        }
        let mut connection = self.database.connection().await?;
        let transaction = Transaction::new(
            &mut connection,
            if language.is_some() || theme.is_some() {
                TransactionBehavior::Immediate
            } else {
                TransactionBehavior::Deferred
            },
        )
        .await
        .map_err(|_| IdentityError::Database)?;
        let result = async {
            let row = query(include_str!("profile_read.sql"))
                .bind(user_id)
                .fetch_optional(&*transaction)
                .await?
                .ok_or(IdentityError::Unauthorized)?;
            ensure_row_allowed(&row)?;
            let mut profile = decode_profile(row)?;
            if with_settings && (language.is_some() || theme.is_some()) {
                let row = query(include_str!("profile_settings.sql"))
                    .bind(language)
                    .bind(theme)
                    .bind(user_id)
                    .fetch_optional(&*transaction)
                    .await?
                    .ok_or(IdentityError::LanguageRevisionOverflow)?;
                let id: i64 = row.try_get(0).map_err(|_| IdentityError::Database)?;
                profile.language = row.try_get(1).map_err(|_| IdentityError::Database)?;
                profile.theme = row.try_get(2).map_err(|_| IdentityError::Database)?;
                profile.updated_at = query(include_str!("profile_read.sql"))
                    .bind(id)
                    .fetch_one(&*transaction)
                    .await?
                    .try_get(9)
                    .map_err(|_| IdentityError::Database)?;
            }
            if self.owner_id == Some(profile.id) {
                profile.role = Role::Owner;
            }
            let settings = UserSettings {
                user_id,
                language: profile.language.clone(),
                theme: profile.theme.clone(),
            };
            Ok((profile, settings))
        }
        .await;
        finish(transaction, result).await
    }
}

fn validate_profile(profile: &TrustedProfile) -> Result<(), IdentityError> {
    if profile
        .username
        .as_ref()
        .is_some_and(|v| v.is_empty() || v.len() > 64)
        || profile
            .first_name
            .as_ref()
            .is_some_and(|v| v.is_empty() || v.len() > 128)
        || profile
            .photo_url
            .as_ref()
            .is_some_and(|v| !valid_photo_url(v))
    {
        return Err(IdentityError::InvalidProfile);
    }
    Ok(())
}

fn valid_photo_url(value: &str) -> bool {
    if value.len() > 512 || value.chars().any(char::is_whitespace) {
        return false;
    }
    let Some(rest) = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority_valid = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split_once(']').is_some_and(|(host, suffix)| {
            !host.is_empty()
                && host
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.'))
                && valid_port_suffix(suffix)
        })
    } else {
        let (host, port_ok) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, valid_port(port)),
            Some(_) => ("", false),
            None => (authority, true),
        };
        port_ok
            && !host.is_empty()
            && !host.starts_with('.')
            && !host.ends_with('.')
            && host
                .split('.')
                .all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-'))
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    };
    authority_valid
        && !authority.contains('@')
        && !authority.contains('\\')
        && !value.chars().any(|c| c.is_control())
}

fn valid_port_suffix(suffix: &str) -> bool {
    suffix.is_empty() || (suffix.starts_with(':') && valid_port(&suffix[1..]))
}

fn valid_port(port: &str) -> bool {
    !port.is_empty() && port.parse::<u16>().is_ok()
}

fn ensure_row_allowed(row: &yoink_db::Row) -> Result<(), IdentityError> {
    let blocked: i64 = row.try_get(10).map_err(|_| IdentityError::Database)?;
    if blocked != 0 {
        return Err(IdentityError::Forbidden);
    }
    Ok(())
}

fn decode_profile(row: yoink_db::Row) -> Result<UserProfile, IdentityError> {
    let role: String = row.try_get(4).map_err(|_| IdentityError::Database)?;
    Ok(UserProfile {
        id: row.try_get(0).map_err(|_| IdentityError::Database)?,
        username: row.try_get(1).map_err(|_| IdentityError::Database)?,
        first_name: row.try_get(2).map_err(|_| IdentityError::Database)?,
        photo_url: row.try_get(3).map_err(|_| IdentityError::Database)?,
        role: role.parse().map_err(|_| IdentityError::UnknownRole)?,
        language: row.try_get(5).map_err(|_| IdentityError::Database)?,
        theme: row.try_get(6).map_err(|_| IdentityError::Database)?,
        ban_until: row.try_get(7).map_err(|_| IdentityError::Database)?,
        created_at: row.try_get(8).map_err(|_| IdentityError::Database)?,
        updated_at: row.try_get(9).map_err(|_| IdentityError::Database)?,
    })
}

async fn finish<T>(
    transaction: turso::transaction::Transaction<'_>,
    result: Result<T, IdentityError>,
) -> Result<T, IdentityError> {
    match result {
        Ok(value) => {
            transaction
                .commit()
                .await
                .map_err(|_| IdentityError::Database)?;
            Ok(value)
        }
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}
