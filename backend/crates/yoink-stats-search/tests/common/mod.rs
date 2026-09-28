use std::time::Duration;

use yoink_db::query;
use yoink_stats_search::{SearchRequest, SearchService};
use yoink_storage::Store;

pub struct Fixture {
    // Retains the backing files even in integration targets that do not inspect paths.
    #[allow(dead_code)]
    pub directory: tempfile::TempDir,
    pub store: Store,
    pub search: SearchService,
}

pub async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Store::open(
        &directory.path().join("source.db"),
        true,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    store.migrate().await.unwrap();
    let search = SearchService::open(store.database().clone(), directory.path().join("index"))
        .await
        .unwrap();
    Fixture {
        directory,
        store,
        search,
    }
}

pub async fn insert(store: &Store, id: i64, chat: i64, date: i64, text: Option<&str>, kind: &str) {
    query("INSERT INTO stats_messages (id, message_id, chat_id, date, text, msg_type) VALUES (?1, ?1, ?2, ?3, ?4, ?5)")
        .bind(id).bind(chat).bind(date).bind(text).bind(kind)
        .execute(store.database()).await.unwrap();
}

pub fn request(chat_id: i64, query_text: &str) -> SearchRequest {
    SearchRequest {
        chat_id,
        query: Some(query_text.into()),
        limit: 500,
        ..SearchRequest::default()
    }
}
