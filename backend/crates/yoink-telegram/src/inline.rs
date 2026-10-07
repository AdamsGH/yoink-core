//! Inline cards contain visible URL intent only; selection uses ordinary admission.
use std::convert::Infallible;

use crate::inline_search::{QueryLifetime, Searches};
use teloxide::{
    prelude::*,
    types::{
        InlineQuery, InlineQueryResult, InlineQueryResultArticle, InlineQueryResultsButton,
        InlineQueryResultsButtonKind, InputMessageContent, InputMessageContentText,
        LinkPreviewOptions,
    },
};
use yoink_access::effective_grant;
use yoink_i18n::{Key, Language};
use yoink_identity::{IdentitySnapshot, IdentityStore, registered_features};
use yoink_worker_protocol::{MAX_SEARCH_QUERY_BYTES, MAX_SEARCH_QUERY_CHARS, SearchEntry};

use crate::{MessageDependencies, reply_language_with_identity};

const MAX_QUERY_BYTES: usize = 4096;

pub(super) fn granted(snapshot: &IdentitySnapshot) -> bool {
    // No destination is known. In particular, chat_type cannot supply group grants.
    snapshot.group.is_none()
        && registered_features()
            .into_iter()
            .find(|spec| spec.plugin == "dl" && spec.feature == "inline")
            .is_some_and(|spec| {
                effective_grant(
                    &snapshot.principal,
                    &spec,
                    &snapshot.grants,
                    &snapshot.providers,
                    snapshot.now,
                )
                .is_some()
            })
}

async fn authorized(store: Option<&IdentityStore>, query: &InlineQuery) -> bool {
    if query.from.is_bot || query.from.id.0 == 0 || query.id.0.is_empty() || query.id.0.len() > 256
    {
        return false;
    }
    authorized_actor(store, query.from.id.0).await
}

pub(super) async fn authorized_actor(store: Option<&IdentityStore>, actor: u64) -> bool {
    let (Some(store), Ok(actor)) = (store, i64::try_from(actor)) else {
        return false;
    };
    store
        .snapshot(actor, None)
        .await
        .ok()
        .flatten()
        .is_some_and(|snapshot| granted(&snapshot))
}

#[derive(Debug, PartialEq)]
enum Intent<'a> {
    Short,
    Text,
    Url(&'a str),
    Invalid,
}

fn classify(query: &str) -> Intent<'_> {
    if query.len() > MAX_QUERY_BYTES || query.chars().any(char::is_control) {
        return Intent::Invalid;
    }
    let query = query.trim();
    if query.len() > MAX_QUERY_BYTES {
        return Intent::Invalid;
    }
    if query.chars().count() < 2 {
        return Intent::Short;
    }
    let first = query.split_whitespace().next().unwrap_or_default();
    let looks_url = first.contains("://")
        || first.to_ascii_lowercase().starts_with("http:")
        || first.to_ascii_lowercase().starts_with("https:")
        || first.to_ascii_lowercase().starts_with("http//")
        || first.to_ascii_lowercase().starts_with("https//")
        || ["ftp:", "file:", "data:", "javascript:", "tg:", "magnet:"]
            .iter()
            .any(|scheme| first.to_ascii_lowercase().starts_with(scheme))
        || first.starts_with("www.");
    if !looks_url {
        return if query.len() > MAX_SEARCH_QUERY_BYTES
            || query.chars().count() > MAX_SEARCH_QUERY_CHARS
            || query
                .chars()
                .any(yoink_worker_protocol::unsafe_display_char)
        {
            Intent::Invalid
        } else {
            Intent::Text
        };
    }
    let Some((scheme, authority)) = first.split_once("://") else {
        return Intent::Invalid;
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https")
        || authority.is_empty()
        || authority.starts_with('/')
        || authority
            .split('/')
            .next()
            .is_some_and(|authority| authority.contains('@'))
    {
        return Intent::Invalid;
    }
    let Ok(url) = url::Url::parse(first) else {
        return Intent::Invalid;
    };
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port() == Some(0)
        || first.contains('\\')
        || first.chars().any(char::is_control)
        || crate::plain_url::reserved_music_host(url.host_str().unwrap_or_default())
    {
        return Intent::Invalid;
    }
    // Retain the exact URL and clip tokens, not a normalized URL or a cache ID.
    let Ok(start) = crate::media_command::query_start(first) else {
        return Intent::Invalid;
    };
    let times: Vec<_> = query.split_whitespace().skip(1).collect();
    let range = match times.as_slice() {
        [] => return Intent::Url(query),
        [end] => match start {
            Some(start) => (start, *end),
            None => return Intent::Invalid,
        },
        [start, end] => match crate::media_command::parse_time(start) {
            Ok(start) => (start, *end),
            Err(()) => return Intent::Invalid,
        },
        _ => return Intent::Invalid,
    };
    let Ok(value) = crate::media_command::parse_time(range.1) else {
        return Intent::Invalid;
    };
    let end = if range.1.contains(':') {
        value
    } else {
        range.0 + value
    };
    if yoink_domain::ClipRange::new(range.0, end).is_err() {
        return Intent::Invalid;
    }
    Intent::Url(query)
}

pub(super) fn help(language: Language) -> String {
    crate::translate(language, Key::InlineHelp, &[])
}

fn card(intent: &str, language: Language) -> InlineQueryResult {
    // Stable across restarts and independent of randomized HashMap hashing.
    let hash = intent
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    let content = InputMessageContentText::new(intent).link_preview_options(LinkPreviewOptions {
        is_disabled: true,
        url: None,
        prefer_small_media: false,
        prefer_large_media: false,
        show_above_text: false,
    });
    InlineQueryResultArticle::new(
        format!("url-{hash:016x}"),
        crate::translate(language, Key::InlineUrlTitle, &[]),
        InputMessageContent::Text(content),
    )
    .into()
}

fn search_card(entry: SearchEntry, language: Language) -> InlineQueryResult {
    let mut result = card(&entry.url, language);
    if let InlineQueryResult::Article(article) = &mut result {
        article.title = if entry.title.is_empty() {
            crate::translate(language, Key::InlineUnknownTitle, &[])
        } else {
            entry.title
        };
        let mut parts = Vec::new();
        if !entry.channel.is_empty() {
            parts.push(entry.channel);
        }
        if let Some(seconds) = entry.duration_seconds {
            parts.push(if seconds >= 3600 {
                format!(
                    "{}:{:02}:{:02}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60
                )
            } else {
                format!("{}:{:02}", seconds / 60, seconds % 60)
            });
        }
        if let Some(views) = entry.view_count {
            parts.push(crate::translate(
                language,
                Key::InlineViews,
                &[("count", &views.to_string())],
            ));
        }
        article.description = Some(parts.join(" | ").chars().take(256).collect());
    }
    result
}

async fn answer(
    bot: &Bot,
    query: &InlineQuery,
    results: Vec<InlineQueryResult>,
    hint: Option<Key>,
    language: Language,
    lifetime: &QueryLifetime,
) {
    let mut request = bot
        .answer_inline_query(query.id.clone(), results)
        .is_personal(true)
        .cache_time(0);
    if let Some(hint) = hint {
        request = request.button(InlineQueryResultsButton {
            text: crate::translate(language, hint, &[]),
            kind: InlineQueryResultsButtonKind::StartParameter("search_help".into()),
        });
    }
    if matches!(lifetime.wait(async { request.await }).await, Some(Err(_))) {
        tracing::warn!("Telegram inline answer failed");
    }
}

async fn process(
    bot: Bot,
    query: InlineQuery,
    dependencies: MessageDependencies,
    searches: Searches,
    lifetime: QueryLifetime,
    search_allowed: bool,
) {
    let store = dependencies.identities.as_deref();
    let Some(allowed) = lifetime.wait(authorized(store, &query)).await else {
        return;
    };
    if !allowed {
        answer(&bot, &query, vec![], None, Language::English, &lifetime).await;
        return;
    }
    if lifetime.cancel.is_cancelled() {
        return;
    }
    let Some(language) = lifetime
        .wait(reply_language_with_identity(
            dependencies.downloads.as_ref(),
            store,
            query.from.id.0,
            query.from.language_code.as_deref(),
        ))
        .await
    else {
        return;
    };
    if lifetime.cancel.is_cancelled() {
        return;
    }
    let (results, hint) = if !search_allowed {
        (vec![], Some(Key::InlineBusy))
    } else {
        match classify(&query.query) {
            Intent::Url(intent) => (vec![card(intent, language)], None),
            Intent::Short => (vec![], Some(Key::InlineShort)),
            Intent::Invalid => (vec![], Some(Key::InlineInvalid)),
            Intent::Text => {
                if let (Some(service), Ok(_permit)) = (
                    dependencies.downloads.as_ref(),
                    searches.slots.clone().try_acquire_owned(),
                ) {
                    match service
                        .search_youtube(
                            query.query.trim(),
                            lifetime.cancel.clone(),
                            lifetime.remaining(),
                        )
                        .await
                    {
                        Ok(entries) if entries.is_empty() => (vec![], Some(Key::InlineEmpty)),
                        Ok(entries) => (
                            entries
                                .into_iter()
                                .map(|entry| search_card(entry, language))
                                .collect(),
                            None,
                        ),
                        Err(_) => (vec![], Some(Key::InlineUnavailable)),
                    }
                } else {
                    (
                        vec![],
                        Some(if dependencies.downloads.is_none() {
                            Key::InlineUnavailable
                        } else {
                            Key::InlineBusy
                        }),
                    )
                }
            }
        }
    };
    // Fresh database identity, including bans and revoked explicit grants, after lookup.
    let Some(allowed) = lifetime.wait(authorized(store, &query)).await else {
        return;
    };
    if !allowed {
        answer(&bot, &query, vec![], None, language, &lifetime).await;
    } else {
        answer(&bot, &query, results, hint, language, &lifetime).await;
    }
}

pub(super) async fn handle(
    bot: Bot,
    query: InlineQuery,
    dependencies: MessageDependencies,
    searches: Searches,
) -> Result<(), Infallible> {
    if query.from.is_bot || query.from.id.0 == 0 {
        return Ok(());
    }
    let Some((lifetime, allowed)) = searches.begin(query.from.id.0) else {
        return Ok(());
    };
    // Every path (including rate rejection) is tracked, cancellation-aware and
    // uses the same admission deadline. The dispatcher never waits for DB/HTTP.
    searches.spawn(
        lifetime.clone(),
        process(
            bot,
            query,
            dependencies,
            searches.clone(),
            lifetime,
            allowed,
        ),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn throttled_handler_returns_and_drains_while_database_authorization_is_held() {
        use std::{sync::Arc, time::Duration};
        let directory = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let database = yoink_db::Database::open(
            &directory.path().join("held.db"),
            true,
            Duration::from_secs(300),
        )
        .await
        .unwrap();
        let identities = Arc::new(IdentityStore::new(database.clone(), Some(42), false));
        // Database has one connection: both initial and throttled authorization
        // must wait here until the guard is released. No schema error can shortcut it.
        let held = database.connection().await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        for supersede in [true, false] {
            let searches = Searches::new(
                crate::InlineSearchConfig::default(),
                tokio_util::sync::CancellationToken::new(),
            );
            assert!(searches.begin(43).unwrap().1);
            let query: InlineQuery = serde_json::from_value(serde_json::json!({"id":"held-throttle", "from":{"id":43,"is_bot":false,"first_name":"Fixture"}, "query":"cats", "offset":""})).unwrap();
            let dependencies = MessageDependencies {
                inbox: None,
                downloads: None,
                identities: Some(identities.clone()),
                upload: crate::CookieUploadDependency {
                    config: None,
                    base: base.clone(),
                    client: reqwest::Client::new(),
                },
                cookie_url: crate::CookieUrlDependency(None),
            };
            tokio::time::timeout(
                Duration::from_millis(100),
                handle(
                    Bot::new("7:fixture").set_api_url(base.clone()),
                    query,
                    dependencies,
                    searches.clone(),
                ),
            )
            .await
            .expect("throttled path blocked dispatcher on database")
            .unwrap();
            tokio::task::yield_now().await;
            assert_eq!(
                searches.tasks.len(),
                1,
                "throttled auth must be tracked while DB is held"
            );
            if supersede {
                assert!(!searches.begin(43).unwrap().1);
            } else {
                searches.stop.cancel();
            }
            searches.tasks.close();
            tokio::time::timeout(Duration::from_millis(100), searches.tasks.wait())
                .await
                .expect("held throttled authorization did not cancel");
            assert!(searches.tasks.is_empty());
            assert!(
                tokio::time::timeout(Duration::from_millis(1), listener.accept())
                    .await
                    .is_err(),
                "cancelled throttled query answered"
            );
            searches.shutdown().await;
        }
        drop(held);
        database.close().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn held_database_time_reduces_real_telegram_answer_budget() {
        use std::os::unix::fs::PermissionsExt;
        use std::{sync::Arc, time::Duration};
        use tokio::io::AsyncReadExt;
        // Prevent virtual-clock auto-advance while real loopback IO is becoming
        // ready. Only the explicit advances below consume the admission budget.
        let clock_stop = tokio_util::sync::CancellationToken::new();
        let keep_clock = clock_stop.clone();
        let clock = tokio::spawn(async move {
            while !keep_clock.is_cancelled() {
                tokio::task::yield_now().await;
            }
        });
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let database = yoink_db::Database::open(
            &directory.path().join("answer.db"),
            true,
            Duration::from_secs(300),
        )
        .await
        .unwrap();
        yoink_db::raw_sql(include_str!("../../yoink-identity/src/sql/test_schema.sql"))
            .execute(&database)
            .await
            .unwrap();
        yoink_db::raw_sql(include_str!(
            "../../yoink-storage/migrations-turso/0014_native_membership.sql"
        ))
        .execute(&database)
        .await
        .unwrap();
        yoink_db::raw_sql("INSERT INTO users VALUES (43, 'restricted', 'en', NULL); INSERT INTO user_permissions VALUES (43, 'dl', 'inline', NULL)").execute(&database).await.unwrap();
        let identities = Arc::new(IdentityStore::new(database.clone(), Some(42), false));
        let held = database.connection().await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let (received, request) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let size = stream.read(&mut buffer).await.unwrap();
                assert!(size > 0);
                bytes.extend_from_slice(&buffer[..size]);
                if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&bytes[..end]);
                    let length = header
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        received
                            .send(
                                serde_json::from_slice::<serde_json::Value>(
                                    &bytes[end + 4..end + 4 + length],
                                )
                                .unwrap(),
                            )
                            .unwrap();
                        // The real Telegram answer is held: never send HTTP headers.
                        return std::future::pending::<()>().await;
                    }
                }
            }
        });
        let searches = Searches::new(
            crate::InlineSearchConfig {
                timeout: Duration::from_secs(10),
                ..crate::InlineSearchConfig::default()
            },
            tokio_util::sync::CancellationToken::new(),
        );
        let query: InlineQuery = serde_json::from_value(serde_json::json!({"id":"cumulative-answer", "from":{"id":43,"is_bot":false,"first_name":"Fixture"}, "query":"https://example.com/video", "offset":""})).unwrap();
        let dependencies = MessageDependencies {
            inbox: None,
            downloads: None,
            identities: Some(identities),
            upload: crate::CookieUploadDependency {
                config: None,
                base: base.clone(),
                client: reqwest::Client::new(),
            },
            cookie_url: crate::CookieUrlDependency(None),
        };
        let admitted = tokio::time::Instant::now();
        handle(
            Bot::new("7:fixture").set_api_url(base),
            query,
            dependencies,
            searches.clone(),
        )
        .await
        .unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(7)).await;
        assert_eq!(searches.tasks.len(), 1);
        drop(held);
        let payload = request.await.unwrap();
        assert_eq!(payload["inline_query_id"], "cumulative-answer");
        assert_eq!(
            payload["results"][0]["input_message_content"]["message_text"],
            "https://example.com/video"
        );
        assert_eq!(
            tokio::time::Instant::now() - admitted,
            Duration::from_secs(7)
        );
        tokio::time::advance(Duration::from_secs(3)).await;
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        assert!(
            searches.tasks.is_empty(),
            "held answer got a new timeout instead of remaining admission budget"
        );
        assert_eq!(
            tokio::time::Instant::now() - admitted,
            Duration::from_secs(10)
        );
        searches.shutdown().await;
        server.abort();
        let _ = server.await;
        clock_stop.cancel();
        clock.await.unwrap();
        database.close().await.unwrap();
    }

    #[tokio::test]
    async fn real_raw_private_help_honors_explicit_grant_revocation_and_ban() {
        use std::{sync::Arc, time::Duration};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        use yoink_db::{Database, raw_sql};
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let database = Database::open(
            &directory.path().join("help.db"),
            true,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        raw_sql(include_str!("../../yoink-identity/src/sql/test_schema.sql"))
            .execute(&database)
            .await
            .unwrap();
        raw_sql(include_str!(
            "../../yoink-storage/migrations-turso/0014_native_membership.sql"
        ))
        .execute(&database)
        .await
        .unwrap();
        raw_sql("INSERT INTO users VALUES (43, 'restricted', 'ru', NULL); INSERT INTO user_permissions VALUES (43, 'dl', 'inline', NULL)").execute(&database).await.unwrap();
        let identities = Arc::new(IdentityStore::new(database.clone(), Some(42), false));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(3);
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (offset, length) = loop {
                    let mut buffer = [0; 4096];
                    let n = stream.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..end]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            break (end + 4, length);
                        }
                    }
                };
                let value: serde_json::Value =
                    serde_json::from_slice(&bytes[offset..offset + length]).unwrap();
                sender.send(value).await.unwrap();
                let body = serde_json::json!({"ok": true, "result": {"message_id": 100, "date": 0, "chat": {"id": 43, "type": "private"}, "text": "Fixture"}}).to_string();
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).as_bytes()).await.unwrap();
            }
        });
        let bot = Bot::new("7:fixture").set_api_url(base.clone());
        let system = yoink_application::SystemService::new("fixture");
        system.set_ready(true);
        for case in 0..3 {
            if case == 1 {
                raw_sql("DELETE FROM user_permissions")
                    .execute(&database)
                    .await
                    .unwrap();
            }
            if case == 2 {
                raw_sql("INSERT INTO user_permissions VALUES (43, 'dl', 'inline', NULL); UPDATE users SET role='banned' WHERE id=43").execute(&database).await.unwrap();
            }
            let raw = serde_json::json!({"update_id": case+1, "message": {"message_id": case+10, "date": 0, "from": {"id": 43, "is_bot": false, "first_name": "Fixture", "language_code": "ru"}, "chat": {"id": 43, "type": "private"}, "text": "/start search_help"}});
            let update: Update =
                serde_json::from_slice(&serde_json::to_vec(&raw).unwrap()).unwrap();
            let teloxide::types::UpdateKind::Message(message) = update.kind else {
                panic!("raw help not decoded");
            };
            let dependencies = crate::MessageDependencies {
                inbox: None,
                downloads: None,
                identities: Some(identities.clone()),
                upload: crate::CookieUploadDependency {
                    config: None,
                    base: base.clone(),
                    client: reqwest::Client::new(),
                },
                cookie_url: crate::CookieUrlDependency(None),
            };
            crate::handle_message(
                bot.clone(),
                message,
                crate::TelegramIdentity {
                    owner_id: 42,
                    bot_id: 7,
                    groups_enabled: false,
                    identity_enabled: true,
                },
                "fixture".into(),
                system.clone(),
                dependencies,
            )
            .await
            .unwrap();
            let sent = receiver.recv().await.unwrap();
            assert_eq!(
                sent["text"],
                if case == 0 {
                    help(Language::Russian)
                } else {
                    crate::translate(Language::Russian, Key::ErrorDenied, &[])
                }
            );
        }
        server.await.unwrap();
        assert_eq!(database.path().parent().unwrap(), directory.path());
    }

    #[tokio::test]
    async fn current_database_identity_revocation_and_failure_fail_closed() {
        use std::time::Duration;
        use yoink_db::{Database, raw_sql};
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let database = Database::open(
            &directory.path().join("inline.db"),
            true,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        raw_sql(include_str!("../../yoink-identity/src/sql/test_schema.sql"))
            .execute(&database)
            .await
            .unwrap();
        raw_sql(include_str!(
            "../../yoink-storage/migrations-turso/0014_native_membership.sql"
        ))
        .execute(&database)
        .await
        .unwrap();
        raw_sql("INSERT INTO users VALUES (42, 'user', 'en', NULL), (43, 'restricted', 'en', NULL), (44, 'user', 'en', NULL); INSERT INTO user_permissions VALUES (43, 'dl', 'inline', NULL)").execute(&database).await.unwrap();
        let store = IdentityStore::new(database.clone(), Some(42), false);
        let query = |actor| {
            serde_json::from_value::<InlineQuery>(serde_json::json!({"id": "fixture", "from": {"id": actor, "is_bot": false, "first_name": "Fixture"}, "query": "cats", "offset": "", "chat_type": "supergroup"})).unwrap()
        };
        assert!(!authorized(None, &query(42)).await);
        assert!(!authorized(Some(&store), &query(999)).await);
        for actor in [42, 43, 44] {
            assert!(authorized(Some(&store), &query(actor)).await);
        }
        // This is the same fresh lookup used immediately before sending an answer.
        raw_sql("DELETE FROM user_permissions WHERE user_id=43")
            .execute(&database)
            .await
            .unwrap();
        assert!(!authorized(Some(&store), &query(43)).await);
        raw_sql("UPDATE users SET role='banned' WHERE id=42")
            .execute(&database)
            .await
            .unwrap();
        assert!(!authorized(Some(&store), &query(42)).await);
        raw_sql("DROP TABLE user_permissions")
            .execute(&database)
            .await
            .unwrap();
        assert!(!authorized(Some(&store), &query(44)).await);
    }

    #[test]
    fn current_inline_grants_and_bans_never_infer_group_authority() {
        use chrono::{Duration, Utc};
        use yoink_access::{Grant, Principal, Role};
        let now = Utc::now();
        let mut snapshot = IdentitySnapshot {
            principal: Principal {
                user_id: 43,
                role: Role::Restricted,
                ban_until: None,
            },
            language: "en".into(),
            grants: vec![],
            providers: vec![],
            now,
            group: None,
        };
        assert!(!granted(&snapshot));
        snapshot.principal.role = Role::User;
        assert!(granted(&snapshot));
        snapshot.principal.role = Role::Restricted;
        snapshot.grants.push(Grant {
            plugin: "dl".into(),
            feature: "inline".into(),
            expires_at: Some(now + Duration::seconds(1)),
        });
        assert!(granted(&snapshot));
        snapshot.grants[0].expires_at = Some(now);
        assert!(!granted(&snapshot));
        snapshot.grants[0].expires_at = None;
        snapshot.principal.ban_until = Some(now + Duration::seconds(1));
        assert!(!granted(&snapshot));
        snapshot.principal.role = Role::Owner;
        assert!(!granted(&snapshot));
        snapshot.principal.ban_until = Some(now);
        assert!(granted(&snapshot));
        snapshot.principal.role = Role::Banned;
        assert!(!granted(&snapshot));
        snapshot.principal.role = Role::Owner;
        snapshot.principal.user_id = 0;
        assert!(!granted(&snapshot));
    }

    #[test]
    fn private_help_deep_link_uses_exact_command_and_localized_bounded_text() {
        assert!(crate::inline_help_command("/start search_help", "yoink"));
        assert!(crate::inline_help_command(
            "/start@YOINK search_help",
            "yoink"
        ));
        for text in [
            "/start@other search_help",
            "/start search_help extra",
            "/start",
            "/help search_help",
        ] {
            assert!(!crate::inline_help_command(text, "yoink"));
        }
        for language in [Language::English, Language::Russian] {
            assert!(help(language).len() < 1024);
        }
        assert_ne!(help(Language::English), help(Language::Russian));
    }

    #[test]
    fn url_intent_is_preserved_and_never_becomes_text_search() {
        for query in [
            "https://youtube.com/watch?v=abcdefghijk&t=10",
            "https://example.com/a 10 20",
            "https://example.com/a?t=10 20",
        ] {
            assert_eq!(classify(query), Intent::Url(query));
        }
        for query in [
            "https://",
            "https://user:secret@example.com/a",
            "ftp://example.com/a",
            "www.example.com",
            "http:broken",
            "https//example.com/a",
            "ftp:example.com/a",
            "https://@example.com/a",
            "https://music.youtube.com/watch?v=x",
            "https://spotify.com/a",
            "https://example.com/a extra",
            "https://example.com/a 20 00:10",
            "https://example.com\\evil/a",
        ] {
            assert_eq!(classify(query), Intent::Invalid, "{query}");
        }
        for text in ["cats video", "http2 explained", "http proxy setup"] {
            assert_eq!(classify(text), Intent::Text);
        }
        assert_eq!(classify(""), Intent::Short);
        assert_eq!(classify("x"), Intent::Short);
        assert_eq!(classify(&"x".repeat(MAX_QUERY_BYTES + 1)), Intent::Invalid);
    }
    #[test]
    fn cards_have_visible_text_no_media_no_thumbnail_and_disabled_preview() {
        let value =
            serde_json::to_value(card("https://example.com/a 10 20", Language::English)).unwrap();
        assert_eq!(value["type"], "article");
        assert_eq!(
            value["input_message_content"]["message_text"],
            "https://example.com/a 10 20"
        );
        assert_eq!(
            value["input_message_content"]["link_preview_options"]["is_disabled"],
            true
        );
        for key in ["thumbnail_url", "video_file_id", "video_url", "photo_url"] {
            assert!(value.get(key).is_none());
        }
        assert_eq!(
            value["id"],
            serde_json::to_value(card("https://example.com/a 10 20", Language::Russian)).unwrap()["id"]
        );
    }
}
