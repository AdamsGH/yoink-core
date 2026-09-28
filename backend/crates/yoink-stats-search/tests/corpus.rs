use std::{path::PathBuf, time::Duration};

use serde_json::Value;
use yoink_db::{query, raw_sql};
use yoink_stats_search::{SearchRequest, SearchService};
use yoink_storage::Store;

async fn fixture() -> (tempfile::TempDir, Store, SearchService) {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let db_path = directory.path().join("search.db");
    let store = Store::open(&db_path, true, Duration::from_secs(5))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let service = SearchService::open(store.database().clone(), directory.path().join("tantivy"))
        .await
        .unwrap();
    (directory, store, service)
}

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/stats-search-fixture/cases.json")
}

#[tokio::test]
async fn fixed_contract_corpus_matches_tantivy_expectations() {
    let (_directory, store, search) = fixture().await;
    let corpus: Value = serde_json::from_slice(&std::fs::read(corpus_path()).unwrap()).unwrap();
    for message in corpus["messages"].as_array().unwrap() {
        query("INSERT INTO stats_messages (id, message_id, chat_id, date, text, caption) VALUES (?1, ?1, -555, 100, ?2, ?3)")
            .bind(message["id"].as_i64().unwrap())
            .bind(message["text"].as_str())
            .bind(message["caption"].as_str())
            .execute(store.database()).await.unwrap();
    }
    for case in corpus["queries"].as_array().unwrap() {
        let query_text = case["query"].as_str().unwrap();
        let page = search
            .search(SearchRequest {
                chat_id: -555,
                query: Some(query_text.to_owned()),
                limit: 100,
                ..SearchRequest::default()
            })
            .await
            .unwrap();
        let mut ids: Vec<i64> = page.items.iter().map(|message| message.id).collect();
        ids.sort_unstable();
        let expected: Vec<i64> = case["tantivy"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_i64().unwrap())
            .collect();
        assert_eq!(ids, expected, "{}", case["label"]);
        assert_eq!(page.total, expected.len() as u64, "{}", case["label"]);
    }
}

#[tokio::test]
async fn refresh_tracks_updates_deletes_and_chat_scope() {
    let (_directory, store, search) = fixture().await;
    query("INSERT INTO stats_messages (id, message_id, chat_id, date, from_user, msg_type, text) VALUES (1, 1, -1, 100, 7, 'text', 'old word'), (2, 2, -2, 100, 7, 'text', 'old word')")
        .execute(store.database()).await.unwrap();
    assert_eq!(
        search
            .search(SearchRequest {
                chat_id: -1,
                query: Some("old".into()),
                limit: 20,
                ..SearchRequest::default()
            })
            .await
            .unwrap()
            .total,
        1
    );
    query("UPDATE stats_messages SET text = 'new word', from_user = 9, date = 200, msg_type = 'photo' WHERE id = 1")
        .execute(store.database()).await.unwrap();
    assert_eq!(
        search
            .search(SearchRequest {
                chat_id: -1,
                query: Some("old".into()),
                limit: 20,
                ..SearchRequest::default()
            })
            .await
            .unwrap()
            .total,
        0
    );
    let page = search
        .search(SearchRequest {
            chat_id: -1,
            query: Some("new".into()),
            from_user: Some(9),
            from_date: Some(chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()),
            to_date: Some(chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()),
            msg_type: Some("photo".into()),
            limit: 20,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(page.total, 1);
    query("DELETE FROM stats_messages WHERE id = 1")
        .execute(store.database())
        .await
        .unwrap();
    assert_eq!(
        search
            .search(SearchRequest {
                chat_id: -1,
                query: Some("new".into()),
                limit: 20,
                ..SearchRequest::default()
            })
            .await
            .unwrap()
            .total,
        0
    );
}

#[tokio::test]
async fn filters_totals_pagination_rollback_reopen_and_rebuild() {
    let (_directory, store, search) = fixture().await;
    query("INSERT INTO stats_messages (id, message_id, chat_id, date, from_user, msg_type, text) VALUES (10, 10, -8, 86400000000, 4, 'text', 'alpha beta'), (11, 11, -8, 86400000000, 4, 'photo', 'alpha beta'), (12, 12, -8, 172800000000, 5, 'text', 'alpha beta'), (13, 13, -9, 86400000000, 4, 'text', 'alpha beta')")
        .execute(store.database()).await.unwrap();
    let day = chrono::NaiveDate::from_ymd_opt(1970, 1, 2).unwrap();
    let page = search
        .search(SearchRequest {
            chat_id: -8,
            query: Some("alpha beta".into()),
            from_user: Some(4),
            from_date: Some(day),
            to_date: Some(day),
            limit: 1,
            offset: 0,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.items[0].id, 11);
    let next = search
        .search(SearchRequest {
            chat_id: -8,
            query: Some("alpha beta".into()),
            from_user: Some(4),
            from_date: Some(day),
            to_date: Some(day),
            limit: 1,
            offset: 1,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(next.total, 2);
    assert_eq!(next.items[0].id, 10);
    let typed = search
        .search(SearchRequest {
            chat_id: -8,
            query: Some("alpha".into()),
            msg_type: Some("text".into()),
            limit: 20,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(typed.total, 2);

    raw_sql("BEGIN IMMEDIATE; INSERT INTO stats_messages (id, message_id, chat_id, date, text) VALUES (99, 99, -8, 1, 'rolledback'); ROLLBACK;")
        .execute(store.database()).await.unwrap();
    assert_eq!(
        search
            .search(SearchRequest {
                chat_id: -8,
                query: Some("rolledback".into()),
                limit: 10,
                ..SearchRequest::default()
            })
            .await
            .unwrap()
            .total,
        0
    );

    let index_dir = _directory.path().join("tantivy");
    drop(search);
    let reopened = SearchService::open(store.database().clone(), &index_dir)
        .await
        .unwrap();
    let before = reopened
        .search(SearchRequest {
            chat_id: -8,
            query: Some("alpha".into()),
            limit: 20,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(before.total, 3);
    reopened.rebuild().await.unwrap();
    let after = reopened
        .search(SearchRequest {
            chat_id: -8,
            query: Some("alpha".into()),
            limit: 20,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(after.total, 3);
}
