//! Pre-admission refresh only. Provider HTTP belongs to credential-management worker.
use super::{NetworkContext, ProfileError, ProfileStore, cookie_version, normalize_domain, oauth};
use chrono::{DateTime, Utc};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use turso::transaction::TransactionBehavior;
use yoink_db::Transaction;
use yoink_db::query;
use yoink_worker_protocol::youtube_auth::{MAX_OAUTH_CONTENT_BYTES, YoutubeOAuth};

/// Exchange only: implementations must not log secrets or write credential rows.
/// The execution owner bounds this future and owns authorization/version CAS below.
pub trait OAuthRefreshProvider: Send + Sync {
    fn exchange<'a>(
        &'a self,
        tokens: &'a YoutubeOAuth,
    ) -> Pin<Box<dyn Future<Output = Result<YoutubeOAuth, ProfileError>> + Send + 'a>>;
}

impl ProfileStore {
    pub fn with_oauth_refresh(mut self, provider: Arc<dyn OAuthRefreshProvider>) -> Self {
        self.oauth_refresh = Some(provider);
        self
    }

    pub fn with_oauth_refresh_timeout(mut self, timeout: Duration) -> Result<Self, ProfileError> {
        if timeout.is_zero() || timeout > Duration::from_secs(60) {
            return Err(ProfileError::InvalidPreferences);
        }
        self.oauth_refresh_timeout = timeout;
        Ok(self)
    }

    pub(super) async fn refresh_before_admission(
        &self,
        context: &NetworkContext<'_>,
    ) -> Result<(), ProfileError> {
        if normalize_domain(context.domain)? != "youtube.com" {
            return Ok(());
        }
        let actor = super::valid_actor(context.actor_id)?;
        super::validate_identity(context.identity, actor)?;
        // Snapshot without holding a transaction across remote work.
        let connection = self.database.connection().await?;
        let settings = query(include_str!("network_settings.sql"))
            .bind(actor)
            .fetch_optional(&*connection)
            .await?;
        let mode: String = match settings {
            Some(row) => row
                .try_get("youtube_auth_mode")
                .map_err(|_| ProfileError::Database)?,
            None => return Ok(()),
        };
        if !super::is_oauth_mode(&mode)
            || super::matches_no_cookie_domain("youtube.com", context.no_cookie_domains)?
        {
            return Ok(());
        }
        let candidates = self
            .cookie_candidates(&connection, actor, "youtube.com", false)
            .await?;
        for candidate in candidates {
            let row = query(include_str!("cookie_by_id.sql"))
                .bind(candidate.id)
                .bind(MAX_OAUTH_CONTENT_BYTES as i64)
                .fetch_optional(&*connection)
                .await?
                .ok_or(ProfileError::StaleReference)?;
            let owner: i64 = row.try_get("user_id").map_err(|_| ProfileError::Database)?;
            let pool: bool = row.try_get("is_pool").map_err(|_| ProfileError::Database)?;
            let valid: bool = row
                .try_get("is_valid")
                .map_err(|_| ProfileError::Database)?;
            let domain: String = row.try_get("domain").map_err(|_| ProfileError::Database)?;
            if owner != actor || pool || !valid || normalize_domain(&domain)? != "youtube.com" {
                return Err(ProfileError::StaleReference);
            }
            let content: String = row
                .try_get("content")
                .map_err(|_| ProfileError::InvalidCookie)?;
            if !super::is_oauth(&content) {
                continue;
            }
            let tokens = oauth::decode(&content)?;
            if oauth::require_fresh(&tokens, Utc::now()).is_ok() {
                return Ok(());
            }
            let updated: DateTime<Utc> = row
                .try_get("updated_at")
                .map_err(|_| ProfileError::Database)?;
            let expected = cookie_version(&content, candidate.id, actor, &domain, false, updated);
            let user = query("SELECT role FROM users WHERE id=?1 AND role<>'banned' AND (ban_until IS NULL OR ban_until<=CAST(unixepoch('subsec')*1000000 AS INTEGER)) AND EXISTS(SELECT 1 FROM dl_user_settings WHERE user_id=?1 AND youtube_auth_mode IN ('oauth','oauth2'))").bind(actor).fetch_optional(&*connection).await?.ok_or(ProfileError::SharedAccessDenied)?;
            let role: String = user.try_get("role").map_err(|_| ProfileError::Database)?;
            let provider = self
                .oauth_refresh
                .as_ref()
                .ok_or(ProfileError::UnsupportedAuthentication)?;
            drop(connection);
            let refreshed =
                tokio::time::timeout(self.oauth_refresh_timeout, provider.exchange(&tokens))
                    .await
                    .map_err(|_| ProfileError::Unavailable)??;
            oauth::require_fresh(&refreshed, Utc::now())?;
            let replacement = refreshed
                .encode_content()
                .map_err(|_| ProfileError::InvalidCookie)?;
            oauth::decode(&replacement)?;
            let mut connection = self.database.connection().await?;
            let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate).await?;
            let current = query(include_str!("cookie_by_id.sql"))
                .bind(candidate.id)
                .bind(MAX_OAUTH_CONTENT_BYTES as i64)
                .fetch_optional(&*tx)
                .await?
                .ok_or(ProfileError::StaleReference)?;
            let current_content: String = current
                .try_get("content")
                .map_err(|_| ProfileError::StaleReference)?;
            let owner: i64 = current
                .try_get("user_id")
                .map_err(|_| ProfileError::Database)?;
            let pool: bool = current
                .try_get("is_pool")
                .map_err(|_| ProfileError::Database)?;
            let valid: bool = current
                .try_get("is_valid")
                .map_err(|_| ProfileError::Database)?;
            let current_domain: String = current
                .try_get("domain")
                .map_err(|_| ProfileError::Database)?;
            let current_updated = current
                .try_get("updated_at")
                .map_err(|_| ProfileError::Database)?;
            if !valid
                || owner != actor
                || pool
                || cookie_version(
                    &current_content,
                    candidate.id,
                    owner,
                    &current_domain,
                    pool,
                    current_updated,
                ) != expected
            {
                return Err(ProfileError::StaleReference);
            }
            // Atomic user ban, auth-mode and immutable row-version CAS after HTTP.
            let result = query("UPDATE cookies SET content=?1, updated_at=max(updated_at+1,CAST(unixepoch('subsec')*1000000 AS INTEGER)) WHERE id=?2 AND updated_at=?3 AND updated_at>=0 AND updated_at<9223372036854775807 AND user_id=?4 AND is_valid=1 AND is_pool=0 AND EXISTS(SELECT 1 FROM users WHERE id=?4 AND role=?5 AND role<>'banned' AND (ban_until IS NULL OR ban_until<=CAST(unixepoch('subsec')*1000000 AS INTEGER))) AND EXISTS(SELECT 1 FROM dl_user_settings WHERE user_id=?4 AND youtube_auth_mode IN ('oauth','oauth2')) RETURNING id")
                .bind(replacement).bind(candidate.id).bind(updated).bind(actor).bind(role).fetch_optional(&*tx).await?;
            if result.is_none() {
                return Err(ProfileError::StaleReference);
            }
            tx.commit().await?;
            return Ok(());
        }
        Ok(())
    }
}
