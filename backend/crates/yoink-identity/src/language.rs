use turso::transaction::{Transaction, TransactionBehavior};
use yoink_db::query;

use crate::{IdentityError, IdentityStore};

/// The language value and its optimistic-concurrency revision (users.updated_at).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguagePreference {
    pub code: String,
    pub revision: i64,
}

impl IdentityStore {
    /// Reads the persisted language for any existing identity. Unknown stored codes
    /// are returned unchanged so callers can choose their own fallback.
    pub async fn language_preference(
        &self,
        user_id: i64,
    ) -> Result<Option<LanguagePreference>, IdentityError> {
        if user_id <= 0 || self.owner_id.is_some_and(|id| id <= 0) {
            return Err(IdentityError::InvalidId);
        }
        let row = query(include_str!("language_read.sql"))
            .bind(user_id)
            .fetch_optional(&self.database)
            .await?;
        row.map(|row| {
            Ok(LanguagePreference {
                code: row.try_get(0).map_err(|_| IdentityError::Database)?,
                revision: row.try_get(1).map_err(|_| IdentityError::Database)?,
            })
        })
        .transpose()
    }

    /// Changes the configured owner's language only if the callback revision is
    /// current. updated_at itself is advanced, including when code is unchanged.
    pub async fn change_language(
        &self,
        user_id: i64,
        code: &str,
        expected_revision: i64,
    ) -> Result<LanguagePreference, IdentityError> {
        if user_id <= 0 || self.owner_id.is_some_and(|id| id <= 0) {
            return Err(IdentityError::InvalidId);
        }
        if !matches!(code, "en" | "ru") {
            return Err(IdentityError::InvalidLanguage);
        }
        if expected_revision < 0 {
            return Err(IdentityError::LanguageConflict);
        }
        if self.owner_id != Some(user_id) {
            return Err(IdentityError::LanguageDenied);
        }

        let mut connection = self.database.connection().await?;
        let transaction = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| IdentityError::Database)?;
        let result = self
            .change_language_in_transaction(&transaction, user_id, code, expected_revision)
            .await;
        match result {
            Ok(preference) => {
                transaction
                    .commit()
                    .await
                    .map_err(|_| IdentityError::Database)?;
                Ok(preference)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    async fn change_language_in_transaction(
        &self,
        transaction: &Transaction<'_>,
        user_id: i64,
        code: &str,
        expected_revision: i64,
    ) -> Result<LanguagePreference, IdentityError> {
        let row = query(include_str!("language_change.sql"))
            .bind(code)
            .bind(user_id)
            .bind(expected_revision)
            .bind(self.owner_id == Some(user_id))
            .fetch_optional(&**transaction)
            .await?;
        let Some(row) = row else {
            let allowed = query(include_str!("language_authorized.sql"))
                .bind(user_id)
                .bind(self.owner_id == Some(user_id))
                .fetch_optional(&**transaction)
                .await?;
            if allowed.is_none() {
                return Err(IdentityError::LanguageDenied);
            }
            let revision = query(include_str!("language_revision.sql"))
                .bind(user_id)
                .fetch_optional(&**transaction)
                .await?
                .and_then(|row| row.try_get::<i64, _>(0).ok());
            return Err(if revision == Some(i64::MAX) {
                IdentityError::LanguageRevisionOverflow
            } else {
                IdentityError::LanguageConflict
            });
        };
        Ok(LanguagePreference {
            code: row.try_get(0).map_err(|_| IdentityError::Database)?,
            revision: row.try_get(1).map_err(|_| IdentityError::Database)?,
        })
    }
}
