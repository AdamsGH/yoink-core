use super::{AdminError, admin_auth, finish_admin};
use crate::IdentityStore;
use std::collections::BTreeMap;
use turso::transaction::TransactionBehavior;
use yoink_db::Transaction;
use yoink_db::query;
use yoink_domain::downloader_settings::{DownloaderSettings, DownloaderSettingsPatch};

async fn read(tx: &Transaction<'_>) -> Result<BTreeMap<String, String>, AdminError> {
    query(include_str!("../admin_settings.sql"))
        .fetch_all(&**tx)
        .await
        .map_err(|_| AdminError::Unavailable)?
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get("key").map_err(|_| AdminError::Unavailable)?,
                row.try_get("value").map_err(|_| AdminError::Unavailable)?,
            ))
        })
        .collect()
}

impl IdentityStore {
    pub async fn downloader_admin_settings(
        &self,
        actor: i64,
    ) -> Result<DownloaderSettings, AdminError> {
        let mut connection = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Deferred)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            DownloaderSettings::from_settings(&read(&tx).await?)
                .map_err(|_| AdminError::Unavailable)
        }
        .await;
        finish_admin(tx, result).await
    }

    pub async fn patch_downloader_admin_settings(
        &self,
        actor: i64,
        patch: &DownloaderSettingsPatch,
    ) -> Result<DownloaderSettings, AdminError> {
        let mut connection = self
            .database
            .connection()
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| AdminError::Unavailable)?;
        let result = async {
            admin_auth(&tx, actor, self.owner_id, true, false).await?;
            let changes = patch.values();
            let mut values = read(&tx).await?;
            values.extend(changes.clone());
            let settings =
                DownloaderSettings::from_settings(&values).map_err(|_| AdminError::InvalidInput)?;
            for (key, value) in changes {
                query(include_str!("../admin_setting_upsert.sql"))
                    .bind(key)
                    .bind(value)
                    .execute(&*tx)
                    .await
                    .map_err(|_| AdminError::Unavailable)?;
            }
            Ok(settings)
        }
        .await;
        finish_admin(tx, result).await
    }
}
