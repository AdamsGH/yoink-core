mod support;
use std::time::Duration;
use yoink_db::query;
use yoink_inbox::{
    Error,
    contracts::{Call, Content, ContentProvider, Insight},
    github::GithubClient,
    worker::Worker,
};
struct Unused;
impl ContentProvider for Unused {
    fn extract<'a>(&'a self, _: i64, _: &'a str) -> Call<'a, Content> {
        Box::pin(async { panic!("stars job must not fetch content") })
    }
}
struct Failure(&'static str);
impl Insight for Failure {
    fn github_token(&self, _: i64, _: bool) -> Call<'_, Option<String>> {
        Box::pin(async move {
            Err(match self.0 {
                "terminal" => Error::Completion("auth_failed"),
                "cancelled" => Error::Cancelled,
                "denied" => Error::Forbidden,
                "rate" => Error::RateLimited,
                _ => Error::External,
            })
        })
    }
    fn complete<'a>(&'a self, _: i64, _: &'a str) -> Call<'a, String> {
        Box::pin(async { panic!("unused") })
    }
    fn language(&self, _: i64) -> Call<'_, String> {
        Box::pin(async { panic!("unused") })
    }
}
#[tokio::test]
async fn actual_worker_reports_terminal_retry_and_shutdown_distinctly() {
    for (kind, retry, status) in [
        ("terminal", false, "auth_failed"),
        ("denied", false, "auth_failed"),
        ("rate", true, "rate_limited"),
        ("transport", true, "error"),
    ] {
        let (_dir, store) = support::db().await;
        store.enqueue(1, None, "stars_sync", 1).await.unwrap();
        let failure = Failure(kind);
        let github = GithubClient::new(Duration::from_secs(1), None).unwrap();
        let worker = Worker {
            store: &store,
            auth: &support::Auth,
            insight: &failure,
            content: &Unused,
            github: &github,
            lease_micros: 100,
            max_attempts: 3,
            retry_micros: 10,
        };
        assert!(worker.tick(|| 2).await.unwrap());
        let row = query("SELECT last_status FROM inbox_gh_sync_state WHERE user_id=1")
            .fetch_one(&store.db)
            .await
            .unwrap();
        assert_eq!(row.try_get::<String, _>("last_status").unwrap(), status);
        assert_eq!(store.claim(12, 100, "next").await.unwrap().is_some(), retry);
    }
}
#[test]
fn terminal_and_shutdown_http_contract() {
    use axum::{http::StatusCode, response::IntoResponse};
    assert_eq!(
        Error::Completion("auth_failed").into_response().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        Error::Cancelled.into_response().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        Error::Forbidden.into_response().status(),
        StatusCode::FORBIDDEN
    );
}

struct Classifier(bool);
impl Insight for Classifier {
    fn github_token(&self, _: i64, _: bool) -> Call<'_, Option<String>> {
        Box::pin(async { panic!("classify must not request GitHub") })
    }
    fn complete<'a>(&'a self, _: i64, _: &'a str) -> Call<'a, String> {
        Box::pin(async move {
            if self.0 {
                Err(Error::Cancelled)
            } else {
                Ok(r#"{"summary":"eventual real success","categories":[]}"#.into())
            }
        })
    }
    fn language(&self, _: i64) -> Call<'_, String> {
        Box::pin(async { Ok("en".into()) })
    }
}
async fn classify_fixture() -> (tempfile::TempDir, yoink_inbox::store::Store, i64) {
    let (dir, store) = support::db().await;
    let id = store
        .ingest(1, "https://example.com/interrupted", "web", 1)
        .await
        .unwrap()
        .item_id;
    let enrich = store.claim(1, 100, "enrich").await.unwrap().unwrap();
    store
        .apply_content(
            &enrich,
            &Content {
                content_text: Some("Fixture facts".into()),
                ..Default::default()
            },
            2,
        )
        .await
        .unwrap();
    (dir, store, id)
}
async fn job_state(
    store: &yoink_inbox::store::Store,
    kind: &str,
) -> (
    String,
    i64,
    i64,
    Option<String>,
    Option<i64>,
    Option<String>,
) {
    let r = query("SELECT state,attempts,available_at,lease_token,lease_until,last_error FROM inbox_jobs WHERE kind=?").bind(kind).fetch_one(&store.db).await.unwrap();
    (
        r.try_get("state").unwrap(),
        r.try_get("attempts").unwrap(),
        r.try_get("available_at").unwrap(),
        r.try_get("lease_token").unwrap(),
        r.try_get("lease_until").unwrap(),
        r.try_get("last_error").unwrap(),
    )
}
#[tokio::test]
async fn final_attempt_repeated_classify_interruptions_preserve_item_then_succeed() {
    let (_dir, store, id) = classify_fixture().await;
    // Two real failed attempts already consumed; the next claim is the final one.
    query("UPDATE inbox_jobs SET attempts=2,last_error='prior real failure' WHERE kind='classify'")
        .execute(&store.db)
        .await
        .unwrap();
    let before = store.item(1, id).await.unwrap();
    let github = GithubClient::new(Duration::from_secs(1), None).unwrap();
    let cancelled = Classifier(true);
    let mut worker = Worker {
        store: &store,
        auth: &support::Auth,
        insight: &cancelled,
        content: &Unused,
        github: &github,
        lease_micros: 100,
        max_attempts: 3,
        retry_micros: 10,
    };
    for now in [3, 13, 23, 33] {
        assert!(worker.tick(|| now).await.unwrap());
        assert_eq!(
            job_state(&store, "classify").await,
            (
                "ready".into(),
                2,
                now + 10,
                None,
                None,
                Some("prior real failure".into())
            )
        );
        assert_eq!(store.item(1, id).await.unwrap(), before);
        assert!(!worker.tick(|| now + 9).await.unwrap());
    }
    let success = Classifier(false);
    worker.insight = &success;
    assert!(worker.tick(|| 43).await.unwrap());
    let state = job_state(&store, "classify").await;
    assert_eq!(
        (state.0, state.1, state.3, state.4),
        ("done".into(), 3, None, None)
    );
    let item = store.item(1, id).await.unwrap();
    assert_eq!(item["llm_status"], "success");
    assert_eq!(item["status"], "classified");
    assert_eq!(item["summary"], "eventual real success");
}
#[tokio::test]
async fn stars_interruption_never_creates_or_overwrites_failure_metadata() {
    let (_dir, store) = support::db().await;
    store.enqueue(1, None, "stars_sync", 1).await.unwrap();
    let cancelled = Failure("cancelled");
    let github = GithubClient::new(Duration::from_secs(1), None).unwrap();
    let worker = Worker {
        store: &store,
        auth: &support::Auth,
        insight: &cancelled,
        content: &Unused,
        github: &github,
        lease_micros: 100,
        max_attempts: 1,
        retry_micros: 10,
    };
    assert!(worker.tick(|| 2).await.unwrap());
    assert!(
        query("SELECT * FROM inbox_gh_sync_state WHERE user_id=1")
            .fetch_optional(&store.db)
            .await
            .unwrap()
            .is_none()
    );
    query("INSERT INTO inbox_gh_sync_state(user_id,last_status,last_error,created_at,updated_at) VALUES(1,'success',NULL,1,1)").execute(&store.db).await.unwrap();
    for now in [12, 22] {
        assert!(worker.tick(|| now).await.unwrap());
        assert_eq!(
            job_state(&store, "stars_sync").await,
            ("ready".into(), 0, now + 10, None, None, None)
        );
        let r = query(
            "SELECT last_status,last_error,updated_at FROM inbox_gh_sync_state WHERE user_id=1",
        )
        .fetch_one(&store.db)
        .await
        .unwrap();
        assert_eq!(r.try_get::<String, _>("last_status").unwrap(), "success");
        assert!(
            r.try_get::<Option<String>, _>("last_error")
                .unwrap()
                .is_none()
        );
        assert_eq!(r.try_get::<i64, _>("updated_at").unwrap(), 1);
    }
}
#[tokio::test]
async fn interrupted_store_outcomes_are_lease_fenced_and_idempotent() {
    let (_dir, store, id) = classify_fixture().await;
    let before = store.item(1, id).await.unwrap();
    let stale = store.claim(3, 10, "old").await.unwrap().unwrap();
    // At the exact expiry boundary, the old outcome cannot restore an attempt.
    assert!(
        !store
            .fail_job(&stale, 13, "cancelled", Some(23))
            .await
            .unwrap()
    );
    let current = store.claim(13, 100, "new").await.unwrap().unwrap();
    assert_eq!(current.attempts, 2);
    let running = job_state(&store, "classify").await;
    assert!(
        !store
            .fail_job(&stale, 14, "cancelled", Some(24))
            .await
            .unwrap()
    );
    assert_eq!(job_state(&store, "classify").await, running);
    assert!(
        store
            .fail_job(&current, 14, "cancelled", Some(24))
            .await
            .unwrap()
    );
    let ready = job_state(&store, "classify").await;
    assert_eq!(ready, ("ready".into(), 1, 24, None, None, None));
    assert!(
        !store
            .fail_job(&current, 15, "cancelled", Some(25))
            .await
            .unwrap()
    );
    assert_eq!(job_state(&store, "classify").await, ready);
    let next = store.claim(24, 100, "third").await.unwrap().unwrap();
    let running = job_state(&store, "classify").await;
    assert!(
        !store
            .fail_job(&current, 25, "cancelled", Some(35))
            .await
            .unwrap()
    );
    assert_eq!(job_state(&store, "classify").await, running);
    assert!(
        store
            .apply_classification(
                &next,
                &yoink_inbox::pipeline::parse(r#"{"summary":"success"}"#),
                25
            )
            .await
            .unwrap()
    );
    let done = job_state(&store, "classify").await;
    assert!(
        !store
            .fail_job(&next, 26, "cancelled", Some(36))
            .await
            .unwrap()
    );
    assert_eq!(job_state(&store, "classify").await, done);
    assert_eq!(before["llm_status"], serde_json::Value::Null);
    assert_eq!(store.item(1, id).await.unwrap()["llm_status"], "success");
}
#[tokio::test]
async fn cancellation_requires_future_retry_and_keeps_real_error_budget() {
    let (_dir, store) = support::db().await;
    store.enqueue(1, None, "stars_sync", 1).await.unwrap();
    let claim = store.claim(2, 100, "initial").await.unwrap().unwrap();
    let running = job_state(&store, "stars_sync").await;
    for retry in [None, Some(2), Some(1)] {
        assert!(matches!(
            store.fail_job(&claim, 2, "cancelled", retry).await,
            Err(Error::Invalid)
        ));
        assert_eq!(job_state(&store, "stars_sync").await, running);
    }
    assert!(store.fail_job(&claim, 2, "error", Some(12)).await.unwrap());
    let github = GithubClient::new(Duration::from_secs(1), None).unwrap();
    let cancelled = Failure("cancelled");
    let mut worker = Worker {
        store: &store,
        auth: &support::Auth,
        insight: &cancelled,
        content: &Unused,
        github: &github,
        lease_micros: 100,
        max_attempts: 2,
        retry_micros: 10,
    };
    assert!(worker.tick(|| 12).await.unwrap());
    assert_eq!(job_state(&store, "stars_sync").await.1, 1);
    let failure = Failure("transport");
    worker.insight = &failure;
    assert!(worker.tick(|| 22).await.unwrap());
    let state = job_state(&store, "stars_sync").await;
    assert_eq!((state.0, state.1), ("failed".into(), 2));
    assert!(!worker.tick(|| 32).await.unwrap());
}
