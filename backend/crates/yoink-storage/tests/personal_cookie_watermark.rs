mod common;
use common::unmigrated_store;
use std::time::Duration;
use yoink_storage::Store;

#[tokio::test]
async fn personal_cookie_watermark_is_durable_and_rejects_stale_commands() {
    let disk = unmigrated_store().await;
    disk.migrate().await.unwrap();
    let path = disk.path();
    for id in [80, 79, 90] {
        yoink_db::query(include_str!(
            "fixtures/advance_personal_cookie_watermark.sql"
        ))
        .bind(id)
        .execute(disk.database())
        .await
        .unwrap();
    }
    let stored: i64 = yoink_db::query(include_str!("fixtures/read_personal_cookie_watermark.sql"))
        .fetch_one(disk.database())
        .await
        .unwrap()
        .try_get("message_id")
        .unwrap();
    assert_eq!(stored, 90);
    disk.close().await.unwrap();
    let reopened = Store::open(&path, false, Duration::from_secs(5))
        .await
        .unwrap();
    let stored: i64 = yoink_db::query(include_str!("fixtures/read_personal_cookie_watermark.sql"))
        .fetch_one(reopened.database())
        .await
        .unwrap()
        .try_get("message_id")
        .unwrap();
    assert_eq!(stored, 90);
    reopened.close().await.unwrap();
}
