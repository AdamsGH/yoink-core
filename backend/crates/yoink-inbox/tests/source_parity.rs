mod support;
use serde_json::{Value, json};
use std::sync::Mutex;
use yoink_db::query;
use yoink_inbox::{
    Error,
    catalog::Catalog,
    contracts::Call,
    feed::Filters,
    github::{Github, StarSnapshot},
    github_content::{self, Body},
    stars::StarFilters,
    validation,
};
fn oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/source.json")).unwrap()
}
struct Provider {
    fixture: Value,
    requests: Mutex<Vec<String>>,
}
impl Github for Provider {
    fn star<'a>(
        &'a self,
        _token: &'a str,
        _owner: &'a str,
        _repo: &'a str,
        _star: bool,
    ) -> Call<'a, ()> {
        Box::pin(async { Err(Error::External) })
    }
    fn is_starred<'a>(
        &'a self,
        _token: &'a str,
        _owner: &'a str,
        _repo: &'a str,
    ) -> Call<'a, bool> {
        Box::pin(async { Err(Error::External) })
    }
    fn stars<'a>(&'a self, _token: &'a str, _etag: Option<&'a str>) -> Call<'a, StarSnapshot> {
        Box::pin(async { Err(Error::External) })
    }
    fn graphql<'a>(
        &'a self,
        _token: &'a str,
        _query: &'a str,
        _vars: Value,
        _mutation: bool,
    ) -> Call<'a, Value> {
        Box::pin(async { Err(Error::External) })
    }
    fn raw_file<'a>(&'a self, token: Option<&'a str>, path: &'a str) -> Call<'a, String> {
        Box::pin(async move {
            assert_eq!(token, Some("fixture-secret"));
            self.requests.lock().unwrap().push(path.into());
            self.fixture["raw_responses"][path]
                .as_str()
                .map(str::to_owned)
                .ok_or(Error::NotFound)
        })
    }
    fn get_json<'a>(&'a self, token: Option<&'a str>, path: &'a str) -> Call<'a, Value> {
        Box::pin(async move {
            assert_eq!(token, Some("fixture-secret"));
            self.requests.lock().unwrap().push(path.into());
            let value = &self.fixture["github_responses"][path];
            if value.is_null() {
                Err(Error::NotFound)
            } else {
                Ok(value.clone())
            }
        })
    }
}
#[tokio::test]
async fn preserved_source_github_routes_and_content_projections() {
    let f = oracle();
    let g = Provider {
        fixture: f.clone(),
        requests: Mutex::new(vec![]),
    };
    for case in f["github"].as_array().unwrap() {
        let url = case["url"].as_str().unwrap();
        let route = github_content::parse(url);
        if case["route"].is_null() {
            assert!(route.is_none(), "{url}");
            continue;
        }
        let route = route.unwrap();
        let body = github_content::fetch(&g, Some("fixture-secret"), &route).await;
        if case["error"] == true {
            assert!(body.is_err(), "{url}");
            continue;
        }
        let expected = case["fallback"].as_str().unwrap();
        match body.unwrap() {
            Body::Plain(s) => assert_eq!(s, expected, "{url}"),
            Body::Structured { prefix, value } => {
                assert!(expected.starts_with(&prefix), "{url}");
                assert_eq!(
                    value,
                    serde_json::from_str::<Value>(&expected[prefix.len()..]).unwrap(),
                    "{url}"
                );
                assert!(!case["toon"].as_str().unwrap().is_empty());
            }
        }
    }
    let paths = g.requests.lock().unwrap();
    assert!(paths.iter().any(|p| p == "users/a/repos?per_page=2"));
    assert!(
        paths
            .iter()
            .any(|p| p == "repos/a/b/pulls/1/comments?per_page=20")
    );
    assert!(
        !paths
            .iter()
            .any(|p| p.contains("compare/") || p.contains("download/") || p.contains("commit/"))
    );
}
#[test]
fn preserved_pydantic_coercions_defaults_limits_and_patch_fields() {
    for case in oracle()["validation"].as_array().unwrap() {
        let name = case["schema"].as_str().unwrap();
        let b = &case["body"];
        let got = match name {
            "InboxCategoryCreate" => validation::catalogue(b, Catalog::Category, true),
            "InboxGhFolderCreate" => validation::catalogue(b, Catalog::Folder, true),
            "InboxGhFolderPatch" => validation::folder_patch(b),
            "InboxRuleCreate" => validation::catalogue(b, Catalog::Rule, true),
            "InboxRuleUpdate" => validation::catalogue(b, Catalog::Rule, false),
            "InboxTeamCreate" => validation::team(b),
            _ => panic!("unexpected schema"),
        };
        // Source accepts enabled=null but subsequently violates native NOT NULL;
        // module contract returns 422 rather than a persistence failure/default.
        if name == "InboxRuleUpdate" && b["enabled"].is_null() && b.get("enabled").is_some() {
            assert!(got.is_err());
            continue;
        }
        if case["error"] == true {
            assert!(got.is_err(), "{case}");
            continue;
        }
        let got = got.unwrap_or_else(|e| panic!("{case}: {e}"));
        for (k, v) in got.as_object().unwrap() {
            assert_eq!(v, &case["expected"][k], "{name} {b} field {k}")
        }
    }
    assert!(validation::bounded_optional(Some(&"🙂".repeat(4001)), 4000).is_err());
    assert!(validation::bounded_optional(Some(&"🙂".repeat(4000)), 4000).is_ok());
}
#[tokio::test]
async fn unicode_ilike_patterns_filter_before_keyset_and_offset_pagination() {
    let (_d, s) = support::db().await;
    let mut ids = vec![];
    for (n, title) in ["ПРИВЕТ", "not matching", "привет", "Café_100%", "CaféX100z"]
        .iter()
        .enumerate()
    {
        let id = s
            .ingest(1, &format!("https://example.com/{n}"), "api", n as i64 + 1)
            .await
            .unwrap()
            .item_id;
        query("UPDATE inbox_items SET title=? WHERE id=?")
            .bind(title)
            .bind(id)
            .execute(&s.db)
            .await
            .unwrap();
        ids.push(id);
    }
    let f = Filters {
        search: Some("прИв".into()),
        limit: Some(1),
        ..Default::default()
    };
    let first = s.feed(1, &f).await.unwrap();
    assert_eq!(first["items"][0]["id"], ids[2]);
    let second = s
        .feed(
            1,
            &Filters {
                cursor: first["next_cursor"].as_str().map(str::to_owned),
                ..f
            },
        )
        .await
        .unwrap();
    assert_eq!(second["items"][0]["id"], ids[0]);
    assert!(second["next_cursor"].is_null());
    let escaped = s
        .feed(
            1,
            &Filters {
                search: Some(r"Café\_100\%".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(escaped["items"].as_array().unwrap().len(), 1);
    for (n, name) in ["ПРИВЕТ/b", "other/x", "привет/a"].iter().enumerate() {
        query("INSERT INTO inbox_gh_stars(user_id,gh_repo_id,full_name,owner_login,html_url,last_synced_at,starred_at) VALUES(1,?,?,?,?,1,?)").bind(n as i64+100).bind(name).bind("user").bind("https://github.com/a/b").bind(n as i64).execute(&s.db).await.unwrap();
    }
    let page = s
        .stars(
            1,
            &StarFilters {
                search: Some("прИв".into()),
                sort: Some("name".into()),
                offset: Some(1),
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page["items"][0]["full_name"], "привет/a");
    assert!(
        s.feed(
            1,
            &Filters {
                search: Some("x".into()),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    assert!(
        s.stars(
            1,
            &StarFilters {
                limit: Some(101),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn source_ascii_slug_fallback_truncation_owner_scoping_and_bounded_suffix() {
    let (_d, s) = support::db().await;
    assert_eq!(yoink_inbox::normalize::slug("Café 日本語 API"), "caf-api");
    let a = s
        .save_catalogue(1, Catalog::Folder, None, &json!({"name":"日本語"}), 1)
        .await
        .unwrap();
    let b = s
        .save_catalogue(1, Catalog::Folder, None, &json!({"name":"日本語"}), 2)
        .await
        .unwrap();
    assert_eq!(a["slug"], "folder");
    assert_eq!(b["slug"], "folder-2");
    assert_eq!(
        s.save_catalogue(2, Catalog::Folder, None, &json!({"name":"日本語"}), 2)
            .await
            .unwrap()["slug"],
        "folder"
    );
    let long = "a".repeat(64);
    s.save_catalogue(
        1,
        Catalog::Folder,
        None,
        &json!({"name":"Long","slug":long}),
        3,
    )
    .await
    .unwrap();
    // Source truncates BEFORE checking suffix; all -2..-19 collapse to this
    // same candidate. Its final fallback is base+'-x' (66 chars), not invented
    // lossless suffix truncation or an unbounded uniqueness loop.
    assert!(matches!(
        s.save_catalogue(
            1,
            Catalog::Folder,
            None,
            &json!({"name":"Long2","slug":long}),
            4
        )
        .await,
        Err(Error::Invalid)
    ));
}

#[test]
fn provider_decoration_is_exact_preserved_source_output() {
    let f = oracle();
    assert_eq!(
        yoink_inbox::enrich::strip_provider_chrome(f["chrome"]["input"].as_str().unwrap()),
        f["chrome"]["expected"].as_str().unwrap()
    );
}
