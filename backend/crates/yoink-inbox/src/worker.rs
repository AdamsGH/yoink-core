//! Runtime calls tick under its shutdown/cancellation owner; idle is false.
//! Only repeatable reads/completions enter this durable queue. Writes with
//! unknown external outcomes must never be put on its retry path.
use crate::{
    Error, Result,
    contracts::{Authorization, ContentProvider, Insight},
    github::Github,
    pipeline,
    store::Store,
};
pub struct Worker<'a> {
    pub store: &'a Store,
    pub auth: &'a dyn Authorization,
    pub insight: &'a dyn Insight,
    pub content: &'a dyn ContentProvider,
    pub github: &'a dyn Github,
    pub lease_micros: i64,
    pub max_attempts: i64,
    pub retry_micros: i64,
}
impl Worker<'_> {
    pub async fn tick(&self, clock: impl Fn() -> i64) -> Result<bool> {
        if self.max_attempts < 1 || self.retry_micros < 1 {
            return Err(Error::Invalid);
        }
        let token = uuid::Uuid::new_v4().to_string();
        let Some(job) = self.store.claim(clock(), self.lease_micros, &token).await? else {
            return Ok(false);
        };
        let result = pipeline::execute(
            self.store,
            &job,
            self.auth,
            self.insight,
            self.content,
            self.github,
            &clock,
        )
        .await;
        if let Err(err) = result {
            let retry = job.attempts < self.max_attempts
                && matches!(
                    err,
                    Error::External | Error::RateLimited | Error::Database(_)
                );
            let now = clock();
            self.store
                .fail_job(
                    &job,
                    now,
                    if matches!(err, Error::Forbidden) {
                        "auth_failed"
                    } else if matches!(err, Error::RateLimited) {
                        "rate_limited"
                    } else {
                        "error"
                    },
                    if retry {
                        Some(now.checked_add(self.retry_micros).ok_or(Error::Invalid)?)
                    } else {
                        None
                    },
                )
                .await?;
        }
        Ok(true)
    }
}
