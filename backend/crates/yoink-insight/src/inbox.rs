//! Actual adapter for yoink_inbox::contracts::Insight. The macro creates the
//! caller-local wrapper needed by Rust's orphan rule without introducing a cyclic
//! product dependency or referring to a neighboring checkout in Cargo manifests.
use crate::{Error, Insight};
use tokio_util::sync::CancellationToken;
use yoink_db::query;
#[derive(Clone)]
pub struct InboxAdapter {
    insight: Insight,
    shutdown: CancellationToken,
}
impl std::fmt::Debug for InboxAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InboxInsightAdapter([REDACTED])")
    }
}
impl InboxAdapter {
    pub fn new(insight: Insight, shutdown: CancellationToken) -> Self {
        Self { insight, shutdown }
    }
    /// Only the private internal worker receives this secret. Read and write tokens
    /// are independent: a read credential is never promoted to a write credential.
    pub async fn github_token(&self, user: i64, write: bool) -> Result<Option<String>, Error> {
        if self.shutdown.is_cancelled() {
            return Err(Error::Code("cancelled"));
        }
        let c = self.insight.db.connection().await?;
        if write {
            self.insight
                .authorize(&*c, user, Some("inbox:gh_write"), false)
                .await?;
        } else {
            // Enrichment belongs to ingestion; stars sync is an independent read use.
            self.insight.authorize(&*c, user, None, false).await?;
            if self
                .insight
                .authorize(&*c, user, Some("inbox:ingest"), false)
                .await
                .is_err()
            {
                self.insight
                    .authorize(&*c, user, Some("inbox:gh_sync"), false)
                    .await?;
            }
        }
        let sql = if write {
            "SELECT github_token_public_repo AS token FROM insight_user_settings WHERE user_id=?"
        } else {
            "SELECT github_token AS token FROM insight_user_settings WHERE user_id=?"
        };
        Ok(query(sql)
            .bind(user)
            .fetch_optional(&*c)
            .await?
            .map(|r| r.try_get::<Option<String>, _>("token"))
            .transpose()?
            .flatten()
            .filter(|s| !s.is_empty()))
    }
    /// Inbox classification is authorized by Inbox's own classify grant. It does
    /// not require a separate TLDR grant, but uses the same current Insight routes.
    pub async fn complete(&self, user: i64, prompt: &str) -> Result<String, Error> {
        let feature = {
            let c = self.insight.db.connection().await?;
            if self
                .insight
                .authorize(&*c, user, Some("inbox:classify"), false)
                .await
                .is_ok()
            {
                "inbox:classify"
            } else {
                self.insight
                    .authorize(&*c, user, Some("inbox:gh_organise"), false)
                    .await?;
                "inbox:gh_organise"
            }
        };
        self.insight
            .complete_for_feature(user, feature, prompt, self.shutdown.child_token())
            .await
    }
    pub async fn language(&self, user: i64) -> Result<String, Error> {
        if self.shutdown.is_cancelled() {
            return Err(Error::Code("cancelled"));
        }
        let c = self.insight.db.connection().await?;
        self.insight.authorize(&*c, user, None, false).await?;
        let row=query("SELECT coalesce(a.lang,'en') AS lang FROM users u LEFT JOIN insight_access a ON a.user_id=u.id WHERE u.id=?").bind(user).fetch_one(&*c).await?;
        Ok(row.try_get("lang")?)
    }
}
/// Instantiate in a crate which depends on both yoink-insight and yoink-inbox:
/// `yoink_insight::impl_inbox_adapter!(pub RuntimeInboxInsight);`.
/// This emits the complete, exact foreign-trait implementation, not a callback
/// scaffold. No coordinator method bodies or secret/error handling are needed.
#[macro_export]
macro_rules! impl_inbox_adapter {
 ($visibility:vis $name:ident) => {
  #[derive(Clone,Debug)]
  $visibility struct $name($crate::inbox::InboxAdapter);
  impl $name {
   $visibility fn new(insight:$crate::Insight,shutdown:tokio_util::sync::CancellationToken)->Self{Self($crate::inbox::InboxAdapter::new(insight,shutdown))}
   fn map_error(error:$crate::Error)->yoink_inbox::Error{match error {
    $crate::Error::Code("access_denied"|"route_changed"|"credential_changed")=>yoink_inbox::Error::Forbidden,
    $crate::Error::Code("cancelled")=>yoink_inbox::Error::Cancelled,
    $crate::Error::Code("provider_rate_limited")=>yoink_inbox::Error::RateLimited,
    $crate::Error::Code("provider_unavailable"|"timeout") | $crate::Error::Database=>yoink_inbox::Error::External,
    $crate::Error::Code("invalid_request")=>yoink_inbox::Error::Invalid,
     $crate::Error::Code(code)=>yoink_inbox::Error::Completion(match code {
      "auth_failed"=>"auth_failed", "quota_exceeded"=>"quota_exceeded", "insufficient_credits"=>"insufficient_credits", "model_not_found"=>"model_not_found", "no_route"|"unsafe_endpoint"|"invalid_config"|"model_not_allowed"=>"invalid_config", _=>"completion_failed",
    }),
   }}
  }
  impl yoink_inbox::contracts::Insight for $name {
   fn github_token(&self,user:i64,write:bool)->yoink_inbox::contracts::Call<'_,Option<String>>{Box::pin(async move{self.0.github_token(user,write).await.map_err(Self::map_error)})}
   fn complete<'a>(&'a self,user:i64,prompt:&'a str)->yoink_inbox::contracts::Call<'a,String>{Box::pin(async move{self.0.complete(user,prompt).await.map_err(Self::map_error)})}
   fn language(&self,user:i64)->yoink_inbox::contracts::Call<'_,String>{Box::pin(async move{self.0.language(user).await.map_err(Self::map_error)})}
  }
 };
}
