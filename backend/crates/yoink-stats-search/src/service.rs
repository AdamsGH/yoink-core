use crate::{
    SearchPage, SearchRequest,
    engine::{CommitBinding, NativeIndex},
    source,
};
use std::{
    path::Path,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Mutex;
use turso::transaction::{Transaction, TransactionBehavior};
use yoink_db::{Database, query};

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod recovery_tests;
#[cfg(test)]
#[path = "test_support.rs"]
mod test_support;

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("Stats search database operation failed")]
    Database,
    #[error("Stats search index failed: {0}")]
    Index(String),
    #[error("invalid Stats search request: {0}")]
    InvalidInput(&'static str),
    #[error("blocking Stats search task failed; close and reopen the search service")]
    Task,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshMode {
    Unchanged,
    Incremental,
    Rebuild,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshReport {
    pub mode: RefreshMode,
    /// Documents processed, including tombstones for incremental refreshes.
    pub documents: u64,
    pub revision: i64,
}

pub struct SearchService {
    database: Database,
    index: Arc<StdMutex<NativeIndex>>,
    operation: Mutex<()>,
    needs_rebuild: AtomicBool,
    binding: CommitBinding,
    #[cfg(test)]
    pause: StdMutex<Option<Arc<test_support::Pause>>>,
}

impl SearchService {
    pub async fn open(
        database: Database,
        index_dir: impl AsRef<Path>,
    ) -> Result<Self, SearchError> {
        database
            .verify_files()
            .await
            .map_err(|_| SearchError::Database)?;
        let path = index_dir.as_ref().to_path_buf();
        let database_path = database.path().to_path_buf();
        let (index, binding) = tokio::task::spawn_blocking(move || {
            let binding = source::file_binding(&database_path)?;
            let index = NativeIndex::open(&path).map_err(SearchError::Index)?;
            Ok::<_, SearchError>((index, binding))
        })
        .await
        .map_err(|_| SearchError::Task)??;
        let service = Self {
            database,
            index: Arc::new(StdMutex::new(index)),
            operation: Mutex::new(()),
            needs_rebuild: AtomicBool::new(false),
            binding,
            #[cfg(test)]
            pause: StdMutex::new(None),
        };
        service.refresh().await?;
        Ok(service)
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchPage, SearchError> {
        source::validate(&request)?;
        let _operation = self.operation.lock().await;
        self.database
            .verify_files()
            .await
            .map_err(|_| SearchError::Database)?;
        let mut connection = self
            .database
            .connection()
            .await
            .map_err(|_| SearchError::Database)?;
        // Keep refresh, query and hydration in one native snapshot.
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| SearchError::Database)?;
        self.synchronize(&tx, false).await?;
        let (total, ids) = self.blocking(move |index| index.search(&request)).await?;
        #[cfg(test)]
        self.pause_at(test_support::Stage::BeforeHydration).await;
        let mut items = Vec::with_capacity(ids.len());
        for id in ids {
            let row = query(include_str!("../sql/get_message.sql"))
                .bind(id)
                .fetch_one(&*tx)
                .await
                .map_err(|_| SearchError::Database)?;
            items.push(source::decode_message(&row)?);
        }
        tx.commit().await.map_err(|_| SearchError::Database)?;
        self.needs_rebuild.store(false, Ordering::Release);
        Ok(SearchPage { items, total })
    }

    pub async fn refresh(&self) -> Result<RefreshReport, SearchError> {
        self.refresh_inner(false).await
    }

    pub async fn rebuild(&self) -> Result<RefreshReport, SearchError> {
        self.refresh_inner(true).await
    }

    async fn refresh_inner(&self, force: bool) -> Result<RefreshReport, SearchError> {
        let _operation = self.operation.lock().await;
        self.database
            .verify_files()
            .await
            .map_err(|_| SearchError::Database)?;
        let mut connection = self
            .database
            .connection()
            .await
            .map_err(|_| SearchError::Database)?;
        let tx = Transaction::new(&mut connection, TransactionBehavior::Immediate)
            .await
            .map_err(|_| SearchError::Database)?;
        let report = self.synchronize(&tx, force).await?;
        tx.commit().await.map_err(|_| SearchError::Database)?;
        self.needs_rebuild.store(false, Ordering::Release);
        Ok(report)
    }

    async fn synchronize(
        &self,
        tx: &Transaction<'_>,
        force: bool,
    ) -> Result<RefreshReport, SearchError> {
        let state = query(include_str!("../sql/state.sql"))
            .fetch_one(&**tx)
            .await
            .map_err(|_| SearchError::Database)?;
        let revision: i64 = state
            .try_get("source_revision")
            .map_err(|_| SearchError::Database)?;
        let acknowledged: i64 = state
            .try_get("acknowledged_revision")
            .map_err(|_| SearchError::Database)?;
        let mut binding = self.binding.clone();
        binding.database_id = state
            .try_get("database_id")
            .map_err(|_| SearchError::Database)?;
        binding.revision = acknowledged;
        let payload = self.blocking(|index| index.binding()).await?;
        let rebuild = force
            || self.needs_rebuild.load(Ordering::Acquire)
            || payload.as_ref() != Some(&binding);
        let dirty: i64 = query(include_str!("../sql/dirty_count.sql"))
            .fetch_one(&**tx)
            .await
            .map_err(|_| SearchError::Database)?
            .try_get("count")
            .map_err(|_| SearchError::Database)?;
        if !rebuild && revision == acknowledged && dirty == 0 {
            return Ok(RefreshReport {
                mode: RefreshMode::Unchanged,
                documents: 0,
                revision,
            });
        }
        let rebuild = rebuild || (revision != acknowledged && dirty == 0);
        self.needs_rebuild.store(true, Ordering::Release);
        if rebuild {
            self.blocking(|index| index.reset()).await?;
        }
        let mut cursor: Option<i64> = None;
        let mut documents = 0;
        loop {
            let sql = if rebuild {
                include_str!("../sql/rebuild_batch.sql")
            } else {
                include_str!("../sql/dirty_batch.sql")
            };
            let rows = query(sql)
                .bind(cursor)
                .bind(256_i64)
                .fetch_all(&**tx)
                .await
                .map_err(|_| SearchError::Database)?;
            if rows.is_empty() {
                break;
            }
            let mut changes = Vec::with_capacity(rows.len());
            for row in rows {
                let id: i64 = row.try_get("dirty_id").map_err(|_| SearchError::Database)?;
                let present: Option<i64> = row.try_get("id").map_err(|_| SearchError::Database)?;
                let message = present.map(|_| source::decode_message(&row)).transpose()?;
                changes.push((id, message));
                cursor = Some(id);
                documents += 1;
            }
            self.blocking(move |index| {
                for (id, message) in changes {
                    index.update(id, message.as_ref())?;
                }
                Ok(())
            })
            .await?;
            #[cfg(test)]
            self.pause_at(test_support::Stage::AfterBatch).await;
        }
        binding.revision = revision;
        self.blocking(move |index| index.commit(&binding)).await?;
        #[cfg(test)]
        self.pause_at(test_support::Stage::AfterIndexCommit).await;
        // Acknowledgement never leads the committed, reloaded index.
        query(include_str!("../sql/clear_dirty.sql"))
            .execute(&**tx)
            .await
            .map_err(|_| SearchError::Database)?;
        query(include_str!("../sql/acknowledge.sql"))
            .bind(revision)
            .execute(&**tx)
            .await
            .map_err(|_| SearchError::Database)?;
        Ok(RefreshReport {
            mode: if rebuild {
                RefreshMode::Rebuild
            } else {
                RefreshMode::Incremental
            },
            documents,
            revision,
        })
    }

    async fn blocking<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut NativeIndex) -> Result<T, String> + Send + 'static,
    ) -> Result<T, SearchError> {
        let index = self.index.clone();
        tokio::task::spawn_blocking(move || {
            let mut index = index.lock().map_err(|_| SearchError::Task)?;
            action(&mut index).map_err(SearchError::Index)
        })
        .await
        .map_err(|_| SearchError::Task)?
    }
}
