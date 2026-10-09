use crate::{Error, Stats};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Params {
    pub chat_id: i64,
    pub days: Option<i64>,
    pub limit: Option<usize>,
    pub user_id: Option<i64>,
    pub target_user_id: Option<i64>,
    pub year: Option<i32>,
    pub start: Option<NaiveDate>,
    pub end: Option<NaiveDate>,
    pub msg_type: Option<String>,
    pub q: Option<String>,
}
pub(crate) fn validate_days(days: Option<i64>) -> Result<(), Error> {
    if days.is_some_and(|d| !(1..=3650).contains(&d)) {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn dt(n: i64) -> Result<DateTime<Utc>, Error> {
    DateTime::from_timestamp_micros(n).ok_or(Error::Database)
}
pub(crate) fn iso(n: Option<i64>) -> Result<Value, Error> {
    Ok(n.map(dt)
        .transpose()?
        .map(|d| json!(d.to_rfc3339()))
        .unwrap_or(Value::Null))
}
pub(crate) fn rounded(n: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    (n * scale).round() / scale
}
pub(crate) fn average(count: usize, first: Option<i64>, last: Option<i64>) -> f64 {
    match (first, last) {
        (Some(f), Some(l)) => rounded(count as f64 / ((l - f) / 86_400_000_000).max(1) as f64, 2),
        _ => 0.0,
    }
}
pub(crate) fn ranked<K: Ord + Clone>(counts: &BTreeMap<K, usize>, limit: usize) -> Vec<(K, usize)> {
    let mut result: Vec<_> = counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
    result.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    result.truncate(limit);
    result
}
impl Stats {
    pub async fn analytics(&self, user: i64, endpoint: &str, p: Params) -> Result<Value, Error> {
        validate_days(p.days)?;
        let access = self
            .authorize(
                user,
                p.chat_id,
                endpoint == "top-reactions",
                endpoint == "members",
            )
            .await?;
        let max_limit = if matches!(
            endpoint,
            "avg-message-length" | "response-time" | "top-reactions"
        ) {
            50
        } else {
            100
        };
        let limit = p.limit.unwrap_or(
            if matches!(
                endpoint,
                "words" | "mention-stats" | "ecdf" | "command-words" | "command-mentions"
            ) {
                20
            } else {
                10
            },
        );
        if !(1..=max_limit).contains(&limit)
            || matches!((p.start,p.end),(Some(s),Some(e)) if s>e)
            || p.end.is_some_and(|d| d.succ_opt().is_none())
            || p.year.is_some_and(|y| !(1..=9998).contains(&y))
            || p.user_id.is_some_and(|u| u <= 0)
            || p.target_user_id.is_some_and(|u| u <= 0)
            || p.q.as_ref().is_some_and(|q| {
                q.len() > 16_384
                    || yoink_stats_search::tokenize(q)
                        .into_iter()
                        .collect::<BTreeSet<_>>()
                        .len()
                        > 64
            })
            || p.msg_type.as_ref().is_some_and(|t| t.len() > 128)
        {
            return Err(Error::Invalid);
        }
        if matches!(endpoint, "chat-admins" | "member-events" | "top-reactions")
            || (endpoint == "members" && p.q.is_none())
        {
            let mut conn = self.0.db.connection().await?;
            self.authorize_on(
                &mut conn,
                user,
                p.chat_id,
                endpoint == "top-reactions",
                endpoint == "members",
                access,
            )
            .await?;
            return crate::aggregates::aggregate(&conn, endpoint, &p, limit)
                .await?
                .ok_or(Error::Invalid);
        }
        crate::filtered::aggregate(self, user, access, endpoint, &p, limit).await
    }
}
