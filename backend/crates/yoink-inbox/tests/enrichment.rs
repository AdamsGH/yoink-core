mod support;
use serde_json::json;
use std::{collections::BTreeMap, sync::Mutex};
use support::*;
use yoink_db::query;
use yoink_inbox::{
    Error,
    catalog::Catalog,
    contracts::Call,
    enrich::{Extraction, FallbackContent, Stages},
    github::StarSnapshot,
    store::Store,
};
struct RawStages {
    denied_at: Option<String>,
    cancelled_at: Option<&'static str>,
    base: url::Url,
    client: reqwest::Client,
    readability: Option<Extraction>,
    trafilatura: Option<String>,
    steps: Mutex<Vec<&'static str>>,
}
impl RawStages {
    fn new(f: &Fixture) -> Self {
        Self {
            denied_at: None,
            cancelled_at: None,
            base: f.endpoint(),
            client: reqwest::Client::new(),
            readability: None,
            trafilatura: None,
            steps: Mutex::new(vec![]),
        }
    }
}
impl Stages for RawStages {
    fn fetch<'a>(
        &'a self,
        _user: i64,
        url: &'a str,
        headers: &'a BTreeMap<String, String>,
    ) -> Call<'a, String> {
        Box::pin(async move {
            assert!(!headers.contains_key("Authorization"));
            if self.cancelled_at == Some(url) {
                return Err(Error::Cancelled);
            }
            if self.denied_at.as_deref() == Some(url) {
                return Err(Error::Forbidden);
            }
            let mut endpoint = self.base.join("content").unwrap();
            endpoint.query_pairs_mut().append_pair("target", url);
            let mut request = self.client.get(endpoint);
            for (k, v) in headers {
                request = request.header(k, v)
            }
            let response = request.send().await.map_err(|_| Error::External)?;
            if !response.status().is_success() {
                return Err(Error::External);
            }
            response.text().await.map_err(|_| Error::External)
        })
    }
    fn readability<'a>(&'a self, _html: &'a str, _url: &'a str) -> Call<'a, Extraction> {
        Box::pin(async {
            self.steps.lock().unwrap().push("readability");
            if self.cancelled_at == Some("readability") {
                return Err(Error::Cancelled);
            }
            self.readability.clone().ok_or(Error::External)
        })
    }
    fn trafilatura<'a>(&'a self, _html: &'a str) -> Call<'a, String> {
        Box::pin(async {
            self.steps.lock().unwrap().push("trafilatura");
            if self.cancelled_at == Some("trafilatura") {
                return Err(Error::Cancelled);
            }
            self.trafilatura.clone().ok_or(Error::External)
        })
    }
}
fn engine<'a>(stages: &'a RawStages, f: &'a Fixture) -> FallbackContent<'a> {
    FallbackContent {
        stages,
        github: &f.client,
        insight: &Keys,
        max_chars: 400,
    }
}
#[tokio::test]
async fn llms_readability_and_original_og_seed() {
    let f = Fixture::new().await;
    {
        let mut r = f.state.lock().await;
        r.pages.insert("https://example.com/llms.txt".into(),"# Documentation links and a sufficiently long intro\n[Article](https://example.com/article.md)".into());
        r.pages.insert(
            "https://example.com/article.md".into(),
            "<article>primary HTML</article>".into(),
        );
        r.pages.insert("https://example.com/article".into(),"<meta property='og:description' content='seed'><meta name='author' content='Ada'><link rel='icon' href='/i.png'>".into());
    }
    let mut stages = RawStages::new(&f);
    stages.readability = Some(Extraction {
        text: "article text ".repeat(50),
        title: Some("Readable".into()),
    });
    let c = engine(&stages, &f)
        .extract_content(1, "https://example.com/article")
        .await
        .unwrap();
    assert_eq!(c.title.as_deref(), Some("Readable"));
    assert_eq!(c.summary_seed.as_deref(), Some("seed"));
    assert_eq!(c.author.as_deref(), Some("Ada"));
    assert_eq!(c.favicon_url.as_deref(), Some("https://example.com/i.png"));
    assert_eq!(c.crawl_status.as_deref(), Some("success"));
    assert!(c.content_text.unwrap().ends_with("[Content truncated]"));
    assert_eq!(*stages.steps.lock().unwrap(), vec!["readability"]);
    assert_eq!(
        f.state.lock().await.raw_requests,
        vec![
            "https://example.com/llms.txt",
            "https://example.com/article.md",
            "https://example.com/article"
        ]
    );
}
#[tokio::test]
async fn trafilatura_then_provider_chrome_then_raw_then_failed() {
    let f = Fixture::new().await;
    f.state.lock().await.pages.insert(
        "https://example.com/a".into(),
        format!(
            "<title>Raw title</title><article>{}</article>",
            "Raw body words ".repeat(50)
        ),
    );
    let mut stages = RawStages::new(&f);
    stages.readability = Some(Extraction {
        text: "short".into(),
        title: None,
    });
    stages.trafilatura = Some("trafilatura content ".repeat(30));
    let c = engine(&stages, &f)
        .extract_content(1, "https://example.com/a")
        .await
        .unwrap();
    assert!(c.content_text.unwrap().starts_with("trafilatura"));
    assert_eq!(
        *stages.steps.lock().unwrap(),
        vec!["readability", "trafilatura"]
    );
    stages.trafilatura = None;
    {
        let mut r = f.state.lock().await;
        r.pages.insert(
            "https://r.jina.ai/https://example.com/a".into(),
            format!("---\n{}\n---\n---", "metadata:".repeat(30)),
        );
        r.pages.insert(
            "https://curl.md/https://example.com/a".into(),
            format!(
                "---\nmetadata: hidden\n---\n{}\n\n---\nPowered by [curl.md](https://curl.md)",
                "Useful article body ".repeat(30)
            ),
        );
    }
    let c = engine(&stages, &f)
        .extract_content(1, "https://example.com/a")
        .await
        .unwrap();
    assert!(c.content_text.unwrap().starts_with("Useful article"));
    {
        let mut r = f.state.lock().await;
        r.pages.remove("https://r.jina.ai/https://example.com/a");
        r.pages.remove("https://curl.md/https://example.com/a");
    }
    let c = engine(&stages, &f)
        .extract_content(1, "https://example.com/a")
        .await
        .unwrap();
    assert!(c.content_text.unwrap().contains("Raw body words"));
    assert_eq!(c.title.as_deref(), Some("Raw title"));
    f.state.lock().await.pages.insert(
        "https://example.com/a".into(),
        "<meta property='og:description' content='Still seed'>".into(),
    );
    let c = engine(&stages, &f)
        .extract_content(1, "https://example.com/a")
        .await
        .unwrap();
    assert_eq!(c.crawl_status.as_deref(), Some("failed"));
    assert_eq!(c.summary_seed.as_deref(), Some("Still seed"));
}
#[tokio::test]
async fn github_api_readme_and_provider_failure_fallback_never_sends_token_to_providers() {
    let f = Fixture::new().await;
    let stages = RawStages::new(&f);
    let c = engine(&stages, &f)
        .extract_content(1, "https://github.com/a/b")
        .await
        .unwrap();
    assert_eq!(c.github_repo_id, Some(10));
    assert_eq!(c.title.as_deref(), Some("a/b"));
    assert_eq!(c.summary_seed.as_deref(), Some("Repo seed"));
    assert!(c.content_text.unwrap().contains("Readme"));
    assert!(f.state.lock().await.raw_requests.is_empty());
    {
        let mut r = f.state.lock().await;
        r.github_unavailable = true;
        r.pages.insert(
            "https://github.com/a/b".into(),
            "<article>GitHub shell should never be body</article>".repeat(30),
        );
        r.pages.insert(
            "https://r.jina.ai/https://github.com/a/b".into(),
            "Provider content ".repeat(40),
        );
    }
    let c = engine(&stages, &f)
        .extract_content(1, "https://github.com/a/b")
        .await
        .unwrap();
    assert!(c.content_text.unwrap().starts_with("Provider content"));
    assert!(stages.steps.lock().unwrap().is_empty());
    assert!(
        !f.state
            .lock()
            .await
            .raw_requests
            .iter()
            .any(|u| u.ends_with("/llms.txt"))
    );
    f.state
        .lock()
        .await
        .pages
        .remove("https://r.jina.ai/https://github.com/a/b");
    let c = engine(&stages, &f)
        .extract_content(1, "https://github.com/a/b")
        .await
        .unwrap();
    assert_eq!(c.crawl_status.as_deref(), Some("failed"));
    assert!(c.content_text.is_none());
}
async fn sync(s: &Store, rows: Vec<serde_json::Value>, now: i64) {
    s.enqueue(1, None, "stars_sync", now).await.unwrap();
    let mut j = s.claim(now, 100, "sync").await.unwrap().unwrap();
    while j.kind != "stars_sync" {
        s.finish(&j, now, None).await.unwrap();
        j = s.claim(now, 100, "sync").await.unwrap().unwrap();
    }
    s.apply_stars(
        &j,
        &StarSnapshot {
            rows,
            etag: None,
            can_unstar: true,
            not_modified: false,
        },
        now + 1,
    )
    .await
    .unwrap();
}
#[tokio::test]
async fn bridge_before_and_after_sync_star_rules_language_null_and_manual_preservation() {
    let (_d, s) = db().await;
    let a = s
        .ingest(1, "https://github.com/A/B.git/tree/main", "web", 1)
        .await
        .unwrap()
        .item_id;
    let other = s
        .ingest(2, "https://github.com/a/b", "web", 1)
        .await
        .unwrap()
        .item_id;
    let cat = s
        .save_catalogue(1, Catalog::Category, None, &json!({"name":"Manual"}), 2)
        .await
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    s.assign_categories(1, a, &[cat], 3).await.unwrap();
    for (priority, lang, label) in [(1, "Rust", "Rust label"), (2, "Python", "Wrong label")] {
        s.save_catalogue(1,Catalog::Rule,None,&json!({"name":label,"priority":priority,"trigger":"star_synced","conditions":[{"field":"language","op":"eq","value":lang}],"actions":[{"type":"add_category","params":{"category_name":label}}]}),4).await.unwrap();
    }
    let row = json!({"repo":{"id":10,"full_name":"a/b","owner":{"login":"a"},"html_url":"https://github.com/a/b","language":"Rust"}});
    sync(&s, vec![row.clone()], 5).await;
    let bridge = query("SELECT item_id,gh_star_id FROM inbox_item_to_star")
        .fetch_all(&s.db)
        .await
        .unwrap();
    assert_eq!(bridge.len(), 1);
    assert_eq!(bridge[0].try_get::<i64, _>("item_id").unwrap(), a);
    let star: i64 = bridge[0].try_get("gh_star_id").unwrap();
    let i = s.item(1, a).await.unwrap();
    assert_eq!(i["categories"].as_array().unwrap().len(), 2);
    assert!(
        i["categories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["attached_by"] == "user")
    );
    assert!(
        s.item(2, other).await.unwrap()["categories"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    query("UPDATE inbox_gh_stars SET ai_summary='Keep',ai_labels='[\"Existing\",\"Rust label\"]' WHERE id=?").bind(star).execute(&s.db).await.unwrap();
    let b = s
        .ingest(1, "https://github.com/a/b/issues/42", "web", 7)
        .await
        .unwrap()
        .item_id;
    let n: i64 = query("SELECT count(*) AS n FROM inbox_item_to_star WHERE item_id=?")
        .bind(b)
        .fetch_one(&s.db)
        .await
        .unwrap()
        .try_get("n")
        .unwrap();
    assert_eq!(n, 1);
    sync(&s, vec![row], 8).await;
    let r = query("SELECT ai_summary,ai_labels FROM inbox_gh_stars WHERE id=?")
        .bind(star)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(r.try_get::<String, _>("ai_summary").unwrap(), "Keep");
    assert_eq!(
        r.try_get::<serde_json::Value, _>("ai_labels").unwrap(),
        json!(["Existing", "Rust label"])
    );
    sync(&s, vec![], 10).await;
    let n: i64 = query("SELECT count(*) AS n FROM inbox_item_to_star")
        .fetch_one(&s.db)
        .await
        .unwrap()
        .try_get("n")
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(
        s.item(1, a).await.unwrap()["categories"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn content_projection_preserves_existing_card_and_links_numeric_repo_identity() {
    let (_d, s) = db().await;
    let star = seed_star(&s).await;
    let item = s
        .ingest(1, "https://github.com/renamed/alias", "web", 1)
        .await
        .unwrap()
        .item_id;
    query("UPDATE inbox_items SET summary='Manual summary',og_image_url='manual image',author='Manual author' WHERE id=?").bind(item).execute(&s.db).await.unwrap();
    let j = s.claim(1, 100, "content").await.unwrap().unwrap();
    let c = yoink_inbox::contracts::Content {
        title: Some("Repo".into()),
        summary_seed: Some("Seed".into()),
        og_image_url: Some("api avatar".into()),
        author: Some("OG author".into()),
        github_repo_id: Some(10),
        content_text: Some("Content".into()),
        crawl_status: Some("success".into()),
        ..Default::default()
    };
    s.apply_content(&j, &c, 2).await.unwrap();
    let i = s.item(1, item).await.unwrap();
    assert_eq!(i["summary"], "Manual summary");
    assert_eq!(i["og_image_url"], "manual image");
    assert_eq!(i["author"], "Manual author");
    let row = query("SELECT gh_star_id FROM inbox_item_to_star WHERE item_id=?")
        .bind(item)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(row.try_get::<i64, _>("gh_star_id").unwrap(), star);
}
#[tokio::test]
async fn star_rules_null_language_does_not_match_eq_and_archive_only_owned_links() {
    let (_d, s) = db().await;
    let item = s
        .ingest(1, "https://github.com/a/b", "web", 1)
        .await
        .unwrap()
        .item_id;
    let other = s
        .ingest(2, "https://github.com/a/b", "web", 1)
        .await
        .unwrap()
        .item_id;
    for (priority, condition, action) in [
        (
            1,
            json!({"field":"language","op":"eq","value":"Rust"}),
            json!({"type":"add_category","params":{"category_name":"Never"}}),
        ),
        (
            2,
            json!({"field":"language","op":"ne","value":"Rust"}),
            json!({"type":"archive","params":{}}),
        ),
    ] {
        s.save_catalogue(1,Catalog::Rule,None,&json!({"name":format!("rule{priority}"),"trigger":"star_synced","priority":priority,"conditions":[condition],"actions":[action]}),2).await.unwrap();
    }
    sync(&s,vec![json!({"repo":{"id":10,"full_name":"a/b","owner":{"login":"a"},"html_url":"https://github.com/a/b","language":null}})],3).await;
    assert_eq!(s.item(1, item).await.unwrap()["status"], "archived");
    assert_eq!(s.item(2, other).await.unwrap()["status"], "pending");
    assert!(
        s.item(1, item).await.unwrap()["categories"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn confirmed_reply_hook_is_replay_safe_owned_and_ban_checked() {
    use yoink_inbox::telegram::{ReplyTarget, bind_reply, reply_target};
    let (_d, s) = db().await;
    let item = s
        .ingest(1, "https://example.com/reply", "bot", 1)
        .await
        .unwrap()
        .item_id;
    let target = ReplyTarget {
        chat_id: -100,
        message_id: 42,
    };
    assert!(bind_reply(&s, &Auth, 1, item, target).await.unwrap());
    assert!(bind_reply(&s, &Auth, 1, item, target).await.unwrap());
    assert!(!bind_reply(&s, &Auth, 2, item, target).await.unwrap());
    assert!(
        !bind_reply(
            &s,
            &Auth,
            1,
            item,
            ReplyTarget {
                chat_id: 200,
                message_id: 43
            }
        )
        .await
        .unwrap()
    );
    assert_eq!(
        reply_target(&s, &Auth, 1, item).await.unwrap(),
        Some(target)
    );
    assert!(reply_target(&s, &Auth, 3, item).await.is_err());
    assert!(s.item(1, item).await.unwrap().get("tg_chat_id").is_none());
    s.archive(1, item, false, 2).await.unwrap();
    assert!(reply_target(&s, &Auth, 1, item).await.unwrap().is_none());
}

#[tokio::test]
async fn policy_denials_at_each_fetch_do_not_become_fallback_misses() {
    let f = Fixture::new().await;
    for denied in [
        "https://example.com/llms.txt",
        "https://example.com/a",
        "https://r.jina.ai/https://example.com/a",
        "https://curl.md/https://example.com/a",
    ] {
        let mut stages = RawStages::new(&f);
        stages.denied_at = Some(denied.into());
        assert!(
            matches!(
                engine(&stages, &f)
                    .extract_content(1, "https://example.com/a")
                    .await,
                Err(Error::Forbidden)
            ),
            "{denied}"
        );
    }
}

#[tokio::test]
async fn cancellation_at_each_fetch_and_extractor_stops_fallback() {
    let f = Fixture::new().await;
    f.state.lock().await.pages.insert(
        "https://example.com/a".into(),
        "<article>Fixture content</article>".into(),
    );
    for cancelled in [
        "https://example.com/llms.txt",
        "https://example.com/a",
        "https://r.jina.ai/https://example.com/a",
        "https://curl.md/https://example.com/a",
        "readability",
        "trafilatura",
    ] {
        let mut stages = RawStages::new(&f);
        stages.cancelled_at = Some(cancelled);
        assert!(
            matches!(
                engine(&stages, &f)
                    .extract_content(1, "https://example.com/a")
                    .await,
                Err(Error::Cancelled)
            ),
            "{cancelled}"
        );
    }
}
