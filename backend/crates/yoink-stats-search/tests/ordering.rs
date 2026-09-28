mod common;

use common::{fixture, insert, request};
use yoink_db::query;
use yoink_stats_search::SearchRequest;

#[tokio::test]
async fn equal_dates_order_by_signed_id_across_pages_and_refreshes() {
    let f = fixture().await;
    for id in [9, -4, 1, 7] {
        insert(&f.store, id, i64::MIN, -1, Some("equal timestamp"), "text").await;
    }
    query("UPDATE stats_messages SET message_id = ?1, from_user = ?2 WHERE id = 9")
        .bind(i64::MAX)
        .bind(i64::MIN)
        .execute(f.store.database())
        .await
        .unwrap();
    let all = f.search.search(request(i64::MIN, "equal")).await.unwrap();
    assert_eq!(
        all.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![9, 7, 1, -4]
    );
    assert_eq!(all.items[0].message_id, i64::MAX);
    assert_eq!(all.items[0].from_user, Some(i64::MIN));
    let page = f
        .search
        .search(SearchRequest {
            limit: 2,
            offset: 1,
            ..request(i64::MIN, "equal")
        })
        .await
        .unwrap();
    assert_eq!(page.total, 4);
    assert_eq!(
        page.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![7, 1]
    );
    query("UPDATE stats_messages SET text = 'equal replacement' WHERE id = -4")
        .execute(f.store.database())
        .await
        .unwrap();
    let refreshed = f.search.search(request(i64::MIN, "equal")).await.unwrap();
    assert_eq!(
        refreshed.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![9, 7, 1, -4]
    );
}
