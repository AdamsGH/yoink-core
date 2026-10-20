//! Transport-neutral, ordered membership observations. Administrative policies
//! and imported permissions are never rewritten by this service.
use chrono::{DateTime, Utc};
use serde::Serialize;
use turso::transaction::TransactionBehavior;
use yoink_access::Grant;
use yoink_db::Transaction;
use yoink_db::query;

use crate::{IdentityError, IdentityStore, TrustedProfile};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum MembershipSubject {
    Bot,
    User(TrustedProfile),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum MemberStatus {
    Creator,
    Administrator,
    Member,
    Restricted,
    Left,
    Kicked,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum ObservedTag {
    Unknown,
    Known(Option<String>),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MembershipObservation {
    pub source_id: i64,
    pub update_id: u32,
    pub occurred_at: DateTime<Utc>,
    pub group_id: i64,
    pub title: Option<String>,
    pub subject: MembershipSubject,
    pub status: MemberStatus,
    pub present: bool,
    pub tag: ObservedTag,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MembershipOutcome {
    Applied,
    Duplicate,
    Stale,
    Suppressed,
    UnknownGroup,
}

impl MembershipObservation {
    fn validate(&self) -> Result<(), IdentityError> {
        if self.source_id <= 0 || self.group_id >= 0 {
            return Err(IdentityError::InvalidId);
        }
        // Store microseconds without silently truncating a supplied ordering key.
        if self.occurred_at.timestamp() < 0
            || !self
                .occurred_at
                .timestamp_subsec_nanos()
                .is_multiple_of(1000)
            || self
                .title
                .as_ref()
                .is_some_and(|t| t.chars().count() > 256 || t.chars().any(char::is_control))
            || match self.status {
                MemberStatus::Creator | MemberStatus::Administrator | MemberStatus::Member => {
                    !self.present
                }
                MemberStatus::Left | MemberStatus::Kicked => self.present,
                MemberStatus::Restricted => false,
            }
        {
            return Err(IdentityError::InvalidMembership);
        }
        match &self.subject {
            MembershipSubject::Bot if self.tag != ObservedTag::Unknown => {
                return Err(IdentityError::InvalidMembership);
            }
            MembershipSubject::User(profile) => {
                if profile.user_id <= 0 || profile.user_id == self.source_id {
                    return Err(IdentityError::InvalidId);
                }
                crate::profile::validate_profile(profile)?;
            }
            _ => {}
        }
        if let ObservedTag::Known(Some(tag)) = &self.tag
            && (tag.trim().is_empty() || tag.chars().any(char::is_control))
        {
            return Err(IdentityError::InvalidMembership);
        }
        Ok(())
    }
}

fn db<T>(value: Result<T, yoink_db::Error>) -> Result<T, IdentityError> {
    value.map_err(|_| IdentityError::Database)
}

impl IdentityStore {
    pub async fn observe_membership(
        &self,
        observation: &MembershipObservation,
    ) -> Result<MembershipOutcome, IdentityError> {
        observation.validate()?;
        let mut connection = self.database.connection().await?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| IdentityError::Database)?;
        let result = async {
            let outcome = self.observe_in_transaction(&tx, observation).await?;
            let restored = if matches!(
                outcome,
                MembershipOutcome::Applied
                    | MembershipOutcome::Suppressed
                    | MembershipOutcome::UnknownGroup
            ) {
                let user_id = match &observation.subject {
                    MembershipSubject::Bot => 0,
                    MembershipSubject::User(profile) => profile.user_id,
                };
                crate::menu_contacts::observe(
                    &tx,
                    crate::MenuContact {
                        source_id: observation.source_id,
                        chat_id: observation.group_id,
                        user_id,
                    },
                    crate::MenuContactObservation {
                        occurred_at: observation.occurred_at.timestamp_micros(),
                        update_id: observation.update_id,
                    },
                    observation.present,
                )
                .await?
            } else {
                false
            };
            Ok((outcome, restored))
        }
        .await;
        let (outcome, contact_restored) = match result {
            Ok(outcome) => {
                tx.commit().await.map_err(|_| IdentityError::Database)?;
                outcome
            }
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        drop(connection);
        if outcome == MembershipOutcome::Applied || contact_restored {
            self.invalidate();
        }
        Ok(outcome)
    }

    async fn observe_in_transaction(
        &self,
        tx: &Transaction<'_>,
        o: &MembershipObservation,
    ) -> Result<MembershipOutcome, IdentityError> {
        if query(include_str!("sql/membership_group_suppression_read.sql"))
            .bind(o.group_id)
            .fetch_optional(&**tx)
            .await?
            .is_some()
        {
            return Ok(MembershipOutcome::Suppressed);
        }
        let payload = serde_json::to_string(o).map_err(|_| IdentityError::InvalidMembership)?;
        let old = match &o.subject {
            MembershipSubject::Bot => {
                query(include_str!("sql/membership_bot_order.sql"))
                    .bind(o.source_id)
                    .bind(o.group_id)
                    .fetch_optional(&**tx)
                    .await?
            }
            MembershipSubject::User(p) => {
                query(include_str!("sql/membership_user_order.sql"))
                    .bind(o.source_id)
                    .bind(o.group_id)
                    .bind(p.user_id)
                    .fetch_optional(&**tx)
                    .await?
            }
        };
        if let Some(row) = old {
            let key = (db(row.try_get::<i64, _>(0))?, db(row.try_get::<i64, _>(1))?);
            let incoming = (o.occurred_at.timestamp_micros(), i64::from(o.update_id));
            if incoming < key {
                return Ok(MembershipOutcome::Stale);
            }
            if incoming == key {
                return if db(row.try_get::<String, _>(2))? == payload {
                    Ok(MembershipOutcome::Duplicate)
                } else {
                    Err(IdentityError::MembershipConflict)
                };
            }
        }
        let known = query(include_str!("sql/membership_group_exists.sql"))
            .bind(o.group_id)
            .fetch_optional(&**tx)
            .await?
            .is_some();
        if !known {
            if !matches!(o.subject, MembershipSubject::Bot) {
                return Ok(MembershipOutcome::UnknownGroup);
            }
            if o.present {
                query(include_str!("sql/membership_group_discover.sql"))
                    .bind(o.group_id)
                    .bind(o.title.as_deref())
                    .bind(o.occurred_at.timestamp_micros())
                    .execute(&**tx)
                    .await?;
            }
        }
        let bot = query(include_str!("sql/membership_bot_state.sql"))
            .bind(o.source_id)
            .bind(o.group_id)
            .fetch_optional(&**tx)
            .await?;
        let generation = bot
            .as_ref()
            .map(|r| db(r.try_get::<i64, _>(1)))
            .transpose()?
            .unwrap_or(0);
        match &o.subject {
            MembershipSubject::Bot => {
                let old_present = bot
                    .as_ref()
                    .map(|r| db(r.try_get::<bool, _>(0)))
                    .transpose()?;
                // First observation also fences imported/previously unknown bot gaps.
                let generation = if old_present != Some(o.present) {
                    generation
                        .checked_add(1)
                        .ok_or(IdentityError::InvalidMembership)?
                } else {
                    generation
                };
                query(include_str!("sql/membership_bot_upsert.sql"))
                    .bind(o.source_id)
                    .bind(o.group_id)
                    .bind(o.occurred_at.timestamp_micros())
                    .bind(i64::from(o.update_id))
                    .bind(payload)
                    .bind(o.present)
                    .bind(generation)
                    .execute(&**tx)
                    .await?;
            }
            MembershipSubject::User(p) => {
                // Cross-subject lifecycle fence: delayed user events from before the
                // latest bot transition cannot reconfirm tags in the new generation.
                let generation = if let Some(bot) = &bot {
                    let key = (db(bot.try_get::<i64, _>(2))?, db(bot.try_get::<i64, _>(3))?);
                    if (o.occurred_at.timestamp_micros(), i64::from(o.update_id)) < key {
                        generation.saturating_sub(1)
                    } else {
                        generation
                    }
                } else {
                    generation
                };
                ensure_membership_user(tx, p).await?;
                let (known_tag, tag) = match &o.tag {
                    ObservedTag::Unknown => (false, None),
                    ObservedTag::Known(tag) => (true, tag.as_deref()),
                };
                query(include_str!("sql/membership_user_upsert.sql"))
                    .bind(o.source_id)
                    .bind(o.group_id)
                    .bind(p.user_id)
                    .bind(o.occurred_at.timestamp_micros())
                    .bind(i64::from(o.update_id))
                    .bind(payload)
                    .bind(o.present)
                    .bind(generation)
                    .bind(tag)
                    .bind(known_tag)
                    .execute(&**tx)
                    .await?;
            }
        }
        Ok(MembershipOutcome::Applied)
    }

    /// Unknown imported membership preserves baseline menu eligibility. Known
    /// departures, suppressed groups and known bot absence revoke member scopes.
    pub async fn membership_eligible(
        &self,
        source_id: i64,
        group_id: i64,
        user_id: i64,
    ) -> Result<bool, IdentityError> {
        if source_id <= 0 || group_id >= 0 || user_id <= 0 {
            return Err(IdentityError::InvalidId);
        }
        db(query(include_str!("sql/membership_eligible.sql"))
            .bind(source_id)
            .bind(group_id)
            .bind(user_id)
            .fetch_one(&self.database)
            .await?
            .try_get(0))
    }

    /// Durable cleanup targets include deleted groups and absent users/bots.
    /// Returned tuples are (user_id, group_id), matching access_targets_after.
    pub async fn membership_cleanup_targets(
        &self,
        source_id: i64,
    ) -> Result<Vec<(i64, i64)>, IdentityError> {
        if source_id <= 0 {
            return Err(IdentityError::InvalidId);
        }
        query(include_str!("sql/membership_cleanup_targets.sql"))
            .bind(source_id)
            .fetch_all(&self.database)
            .await?
            .into_iter()
            .map(|r| Ok((db(r.try_get(0))?, db(r.try_get(1))?)))
            .collect()
    }
}

/// Insert missing users under the same policy as signed registration, without
/// touching existing metadata, bans, global roles or preference revisions.
async fn ensure_membership_user(
    tx: &Transaction<'_>,
    p: &TrustedProfile,
) -> Result<(), IdentityError> {
    if query(include_str!("sql/membership_user_exists.sql"))
        .bind(p.user_id)
        .fetch_optional(&**tx)
        .await?
        .is_some()
    {
        return Ok(());
    }
    let role = crate::profile::registration_role(tx).await?;
    query(include_str!("profile_insert.sql"))
        .bind(p.user_id)
        .bind(p.username.as_deref())
        .bind(p.first_name.as_deref())
        .bind(p.photo_url.as_deref())
        .bind(role.to_string())
        .execute(&**tx)
        .await?;
    Ok(())
}

/// Global feature grants are a union of contributions, never materialized into
/// the imported unique permission table. A corrupt map produces no auto grants.
pub(crate) async fn automatic_grants(
    tx: &turso::Connection,
    user_id: i64,
) -> Result<Vec<Grant>, IdentityError> {
    let raw = query(include_str!("admin_tag_map_read.sql"))
        .fetch_optional(tx)
        .await?
        .map(|r| db(r.try_get::<Option<String>, _>(0)))
        .transpose()?
        .flatten();
    let Some(raw) = raw else {
        return Ok(vec![]);
    };
    let Ok(map) = crate::admin::decode_tag_map(&raw) else {
        return Ok(vec![]);
    };
    let rows = query(include_str!("sql/membership_active_tags.sql"))
        .bind(user_id)
        .fetch_all(tx)
        .await?;
    let tags: std::collections::BTreeSet<String> = rows
        .into_iter()
        .map(|r| db(r.try_get(0)))
        .collect::<Result<_, _>>()?;
    let suppressed = query(include_str!("sql/membership_feature_suppressions.sql"))
        .bind(user_id)
        .fetch_all(tx)
        .await?;
    let suppressed: std::collections::BTreeSet<(String, String)> = suppressed
        .into_iter()
        .map(|r| Ok((db(r.try_get(0))?, db(r.try_get(1))?)))
        .collect::<Result<_, IdentityError>>()?;
    let mut keys = std::collections::BTreeSet::new();
    for entry in map {
        if tags.contains(&entry.tag) {
            for key in entry.features {
                if let Some((plugin, feature)) = key.split_once(':') {
                    let key = (plugin.to_owned(), feature.to_owned());
                    if !suppressed.contains(&key) {
                        keys.insert(key);
                    }
                }
            }
        }
    }
    Ok(keys
        .into_iter()
        .map(|(plugin, feature)| Grant {
            plugin,
            feature,
            expires_at: None,
        })
        .collect())
}
