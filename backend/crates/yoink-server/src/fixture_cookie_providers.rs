//! Offline providers for the opt-in native browser fixture only.
//! Control and counters contain no credentials, jar contents, or device IDs.
//! Modes/counters survive restart; scenarios reset them through fixtureSql.
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

use crate::fixture_refresh_gate::RefreshGate;
use yoink_downloads::{
    DownloadError,
    cookie_oauth::{OAuthCredential, OAuthProvider, ProviderDevice, ProviderFuture, ProviderPoll},
    cookie_web_live::{AccountInfo, AccountProvider, AccountResult},
    profile::{ProfileError, oauth_refresh::OAuthRefreshProvider},
};
use yoink_storage::Store;

pub const CONTROL_SCHEMA: &str = r#"
INSERT OR IGNORE INTO bot_settings (key, value, updated_at)
VALUES ('fixture_cookie_provider_control',
    '{"oauth_mode":"pending","account_mode":"valid","oauth_start_calls":0,"oauth_poll_calls":0,"oauth_refresh_calls":0,"account_fetch_calls":0}',
    0);
"#;

/// Called only after fixture_main sees the exact explicit opt-in value.
/// Never resets scenario controls or call evidence on restart.
#[cfg(test)]
pub async fn install(
    store: Arc<Store>,
) -> Result<
    (
        Arc<dyn OAuthProvider>,
        Arc<dyn AccountProvider>,
        Arc<dyn OAuthRefreshProvider>,
    ),
    DownloadError,
> {
    install_with_gate(store, Arc::new(RefreshGate::default())).await
}

pub async fn install_with_gate(
    store: Arc<Store>,
    refresh_gate: Arc<RefreshGate>,
) -> Result<
    (
        Arc<dyn OAuthProvider>,
        Arc<dyn AccountProvider>,
        Arc<dyn OAuthRefreshProvider>,
    ),
    DownloadError,
> {
    store
        .database()
        .connection()
        .await
        .map_err(|_| DownloadError::Executor)?
        .execute_batch(CONTROL_SCHEMA)
        .await
        .map_err(|_| DownloadError::Executor)?;
    let provider = Arc::new(FixtureCookieProviders {
        store,
        refresh_gate,
    });
    Ok((provider.clone(), provider.clone(), provider))
}

struct FixtureCookieProviders {
    store: Arc<Store>,
    refresh_gate: Arc<RefreshGate>,
}

impl FixtureCookieProviders {
    // Only compile-time SQL is passed here. Holding the shared connection guard
    // makes increment + mode read indivisible with respect to fixtureSql and
    // concurrent provider calls. Missing/broken control fails closed, never live.
    async fn called(&self, update: &str, select: &str) -> Result<String, DownloadError> {
        let connection = self
            .store
            .database()
            .connection()
            .await
            .map_err(|_| DownloadError::Executor)?;
        // The row has no fixture-specific SQL constraints. Validate its full
        // shape before mutating evidence; invalid/missing state never falls
        // back to a live provider or silently recreates defaults.
        {
            let mut rows = connection
                .query(
                    "SELECT value FROM bot_settings WHERE key='fixture_cookie_provider_control'",
                    (),
                )
                .await
                .map_err(|_| DownloadError::Executor)?;
            let row = rows
                .next()
                .await
                .map_err(|_| DownloadError::Executor)?
                .ok_or(DownloadError::Executor)?;
            let turso::Value::Text(value) =
                row.get_value(0).map_err(|_| DownloadError::Executor)?
            else {
                return Err(DownloadError::Executor);
            };
            validate_control(&value)?;
        }
        connection
            .execute(update, ())
            .await
            .map_err(|_| DownloadError::Executor)?;
        let mut rows = connection
            .query(select, ())
            .await
            .map_err(|_| DownloadError::Executor)?;
        let row = rows
            .next()
            .await
            .map_err(|_| DownloadError::Executor)?
            .ok_or(DownloadError::Executor)?;
        match row.get_value(0).map_err(|_| DownloadError::Executor)? {
            turso::Value::Text(mode) => Ok(mode),
            _ => Err(DownloadError::Executor),
        }
    }
}

fn validate_control(value: &str) -> Result<(), DownloadError> {
    let value: serde_json::Value =
        serde_json::from_str(value).map_err(|_| DownloadError::Executor)?;
    let object = value.as_object().ok_or(DownloadError::Executor)?;
    if !matches!(
        object.get("oauth_mode").and_then(serde_json::Value::as_str),
        Some("pending" | "ok" | "expired" | "error" | "start_error")
    ) || !matches!(
        object
            .get("account_mode")
            .and_then(serde_json::Value::as_str),
        Some("valid" | "invalid" | "error")
    ) {
        return Err(DownloadError::Executor);
    }
    for counter in [
        "oauth_start_calls",
        "oauth_poll_calls",
        "oauth_refresh_calls",
        "account_fetch_calls",
    ] {
        let count = object
            .get(counter)
            .and_then(serde_json::Value::as_i64)
            .ok_or(DownloadError::Executor)?;
        if !(0..i64::MAX).contains(&count) {
            return Err(DownloadError::Executor);
        }
    }
    Ok(())
}

fn credential() -> OAuthCredential {
    // Fixed, nonfunctional synthetic credentials. No input secrets are echoed.
    OAuthCredential {
        access_token: "yoink-fixture-synthetic-access-not-a-real-token".into(),
        refresh_token: "yoink-fixture-synthetic-refresh-not-a-real-token".into(),
        expires_at: "2099-01-01T00:00:00Z".into(),
    }
}

impl OAuthProvider for FixtureCookieProviders {
    fn start(&self) -> ProviderFuture<'_, ProviderDevice> {
        Box::pin(async move {
            let mode = self.called(
                "UPDATE bot_settings SET value=json_set(value, '$.oauth_start_calls', json_extract(value, '$.oauth_start_calls')+1), updated_at=0 WHERE key='fixture_cookie_provider_control'",
                "SELECT json_extract(value, '$.oauth_mode') FROM bot_settings WHERE key='fixture_cookie_provider_control'",
            ).await?;
            if mode == "start_error" {
                return Err(DownloadError::Executor);
            }
            Ok(ProviderDevice {
                device_code: "yoink-fixture-synthetic-device-not-a-real-id".into(),
                // The native service requires a Google/YouTube HTTPS origin.
                // This is display-only: this provider never opens it. Browser
                // scenarios must not follow the external verification link.
                verification_url: "https://www.google.com/device".into(),
                user_code: "FIXTURE-OFFLINE".into(),
                expires_in: 1800,
                interval: 1,
            })
        })
    }

    fn poll<'a>(&'a self, _: &'a str, _: i64) -> ProviderFuture<'a, ProviderPoll> {
        Box::pin(async move {
            let mode = self.called(
                "UPDATE bot_settings SET value=json_set(value, '$.oauth_poll_calls', json_extract(value, '$.oauth_poll_calls')+1), updated_at=0 WHERE key='fixture_cookie_provider_control'",
                "SELECT json_extract(value, '$.oauth_mode') FROM bot_settings WHERE key='fixture_cookie_provider_control'",
            ).await?;
            Ok(match mode.as_str() {
                "pending" => ProviderPoll::Pending,
                "ok" => ProviderPoll::Ready(credential()),
                "expired" => ProviderPoll::Expired,
                "error" | "start_error" => ProviderPoll::Error,
                _ => return Err(DownloadError::Executor),
            })
        })
    }

    fn refresh<'a>(
        &'a self,
        _: &'a OAuthCredential,
        _: i64,
    ) -> ProviderFuture<'a, OAuthCredential> {
        Box::pin(async move {
            let mode = self.called(
                "UPDATE bot_settings SET value=json_set(value, '$.oauth_refresh_calls', json_extract(value, '$.oauth_refresh_calls')+1), updated_at=0 WHERE key='fixture_cookie_provider_control'",
                "SELECT json_extract(value, '$.oauth_mode') FROM bot_settings WHERE key='fixture_cookie_provider_control'",
            ).await?;
            match mode.as_str() {
                "ok" => Ok(credential()),
                _ => Err(DownloadError::Executor),
            }
        })
    }
}

// Pre-admission refresh uses a distinct trait from the cookie UI provider.
// Delegate to the same offline mode/counter implementation, never live HTTP.
impl OAuthRefreshProvider for FixtureCookieProviders {
    fn exchange<'a>(
        &'a self,
        tokens: &'a OAuthCredential,
    ) -> Pin<Box<dyn Future<Output = Result<OAuthCredential, ProfileError>> + Send + 'a>> {
        Box::pin(async move {
            let result = OAuthProvider::refresh(self, tokens, 0)
                .await
                .map_err(|_| ProfileError::Unavailable);
            self.refresh_gate.wait().await?;
            result
        })
    }
}

impl AccountProvider for FixtureCookieProviders {
    fn fetch<'a>(&'a self, _: &'a str, _: &'a str, _: i64) -> ProviderFuture<'a, AccountResult> {
        Box::pin(async move {
            let mode = self.called(
                "UPDATE bot_settings SET value=json_set(value, '$.account_fetch_calls', json_extract(value, '$.account_fetch_calls')+1), updated_at=0 WHERE key='fixture_cookie_provider_control'",
                "SELECT json_extract(value, '$.account_mode') FROM bot_settings WHERE key='fixture_cookie_provider_control'",
            ).await?;
            let info = match mode.as_str() {
                "valid" => Some(AccountInfo {
                    label: "Offline fixture account".into(),
                    avatar_url: None,
                }),
                "invalid" => None,
                _ => return Err(DownloadError::Executor),
            };
            Ok(AccountResult {
                info,
                updates: BTreeMap::new(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn synthetic_credentials_are_valid_and_debug_redacted() {
        let value = credential();
        value.validate().unwrap();
        assert_eq!(format!("{value:?}"), "YoutubeOAuth([REDACTED])");
        assert_eq!(credential(), value);
    }

    #[tokio::test]
    async fn refresh_gate_waits_after_provider_work_without_database_guard() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .await
                .unwrap();
        }
        let store = Arc::new(
            Store::open(
                &directory.path().join("refresh.db"),
                true,
                Duration::from_secs(2),
            )
            .await
            .unwrap(),
        );
        store.migrate().await.unwrap();
        let gate = Arc::new(RefreshGate::default());
        let (_, _, refresh) = install_with_gate(store.clone(), gate.clone())
            .await
            .unwrap();
        store.database().connection().await.unwrap().execute(
            "UPDATE bot_settings SET value=json_set(value,'$.oauth_mode','ok') WHERE key='fixture_cookie_provider_control'", ()
        ).await.unwrap();
        gate.command("oauth_refresh_arm").unwrap();
        let task = tokio::spawn(async move { refresh.exchange(&credential()).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            while gate.command("oauth_refresh_state").unwrap()["entered"] != true {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let connection =
            tokio::time::timeout(Duration::from_secs(1), store.database().connection())
                .await
                .unwrap()
                .unwrap();
        assert!(!task.is_finished());
        let mut rows = connection.query("SELECT json_extract(value,'$.oauth_refresh_calls') FROM bot_settings WHERE key='fixture_cookie_provider_control'", ()).await.unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            1
        );
        drop(rows);
        gate.command("oauth_refresh_release").unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
        drop(connection);
        store.close().await.unwrap();
    }

    #[tokio::test]
    async fn modes_counters_and_restart_are_deterministic_without_network() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let store = Arc::new(
            Store::open(
                &directory.path().join("providers.db"),
                true,
                Duration::from_secs(2),
            )
            .await
            .unwrap(),
        );
        store.migrate().await.unwrap();
        let (oauth, account, refresh) = install(store.clone()).await.unwrap();
        store.verify_schema().await.unwrap();
        let device = oauth.start().await.unwrap();
        assert_eq!(device.interval, 1);
        assert!(matches!(
            oauth.poll("ignored", 1).await.unwrap(),
            ProviderPoll::Pending
        ));
        for mode in ["ok", "expired", "error", "start_error"] {
            store
                .database()
                .connection()
                .await
                .unwrap()
                .execute(
                    "UPDATE bot_settings SET value=json_set(value, '$.oauth_mode', ?1), updated_at=0 WHERE key='fixture_cookie_provider_control'",
                    [mode],
                )
                .await
                .unwrap();
            let result = oauth.poll("ignored", 1).await.unwrap();
            assert!(matches!(
                (mode, result),
                ("ok", ProviderPoll::Ready(_))
                    | ("expired", ProviderPoll::Expired)
                    | ("error" | "start_error", ProviderPoll::Error)
            ));
            assert_eq!(oauth.start().await.is_err(), mode == "start_error");
            assert_eq!(oauth.refresh(&credential(), 1).await.is_ok(), mode == "ok");
            assert_eq!(refresh.exchange(&credential()).await.is_ok(), mode == "ok");
        }
        for mode in ["valid", "invalid", "error"] {
            store
                .database()
                .connection()
                .await
                .unwrap()
                .execute(
                    "UPDATE bot_settings SET value=json_set(value, '$.account_mode', ?1), updated_at=0 WHERE key='fixture_cookie_provider_control'",
                    [mode],
                )
                .await
                .unwrap();
            let result = account.fetch("ignored", "must-not-be-recorded", 1).await;
            match mode {
                "valid" => {
                    let result = result.unwrap();
                    assert!(result.info.is_some());
                    assert!(result.updates.is_empty());
                }
                "invalid" => assert!(result.unwrap().info.is_none()),
                _ => assert!(result.is_err()),
            }
        }
        // Reinstall preserves modes and counters, including failed calls.
        let _ = install(store.clone()).await.unwrap();
        let row = yoink_db::query("SELECT json_extract(value, '$.oauth_mode') AS oauth_mode, json_extract(value, '$.oauth_start_calls') AS oauth_start_calls, json_extract(value, '$.oauth_poll_calls') AS oauth_poll_calls, json_extract(value, '$.oauth_refresh_calls') AS oauth_refresh_calls, json_extract(value, '$.account_fetch_calls') AS account_fetch_calls FROM bot_settings WHERE key='fixture_cookie_provider_control'")
            .fetch_one(store.database())
            .await
            .unwrap();
        assert_eq!(
            row.try_get::<String, _>("oauth_mode").unwrap(),
            "start_error"
        );
        for (column, expected) in [
            ("oauth_start_calls", 5),
            ("oauth_poll_calls", 5),
            ("oauth_refresh_calls", 8),
            ("account_fetch_calls", 3),
        ] {
            assert_eq!(row.try_get::<i64, _>(column).unwrap(), expected);
        }
        // Malformed JSON, malformed shape, invalid modes and counters all
        // fail closed for every provider entry point, including admission.
        for value in [
            "not-json",
            "{}",
            r#"{"oauth_mode":"unknown"}"#,
            r#"{"oauth_mode":"ok","account_mode":"valid","oauth_start_calls":-1,"oauth_poll_calls":0,"oauth_refresh_calls":0,"account_fetch_calls":0}"#,
        ] {
            store
                .database()
                .connection()
                .await
                .unwrap()
                .execute(
                    "UPDATE bot_settings SET value=?1 WHERE key='fixture_cookie_provider_control'",
                    [value],
                )
                .await
                .unwrap();
            assert!(oauth.start().await.is_err());
            assert!(oauth.poll("ignored", 1).await.is_err());
            assert!(oauth.refresh(&credential(), 1).await.is_err());
            assert!(refresh.exchange(&credential()).await.is_err());
            assert!(account.fetch("ignored", "ignored", 1).await.is_err());
        }
        store
            .database()
            .connection()
            .await
            .unwrap()
            .execute(
                "DELETE FROM bot_settings WHERE key='fixture_cookie_provider_control'",
                (),
            )
            .await
            .unwrap();
        assert!(oauth.start().await.is_err());
        assert!(oauth.poll("ignored", 1).await.is_err());
        assert!(refresh.exchange(&credential()).await.is_err());
        assert!(account.fetch("ignored", "ignored", 1).await.is_err());
        store.verify_schema().await.unwrap();
        store.close().await.unwrap();
    }
}
