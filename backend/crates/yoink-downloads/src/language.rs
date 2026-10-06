use yoink_domain::DeliveryTarget;
use yoink_identity::IdentityError;

use crate::{DownloadError, DownloadService, LanguagePreference};

fn map_identity(error: IdentityError) -> DownloadError {
    match error {
        IdentityError::LanguageDenied | IdentityError::InvalidId => DownloadError::Denied,
        IdentityError::InvalidLanguage => DownloadError::InvalidPreferences,
        IdentityError::LanguageConflict | IdentityError::LanguageRevisionOverflow => {
            DownloadError::Conflict
        }
        _ => DownloadError::Database,
    }
}

impl DownloadService {
    /// Returns a user's persisted UI language, leaving fallback policy to callers.
    pub async fn preferred_language(&self, actor: u64) -> Result<Option<String>, DownloadError> {
        let actor = i64::try_from(actor).map_err(|_| DownloadError::Denied)?;
        if actor <= 0 {
            return Err(DownloadError::Denied);
        }
        Ok(self
            .identities
            .language_preference(actor)
            .await
            .map_err(map_identity)?
            .map(|preference| preference.code))
    }

    pub async fn language_settings(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<LanguagePreference, DownloadError> {
        self.authorize_language_target(actor, target).await?;
        self.identities
            .language_preference(actor as i64)
            .await
            .map_err(map_identity)?
            .ok_or(DownloadError::Denied)
    }

    pub async fn change_language(
        &self,
        actor: u64,
        target: DeliveryTarget,
        code: &str,
        expected_revision: i64,
    ) -> Result<LanguagePreference, DownloadError> {
        self.authorize_language_target(actor, target).await?;
        self.identities
            .change_language(actor as i64, code, expected_revision)
            .await
            .map_err(map_identity)
    }

    async fn authorize_language_target(
        &self,
        actor: u64,
        target: DeliveryTarget,
    ) -> Result<(), DownloadError> {
        if actor == 0
            || actor > i64::MAX as u64
            || target.bot_id() != self.delivery.bot_id()
            || target.chat_id() <= 0
            || target.chat_id() != actor as i64
            || target.thread_id().is_some()
        {
            return Err(DownloadError::Denied);
        }
        self.authorize_chat(actor, target).await
    }
}
