use super::{SearchRequest, SearchService};
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;
use yoink_db::query;
use yoink_storage::Store;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Stage {
    AfterBatch,
    AfterIndexCommit,
    BeforeHydration,
}

pub(super) struct Pause {
    stage: Stage,
    pub entered: Notify,
    pub release: Notify,
}

impl SearchService {
    pub(super) fn arm(&self, stage: Stage) -> Arc<Pause> {
        let pause = Arc::new(Pause {
            stage,
            entered: Notify::new(),
            release: Notify::new(),
        });
        assert!(self.pause.lock().unwrap().replace(pause.clone()).is_none());
        pause
    }

    pub(super) async fn pause_at(&self, stage: Stage) {
        let pause = {
            let mut state = self.pause.lock().unwrap();
            if state.as_ref().is_some_and(|pause| pause.stage == stage) {
                state.take()
            } else {
                None
            }
        };
        if let Some(pause) = pause {
            pause.entered.notify_one();
            pause.release.notified().await;
        }
    }
}

pub(super) async fn fixture() -> (tempfile::TempDir, Store, Arc<SearchService>) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Store::open(&dir.path().join("stats.db"), true, Duration::from_secs(5))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let search = SearchService::open(store.database().clone(), dir.path().join("index"))
        .await
        .unwrap();
    (dir, store, Arc::new(search))
}

pub(super) async fn seed(store: &Store) {
    query("INSERT INTO stats_messages(id, message_id, chat_id, date, text) VALUES (1, 1, -1, 1234567, 'old')")
        .execute(store.database()).await.unwrap();
}

pub(super) fn request(text: &str) -> SearchRequest {
    SearchRequest {
        chat_id: -1,
        query: Some(text.into()),
        limit: 100,
        ..SearchRequest::default()
    }
}

pub(super) async fn wait(pause: &Pause) {
    tokio::time::timeout(Duration::from_secs(5), pause.entered.notified())
        .await
        .unwrap();
}
