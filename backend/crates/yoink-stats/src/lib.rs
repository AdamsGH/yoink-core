//! Native Stats product. The runtime supplies fresh Bot API membership and owns shutdown.
mod aggregates;
mod analytics;
pub mod commands;
mod filtered;
pub mod http;
pub mod imports;
mod ingestion;
pub use analytics::Params;
use chrono::Utc;
use serde_json::Value;
use std::{future::Future, path::Path, pin::Pin, sync::Arc};
use tokio::sync::RwLock;
use tokio_util::task::TaskTracker;
use yoink_access::Role;
use yoink_db::{Database, query};
use yoink_identity::IdentityStore;
pub use yoink_stats_search::{RefreshReport, SearchPage, SearchRequest};

pub type MembershipFuture<'a> =
    Pin<Box<dyn Future<Output = Result<MemberAccess, Error>> + Send + 'a>>;
pub type NameFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<UserName>, Error>> + Send + 'a>>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserName {
    pub user_id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct MemberAccess {
    pub member: bool,
    pub administrator: bool,
}
pub trait MembershipProvider: Send + Sync {
    fn lookup(&self, chat_id: i64, user_id: i64) -> MembershipFuture<'_>;
    /// None means unavailable/unknown, not a request to erase existing metadata.
    /// Some(chat) uses getChatMember; None uses getChat as the legacy fallback.
    fn lookup_name(&self, chat_id: Option<i64>, user_id: i64) -> NameFuture<'_>;
}
#[derive(Clone, Debug)]
pub struct UsernameRefreshSchedule {
    pub first_delay: std::time::Duration,
    pub interval: std::time::Duration,
    pub lookup_timeout: std::time::Duration,
}
impl Default for UsernameRefreshSchedule {
    fn default() -> Self {
        Self {
            first_delay: std::time::Duration::from_secs(60),
            interval: std::time::Duration::from_secs(3600),
            lookup_timeout: std::time::Duration::from_secs(10),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NameRefreshReport {
    pub refreshed: u64,
    pub skipped: u64,
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database_error")]
    Database,
    #[error("access_denied")]
    Forbidden,
    #[error("invalid_request")]
    Invalid,
    #[error("stats_unavailable")]
    Unavailable,
    #[error("search_error")]
    Search(#[from] yoink_stats_search::SearchError),
    #[error("user_session_error")]
    Session(#[from] yoink_user_session::Error),
}
impl From<yoink_db::Error> for Error {
    fn from(_: yoink_db::Error) -> Self {
        Self::Database
    }
}
struct Inner {
    db: Database,
    identity: Arc<IdentityStore>,
    membership: Arc<dyn MembershipProvider>,
    session: Option<Arc<yoink_user_session::Client>>,
    search: RwLock<Option<yoink_stats_search::SearchService>>,
    search_tasks: TaskTracker,
}
#[derive(Clone)]
pub struct Stats(Arc<Inner>);
impl Stats {
    pub async fn open(
        db: Database,
        identity: Arc<IdentityStore>,
        membership: Arc<dyn MembershipProvider>,
        session: Option<Arc<yoink_user_session::Client>>,
        index_dir: impl AsRef<Path>,
    ) -> Result<Self, Error> {
        Self::open_with_search_tasks(
            db,
            identity,
            membership,
            session,
            index_dir,
            TaskTracker::new(),
        )
        .await
    }

    /// The startup owner must close and wait for this tracker after stopping
    /// callers, even when the factory future is canceled before publication.
    pub async fn open_with_search_tasks(
        db: Database,
        identity: Arc<IdentityStore>,
        membership: Arc<dyn MembershipProvider>,
        session: Option<Arc<yoink_user_session::Client>>,
        index_dir: impl AsRef<Path>,
        search_tasks: TaskTracker,
    ) -> Result<Self, Error> {
        query("SELECT bot_id FROM stats_ingestion_watermarks LIMIT 1")
            .fetch_optional(&db)
            .await?;
        let search = yoink_stats_search::SearchService::open_with_tasks(
            db.clone(),
            index_dir,
            search_tasks.clone(),
        )
        .await?;
        Ok(Self(Arc::new(Inner {
            db,
            identity,
            membership,
            session,
            search: RwLock::new(Some(search)),
            search_tasks,
        })))
    }
    pub async fn refresh(&self) -> Result<RefreshReport, Error> {
        Ok(self
            .0
            .search
            .read()
            .await
            .as_ref()
            .ok_or(Error::Unavailable)?
            .refresh()
            .await?)
    }
    pub async fn recover_search(&self) -> Result<RefreshReport, Error> {
        Ok(self
            .0
            .search
            .read()
            .await
            .as_ref()
            .ok_or(Error::Unavailable)?
            .rebuild()
            .await?)
    }
    /// Recover an unreadable index into a fresh private directory, without deleting operator data.
    /// On failure search remains unavailable; retry this method with a corrected path.
    pub async fn recover_search_at(&self, index_dir: impl AsRef<Path>) -> Result<(), Error> {
        let mut guard = self.0.search.write().await;
        *guard = None;
        *guard = Some(
            yoink_stats_search::SearchService::open_with_tasks(
                self.0.db.clone(),
                index_dir,
                self.0.search_tasks.clone(),
            )
            .await?,
        );
        Ok(())
    }
    /// Wait for active search work, flush derived state and release the Tantivy writer lock.
    pub async fn shutdown(&self) -> Result<(), Error> {
        let mut guard = self.0.search.write().await;
        let result = if let Some(search) = guard.take() {
            search.refresh().await.map(|_| ()).map_err(Error::from)
        } else {
            Ok(())
        };
        self.0.search_tasks.close();
        self.0.search_tasks.wait().await;
        result
    }
    pub async fn search(&self, user: i64, request: SearchRequest) -> Result<SearchPage, Error> {
        self.authorize(user, request.chat_id, false, false).await?;
        Ok(self
            .0
            .search
            .read()
            .await
            .as_ref()
            .ok_or(Error::Unavailable)?
            .search(request)
            .await?)
    }
    async fn role(&self, user: i64) -> Result<Role, Error> {
        let snapshot = self
            .0
            .identity
            .snapshot(user, None)
            .await
            .map_err(|e| {
                if e == yoink_identity::IdentityError::Database {
                    Error::Database
                } else {
                    Error::Forbidden
                }
            })?
            .ok_or(Error::Forbidden)?;
        if snapshot.principal.is_blocked(Utc::now()) || snapshot.principal.role < Role::User {
            return Err(Error::Forbidden);
        }
        Ok(snapshot.principal.role)
    }
    async fn authorize(
        &self,
        user: i64,
        chat: i64,
        admin: bool,
        chat_admin: bool,
    ) -> Result<MemberAccess, Error> {
        let role = self.role(user).await?;
        // Stats collects groups only. Never let an elevated role enumerate private dialogs.
        if chat >= 0 {
            return Err(Error::Forbidden);
        }
        if role >= Role::Admin {
            return Ok(MemberAccess::default());
        }
        if admin {
            return Err(Error::Forbidden);
        }
        if chat_admin {
            let access = self.0.membership.lookup(chat, user).await?;
            return if access.administrator {
                Ok(access)
            } else {
                Err(Error::Forbidden)
            };
        }
        if role == Role::Moderator {
            let assigned =
                query("SELECT 1 FROM user_group_policies WHERE user_id=? AND group_id=?")
                    .bind(user)
                    .bind(chat)
                    .fetch_optional(&self.0.db)
                    .await?
                    .is_some();
            return if assigned {
                Ok(MemberAccess::default())
            } else {
                Err(Error::Forbidden)
            };
        }
        let access = self.0.membership.lookup(chat, user).await?;
        if access.member {
            Ok(access)
        } else {
            Err(Error::Forbidden)
        }
    }
    async fn authorize_on(
        &self,
        conn: &mut turso::Connection,
        user: i64,
        chat: i64,
        admin: bool,
        chat_admin: bool,
        access: MemberAccess,
    ) -> Result<(), Error> {
        let snapshot = self
            .0
            .identity
            .snapshot_on(conn, user, None)
            .await
            .map_err(|_| Error::Database)?
            .ok_or(Error::Forbidden)?;
        let role = snapshot.principal.role;
        if chat >= 0 || snapshot.principal.is_blocked(Utc::now()) || role < Role::User {
            return Err(Error::Forbidden);
        }
        if role >= Role::Admin {
            return Ok(());
        }
        if admin {
            return Err(Error::Forbidden);
        }
        if chat_admin {
            return if access.administrator {
                Ok(())
            } else {
                Err(Error::Forbidden)
            };
        }
        if role == Role::Moderator {
            return if query("SELECT 1 FROM user_group_policies WHERE user_id=? AND group_id=?")
                .bind(user)
                .bind(chat)
                .fetch_optional(conn)
                .await?
                .is_some()
            {
                Ok(())
            } else {
                Err(Error::Forbidden)
            };
        }
        if access.member {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }
    pub async fn groups(&self, user: i64) -> Result<Value, Error> {
        self.role(user).await?;
        let rows = query("SELECT g.id,g.title,count(m.id) AS count FROM groups g LEFT JOIN stats_messages m ON m.chat_id=g.id WHERE g.enabled=1 AND g.id<0 GROUP BY g.id,g.title ORDER BY count DESC,g.id").fetch_all(&self.0.db).await?;
        let mut result = Vec::new();
        for row in rows {
            let chat: i64 = row.try_get("id")?;
            match self.authorize(user, chat, false, false).await {
                Ok(access) => {
                    let mut conn = self.0.db.connection().await?;
                    self.authorize_on(&mut conn, user, chat, false, false, access)
                        .await?;
                    result.push(serde_json::json!({"chat_id":chat,"title":row.try_get::<Option<String>,_>("title")?,"message_count":row.try_get::<i64,_>("count")?}));
                }
                Err(Error::Forbidden) => (),
                Err(e) => return Err(e),
            }
        }
        Ok(Value::Array(result))
    }
}
