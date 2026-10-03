//! Anchored fixed-duration quotas over the preserved/imported legacy counters.
//! Called only inside the NEW-operation Immediate transaction, after replay and capacity.
use turso::transaction::Transaction;
use yoink_db::query;

use crate::{QuotaLimits, StoreError};

pub(crate) async fn charge(
    tx: &Transaction<'_>,
    actor: u64,
    limits: QuotaLimits,
    owner: Option<i64>,
    now: i64,
) -> Result<(), StoreError> {
    limits.validate()?;
    if owner == Some(actor as i64) {
        return Ok(());
    }
    let mut counters = Vec::with_capacity(3);
    let mut retry_after_secs = 0;
    for (window, duration_secs, limit) in [
        ("minute", 60_i64, limits.minute),
        ("hour", 3600, limits.hour),
        ("day", 86400, limits.day),
    ] {
        let row = query(include_str!("../sql/quota_get.sql"))
            .bind(actor as i64)
            .bind(window)
            .fetch_optional(&**tx)
            .await
            .map_err(|_| StoreError::Database)?;
        let (count, reset_at) = if let Some(row) = row {
            let count: i64 = row
                .try_get("count")
                .map_err(|_| StoreError::InvalidStoredData)?;
            let reset_at: i64 = row
                .try_get("reset_at")
                .map_err(|_| StoreError::InvalidStoredData)?;
            // Do not silently erase malformed imported state, even if expired.
            if count < 0 {
                return Err(StoreError::InvalidStoredData);
            }
            if reset_at > now {
                (count, reset_at)
            } else {
                (
                    0,
                    now.checked_add(duration_secs * 1_000_000)
                        .ok_or(StoreError::InvalidInput)?,
                )
            }
        } else {
            (
                0,
                now.checked_add(duration_secs * 1_000_000)
                    .ok_or(StoreError::InvalidInput)?,
            )
        };
        if count >= i64::from(limit) {
            let remaining = reset_at
                .checked_sub(now)
                .ok_or(StoreError::InvalidStoredData)?;
            // Ceiling, so a sub-second remainder does not invite a premature retry.
            retry_after_secs = retry_after_secs.max((remaining as u64).div_ceil(1_000_000));
        }
        counters.push((window, count, reset_at));
    }
    if retry_after_secs != 0 {
        return Err(StoreError::QuotaExceeded { retry_after_secs });
    }
    for (window, count, reset_at) in counters {
        query(include_str!("../sql/quota_put.sql"))
            .bind(actor as i64)
            .bind(window)
            .bind(count + 1)
            .bind(reset_at)
            .execute(&**tx)
            .await
            .map_err(|_| StoreError::Database)?;
    }
    Ok(())
}
