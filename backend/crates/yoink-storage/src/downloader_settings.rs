use crate::{AdmissionPolicy, QuotaLimits, StoreError};
use std::collections::BTreeMap;
use yoink_db::Transaction;
use yoink_db::query;
use yoink_domain::{DownloadRequest, downloader_settings::DownloaderSettings};

pub(crate) async fn admission_settings(
    tx: &Transaction<'_>,
    request: &DownloadRequest,
    mut policy: AdmissionPolicy,
    mut quotas: QuotaLimits,
) -> Result<(DownloadRequest, AdmissionPolicy, QuotaLimits), StoreError> {
    let values: BTreeMap<String, String> = query(include_str!("../sql/downloader_settings.sql"))
        .fetch_all(&**tx)
        .await
        .map_err(|_| StoreError::Database)?
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get("key").map_err(|_| StoreError::Database)?,
                row.try_get("value").map_err(|_| StoreError::Database)?,
            ))
        })
        .collect::<Result<_, StoreError>>()?;
    let settings =
        DownloaderSettings::from_settings(&values).map_err(|_| StoreError::InvalidStoredData)?;
    if values.contains_key("dl.download_retries") {
        policy.max_attempts = settings.download_retries;
    }
    if values.contains_key("dl.rate_limit_per_minute") {
        quotas.minute = settings.rate_limit_per_minute;
    }
    if values.contains_key("dl.rate_limit_per_hour") {
        quotas.hour = settings.rate_limit_per_hour;
    }
    if values.contains_key("dl.rate_limit_per_day") {
        quotas.day = settings.rate_limit_per_day;
    }
    let active = [
        "dl.download_timeout",
        "dl.max_file_size_mb",
        "dl.max_file_size_gb",
        "dl.max_playlist_count",
    ]
    .iter()
    .any(|key| values.contains_key(*key));
    let request = request
        .clone()
        .with_downloader_settings(active.then_some(settings))
        .map_err(|_| StoreError::InvalidStoredData)?;
    if active
        && let Some(range) = request.params().playlist()
        && range.entry_count() > settings.max_playlist_count
    {
        return Err(StoreError::InvalidInput);
    }
    Ok((request, policy, quotas))
}
