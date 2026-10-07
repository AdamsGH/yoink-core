use std::fmt;

use turso::transaction::{Transaction, TransactionBehavior};
use yoink_db::query;
use yoink_domain::DeliveryTarget;

use crate::{DownloadError, DownloadService, profile::valid_cookie};

#[derive(Clone, Eq, PartialEq)]
pub struct CookieDomain(String);

impl CookieDomain {
    pub fn parse(value: &str) -> Result<Self, DownloadError> {
        if value.is_empty() || value.len() > 255 || !value.is_ascii() {
            return Err(DownloadError::InvalidInput);
        }
        let canonical = value.trim_matches('.').to_ascii_lowercase();
        if canonical.is_empty()
            || canonical.len() > 253
            || canonical.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
        {
            return Err(DownloadError::InvalidInput);
        }
        Ok(Self(canonical))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CookieDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CookieDomain([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct PersonalCookieEntry {
    pub domain: Option<CookieDomain>,
}

impl fmt::Debug for PersonalCookieEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersonalCookieEntry")
            .field("domain", &self.domain)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersonalCookiePage {
    pub entries: Vec<PersonalCookieEntry>,
    pub next_after: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersonalCookieStatus {
    Missing,
    Invalid,
    Stored { entries: u64 },
}

impl DownloadService {
    pub(super) async fn authorize_cookie_control(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<(), DownloadError> {
        if actor == 0
            || actor > i64::MAX as u64
            || target.chat_id() != actor as i64
            || target.chat_id() <= 0
            || target.thread_id().is_some()
            || target.bot_id() != self.delivery.bot_id()
        {
            return Err(DownloadError::Denied);
        }
        self.cookie_fresh_authorization(actor as i64).await
    }

    async fn cookie_fresh_authorization(&self, actor: i64) -> Result<(), DownloadError> {
        let allowed = query(include_str!("personal_cookies_auth.sql"))
            .bind(actor)
            .bind(chrono::Utc::now())
            .bind(self.identities.is_configured_owner(actor))
            .fetch_optional(self.store.database())
            .await
            .map_err(|_| DownloadError::Database)?;
        if allowed.is_some() {
            Ok(())
        } else {
            Err(DownloadError::Denied)
        }
    }

    pub async fn personal_cookie_upload_allowed(
        &self,
        actor: u64,
        target: DeliveryTarget,
        message_id: i32,
    ) -> Result<bool, DownloadError> {
        self.authorize_cookie_control(actor, target).await?;
        if message_id <= 0 {
            return Err(DownloadError::InvalidInput);
        }
        let watermark = query(include_str!("personal_cookies_upload_preflight.sql"))
            .bind(actor as i64)
            .bind(message_id)
            .fetch_optional(self.store.database())
            .await
            .map_err(|_| DownloadError::Database)?;
        Ok(watermark.is_some())
    }

    pub async fn store_personal_cookie(
        &self,
        actor: u64,
        target: DeliveryTarget,
        message_id: i32,
        domain: &CookieDomain,
        content: &str,
    ) -> Result<Option<u64>, DownloadError> {
        self.authorize_cookie_control(actor, target).await?;
        if message_id <= 0
            || content.len() > yoink_worker_protocol::MAX_COOKIE_JAR_BYTES
            || !valid_cookie(content)
        {
            return Err(DownloadError::InvalidInput);
        }
        let entries = content
            .lines()
            .filter(|line| {
                line.starts_with("#HttpOnly_") || (!line.starts_with('#') && !line.is_empty())
            })
            .count() as u64;
        let mut conn = self
            .store
            .database()
            .connection()
            .await
            .map_err(|_| DownloadError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        let authorized = query(include_str!("personal_cookies_auth.sql"))
            .bind(actor as i64)
            .bind(chrono::Utc::now())
            .bind(self.identities.is_configured_owner(actor as i64))
            .fetch_optional(&*tx)
            .await
            .map_err(|_| DownloadError::Database)?;
        if authorized.is_none() {
            tx.rollback().await.map_err(|_| DownloadError::Database)?;
            return Err(DownloadError::Denied);
        }
        let advanced = query(include_str!("personal_cookies_watermark.sql"))
            .bind(actor as i64)
            .bind(message_id)
            .fetch_optional(&*tx)
            .await
            .map_err(|_| DownloadError::Database)?;
        if advanced.is_none() {
            tx.commit().await.map_err(|_| DownloadError::Database)?;
            return Ok(None);
        }
        let usage = query(include_str!("personal_cookies_usage.sql"))
            .bind(actor as i64)
            .bind(domain.as_str())
            .fetch_one(&*tx)
            .await
            .map_err(|_| DownloadError::Database)?;
        let value = |key: &str| {
            usage
                .try_get::<i64, _>(key)
                .map_err(|_| DownloadError::Database)
        };
        let domains = value("actor_domains")?;
        let removed = value("replaced_domains")?;
        let bytes = value("actor_bytes")?;
        let global = value("global_bytes")?;
        let replaced = value("replaced_bytes")?;
        let limits = self.network_config.personal_cookie_limits;
        let growth_exceeds = |old: i64, new: i64, limit: u64| new > old && new as u64 > limit;
        if growth_exceeds(domains, domains - removed + 1, limits.domains)
            || growth_exceeds(
                bytes,
                bytes - replaced + content.len() as i64,
                limits.actor_bytes,
            )
            || growth_exceeds(
                global,
                global - replaced + content.len() as i64,
                limits.global_bytes,
            )
        {
            // Keep the watermark even on rejection: a replay cannot later install credentials.
            tx.commit().await.map_err(|_| DownloadError::Database)?;
            return Err(DownloadError::PersonalCookieCapacity);
        }
        query(include_str!("personal_cookies_upload_delete.sql"))
            .bind(actor as i64)
            .bind(domain.as_str())
            .execute(&*tx)
            .await
            .map_err(|_| DownloadError::Database)?;
        query(include_str!("personal_cookies_upload_insert.sql"))
            .bind(actor as i64)
            .bind(domain.as_str())
            .bind(content)
            .bind(chrono::Utc::now())
            .execute(&*tx)
            .await
            .map_err(|_| DownloadError::Database)?;
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        Ok(Some(entries))
    }

    pub async fn personal_cookies(
        &self,
        actor: u64,
        target: DeliveryTarget,
        after: Option<i64>,
    ) -> Result<PersonalCookiePage, DownloadError> {
        self.authorize_cookie_control(actor, target).await?;
        let after = after.unwrap_or(i64::MIN);
        let rows = query(include_str!("personal_cookies_list.sql"))
            .bind(actor as i64)
            .bind(after)
            .fetch_all(self.store.database())
            .await
            .map_err(|_| DownloadError::Database)?;
        let has_more = rows.len() > 10;
        let mut entries = Vec::with_capacity(rows.len().min(10));
        let mut last_displayed = None;
        for (index, row) in rows.into_iter().enumerate() {
            let id: i64 = row.try_get("id").map_err(|_| DownloadError::Database)?;
            if index == 10 {
                break;
            }
            let raw: Option<String> = row.try_get("domain").map_err(|_| DownloadError::Database)?;
            entries.push(PersonalCookieEntry {
                domain: raw
                    .as_deref()
                    .and_then(|value| CookieDomain::parse(value).ok()),
            });
            last_displayed = Some(id);
        }
        Ok(PersonalCookiePage {
            entries,
            next_after: if has_more { last_displayed } else { None },
        })
    }

    pub async fn personal_cookie_status(
        &self,
        actor: u64,
        target: DeliveryTarget,
        domain: &CookieDomain,
    ) -> Result<PersonalCookieStatus, DownloadError> {
        self.authorize_cookie_control(actor, target).await?;
        let rows = query(include_str!("personal_cookies_status.sql"))
            .bind(actor as i64)
            .bind(domain.as_str())
            .bind(yoink_worker_protocol::MAX_COOKIE_JAR_BYTES as i64)
            .fetch_all(self.store.database())
            .await
            .map_err(|_| DownloadError::Database)?;
        if rows.is_empty() {
            return Ok(PersonalCookieStatus::Missing);
        }
        if rows.len() > 1 {
            return Ok(PersonalCookieStatus::Invalid);
        }
        let row = &rows[0];
        let too_large: bool = row
            .try_get("too_large")
            .map_err(|_| DownloadError::Database)?;
        let content: Option<String> = row
            .try_get("content")
            .map_err(|_| DownloadError::Database)?;
        let Some(content) = content.filter(|_| !too_large) else {
            return Ok(PersonalCookieStatus::Invalid);
        };
        if !valid_cookie(&content) {
            return Ok(PersonalCookieStatus::Invalid);
        }
        let count = content
            .lines()
            .filter(|line| {
                line.starts_with("#HttpOnly_") || (!line.starts_with('#') && !line.is_empty())
            })
            .count() as u64;
        if count == 0 {
            Ok(PersonalCookieStatus::Invalid)
        } else {
            Ok(PersonalCookieStatus::Stored { entries: count })
        }
    }

    pub async fn remove_personal_cookies(
        &self,
        actor: u64,
        target: DeliveryTarget,
        message_id: i32,
        domain: Option<&CookieDomain>,
    ) -> Result<Option<u64>, DownloadError> {
        self.authorize_cookie_control(actor, target).await?;
        if message_id <= 0 {
            return Err(DownloadError::InvalidInput);
        }
        let mut conn = self
            .store
            .database()
            .connection()
            .await
            .map_err(|_| DownloadError::Database)?;
        let tx = Transaction::new(&mut conn, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        let authorized = query(include_str!("personal_cookies_auth.sql"))
            .bind(actor as i64)
            .bind(chrono::Utc::now())
            .bind(self.identities.is_configured_owner(actor as i64))
            .fetch_optional(&*tx)
            .await
            .map_err(|_| DownloadError::Database)?;
        if authorized.is_none() {
            tx.rollback().await.map_err(|_| DownloadError::Database)?;
            return Err(DownloadError::Denied);
        }
        let advanced = query(include_str!("personal_cookies_watermark.sql"))
            .bind(actor as i64)
            .bind(message_id)
            .fetch_optional(&*tx)
            .await
            .map_err(|_| DownloadError::Database)?;
        if advanced.is_none() {
            tx.commit().await.map_err(|_| DownloadError::Database)?;
            return Ok(None);
        }
        let mut delete = query(include_str!("personal_cookies_delete.sql")).bind(actor as i64);
        delete = delete.bind(domain.map(CookieDomain::as_str));
        let deleted = delete
            .execute(&*tx)
            .await
            .map_err(|_| DownloadError::Database)?;
        let count = deleted.rows_affected();
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        Ok(Some(count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_is_bare_canonical_host_and_debug_is_redacted() {
        let domain = CookieDomain::parse("..ExAmPlE.com.").unwrap();
        assert_eq!(domain.as_str(), "example.com");
        assert!(!format!("{domain:?}").contains("example.com"));
        for bad in [
            "https://example.com",
            "user@example.com",
            "example.com/path",
            "a..b",
            "-a.example",
            "a.example:443",
            "a b.example",
        ] {
            assert!(CookieDomain::parse(bad).is_err(), "accepted {bad}");
        }
    }
}
