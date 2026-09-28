mod common;

use chrono::NaiveDate;
use common::{fixture, insert, request};
use yoink_stats_search::{SearchError, SearchRequest};

#[tokio::test]
async fn rejects_invalid_public_requests_with_diagnostics_and_keeps_search_live() {
    let f = fixture().await;
    insert(&f.store, 1, -9, 0, Some("still available"), "video_note").await;
    let cases = vec![
        (
            SearchRequest {
                chat_id: -9,
                query: Some("x".repeat(16_385)),
                limit: 1,
                ..SearchRequest::default()
            },
            "16384 bytes",
        ),
        (
            SearchRequest {
                chat_id: -9,
                query: Some((0..65).map(|n| format!("term{n} ")).collect()),
                limit: 1,
                ..SearchRequest::default()
            },
            "64 distinct terms",
        ),
        (
            SearchRequest {
                chat_id: -9,
                limit: 501,
                ..SearchRequest::default()
            },
            "at most 500",
        ),
        (
            SearchRequest {
                chat_id: -9,
                offset: 100_001,
                ..SearchRequest::default()
            },
            "at most 100000",
        ),
        (
            SearchRequest {
                chat_id: 0,
                ..SearchRequest::default()
            },
            "nonzero",
        ),
        (
            SearchRequest {
                chat_id: -9,
                from_date: Some(day(2)),
                to_date: Some(day(1)),
                ..SearchRequest::default()
            },
            "must not follow",
        ),
    ];
    for (request, diagnostic) in cases {
        match f.search.search(request).await.unwrap_err() {
            SearchError::InvalidInput(message) => {
                assert!(message.contains(diagnostic), "{message}")
            }
            error => panic!("unexpected error: {error}"),
        }
    }
    assert_eq!(
        f.search.search(request(-9, "still")).await.unwrap().total,
        1
    );
}

#[tokio::test]
async fn utc_microsecond_bounds_and_raw_message_type_matching() {
    let f = fixture().await;
    let start = 86_400_000_000_i64;
    insert(
        &f.store,
        1,
        -11,
        start - 1,
        Some("boundary token"),
        "video_note",
    )
    .await;
    insert(&f.store, 2, -11, start, Some("boundary token"), "video").await;
    insert(
        &f.store,
        3,
        -11,
        start + 86_400_000_000 - 1,
        Some("boundary token"),
        "video_note",
    )
    .await;
    insert(
        &f.store,
        4,
        -11,
        start + 86_400_000_000,
        Some("boundary token"),
        "video_note",
    )
    .await;
    insert(&f.store, 5, -11, -1, Some("boundary token"), "video").await;
    let selected_day = day(2);
    let page = f
        .search
        .search(SearchRequest {
            chat_id: -11,
            query: Some("boundary".into()),
            from_date: Some(selected_day),
            to_date: Some(selected_day),
            limit: 10,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(
        page.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![3, 2]
    );
    let typed = f
        .search
        .search(SearchRequest {
            chat_id: -11,
            query: Some("boundary".into()),
            msg_type: Some("video_note".into()),
            limit: 10,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(
        typed.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![4, 3, 1]
    );
    let prior_epoch_day = f
        .search
        .search(SearchRequest {
            chat_id: -11,
            query: Some("boundary".into()),
            from_date: Some(NaiveDate::from_ymd_opt(1969, 12, 31).unwrap()),
            to_date: Some(NaiveDate::from_ymd_opt(1969, 12, 31).unwrap()),
            limit: 10,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(
        prior_epoch_day
            .items
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        vec![5]
    );
    let epoch_day = f
        .search
        .search(SearchRequest {
            chat_id: -11,
            query: Some("boundary".into()),
            from_date: Some(day(1)),
            to_date: Some(day(1)),
            limit: 10,
            ..SearchRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(
        epoch_day.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![1]
    );
}

fn day(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, day).unwrap()
}
