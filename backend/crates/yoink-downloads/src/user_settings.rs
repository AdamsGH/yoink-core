//! Caller-owned downloader API preferences. No raw engine arguments are accepted.
use crate::{DownloadError, DownloadService, SettingChange, SettingField};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use turso::transaction::TransactionBehavior;
use yoink_access::{AccessPolicy, AccessScope, Role};
use yoink_db::Database;
use yoink_db::Transaction;
use yoink_db::query;
use yoink_identity::{IdentitySnapshot, IdentityStore};

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UserSettingsPatch {
    pub quality: Option<String>,
    pub codec: Option<String>,
    pub container: Option<String>,
    pub proxy_enabled: Option<bool>,
    pub proxy_url: Option<String>,
    pub keyboard: Option<String>,
    pub subs_enabled: Option<bool>,
    pub subs_auto: Option<bool>,
    pub subs_always_ask: Option<bool>,
    pub subs_lang: Option<String>,
    pub split_size: Option<i64>,
    pub nsfw_blur: Option<bool>,
    pub mediainfo: Option<bool>,
    pub send_as_file: Option<bool>,
    pub gallery_zip: Option<bool>,
    pub use_pool_cookies: Option<bool>,
    pub youtube_auth_mode: Option<String>,
}
impl UserSettingsPatch {
    pub fn validate(&self) -> Result<(), DownloadError> {
        for (field, value) in [
            (SettingField::Quality, &self.quality),
            (SettingField::Codec, &self.codec),
            (SettingField::Container, &self.container),
            (SettingField::Keyboard, &self.keyboard),
            (SettingField::SubtitleLanguage, &self.subs_lang),
        ] {
            if let Some(value) = value {
                SettingChange::new(field, value)?;
            }
        }
        if let Some(size) = self.split_size {
            SettingChange::new(SettingField::SplitSize, &size.to_string())?;
        }
        if let Some(url) = &self.proxy_url {
            crate::profile::validate_proxy(url).map_err(|_| DownloadError::InvalidPreferences)?;
        }
        if self
            .youtube_auth_mode
            .as_deref()
            .is_some_and(|v| !["cookies", "oauth", "oauth2"].contains(&v))
        {
            return Err(DownloadError::InvalidPreferences);
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct UserSettings {
    pub user_id: i64,
    pub quality: String,
    pub codec: String,
    pub container: String,
    pub proxy_enabled: bool,
    pub proxy_url: Option<String>,
    pub keyboard: String,
    pub subs_enabled: bool,
    pub subs_auto: bool,
    pub subs_always_ask: bool,
    pub subs_lang: String,
    pub split_size: i64,
    pub nsfw_blur: bool,
    pub mediainfo: bool,
    pub send_as_file: bool,
    pub gallery_zip: bool,
    pub use_pool_cookies: bool,
    pub youtube_auth_mode: String,
    pub audio_codec: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub has_pool_access: bool,
    #[serde(skip)]
    pub revision: i64,
}
/// Database-only module service: the frontend does not need a downloader process or bot.
#[derive(Clone)]
pub struct UserSettingsService {
    database: Database,
    identities: Arc<IdentityStore>,
    profile: crate::profile::ProfileStore,
}
impl UserSettingsService {
    pub fn new(database: Database, identities: Arc<IdentityStore>) -> Self {
        Self {
            profile: crate::profile::ProfileStore::new(database.clone()),
            database,
            identities,
        }
    }
    async fn authorize(&self, actor: u64) -> Result<IdentitySnapshot, DownloadError> {
        if actor == 0 || actor > i64::MAX as u64 {
            return Err(DownloadError::Denied);
        }
        let identity = self
            .identities
            .snapshot(actor as i64, None)
            .await
            .map_err(|_| DownloadError::Database)?
            .ok_or(DownloadError::Denied)?;
        let policy = AccessPolicy {
            min_role: Role::Restricted,
            scopes: vec![AccessScope::Private],
            check_group_enabled: false,
            check_thread_policy: false,
        };
        if !identity.authorize(&policy, None).decision.allowed {
            return Err(DownloadError::Denied);
        }
        Ok(identity)
    }
    pub async fn user_settings(&self, actor: u64) -> Result<UserSettings, DownloadError> {
        let identity = self.authorize(actor).await?;
        let p = self
            .profile
            .preferences(actor)
            .await
            .map_err(crate::map_profile)?
            .ok_or(DownloadError::Denied)?;
        let has_pool_access = crate::profile::pool_allowed(&identity, true);
        let updated_at = p.updated_at.unwrap_or(chrono::DateTime::UNIX_EPOCH);
        let revision = updated_at.timestamp_micros();
        Ok(UserSettings {
            user_id: actor as i64,
            quality: p.quality,
            codec: p.codec,
            container: p.container,
            proxy_enabled: p.proxy_enabled,
            proxy_url: p.proxy_url,
            keyboard: p.keyboard,
            subs_enabled: p.subs_enabled,
            subs_auto: p.subs_auto,
            subs_always_ask: p.subs_always_ask,
            subs_lang: p.subs_lang,
            split_size: p.split_size,
            nsfw_blur: p.nsfw_blur,
            mediainfo: p.mediainfo,
            send_as_file: p.send_as_file,
            gallery_zip: p.gallery_zip,
            use_pool_cookies: p.use_pool_cookies,
            youtube_auth_mode: p.youtube_auth_mode,
            audio_codec: p.audio_codec,
            updated_at,
            has_pool_access,
            revision,
        })
    }
    pub async fn patch_user_settings(
        &self,
        actor: u64,
        patch: UserSettingsPatch,
        expected_revision: Option<i64>,
    ) -> Result<UserSettings, DownloadError> {
        self.authorize(actor).await?;
        patch.validate()?;
        if expected_revision.is_some_and(|v| v < 0) {
            return Err(DownloadError::Conflict);
        }
        let encoded =
            serde_json::to_string(&patch).map_err(|_| DownloadError::InvalidPreferences)?;
        let mut connection = self
            .database
            .connection()
            .await
            .map_err(|_| DownloadError::Database)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        let row = query(include_str!("user_settings_patch.sql"))
            .bind(actor as i64)
            .bind(encoded)
            .bind(expected_revision)
            .bind(chrono::Utc::now())
            .fetch_optional(&*tx)
            .await
            .map_err(|_| DownloadError::Database)?;
        if row.is_none() {
            return Err(DownloadError::Conflict);
        }
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        drop(connection);
        self.user_settings(actor).await
    }
}

impl DownloadService {
    pub fn user_settings_service(&self) -> UserSettingsService {
        UserSettingsService::new(self.store.database().clone(), self.identities.clone())
    }
    pub async fn user_settings(&self, actor: u64) -> Result<UserSettings, DownloadError> {
        self.user_settings_service().user_settings(actor).await
    }
    pub async fn patch_user_settings(
        &self,
        actor: u64,
        patch: UserSettingsPatch,
        revision: Option<i64>,
    ) -> Result<UserSettings, DownloadError> {
        self.user_settings_service()
            .patch_user_settings(actor, patch, revision)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patch_contract_is_closed_and_null_proxy_is_ignored() {
        let patch: UserSettingsPatch =
            serde_json::from_str(r#"{"proxy_url":null,"split_size":524288000,"keyboard":"OFF"}"#)
                .unwrap();
        assert!(patch.validate().is_ok());
        assert!(patch.proxy_url.is_none());
        for bad in [
            r#"{"args_json":{}}"#,
            r#"{"user_id":42}"#,
            r#"{"has_pool_access":true}"#,
        ] {
            assert!(serde_json::from_str::<UserSettingsPatch>(bad).is_err());
        }
        for bad in [
            r#"{"split_size":1}"#,
            r#"{"proxy_url":"file:///etc/passwd"}"#,
            r#"{"keyboard":"shell"}"#,
        ] {
            assert!(
                serde_json::from_str::<UserSettingsPatch>(bad)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
    }
}
