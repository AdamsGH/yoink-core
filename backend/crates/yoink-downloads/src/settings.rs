//! Editable, non-secret preferences. Network credentials and raw engine args are excluded.
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_db::query;
use yoink_domain::{DeliveryTarget, SubtitleOptions};

use crate::{DownloadError, DownloadService, map_profile};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingField {
    Quality,
    Codec,
    Container,
    AudioCodec,
    SendAsFile,
    GalleryZip,
    SubtitleLanguage,
    SubtitleAutomatic,
    NsfwBlur,
}

impl SettingField {
    pub const ALL: [Self; 9] = [
        Self::Quality,
        Self::Codec,
        Self::Container,
        Self::AudioCodec,
        Self::SendAsFile,
        Self::GalleryZip,
        Self::SubtitleLanguage,
        Self::SubtitleAutomatic,
        Self::NsfwBlur,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Quality => "quality",
            Self::Codec => "codec",
            Self::Container => "container",
            Self::AudioCodec => "audio_codec",
            Self::SendAsFile => "send_as_file",
            Self::GalleryZip => "gallery_zip",
            Self::SubtitleLanguage => "subs_lang",
            Self::SubtitleAutomatic => "subs_auto",
            Self::NsfwBlur => "nsfw_blur",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.name() == name)
    }
    pub fn choices(self) -> &'static [&'static str] {
        match self {
            Self::Quality => &[
                "best", "2160", "1440", "1080", "720", "480", "360", "240", "144", "ask",
            ],
            Self::Codec => &["avc1", "av01", "vp09"],
            Self::Container => &["mp4", "mkv"],
            Self::AudioCodec => &["best", "mp4a", "opus", "mp3"],
            Self::SendAsFile | Self::GalleryZip | Self::SubtitleAutomatic | Self::NsfwBlur => {
                &["false", "true"]
            }
            Self::SubtitleLanguage => &["en", "ru", "de", "fr", "es", "zh-Hans"],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingChange {
    field: SettingField,
    value: String,
}
impl SettingChange {
    pub fn new(field: SettingField, value: &str) -> Result<Self, DownloadError> {
        let valid = if field == SettingField::SubtitleLanguage {
            // The imported application's column is limited to eight characters.
            value.len() <= 8 && SubtitleOptions::new(value, false).is_ok()
        } else {
            field.choices().contains(&value)
        };
        if !valid {
            return Err(DownloadError::InvalidPreferences);
        }
        Ok(Self {
            field,
            value: value.to_owned(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditableSettings {
    pub values: Vec<(SettingField, String)>,
    pub revision: i64,
}

impl DownloadService {
    async fn authorize_settings(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<(), DownloadError> {
        if target.chat_id() <= 0 || target.chat_id() as u64 != actor || target.thread_id().is_some()
        {
            return Err(DownloadError::Denied);
        }
        self.authorize_chat(actor, target).await
    }

    pub async fn settings(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<EditableSettings, DownloadError> {
        self.authorize_settings(actor, target).await?;
        let row = query(include_str!("settings_read.sql"))
            .bind(actor as i64)
            .fetch_optional(self.store.database())
            .await
            .map_err(|_| DownloadError::Database)?
            .ok_or(DownloadError::Denied)?;
        let mut fields = Vec::new();
        for (index, field) in SettingField::ALL.into_iter().enumerate() {
            let value: Option<String> = row.try_get(index).map_err(|_| DownloadError::Database)?;
            fields.push((field, value.unwrap_or_else(|| "?".into())));
        }
        Ok(EditableSettings {
            values: fields,
            revision: row
                .try_get(SettingField::ALL.len())
                .map_err(|_| DownloadError::Database)?,
        })
    }

    pub async fn change_setting(
        &self,
        actor: u64,
        target: DeliveryTarget,
        change: SettingChange,
        expected_revision: Option<i64>,
    ) -> Result<(), DownloadError> {
        self.authorize_settings(actor, target).await?;
        let updated = query(include_str!("settings_write.sql"))
            .bind(actor as i64)
            .bind(change.field.name())
            .bind(change.value)
            .bind(chrono::Utc::now())
            .bind(expected_revision)
            .bind(self.identities.is_configured_owner(actor as i64))
            .fetch_optional(self.store.database())
            .await
            .map_err(crate::profile::ProfileError::from)
            .map_err(map_profile)?;
        if updated.is_none() {
            self.authorize_settings(actor, target).await?;
            return Err(DownloadError::Conflict);
        }
        Ok(())
    }
}

/// Secret-free view of the owner's proxy policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProxySettings {
    pub enabled: bool,
    pub configured: bool,
    pub revision: i64,
}

impl DownloadService {
    async fn authorize_proxy_settings(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<(), DownloadError> {
        if !self
            .identities
            .is_configured_owner(i64::try_from(actor).map_err(|_| DownloadError::Denied)?)
            || target.chat_id() <= 0
            || target.chat_id() as u64 != actor
            || target.thread_id().is_some()
        {
            return Err(DownloadError::Denied);
        }
        self.authorize_chat(actor, target).await
    }

    pub async fn proxy_settings(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<ProxySettings, DownloadError> {
        self.authorize_proxy_settings(actor, target).await?;
        let row = query(include_str!("proxy_settings_read.sql"))
            .bind(actor as i64)
            .bind(chrono::Utc::now())
            .bind(self.identities.is_configured_owner(actor as i64))
            .bind(yoink_worker_protocol::MAX_PROXY_URL_BYTES as i64)
            .fetch_optional(self.store.database())
            .await
            .map_err(|_| DownloadError::Database)?;
        let (enabled, personal, revision, too_large): (bool, Option<String>, i64, bool) = match row
        {
            Some(row) => (
                row.try_get("proxy_enabled")
                    .map_err(|_| DownloadError::Database)?,
                row.try_get("proxy_url")
                    .map_err(|_| DownloadError::Database)?,
                row.try_get("revision")
                    .map_err(|_| DownloadError::Database)?,
                row.try_get("proxy_url_too_large")
                    .map_err(|_| DownloadError::Database)?,
            ),
            None => return Err(DownloadError::Denied),
        };
        let configured = !too_large
            && crate::profile::choose_proxy(
                actor,
                personal.as_deref(),
                self.network_config.system_proxy.as_deref(),
            )
            .and_then(|(_, value)| crate::profile::validate_proxy(&value))
            .is_ok();
        Ok(ProxySettings {
            enabled,
            configured,
            revision,
        })
    }

    pub async fn change_proxy_setting(
        &self,
        actor: u64,
        target: DeliveryTarget,
        enabled: bool,
        expected_revision: i64,
    ) -> Result<(), DownloadError> {
        self.authorize_proxy_settings(actor, target).await?;
        if expected_revision < 0 {
            return Err(DownloadError::Conflict);
        }
        let mut connection = self
            .store
            .database()
            .connection()
            .await
            .map_err(|_| DownloadError::Database)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| DownloadError::Database)?;
        let fresh = query(include_str!("proxy_settings_read.sql"))
            .bind(actor as i64)
            .bind(chrono::Utc::now())
            .bind(self.identities.is_configured_owner(actor as i64))
            .bind(yoink_worker_protocol::MAX_PROXY_URL_BYTES as i64)
            .fetch_optional(&*tx)
            .await
            .map_err(|_| DownloadError::Database)?;
        let Some(fresh) = fresh else {
            tx.rollback().await.map_err(|_| DownloadError::Database)?;
            return Err(DownloadError::Denied);
        };
        let revision: i64 = fresh
            .try_get("revision")
            .map_err(|_| DownloadError::Database)?;
        if revision != expected_revision {
            tx.rollback().await.map_err(|_| DownloadError::Database)?;
            // Authorization was checked in this transaction. Do not acquire another
            // database connection while this connection still owns its permit.
            return Err(DownloadError::Conflict);
        }
        if enabled {
            let personal: Option<String> = fresh
                .try_get("proxy_url")
                .map_err(|_| DownloadError::Database)?;
            let too_large: bool = fresh
                .try_get("proxy_url_too_large")
                .map_err(|_| DownloadError::Database)?;
            let configured = !too_large
                && crate::profile::choose_proxy(
                    actor,
                    personal.as_deref(),
                    self.network_config.system_proxy.as_deref(),
                )
                .and_then(|(_, value)| crate::profile::validate_proxy(&value))
                .is_ok();
            if !configured {
                tx.rollback().await.map_err(|_| DownloadError::Database)?;
                return Err(DownloadError::ProxyUnavailable);
            }
        }
        let result = query(include_str!("proxy_settings_write.sql"))
            .bind(actor as i64)
            .bind(enabled)
            .bind(expected_revision)
            .bind(chrono::Utc::now())
            .bind(self.identities.is_configured_owner(actor as i64))
            .fetch_optional(&*tx)
            .await
            .map_err(crate::profile::ProfileError::from)
            .map_err(map_profile)?;
        if result.is_none() {
            tx.rollback().await.map_err(|_| DownloadError::Database)?;
            return Err(DownloadError::Conflict);
        }
        tx.commit().await.map_err(|_| DownloadError::Database)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_fields_and_values() {
        for field in SettingField::ALL {
            assert_eq!(SettingField::parse(field.name()), Some(field));
            for value in field.choices() {
                assert!(SettingChange::new(field, value).is_ok());
            }
            assert!(SettingChange::new(field, "secret\n'invalid").is_err());
        }
        for name in ["proxy_url", "args_json", "user_id", "subs_enabled", "role"] {
            assert!(SettingField::parse(name).is_none());
        }
        assert!(SettingChange::new(SettingField::Quality, "ask").is_ok());
        assert!(SettingChange::new(SettingField::Quality, "bogus").is_err());
        assert!(SettingChange::new(SettingField::SubtitleLanguage, "en-US").is_ok());
        assert!(SettingChange::new(SettingField::SubtitleLanguage, "eng-abcdefgh").is_err());
    }
}
