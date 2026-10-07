use serde_json::json;
use yoink_db::{Database, Executor, query};
use yoink_inbox::{contracts::Content, github::StarSnapshot, pipeline::parse, store::Store};
async fn fixture() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let db = Database::open(
        &dir.path().join("inbox.db"),
        true,
        std::time::Duration::from_secs(10),
    )
    .await
    .unwrap();
    db.batch(include_str!(
        "../../yoink-storage/migrations-turso/0002_application.sql"
    ))
    .await
    .unwrap();
    db.batch(include_str!("../sql/durable_jobs.sql"))
        .await
        .unwrap();
    db.batch(include_str!("../sql/github_mutations.sql"))
        .await
        .unwrap();
    db.batch(include_str!("../sql/notification_outbox.sql"))
        .await
        .unwrap();
    for id in 1..=3 {
        query("INSERT INTO users(id,created_at,updated_at) VALUES(?,1,1)")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
    }
    (dir, Store::new(db))
}
#[tokio::test]
async fn native_catalog() {
    let (_d, s) = fixture().await;
    let rows = query("PRAGMA table_info(inbox_categories)")
        .fetch_all(&s.db)
        .await
        .unwrap();
    let names: Vec<String> = rows.iter().map(|r| r.try_get("name").unwrap()).collect();
    assert!(names.contains(&"normalized_name".into()));
    let rows = query("SELECT sql FROM sqlite_schema WHERE name='inbox_items'")
        .fetch_one(&s.db)
        .await
        .unwrap();
    let sql: String = rows.try_get("sql").unwrap();
    assert!(sql.contains("UNIQUE (user_id, normalized_url)"));
    assert!(sql.contains("\"created_at\" INTEGER NOT NULL"));
}
#[tokio::test]
async fn durable_dedup_isolation_and_delete() {
    let (_d, s) = fixture().await;
    let a = s
        .ingest(1, "https://example.com/a?utm_x=z", "web", 10)
        .await
        .unwrap();
    assert!(a.created);
    let b = s
        .ingest(1, "https://example.com/a#fragment", "web", 11)
        .await
        .unwrap();
    assert!(!b.created);
    assert_eq!(a.item_id, b.item_id);
    assert!(s.item(2, a.item_id).await.is_err());
    let other = s
        .ingest(2, "https://example.com/a", "web", 12)
        .await
        .unwrap();
    assert_ne!(a.item_id, other.item_id);
    s.archive(1, a.item_id, false, 13).await.unwrap();
    assert_eq!(s.item(1, a.item_id).await.unwrap()["status"], "archived");
    assert!(
        !s.ingest(1, "https://example.com/a", "web", 14)
            .await
            .unwrap()
            .created
    );
    s.archive(1, a.item_id, true, 15).await.unwrap();
    let n: i64 = query("SELECT count(*) AS n FROM inbox_jobs WHERE user_id=1")
        .fetch_one(&s.db)
        .await
        .unwrap()
        .try_get("n")
        .unwrap();
    assert_eq!(n, 0);
    assert!(
        s.ingest(1, "https://example.com/a", "web", 16)
            .await
            .unwrap()
            .created
    );
}
#[tokio::test]
async fn crash_reclaim_fences_stale_write() {
    let (_d, s) = fixture().await;
    let id = s
        .ingest(1, "https://example.com", "bot", 1)
        .await
        .unwrap()
        .item_id;
    let a = s.claim(1, 10, "a").await.unwrap().unwrap();
    assert!(s.claim(2, 10, "b").await.unwrap().is_none());
    let b = s.claim(11, 10, "b").await.unwrap().unwrap();
    assert_eq!(a.id, b.id);
    assert!(
        !s.apply_content(
            &a,
            &Content {
                title: Some("stale".into()),
                ..Default::default()
            },
            12
        )
        .await
        .unwrap()
    );
    assert!(
        s.apply_content(
            &b,
            &Content {
                title: Some("fresh".into()),
                ..Default::default()
            },
            12
        )
        .await
        .unwrap()
    );
    assert_eq!(s.item(1, id).await.unwrap()["title"], "fresh");
    let classify = s.claim(12, 10, "c").await.unwrap().unwrap();
    assert_eq!(classify.kind, "classify");
    s.apply_classification(&classify, &parse("{\"summary\":\"summary\"}"), 13)
        .await
        .unwrap();
    assert_eq!(s.item(1, id).await.unwrap()["summary"], "summary");
}
#[tokio::test]
async fn shared_assignment_atomic_and_manual_survives() {
    let (_d, s) = fixture().await;
    let id = s
        .ingest(1, "https://example.com", "web", 1)
        .await
        .unwrap()
        .item_id;
    query("INSERT INTO inbox_teams(id,name,slug,owner_user_id,created_at) VALUES(1,'team','team',2,1)").execute(&s.db).await.unwrap();
    query("INSERT INTO inbox_team_members(team_id,user_id,joined_at) VALUES(1,1,1)")
        .execute(&s.db)
        .await
        .unwrap();
    query("INSERT INTO inbox_categories(id,owner_user_id,shared_with_team_id,name,normalized_name,slug,created_at) VALUES(1,2,1,'Rust','rust','rust',1)").execute(&s.db).await.unwrap();
    s.assign_categories(1, id, &[1], 2).await.unwrap();
    assert!(s.assign_categories(1, id, &[1, 999], 3).await.is_err());
    assert_eq!(
        s.item(1, id).await.unwrap()["categories"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let j = s.claim(1, 100, "x").await.unwrap().unwrap();
    s.apply_content(&j, &Content::default(), 2).await.unwrap();
    let j = s.claim(2, 100, "y").await.unwrap().unwrap();
    s.apply_classification(&j,&parse("{\"categories\":[{\"name\":\"Rust\"},{\"name\":\"A\",\"kind\":\"new\"},{\"name\":\"B\",\"kind\":\"new\"}]}"),4).await.unwrap();
    let item = s.item(1, id).await.unwrap();
    let cats = item["categories"].as_array().unwrap();
    assert_eq!(cats.len(), 2);
    assert_eq!(cats[0]["attached_by"], "user");
}
#[tokio::test]
async fn star_snapshot_preserves_ai_and_empty_removes() {
    let (_d, s) = fixture().await;
    s.enqueue(1, None, "stars_sync", 1).await.unwrap();
    let j = s.claim(1, 100, "one").await.unwrap().unwrap();
    let snap = StarSnapshot {
        rows: vec![
            json!({"repo":{"id":10,"full_name":"a/b","owner":{"login":"a"},"html_url":"https://github.com/a/b"}}),
        ],
        etag: Some("e".into()),
        can_unstar: true,
        not_modified: false,
    };
    assert!(s.apply_stars(&j, &snap, 2).await.unwrap());
    query("UPDATE inbox_gh_stars SET ai_summary='preserved'")
        .execute(&s.db)
        .await
        .unwrap();
    s.enqueue(1, None, "stars_sync", 3).await.unwrap();
    let j = s.claim(3, 100, "two").await.unwrap().unwrap();
    s.apply_stars(&j, &snap, 4).await.unwrap();
    let summary: String = query("SELECT ai_summary FROM inbox_gh_stars")
        .fetch_one(&s.db)
        .await
        .unwrap()
        .try_get("ai_summary")
        .unwrap();
    assert_eq!(summary, "preserved");
    s.enqueue(1, None, "stars_sync", 5).await.unwrap();
    let j = s.claim(5, 100, "three").await.unwrap().unwrap();
    s.apply_stars(
        &j,
        &StarSnapshot {
            rows: vec![],
            ..snap
        },
        6,
    )
    .await
    .unwrap();
    let n: i64 = query("SELECT count(*) AS n FROM inbox_gh_stars")
        .fetch_one(&s.db)
        .await
        .unwrap()
        .try_get("n")
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn catalogue_and_team_controls() {
    use yoink_inbox::catalog::Catalog;
    let (_d, s) = fixture().await;
    let team = s
        .save_team(1, None, &json!({"name":"Team","slug":"team"}), 10)
        .await
        .unwrap();
    let tid = team["id"].as_i64().unwrap();
    assert!(s.team_member(2, tid, 3, Some("member"), 11).await.is_err());
    s.team_member(1, tid, 2, Some("admin"), 11).await.unwrap();
    assert!(s.team_member(2, tid, 1, None, 12).await.is_err());
    assert!(s.team_member(2, tid, 3, Some("admin"), 12).await.is_err());
    s.team_member(2, tid, 3, Some("member"), 12).await.unwrap();
    let category = s
        .save_catalogue(
            1,
            Catalog::Category,
            None,
            &json!({"name":"Shared","shared_with_team_id":tid}),
            13,
        )
        .await
        .unwrap();
    let cid = category["id"].as_i64().unwrap();
    assert!(s.catalogue_one(3, Catalog::Category, cid).await.is_ok());
    assert!(s.delete_catalogue(3, Catalog::Category, cid).await.is_err());
    let folder = s
        .save_catalogue(
            1,
            Catalog::Folder,
            None,
            &json!({"name":"Projects","is_pinned":true,"sort_order":7}),
            14,
        )
        .await
        .unwrap();
    let fid = folder["id"].as_i64().unwrap();
    assert_eq!(folder["is_pinned"], true);
    assert_eq!(folder["sort_order"], 7);
    assert!(
        s.save_catalogue(
            2,
            Catalog::Folder,
            None,
            &json!({"name":"Bad","parent_id":fid}),
            15
        )
        .await
        .is_err()
    );
    assert!(
        s.save_catalogue(1, Catalog::Folder, Some(fid), &json!({"parent_id":fid}), 15)
            .await
            .is_err()
    );
    let rule=s.save_catalogue(1,Catalog::Rule,None,&json!({"name":"archive video","trigger":"item_ingested","conditions":[{"field":"kind","op":"eq","value":"video"}],"actions":[{"type":"archive","params":{}}]}),16).await.unwrap();
    assert_eq!(rule["enabled"], true);
    let item = s
        .ingest(1, "https://youtu.be/video", "web", 17)
        .await
        .unwrap();
    assert_eq!(s.item(1, item.item_id).await.unwrap()["status"], "archived");
    s.team_member(1, tid, 3, None, 18).await.unwrap();
    assert!(s.catalogue_one(3, Catalog::Category, cid).await.is_err());
}
#[tokio::test]
async fn stable_feed_and_filters() {
    use yoink_inbox::feed::Filters;
    let (_d, s) = fixture().await;
    for n in 0..3 {
        s.ingest(1, &format!("https://example.com/{n}"), "web", 10)
            .await
            .unwrap();
    }
    let f = s
        .feed(
            1,
            &Filters {
                limit: Some(2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(f["items"].as_array().unwrap().len(), 2);
    let last = s
        .feed(
            1,
            &Filters {
                limit: Some(2),
                cursor: f["next_cursor"].as_str().map(str::to_owned),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(last["items"].as_array().unwrap().len(), 1);
    let id = f["items"][0]["id"].as_i64().unwrap();
    s.archive(1, id, false, 20).await.unwrap();
    let f = s.feed(1, &Filters::default()).await.unwrap();
    assert_eq!(f["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn retry_and_error_state_are_durable() {
    let (_d, s) = fixture().await;
    s.enqueue(1, None, "stars_sync", 1).await.unwrap();
    let j = s.claim(1, 100, "first").await.unwrap().unwrap();
    assert!(s.fail_job(&j, 2, "rate_limited", Some(10)).await.unwrap());
    assert!(s.claim(9, 100, "early").await.unwrap().is_none());
    let j = s.claim(10, 100, "second").await.unwrap().unwrap();
    assert_eq!(j.attempts, 2);
    assert!(s.fail_job(&j, 11, "no_token", None).await.unwrap());
    let r = query("SELECT last_status FROM inbox_gh_sync_state WHERE user_id=1")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(r.try_get::<String, _>("last_status").unwrap(), "no_token");
    assert!(s.claim(12, 100, "done").await.unwrap().is_none());
    s.set_hint(1, Some("personal"), 12).await.unwrap();
    assert_eq!(s.hint(1).await.unwrap().as_deref(), Some("personal"));
    assert!(s.hint(2).await.unwrap().is_none());
    s.set_hint(1, None, 13).await.unwrap();
    assert!(s.hint(1).await.unwrap().is_none());
}

#[tokio::test]
async fn linked_list_snapshot_retains_pins_and_detaches_removed() {
    use yoink_inbox::{catalog::Catalog, lists::ListSnapshot};
    let (_d, s) = fixture().await;
    s.enqueue(1, None, "stars_sync", 1).await.unwrap();
    let j = s.claim(1, 100, "one").await.unwrap().unwrap();
    let snap = StarSnapshot {
        rows: vec![
            json!({"repo":{"id":10,"full_name":"a/b","owner":{"login":"a"},"html_url":"https://github.com/a/b"}}),
        ],
        etag: None,
        not_modified: false,
        can_unstar: true,
    };
    let list = ListSnapshot {
        id: "list1".into(),
        name: "Projects".into(),
        slug: "projects".into(),
        description: None,
        items: vec![(10, "node1".into())],
    };
    s.apply_stars_with_lists(&j, &snap, Some(&[list]), 2)
        .await
        .unwrap();
    let folders = s.catalogue(1, Catalog::Folder).await.unwrap();
    assert_eq!(folders[0]["gh_list_id"], "list1");
    assert_eq!(folders[0]["star_count"], 1);
    let id = folders[0]["id"].as_i64().unwrap();
    s.save_catalogue(1, Catalog::Folder, Some(id), &json!({"is_pinned":true}), 3)
        .await
        .unwrap();
    s.enqueue(1, None, "stars_sync", 4).await.unwrap();
    let j = s.claim(4, 100, "two").await.unwrap().unwrap();
    s.apply_stars_with_lists(&j, &snap, Some(&[]), 5)
        .await
        .unwrap();
    let f = s.catalogue_one(1, Catalog::Folder, id).await.unwrap();
    assert_eq!(f["is_pinned"], true);
    assert!(f["gh_list_id"].is_null());
}

#[tokio::test]
async fn imported_pending_backfill_is_idempotent_and_excludes_archive() {
    let (_d, s) = fixture().await;
    let pending = s
        .ingest(1, "https://example.com/pending", "web", 1)
        .await
        .unwrap()
        .item_id;
    let enriched = s
        .ingest(1, "https://example.com/enriched", "web", 1)
        .await
        .unwrap()
        .item_id;
    let archived = s
        .ingest(1, "https://example.com/archived", "web", 1)
        .await
        .unwrap()
        .item_id;
    query("UPDATE inbox_items SET status='enriched' WHERE id=?")
        .bind(enriched)
        .execute(&s.db)
        .await
        .unwrap();
    s.archive(1, archived, false, 2).await.unwrap();
    query("DELETE FROM inbox_jobs")
        .execute(&s.db)
        .await
        .unwrap();
    for _ in 0..2 {
        s.db.batch(include_str!("../sql/backfill_jobs.sql"))
            .await
            .unwrap();
    }
    let rows = query("SELECT item_id,kind FROM inbox_jobs ORDER BY item_id")
        .fetch_all(&s.db)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].try_get::<i64, _>("item_id").unwrap(), pending);
    assert_eq!(rows[0].try_get::<String, _>("kind").unwrap(), "enrich");
    assert_eq!(rows[1].try_get::<i64, _>("item_id").unwrap(), enriched);
    assert_eq!(rows[1].try_get::<String, _>("kind").unwrap(), "classify");
}
