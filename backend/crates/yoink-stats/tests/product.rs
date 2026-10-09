use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use yoink_db::query;
use yoink_stats::{
    Error, MemberAccess, MembershipFuture, MembershipProvider, SearchRequest, Stats,
};
use yoink_storage::Store;
struct Members {
    present: AtomicBool,
    admin: AtomicBool,
    name: std::sync::Mutex<Option<yoink_stats::UserName>>,
    hang_names: AtomicBool,
    pause_lookup: AtomicBool,
    lookup_started: tokio::sync::Notify,
    lookup_release: tokio::sync::Semaphore,
    pause_names: AtomicBool,
    names_started: tokio::sync::Notify,
    names_release: tokio::sync::Semaphore,
}
#[tokio::test]
async fn canonical_cross_bot_ordering_and_retained_reaction_dates() {
    let f = fixture().await;
    f.stats
        .observe_raw_update(10, message(1, 1000, "original"))
        .await
        .unwrap();
    f.stats
        .observe_raw_update(10, edit(2, 2000, Some("newer")))
        .await
        .unwrap();
    f.stats
        .observe_raw_update(20, edit(999, 1500, Some("stale")))
        .await
        .unwrap();
    f.stats
        .observe_raw_update(20, edit(1000, 2000, Some("cross-bot tie")))
        .await
        .unwrap();
    assert_eq!(
        query("SELECT text FROM stats_messages")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "newer"
    );
    f.stats
        .observe_raw_update(10, edit(3, 2000, Some("same-bot tie")))
        .await
        .unwrap();
    assert_eq!(
        query("SELECT text FROM stats_messages")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "same-bot tie"
    );
    let reaction = |id, date, keys: Vec<&str>| json!({"update_id":id,"message_reaction":{"chat":{"id":-100,"type":"supergroup"},"message_id":10,"date":date,"user":{"id":2,"first_name":"Alice"},"new_reaction":keys.into_iter().map(|k|json!({"type":"emoji","emoji":k})).collect::<Vec<_>>()}});
    f.stats
        .observe_raw_update(10, reaction(4, 3000, vec!["A"]))
        .await
        .unwrap();
    f.stats
        .observe_raw_update(20, reaction(1, 4000, vec!["A", "B"]))
        .await
        .unwrap();
    let rows = query("SELECT reaction_key,date FROM stats_reactions ORDER BY reaction_key")
        .fetch_all(f.store.database())
        .await
        .unwrap();
    assert_eq!(rows[0].try_get::<i64, _>(1).unwrap(), 3_000_000_000);
    assert_eq!(rows[1].try_get::<i64, _>(1).unwrap(), 4_000_000_000);
    f.stats
        .observe_raw_update(10, reaction(5, 5000, vec![]))
        .await
        .unwrap();
    f.stats
        .observe_raw_update(20, reaction(999, 4500, vec!["A"]))
        .await
        .unwrap();
    f.stats
        .observe_raw_update(20, reaction(1000, 5000, vec!["A"]))
        .await
        .unwrap();
    assert_eq!(count(&f, "stats_reactions").await, 0);
    f.stats
        .observe_raw_update(10, reaction(6, 5000, vec!["B"]))
        .await
        .unwrap();
    assert_eq!(count(&f, "stats_reactions").await, 1);
}

#[tokio::test]
async fn imported_midday_profiles_keep_history_and_persistent_freshness_floor() {
    let f = fixture().await;
    let day = chrono::Utc::now().date_naive();
    let noon = day.and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp();
    query("INSERT INTO stats_user_names(user_id,date,username,display_name) VALUES(2,?,'imported','Imported noon')")
        .bind(noon * 1_000_000)
        .execute(f.store.database())
        .await
        .unwrap();
    for (id, date) in [(1, noon - 3600), (2, noon - 1800)] {
        let mut v = message(id, date, "historical");
        v["message"]["from"]["username"] = json!("old");
        f.stats.observe_raw_update(10, v).await.unwrap();
    }
    assert_eq!(count(&f, "stats_user_names").await, 1);
    let mut newer = message(3, noon + 3600, "changed");
    newer["message"]["from"]["username"] = json!("afternoon");
    f.stats.observe_raw_update(10, newer).await.unwrap();
    assert_eq!(
        query("SELECT username FROM stats_user_latest_name WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "afternoon"
    );
    let mut tie = message(999, noon + 3600, "other bot");
    tie["message"]["message_id"] = json!(11);
    tie["message"]["from"]["username"] = json!("wrong-tie");
    f.stats.observe_raw_update(20, tie).await.unwrap();
    assert_eq!(
        query("SELECT username FROM stats_user_latest_name WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "afternoon"
    );
    assert_eq!(count(&f, "stats_user_names").await, 2);
    assert_eq!(
        query("SELECT event_floor FROM stats_name_baselines WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        noon * 1_000_000
    );
    *f.members.name.lock().unwrap() = Some(yoink_stats::UserName {
        user_id: 2,
        username: Some("refreshed".into()),
        display_name: Some("Refreshed".into()),
    });
    assert_eq!(
        f.stats
            .refresh_usernames(Duration::from_secs(1))
            .await
            .unwrap()
            .refreshed,
        1
    );
    assert_eq!(
        query("SELECT username FROM stats_user_latest_name WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "refreshed"
    );
    assert_eq!(count(&f, "stats_user_names").await, 3);
}

#[tokio::test]
async fn completed_name_refresh_blocks_delayed_profiles_without_changing_imports() {
    let f = fixture().await;
    let second = chrono::Utc::now().timestamp();
    let imported = (second - 120) * 1_000_000 + 123_000;
    query("INSERT INTO stats_user_names(user_id,date,username,display_name) VALUES(2,?,'alice','Alice')")
        .bind(imported).execute(f.store.database()).await.unwrap();
    f.stats
        .observe_raw_update(10, message(1, second - 120, "original"))
        .await
        .unwrap();
    f.stats
        .refresh_usernames(Duration::from_millis(10))
        .await
        .unwrap();
    assert_eq!(
        query("SELECT refresh_floor FROM stats_name_baselines WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<Option<i64>, _>(0)
            .unwrap(),
        None
    );
    f.members.hang_names.store(true, Ordering::SeqCst);
    f.stats
        .refresh_usernames(Duration::from_millis(10))
        .await
        .unwrap();
    f.members.hang_names.store(false, Ordering::SeqCst);
    assert_eq!(
        query("SELECT refresh_floor FROM stats_name_baselines WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<Option<i64>, _>(0)
            .unwrap(),
        None
    );
    let mut allowed = message(2, second - 90, "after failed lookup");
    allowed["message"]["message_id"] = json!(2);
    allowed["message"]["from"]["username"] = json!("after-failure");
    f.stats.observe_raw_update(10, allowed).await.unwrap();
    assert_eq!(
        query("SELECT username FROM stats_user_latest_name WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "after-failure"
    );
    *f.members.name.lock().unwrap() = Some(yoink_stats::UserName {
        user_id: 2,
        username: Some("current".into()),
        display_name: Some("Current".into()),
    });
    assert_eq!(
        f.stats
            .refresh_usernames(Duration::from_secs(1))
            .await
            .unwrap()
            .refreshed,
        1
    );
    let floor = query("SELECT refresh_floor FROM stats_name_baselines WHERE user_id=2")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get::<i64, _>(0)
        .unwrap()
        / 1_000_000;
    assert!(floor >= second);
    let mut delayed = message(3, floor - 60, "queued before lookup");
    delayed["message"]["message_id"] = json!(3);
    delayed["message"]["from"]["username"] = json!("delayed-old");
    f.stats.observe_raw_update(20, delayed).await.unwrap();
    assert_eq!(
        query("SELECT username FROM stats_user_latest_name WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "current"
    );
    let before = count(&f, "stats_user_names").await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    f.stats
        .refresh_usernames(Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(count(&f, "stats_user_names").await, before);
    let previous_floor = floor;
    let floor = query("SELECT refresh_floor FROM stats_name_baselines WHERE user_id=2")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get::<i64, _>(0)
        .unwrap()
        / 1_000_000;
    assert!(floor > previous_floor);
    *f.members.name.lock().unwrap() = None;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        f.stats
            .refresh_usernames(Duration::from_secs(1))
            .await
            .unwrap()
            .skipped,
        1
    );
    assert_eq!(
        query("SELECT refresh_floor FROM stats_name_baselines WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        floor * 1_000_000
    );
    let mut equal = message(4, floor, "equal-second takeover");
    equal["message"]["message_id"] = json!(4);
    equal["message"]["from"]["username"] = json!("equal-second");
    f.stats.observe_raw_update(10, equal.clone()).await.unwrap();
    equal["update_id"] = json!(999);
    equal["message"]["message_id"] = json!(5);
    equal["message"]["from"]["username"] = json!("cross-bot-tie");
    f.stats.observe_raw_update(20, equal).await.unwrap();
    assert_eq!(
        query("SELECT username FROM stats_user_latest_name WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "equal-second"
    );
    assert_eq!(
        query("SELECT date FROM stats_user_names WHERE user_id=2 AND username='alice'")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        imported
    );
    assert_eq!(
        query("SELECT event_floor FROM stats_name_baselines WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        (second - 120) * 1_000_000
    );
}

#[tokio::test]
async fn command_history_validates_days_before_normalization_and_reports_underflow() {
    use yoink_i18n::Language;
    let f = fixture().await;
    for days in [i64::MAX, 0, -1, 3651] {
        for dates in [
            vec!["--end", "2020-02-01"],
            vec!["--start", "2020-01-01", "--end", "2020-02-01"],
        ] {
            let mut args = vec!["history".to_owned()];
            args.extend(dates.into_iter().map(str::to_owned));
            args.extend(["--days".to_owned(), days.to_string()]);
            assert!(
                matches!(
                    f.stats.command(1, -100, &args, Language::English).await,
                    Err(Error::Invalid)
                ),
                "days={days}"
            );
        }
    }
    let args = vec![
        "history".into(),
        "--end".into(),
        chrono::NaiveDate::MIN.to_string(),
        "--days".into(),
        "1".into(),
    ];
    assert!(matches!(
        f.stats.command(1, -100, &args, Language::English).await,
        Err(Error::Invalid)
    ));
    let args = vec![
        "history".into(),
        "--start".into(),
        "2020-01-01".into(),
        "--end".into(),
        chrono::NaiveDate::MAX.to_string(),
    ];
    assert!(matches!(
        f.stats.command(1, -100, &args, Language::English).await,
        Err(Error::Invalid)
    ));
    for days in [1, 3650] {
        for start in [false, true] {
            let mut args = vec![
                "history".into(),
                "--end".into(),
                "2020-02-01".into(),
                "--days".into(),
                days.to_string(),
            ];
            if start {
                args.extend(["--start".into(), "2020-01-01".into()]);
            }
            assert!(
                f.stats
                    .command(1, -100, &args, Language::English)
                    .await
                    .is_ok()
            );
        }
    }
}

#[tokio::test]
async fn name_refresh_revision_preserves_live_same_second_changes_and_takeover() {
    let f = fixture().await;
    let second = chrono::Utc::now().timestamp();
    f.stats
        .observe_raw_update(10, message(1, second, "original"))
        .await
        .unwrap();
    *f.members.name.lock().unwrap() = Some(yoink_stats::UserName {
        user_id: 2,
        username: Some("stale-api".into()),
        display_name: Some("Old".into()),
    });
    f.members.pause_names.store(true, Ordering::SeqCst);
    let stats = f.stats.clone();
    let refresh =
        tokio::spawn(async move { stats.refresh_usernames(Duration::from_secs(5)).await });
    f.members.names_started.notified().await;
    let mut live = message(2, second, "live");
    live["message"]["from"]["username"] = json!("during-scan");
    f.stats.observe_raw_update(10, live).await.unwrap();
    f.members.names_release.add_permits(1);
    assert_eq!(refresh.await.unwrap().unwrap().skipped, 1);
    assert_eq!(
        query("SELECT refresh_floor FROM stats_name_baselines WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<Option<i64>, _>(0)
            .unwrap(),
        None
    );
    assert_eq!(
        query("SELECT username FROM stats_user_latest_name WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "during-scan"
    );
    f.stats
        .refresh_usernames(Duration::from_secs(1))
        .await
        .unwrap();
    let floor = query("SELECT refresh_floor FROM stats_name_baselines WHERE user_id=2")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get::<i64, _>(0)
        .unwrap()
        / 1_000_000;
    let mut after = message(3, floor, "after");
    after["message"]["from"]["username"] = json!("after-scan");
    f.stats.observe_raw_update(10, after).await.unwrap();
    assert_eq!(
        query("SELECT username FROM stats_user_latest_name WHERE user_id=2")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "after-scan"
    );
}

#[tokio::test]
async fn final_identity_rejects_role_and_temporary_bans_committed_during_lookup() {
    for temporary in [false, true] {
        let f = fixture().await;
        f.stats
            .observe_raw_update(10, message(1, 1000, "secret"))
            .await
            .unwrap();
        f.members.pause_lookup.store(true, Ordering::SeqCst);
        let stats = f.stats.clone();
        let request = tokio::spawn(async move {
            stats
                .analytics(
                    2,
                    "overview",
                    yoink_stats::Params {
                        chat_id: -100,
                        ..Default::default()
                    },
                )
                .await
        });
        f.members.lookup_started.notified().await;
        if temporary {
            query("UPDATE users SET ban_until=? WHERE id=2")
                .bind((chrono::Utc::now() + chrono::Duration::hours(1)).timestamp_micros())
                .execute(f.store.database())
                .await
                .unwrap();
        } else {
            query("UPDATE users SET role='banned' WHERE id=2")
                .execute(f.store.database())
                .await
                .unwrap();
        }
        f.members.lookup_release.add_permits(1);
        assert!(matches!(request.await.unwrap(), Err(Error::Forbidden)));
    }
}

#[tokio::test]
async fn legacy_command_defaults_inbound_mentions_words_and_short_quotes() {
    use yoink_i18n::Language;
    let f = fixture().await;
    let historical = chrono::NaiveDate::from_ymd_opt(2020, 1, 15)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    f.stats
        .observe_raw_update(10, message(1, historical, "their что hello"))
        .await
        .unwrap();
    let command = |args: Vec<&str>| args.into_iter().map(str::to_owned).collect::<Vec<_>>();
    let week = f
        .stats
        .command(2, -100, &command(vec!["week"]), Language::English)
        .await
        .unwrap()
        .join("");
    assert!(week.contains("1"), "{week}");
    let history = f
        .stats
        .command(
            2,
            -100,
            &command(vec!["history", "--end", "2020-02-01"]),
            Language::English,
        )
        .await
        .unwrap()
        .join("");
    assert!(history.contains("2020-01-15"), "{history}");
    let words = f
        .stats
        .command(2, -100, &command(vec!["words"]), Language::English)
        .await
        .unwrap()
        .join("");
    assert!(words.contains("что"));
    assert!(!words.contains("their"));
    let http = f
        .stats
        .analytics(
            2,
            "words",
            yoink_stats::Params {
                chat_id: -100,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        http.as_array()
            .unwrap()
            .iter()
            .any(|v| v["word"] == "their")
    );
    assert!(!http.as_array().unwrap().iter().any(|v| v["word"] == "что"));
    let mut inbound = message(2, historical + 1, "@alice");
    inbound["message"]["message_id"] = json!(11);
    inbound["message"]["from"] = json!({"id":99,"first_name":"Bob","username":"bob"});
    f.stats.observe_raw_update(10, inbound).await.unwrap();
    let mentions = f
        .stats
        .command(2, -100, &command(vec!["mention", "2"]), Language::English)
        .await
        .unwrap()
        .join("");
    assert!(
        mentions.contains("incoming") && mentions.contains("bob"),
        "{mentions}"
    );
    query("DELETE FROM stats_messages")
        .execute(f.store.database())
        .await
        .unwrap();
    f.stats
        .observe_raw_update(10, message(3, historical + 2, "hello"))
        .await
        .unwrap();
    let random = f
        .stats
        .command(2, -100, &command(vec!["random"]), Language::English)
        .await
        .unwrap()
        .join("");
    assert!(!random.contains("hello"));
    assert_eq!(
        f.stats
            .analytics(
                2,
                "random",
                yoink_stats::Params {
                    chat_id: -100,
                    ..Default::default()
                }
            )
            .await
            .unwrap()["text"],
        "hello"
    );
}

#[tokio::test]
async fn legacy_response_time_eligibility_rounding_and_filtered_results() {
    let f = fixture().await;
    for (id, author, date, reply) in [
        (1, None, 1000, None),
        (2, Some(2), 1059, Some(1)),
        (3, Some(99), 1000, None),
        (4, Some(2), 1060, Some(3)),
        (5, Some(2), 1060, Some(3)),
    ] {
        query("INSERT INTO stats_messages(message_id,chat_id,date,from_user,reply_to_message,text,msg_type) VALUES(?,-100,?,?,?,'needle','text')").bind(id).bind(date*1_000_000).bind(author).bind(reply).execute(f.store.database()).await.unwrap();
    }
    query("INSERT INTO groups(id,enabled,created_at) VALUES(-100,1,0)")
        .execute(f.store.database())
        .await
        .unwrap();
    for q in [None, Some("needle".into())] {
        let result = f
            .stats
            .analytics(
                2,
                "response-time",
                yoink_stats::Params {
                    chat_id: -100,
                    q,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(result["total_replies"], 3);
        assert_eq!(result["overall_avg"], "1m");
        assert_eq!(result["overall_median"], "1m");
        assert_eq!(result["users"][0]["reply_count"], 2);
        assert_eq!(result["users"][0]["avg"], "1m");
    }
}

#[tokio::test]
async fn filtered_bounded_reducers_apply_sql_date_user_and_type_filters() {
    let f = fixture().await;
    let mut historical = message(1, 1000, "old historical body");
    historical["message"]["from"]["id"] = json!(99);
    f.stats.observe_raw_update(10, historical).await.unwrap();
    for id in 101..1125 {
        query("INSERT INTO stats_messages(message_id,chat_id,date,from_user,text,msg_type) VALUES(?,-100,1579046400000000,2,'needle kept','text')").bind(id).execute(f.store.database()).await.unwrap();
    }
    query("INSERT INTO stats_messages(id,message_id,chat_id,date,from_user,text,msg_type) VALUES(-1,99,-100,1579046400000000,2,'needle kept','text')").execute(f.store.database()).await.unwrap();
    let p = yoink_stats::Params {
        chat_id: -100,
        start: chrono::NaiveDate::from_ymd_opt(2020, 1, 15),
        end: chrono::NaiveDate::from_ymd_opt(2020, 1, 15),
        user_id: Some(2),
        msg_type: Some("text".into()),
        q: Some("impossible".into()),
        ..Default::default()
    };
    assert_eq!(
        f.stats.analytics(2, "top-users", p.clone()).await.unwrap(),
        json!([])
    );
    let mut matching = p;
    matching.q = Some("needle".into());
    let members = f
        .stats
        .analytics(1, "members", matching.clone())
        .await
        .unwrap();
    assert_eq!(members.as_array().unwrap().len(), 2);
    assert_eq!(
        members
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["user_id"] == 99)
            .unwrap()["message_count"],
        0
    );
    let mut impossible_user = matching.clone();
    impossible_user.user_id = Some(77);
    assert_eq!(
        f.stats
            .analytics(2, "top-users", impossible_user)
            .await
            .unwrap(),
        json!([])
    );
    let mut impossible_type = matching.clone();
    impossible_type.msg_type = Some("photo".into());
    assert_eq!(
        f.stats
            .analytics(2, "top-users", impossible_type)
            .await
            .unwrap(),
        json!([])
    );
    assert_eq!(
        f.stats
            .analytics(2, "top-users", matching.clone())
            .await
            .unwrap()[0]["count"],
        1025
    );
    assert_eq!(
        f.stats
            .analytics(2, "streak", matching.clone())
            .await
            .unwrap()["longest"],
        1
    );
    assert_eq!(
        f.stats.analytics(2, "corr", matching).await.unwrap()["correlation"],
        Value::Null
    );
}
impl MembershipProvider for Members {
    fn lookup(&self, _: i64, _: i64) -> MembershipFuture<'_> {
        Box::pin(async {
            if self.pause_lookup.swap(false, Ordering::SeqCst) {
                self.lookup_started.notify_one();
                self.lookup_release.acquire().await.unwrap().forget();
            }
            Ok(MemberAccess {
                member: self.present.load(Ordering::SeqCst),
                administrator: self.admin.load(Ordering::SeqCst),
            })
        })
    }
    fn lookup_name(&self, _: Option<i64>, user: i64) -> yoink_stats::NameFuture<'_> {
        Box::pin(async move {
            let name = self
                .name
                .lock()
                .unwrap()
                .clone()
                .filter(|n| n.user_id == user);
            if self.pause_names.swap(false, Ordering::SeqCst) {
                self.names_started.notify_one();
                self.names_release.acquire().await.unwrap().forget();
            }
            if self.hang_names.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Ok(name)
        })
    }
}
struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    stats: Stats,
    members: Arc<Members>,
}
async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Store::open(&dir.path().join("stats.db"), true, Duration::from_secs(5))
        .await
        .unwrap();
    store.migrate().await.unwrap();
    for (u, role) in [(1, "admin"), (2, "user"), (3, "moderator")] {
        query("INSERT INTO users(id,role,created_at,updated_at) VALUES(?,?,0,0)")
            .bind(u)
            .bind(role)
            .execute(store.database())
            .await
            .unwrap();
    }
    let identity = Arc::new(yoink_identity::IdentityStore::new(
        store.database().clone(),
        None,
        false,
    ));
    let members = Arc::new(Members {
        present: AtomicBool::new(true),
        admin: AtomicBool::new(false),
        name: std::sync::Mutex::new(None),
        hang_names: AtomicBool::new(false),
        pause_lookup: AtomicBool::new(false),
        lookup_started: tokio::sync::Notify::new(),
        lookup_release: tokio::sync::Semaphore::new(0),
        pause_names: AtomicBool::new(false),
        names_started: tokio::sync::Notify::new(),
        names_release: tokio::sync::Semaphore::new(0),
    });
    let stats = Stats::open(
        store.database().clone(),
        identity,
        members.clone(),
        None,
        dir.path().join("index"),
    )
    .await
    .unwrap();
    Fixture {
        dir,
        store,
        stats,
        members,
    }
}
fn message(update: i64, date: i64, text: &str) -> Value {
    json!({"update_id":update,"message":{"message_id":10,"date":date,"chat":{"id":-100,"type":"supergroup","title":"Test"},"from":{"id":2,"first_name":"Alice","username":"alice"},"text":text}})
}
fn edit(update: i64, date: i64, text: Option<&str>) -> Value {
    let mut v = message(update, 1000, text.unwrap_or(""));
    let mut m = v.as_object_mut().unwrap().remove("message").unwrap();
    m["edit_date"] = json!(date);
    m["text"] = json!(text);
    v["edited_message"] = m;
    v
}
async fn count(f: &Fixture, table: &str) -> i64 {
    query(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap()
}

#[tokio::test]
async fn command_result_keeps_private_source_and_transactional_assignment_fence() {
    use yoink_db::Executor;
    use yoink_i18n::Language;
    let f = fixture().await;
    f.stats
        .observe_raw_update(9, message(1, 1000, "private provenance"))
        .await
        .unwrap();
    query("INSERT INTO user_group_policies(user_id,group_id) VALUES(3,-100)")
        .execute(f.store.database())
        .await
        .unwrap();
    let result = f
        .stats
        .command_result(3, 3, &["user".into(), "2".into()], Language::English)
        .await
        .unwrap();
    assert_eq!(result.source_group, Some(-100));
    let access = f
        .stats
        .command_preflight(3, result.source_group)
        .await
        .unwrap();
    let c = f.store.database().connection().await.unwrap();
    c.batch("BEGIN IMMEDIATE").await.unwrap();
    let authority = f
        .stats
        .command_authority_on(&c, 3, result.source_group, access)
        .await
        .unwrap();
    assert_eq!(authority["assigned"], true);
    query("DELETE FROM user_group_policies WHERE user_id=3")
        .execute(&*c)
        .await
        .unwrap();
    assert!(matches!(
        f.stats
            .command_authority_on(&c, 3, result.source_group, access)
            .await,
        Err(Error::Forbidden)
    ));
    c.batch("ROLLBACK").await.unwrap();
    drop(c);
    f.stats.shutdown().await.unwrap();
}

#[tokio::test]
async fn aggregates_abort_when_ingestion_revision_changes_between_batches() {
    for endpoint in ["top-users", "rank", "response-time", "streak", "overview"] {
        let f = fixture().await;
        f.stats
            .observe_raw_update(9, message(1, 1000, "needle stable"))
            .await
            .unwrap();
        for id in 100..612 {
            query("INSERT INTO stats_messages(message_id,chat_id,date,from_user,text,msg_type) VALUES(?,-100,1000000000,2,'needle stable','text')").bind(id).execute(f.store.database()).await.unwrap();
        }
        let stats = f.stats.clone();
        let request = tokio::spawn(async move {
            stats
                .analytics(
                    1,
                    endpoint,
                    yoink_stats::Params {
                        chat_id: -100,
                        q: Some("needle".into()),
                        user_id: matches!(endpoint, "rank" | "streak").then_some(2),
                        ..Default::default()
                    },
                )
                .await
        });
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let writing = f.stats.clone();
        let writer = tokio::spawn(async move {
            for i in 0..4096 {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                writing
                    .observe_raw_update(9, edit(i + 100, 2000 + i, Some("needle writer")))
                    .await
                    .unwrap();
                tokio::task::yield_now().await;
            }
        });
        let result = request.await.unwrap();
        stop.store(true, Ordering::SeqCst);
        writer.await.unwrap();
        assert!(matches!(result, Err(Error::Unavailable)), "{result:?}");
    }
}

#[tokio::test]
async fn replayed_originals_and_stale_edits_do_not_regress_real_search() {
    let f = fixture().await;
    let original = message(100, 1000, "original");
    f.stats
        .observe_raw_update(9, original.clone())
        .await
        .unwrap();
    f.stats
        .observe_raw_update(9, original.clone())
        .await
        .unwrap();
    f.stats
        .observe_raw_update(9, edit(200, 1100, Some("new searchable")))
        .await
        .unwrap();
    f.stats
        .observe_raw_update(9, edit(999, 1050, Some("stale")))
        .await
        .unwrap();
    f.stats.observe_raw_update(9, original).await.unwrap();
    assert_eq!(count(&f, "stats_messages").await, 1);
    let result = f
        .stats
        .search(
            2,
            SearchRequest {
                chat_id: -100,
                query: Some("searchable".into()),
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.items[0].text.as_deref(), Some("new searchable"));
    f.stats
        .observe_raw_update(9, edit(3, 1200, None))
        .await
        .unwrap();
    assert_eq!(
        f.stats
            .search(
                2,
                SearchRequest {
                    chat_id: -100,
                    query: Some("searchable".into()),
                    limit: 10,
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .total,
        0
    );
    f.stats.shutdown().await.unwrap();
    let reopened = Stats::open(
        f.store.database().clone(),
        Arc::new(yoink_identity::IdentityStore::new(
            f.store.database().clone(),
            None,
            false,
        )),
        f.members.clone(),
        None,
        f.dir.path().join("index"),
    )
    .await
    .unwrap();
    assert_eq!(reopened.refresh().await.unwrap().documents, 0);
    assert_eq!(reopened.recover_search().await.unwrap().documents, 1);
}
#[tokio::test]
async fn reaction_replacement_removal_and_old_replay_are_atomic() {
    let f = fixture().await;
    f.stats
        .observe_raw_update(9, message(1, 1000, "hi"))
        .await
        .unwrap();
    let reaction = |update, date, new: Value| json!({"update_id":update,"message_reaction":{"chat":{"id":-100,"type":"group"},"message_id":10,"date":date,"user":{"id":2,"first_name":"A"},"new_reaction":new}});
    let initial = reaction(
        2,
        1001,
        json!([{"type":"emoji","emoji":"+"},{"type":"custom_emoji","custom_emoji_id":"42"}]),
    );
    f.stats
        .observe_raw_update(9, initial.clone())
        .await
        .unwrap();
    f.stats
        .observe_raw_update(9, initial.clone())
        .await
        .unwrap();
    assert_eq!(count(&f, "stats_reactions").await, 2);
    f.stats
        .observe_raw_update(9, reaction(3, 1002, json!([])))
        .await
        .unwrap();
    f.stats.observe_raw_update(9, initial).await.unwrap();
    assert_eq!(count(&f, "stats_reactions").await, 0);
    assert!(
        f.stats
            .observe_raw_update(
                9,
                reaction(4, 1003, json!([{"type":"emoji","emoji":"+"},{}]))
            )
            .await
            .is_err()
    );
    assert_eq!(count(&f, "stats_reactions").await, 0);
    f.stats
        .observe_raw_update(9, reaction(4, 1003, json!([{"type":"paid"}])))
        .await
        .unwrap();
    assert_eq!(count(&f, "stats_reactions").await, 1);
}
#[tokio::test]
async fn group_events_membership_disabled_groups_and_private_updates() {
    let f = fixture().await;
    let mut v = message(1, 1000, "hi");
    v["message"]["new_chat_members"] = json!([{"id":2,"first_name":"A"}]);
    f.stats.observe_raw_update(9, v.clone()).await.unwrap();
    f.stats.observe_raw_update(9, v).await.unwrap();
    assert_eq!(count(&f, "stats_user_events").await, 1);
    let observation = |update, date, status| json!({"update_id":update,"chat_member":{"chat":{"id":-100,"type":"group"},"date":date,"new_chat_member":{"user":{"id":2,"first_name":"A"},"status":status}}});
    f.stats
        .observe_raw_update(9, observation(3, 1002, "left"))
        .await
        .unwrap();
    f.stats
        .observe_raw_update(9, observation(4, 1001, "administrator"))
        .await
        .unwrap();
    let present: bool = query("SELECT in_chat FROM stats_group_members")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert!(!present);
    assert_eq!(count(&f, "stats_chat_admins").await, 0);
    query("UPDATE groups SET enabled=0")
        .execute(f.store.database())
        .await
        .unwrap();
    f.stats
        .observe_raw_update(9, edit(10, 2000, Some("disabled")))
        .await
        .unwrap();
    let mut v = message(11, 2001, "private");
    v["message"]["chat"] = json!({"id":2,"type":"private"});
    f.stats.observe_raw_update(9, v).await.unwrap();
    assert_eq!(count(&f, "stats_messages").await, 1);
}
#[tokio::test]
async fn http_shapes_and_fresh_permissions() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let f = fixture().await;
    f.stats
        .observe_raw_update(9, message(1, 1000, "hello world @alice"))
        .await
        .unwrap();
    let app = yoink_stats::http::router(f.stats.clone());
    let request = |path: &str, user: Option<i64>| {
        let mut r = Request::builder().uri(path).body(Body::empty()).unwrap();
        if let Some(u) = user {
            r.extensions_mut()
                .insert(yoink_stats::http::AuthenticatedUser(u));
        }
        r
    };
    assert_eq!(
        app.clone()
            .oneshot(request("/overview?chat_id=-100", None))
            .await
            .unwrap()
            .status(),
        401
    );
    let r = app
        .clone()
        .oneshot(request("/overview?chat_id=-100", Some(2)))
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value =
        serde_json::from_slice(&to_bytes(r.into_body(), 10000).await.unwrap()).unwrap();
    assert_eq!(body["total_messages"], 1);
    assert!(body["first_date"].as_str().unwrap().contains("1970"));
    assert_eq!(body["avg_per_day"], 1.0);
    for endpoint in [
        "top-users",
        "activity-by-hour",
        "activity-by-day",
        "activity-by-week",
        "history",
        "message-types",
        "words",
        "activity-by-month",
        "ecdf",
        "title-history",
        "mention-stats",
        "daily-activity",
        "member-events",
        "avg-message-length",
        "response-time",
        "media-trend",
        "top-reactions",
        "members",
        "chat-admins",
        "user-stats",
    ] {
        let r = app
            .clone()
            .oneshot(request(
                &format!("/{endpoint}?chat_id=-100&user_id=2"),
                Some(1),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{endpoint}");
        let body: Value =
            serde_json::from_slice(&to_bytes(r.into_body(), 100000).await.unwrap()).unwrap();
        assert!(body.is_object() || body.is_array(), "{endpoint}");
    }
    f.members.present.store(false, Ordering::SeqCst);
    assert_eq!(
        app.clone()
            .oneshot(request("/overview?chat_id=-100", Some(2)))
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        app.clone()
            .oneshot(request("/overview?chat_id=2", Some(1)))
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        app.clone()
            .oneshot(request("/overview?chat_id=-100", Some(3)))
            .await
            .unwrap()
            .status(),
        403
    );
    query("INSERT INTO user_group_policies(user_id,group_id) VALUES(3,-100)")
        .execute(f.store.database())
        .await
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(request("/overview?chat_id=-100", Some(3)))
            .await
            .unwrap()
            .status(),
        200
    );
    query("UPDATE users SET role='banned' WHERE id=1")
        .execute(f.store.database())
        .await
        .unwrap();
    assert_eq!(
        app.oneshot(request("/overview?chat_id=-100", Some(1)))
            .await
            .unwrap()
            .status(),
        403
    );
    assert!(matches!(
        f.stats
            .search(
                1,
                SearchRequest {
                    chat_id: -100,
                    limit: 1,
                    ..Default::default()
                }
            )
            .await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn complete_paged_roster_sync_is_atomic_and_never_registers_users_or_grants() {
    use axum::{Form, Json, Router, extract::State, routing::post};
    use std::{
        collections::HashMap,
        sync::{Mutex, atomic::AtomicUsize},
    };
    let f = fixture().await;
    query("INSERT INTO stats_user_names(user_id,date,username,display_name) VALUES(1000,900123000,'imported-roster','Imported roster')")
        .execute(f.store.database()).await.unwrap();
    f.stats
        .observe_raw_update(9, message(1, 1000, "hi"))
        .await
        .unwrap();
    query("INSERT INTO stats_group_members(chat_id,user_id,in_chat,status,synced_at) VALUES(-100,999,1,'administrator',0)").execute(f.store.database()).await.unwrap();
    query("INSERT INTO stats_chat_admins VALUES(999,-100,'administrator',0)")
        .execute(f.store.database())
        .await
        .unwrap();
    #[derive(Clone)]
    struct Wire {
        mode: Arc<AtomicUsize>,
        calls: Arc<Mutex<Vec<u64>>>,
        pause: Arc<AtomicBool>,
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Semaphore>,
    }
    async fn handler(State(s): State<Wire>, Form(p): Form<HashMap<String, String>>) -> Json<Value> {
        assert_eq!(p["chat_id"], "-100");
        assert_eq!(p["filter"], "members");
        assert_eq!(p["limit"], "200");
        let offset = p["offset"].parse::<u64>().unwrap();
        s.calls.lock().unwrap().push(offset);
        let mode = s.mode.load(Ordering::SeqCst);
        if offset == 0 && s.pause.swap(false, Ordering::SeqCst) {
            s.started.notify_one();
            s.release.acquire().await.unwrap().forget();
        }
        let result = if mode == 2 && offset > 0 {
            json!({"not":"a roster"})
        } else if mode == 1 {
            json!([])
        } else if offset == 0 {
            json!((1000..1200).map(|id|json!({"user":{"id":id,"first_name":"Member","username":format!("u{id}")},"status":if id==1000{"administrator"}else{"member"},"joined_date":500})).collect::<Vec<_>>())
        } else {
            json!([{"user":{"id":1,"first_name":"Existing owner metadata"},"status":"member"},{"user":{"id":1000,"first_name":"Member","username":"u1000"},"status":"administrator"}])
        };
        Json(json!({"ok":true,"result":result}))
    }
    let state = Wire {
        mode: Arc::new(AtomicUsize::new(0)),
        calls: Arc::new(Mutex::new(Vec::new())),
        pause: Arc::new(AtomicBool::new(false)),
        started: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Semaphore::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/userfixture/getChatMembers", post(handler))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let token = f.dir.path().join("test-token");
    std::fs::write(&token, "fixture").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let session = Arc::new(
        yoink_user_session::Client::new(yoink_user_session::Config {
            origin: origin.parse().unwrap(),
            token_file: token,
            timeout: Duration::from_secs(30),
            token_timeout: Duration::from_secs(2),
            max_response_bytes: 100000,
            max_token_bytes: 100,
            max_scan_pages: 500,
            max_scan_topics: 10,
        })
        .unwrap(),
    );
    f.stats.shutdown().await.unwrap();
    let stats = Stats::open(
        f.store.database().clone(),
        Arc::new(yoink_identity::IdentityStore::new(
            f.store.database().clone(),
            None,
            false,
        )),
        f.members.clone(),
        Some(session),
        f.dir.path().join("index"),
    )
    .await
    .unwrap();
    assert!(matches!(
        stats.sync_members(2, -100).await,
        Err(Error::Forbidden)
    ));
    let result = stats.sync_members(1, -100).await.unwrap();
    assert_eq!(*state.calls.lock().unwrap(), vec![0, 200]);
    assert!(
        result
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["user_id"] == 1000 && r["in_chat"] == true && r["username"] == "u1000")
    );
    assert_eq!(count(&f, "users").await, 3);
    assert_eq!(count(&f, "user_permissions").await, 0);
    let role: String = query("SELECT role FROM users WHERE id=1")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(role, "admin");
    assert_eq!(count(&f, "stats_chat_admins").await, 1);
    let name_floor = query("SELECT refresh_floor FROM stats_name_baselines WHERE user_id=1000")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get::<i64, _>(0)
        .unwrap();
    let mut delayed = message(901, name_floor / 1_000_000 - 60, "queued before roster");
    delayed["message"]["message_id"] = json!(901);
    delayed["message"]["from"] =
        json!({"id":1000,"first_name":"Old roster","username":"old-roster"});
    stats.observe_raw_update(9, delayed).await.unwrap();
    assert_eq!(
        query("SELECT username FROM stats_user_latest_name WHERE user_id=1000")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "u1000"
    );
    let mut equal = message(902, name_floor / 1_000_000, "equal-second roster takeover");
    equal["message"]["message_id"] = json!(902);
    equal["message"]["from"] =
        json!({"id":1000,"first_name":"Equal roster","username":"equal-roster"});
    stats.observe_raw_update(9, equal).await.unwrap();
    assert_eq!(
        query("SELECT username FROM stats_user_latest_name WHERE user_id=1000")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "equal-roster"
    );
    assert_eq!(
        query(
            "SELECT date FROM stats_user_names WHERE user_id=1000 AND username='imported-roster'"
        )
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get::<i64, _>(0)
        .unwrap(),
        900123000
    );
    assert_eq!(
        query("SELECT event_floor FROM stats_name_baselines WHERE user_id=1000")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        900000000
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    state.mode.store(2, Ordering::SeqCst);
    let before = count(&f, "stats_group_members").await;
    assert!(stats.sync_members(1, -100).await.is_err());
    assert_eq!(
        query("SELECT refresh_floor FROM stats_name_baselines WHERE user_id=1000")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        name_floor
    );
    assert_eq!(count(&f, "stats_group_members").await, before);
    assert_eq!(count(&f, "stats_chat_admins").await, 1);
    state.mode.store(1, Ordering::SeqCst);
    stats.sync_members(1, -100).await.unwrap();
    let live: i64 = query("SELECT count(*) FROM stats_group_members WHERE in_chat=1")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(live, 0);
    assert_eq!(count(&f, "stats_chat_admins").await, 0);
    stats.observe_raw_update(9,json!({"update_id":900,"chat_member":{"chat":{"id":-100,"type":"group"},"date":2000,"new_chat_member":{"user":{"id":5000,"first_name":"Stale"},"status":"member"}}})).await.unwrap();
    let known: i64 = query("SELECT count(*) FROM stats_group_members WHERE user_id=5000")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(known, 0);
    state.mode.store(0, Ordering::SeqCst);
    state.pause.store(true, Ordering::SeqCst);
    let scanning = stats.clone();
    let scan = tokio::spawn(async move { scanning.sync_members(1, -100).await });
    state.started.notified().await;
    let second = chrono::Utc::now().timestamp();
    stats.observe_raw_update(9,json!({"update_id":1001,"chat_member":{"chat":{"id":-100,"type":"group"},"date":second,"new_chat_member":{"user":{"id":1000,"first_name":"Live departed","username":"gone"},"status":"left"}}})).await.unwrap();
    state.release.add_permits(1);
    scan.await.unwrap().unwrap();
    let departed = query("SELECT in_chat,status FROM stats_group_members WHERE user_id=1000")
        .fetch_one(f.store.database())
        .await
        .unwrap();
    assert!(!departed.try_get::<bool, _>(0).unwrap());
    assert_eq!(departed.try_get::<String, _>(1).unwrap(), "left");
    assert_eq!(
        query("SELECT count(*) FROM stats_chat_admins WHERE user_id=1000")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        0
    );
    assert_eq!(
        query("SELECT username FROM stats_user_latest_name WHERE user_id=1000")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<String, _>(0)
            .unwrap(),
        "gone"
    );
    assert_eq!(
        query("SELECT refresh_floor FROM stats_name_baselines WHERE user_id=1000")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        name_floor
    );
    let floor = query("SELECT event_floor FROM stats_roster_floors WHERE chat_id=-100")
        .fetch_one(f.store.database())
        .await
        .unwrap()
        .try_get::<i64, _>(0)
        .unwrap()
        / 1_000_000;
    stats.observe_raw_update(9,json!({"update_id":1002,"chat_member":{"chat":{"id":-100,"type":"group"},"date":floor,"new_chat_member":{"user":{"id":1001,"first_name":"Same second takeover"},"status":"administrator"}}})).await.unwrap();
    assert!(
        query("SELECT in_chat FROM stats_group_members WHERE user_id=1001")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<bool, _>(0)
            .unwrap()
    );
    state.pause.store(true, Ordering::SeqCst);
    let scanning = stats.clone();
    let older = tokio::spawn(async move { scanning.sync_members(1, -100).await });
    state.started.notified().await;
    state.mode.store(1, Ordering::SeqCst);
    stats.sync_members(1, -100).await.unwrap();
    state.release.add_permits(1);
    let latest = older.await.unwrap().unwrap();
    assert!(
        latest
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["in_chat"] == false)
    );
    assert_eq!(
        query("SELECT count(*) FROM stats_group_members WHERE in_chat=1")
            .fetch_one(f.store.database())
            .await
            .unwrap()
            .try_get::<i64, _>(0)
            .unwrap(),
        0
    );
    server.abort();
}

#[tokio::test]
async fn periodic_name_refresh_preserves_unavailable_names_without_registration() {
    let f = fixture().await;
    let mut v = message(1, 1000, "hi");
    v["message"]["from"] = json!({"id":77,"first_name":"Known","username":"known"});
    f.stats.observe_raw_update(9, v).await.unwrap();
    let report = f
        .stats
        .refresh_usernames(Duration::from_millis(50))
        .await
        .unwrap();
    assert_eq!(report.skipped, 1);
    *f.members.name.lock().unwrap() = Some(yoink_stats::UserName {
        user_id: 77,
        username: Some("fresh".into()),
        display_name: Some("Fresh <Name>".into()),
    });
    let report = f
        .stats
        .refresh_usernames(Duration::from_millis(50))
        .await
        .unwrap();
    assert_eq!(report.refreshed, 1);
    f.members.hang_names.store(true, Ordering::SeqCst);
    let timeout_report = f
        .stats
        .refresh_usernames(Duration::from_millis(10))
        .await
        .unwrap();
    assert_eq!(timeout_report.skipped, 1);
    f.members.hang_names.store(false, Ordering::SeqCst);
    *f.members.name.lock().unwrap() = None;
    f.stats
        .refresh_usernames(Duration::from_millis(50))
        .await
        .unwrap();
    let row = query("SELECT username,display_name FROM stats_user_latest_name WHERE user_id=77")
        .fetch_one(f.store.database())
        .await
        .unwrap();
    assert_eq!(row.try_get::<String, _>(0).unwrap(), "fresh");
    assert_eq!(row.try_get::<String, _>(1).unwrap(), "Fresh <Name>");
    assert_eq!(count(&f, "users").await, 3);
    assert_eq!(count(&f, "user_permissions").await, 0);
}

#[tokio::test]
async fn commands_cover_legacy_subcommands_and_private_resolution_without_leaking_groups() {
    use yoink_i18n::Language;
    let f = fixture().await;
    f.stats
        .observe_raw_update(
            9,
            message(1, chrono::Utc::now().timestamp(), "<hello> world @ALICE"),
        )
        .await
        .unwrap();
    for command in [
        "counts", "hours", "days", "week", "history", "user", "types", "words", "random", "rank",
        "streak", "ecdf", "corr", "delta", "titles", "mention",
    ] {
        let mut args = vec![command.into()];
        if matches!(command, "rank" | "streak" | "corr" | "delta") {
            args.push("2".into());
        }
        for lang in [Language::English, Language::Russian] {
            let result = f.stats.command(2, -100, &args, lang).await.unwrap();
            assert!(!result.is_empty(), "{command}");
            assert!(
                result
                    .iter()
                    .all(|s| s.len() < 4096 && s.ends_with("</pre>"))
            );
        }
    }
    let result = f
        .stats
        .command(2, 2, &["user".into()], Language::English)
        .await
        .unwrap();
    assert!(result[0].contains("Total: 1"));
    f.members.present.store(false, Ordering::SeqCst);
    let result = f
        .stats
        .command(2, 2, &["user".into()], Language::English)
        .await
        .unwrap();
    assert!(result[0].contains("No data."));
    assert!(
        f.stats
            .command(
                1,
                -100,
                &["counts".into(), "--start".into(), "bad".into()],
                Language::English
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn native_deletes_tombstones_and_fresh_directory_recovery_are_real() {
    let f = fixture().await;
    f.stats
        .observe_raw_update(9, message(1, 1000, "indexed"))
        .await
        .unwrap();
    f.stats.refresh().await.unwrap();
    query("DELETE FROM stats_messages WHERE chat_id=-100 AND message_id=10")
        .execute(f.store.database())
        .await
        .unwrap();
    let request = SearchRequest {
        chat_id: -100,
        query: Some("indexed".into()),
        limit: 10,
        ..Default::default()
    };
    assert_eq!(f.stats.search(1, request.clone()).await.unwrap().total, 0);
    f.stats
        .recover_search_at(f.dir.path().join("replacement-index"))
        .await
        .unwrap();
    assert_eq!(f.stats.search(1, request).await.unwrap().total, 0);
    f.stats.shutdown().await.unwrap();
    assert!(matches!(f.stats.refresh().await, Err(Error::Unavailable)));
}

#[tokio::test]
async fn sql_aggregates_match_filtered_reference_including_dates_ties_unicode_and_unknown_users() {
    let f = fixture().await;
    for (i, date) in [1000, 1000, 87000, 173000].into_iter().enumerate() {
        let mut v = message(i as i64 + 1, date, "match \u{1f600}");
        v["message"]["message_id"] = json!(i + 10);
        v["message"]["from"] = json!({"id":if i%2==0{2}else{77},"first_name":"Profile"});
        if i == 2 {
            v["message"].as_object_mut().unwrap().remove("text");
            v["message"]["caption"] = json!("match media");
            v["message"]["photo"] = json!([{"file_id":"fixture"}]);
        }
        if i == 0 {
            v["message"]["new_chat_members"] = json!([{"id":333,"first_name":"Roster"}]);
        }
        if i == 1 {
            v["message"]["new_chat_title"] = json!("Name <Title>");
        }
        if i == 3 {
            v["message"]["reply_to_message"] = json!({"message_id":12});
        }
        f.stats.observe_raw_update(9, v).await.unwrap();
    }
    for endpoint in [
        "overview",
        "top-users",
        "ecdf",
        "message-types",
        "activity-by-hour",
        "activity-by-day",
        "activity-by-week",
        "history",
        "daily-activity",
        "activity-by-month",
        "media-trend",
        "user-stats",
        "avg-message-length",
        "rank",
        "members",
        "title-history",
        "response-time",
        "chat-admins",
        "member-events",
        "top-reactions",
    ] {
        for filtered in [false, true] {
            let params = yoink_stats::Params {
                chat_id: -100,
                user_id: if matches!(endpoint, "user-stats" | "rank") {
                    Some(2)
                } else {
                    None
                },
                year: Some(1970),
                start: if filtered {
                    Some("1970-01-01".parse().unwrap())
                } else {
                    None
                },
                limit: Some(2),
                ..Default::default()
            };
            let optimized = f
                .stats
                .analytics(1, endpoint, params.clone())
                .await
                .unwrap();
            let reference = f
                .stats
                .analytics(
                    1,
                    endpoint,
                    yoink_stats::Params {
                        q: Some("match".into()),
                        ..params
                    },
                )
                .await
                .unwrap();
            assert_eq!(optimized, reference, "{endpoint} filtered={filtered}");
        }
    }
}
