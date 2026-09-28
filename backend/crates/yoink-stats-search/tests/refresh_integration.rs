mod common;

use common::{fixture, insert, request};
use yoink_db::query;
use yoink_stats_search::{RefreshMode, SearchRequest};

#[tokio::test]
async fn reopen_and_refresh_are_noop_and_incremental_batches_coalesce() {
    let f = fixture().await;
    insert(
        &f.store,
        i64::from(i32::MIN),
        -700,
        1,
        Some("initial word"),
        "text",
    )
    .await;
    insert(
        &f.store,
        i64::from(i32::MAX),
        -700,
        2,
        Some("initial word"),
        "text",
    )
    .await;
    let initial = f.search.search(request(-700, "initial")).await.unwrap();
    assert_eq!(initial.total, 2);
    assert_eq!(
        f.search.refresh().await.unwrap().mode,
        RefreshMode::Unchanged
    );

    query("UPDATE stats_messages SET text = 'replacement word' WHERE id = ?1")
        .bind(i64::from(i32::MIN))
        .execute(f.store.database())
        .await
        .unwrap();
    query("UPDATE stats_messages SET text = 'superseded word' WHERE id = ?1")
        .bind(i64::from(i32::MIN))
        .execute(f.store.database())
        .await
        .unwrap();
    query("DELETE FROM stats_messages WHERE id = ?1")
        .bind(i64::from(i32::MAX))
        .execute(f.store.database())
        .await
        .unwrap();
    insert(&f.store, -1, -700, 3, None, "video_note").await;
    query("UPDATE stats_messages SET from_user = -77 WHERE id = -1")
        .execute(f.store.database())
        .await
        .unwrap();
    let report = f.search.refresh().await.unwrap();
    assert_eq!(report.mode, RefreshMode::Incremental);
    assert_eq!(
        report.documents, 3,
        "two changes to one id coalesce with its tombstone"
    );
    let replacement = f.search.search(request(-700, "superseded")).await.unwrap();
    assert_eq!(replacement.total, 1);
    assert_eq!(replacement.items[0].id, i64::from(i32::MIN));
    assert!(
        f.search
            .search(request(-700, "replacement"))
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let null_fields = f
        .search
        .search(SearchRequest {
            chat_id: -700,
            msg_type: Some("video_note".into()),
            limit: 10,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(null_fields.total, 1);
    assert_eq!(null_fields.items[0].text, None);
    assert_eq!(null_fields.items[0].from_user, Some(-77));
    let negative_user = f
        .search
        .search(SearchRequest {
            chat_id: -700,
            from_user: Some(-77),
            limit: 10,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(negative_user.total, 1);
    assert_eq!(
        f.search.refresh().await.unwrap().mode,
        RefreshMode::Unchanged
    );
    let index = f.directory.path().join("index");
    drop(f.search);
    let reopened = yoink_stats_search::SearchService::open(f.store.database().clone(), index)
        .await
        .unwrap();
    assert_eq!(
        reopened.refresh().await.unwrap().mode,
        RefreshMode::Unchanged
    );
}

#[tokio::test]
async fn initial_rebuild_and_dirty_batch_cross_the_256_row_boundary() {
    let f = fixture().await;
    for id in 1..=600 {
        insert(&f.store, id, -808, id, Some("many records"), "text").await;
    }
    let rebuilt = f.search.rebuild().await.unwrap();
    assert_eq!(rebuilt.mode, RefreshMode::Rebuild);
    assert_eq!(rebuilt.documents, 600);
    for id in 1..=600 {
        query("UPDATE stats_messages SET text = 'changed batch' WHERE id = ?1")
            .bind(id)
            .execute(f.store.database())
            .await
            .unwrap();
    }
    let incremental = f.search.refresh().await.unwrap();
    assert_eq!(incremental.mode, RefreshMode::Incremental);
    assert_eq!(incremental.documents, 600);
    let page = f.search.search(request(-808, "changed")).await.unwrap();
    assert_eq!(page.total, 600);
    assert_eq!(page.items.len(), 500);
    assert_eq!(page.items[0].id, 600);
    assert_eq!(page.items[499].id, 101);
    let second = f
        .search
        .search(SearchRequest {
            chat_id: -808,
            query: Some("changed".into()),
            limit: 200,
            offset: 500,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(second.total, 600);
    assert_eq!(
        second.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        (1..=100).rev().collect::<Vec<_>>()
    );
    let empty = f
        .search
        .search(SearchRequest {
            chat_id: -808,
            query: Some("changed".into()),
            limit: 20,
            offset: 900,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(empty.total, 600);
    assert!(empty.items.is_empty());
    let zero = f
        .search
        .search(SearchRequest {
            chat_id: -808,
            query: Some("changed".into()),
            limit: 0,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(zero.total, 600);
    assert!(zero.items.is_empty());
}
