use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::JoinSet,
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use yoink_storage::Store;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_SQL_BYTES: usize = 512 * 1024;
const MAX_ROWS: usize = 1000;
const MAX_COLUMNS: usize = 128;
const MAX_CONNECTIONS: usize = 8;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

fn parse_request(bytes: &[u8]) -> Result<Request, FixtureError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| FixtureError("invalid_request"))?;
    let object = value.as_object().ok_or(FixtureError("invalid_request"))?;
    if object.len() != 3 {
        return Err(FixtureError("invalid_request"));
    }
    let token = object
        .get("token")
        .and_then(Value::as_str)
        .ok_or(FixtureError("invalid_request"))?;
    let sql = object
        .get("sql")
        .and_then(Value::as_str)
        .ok_or(FixtureError("invalid_request"))?;
    let mode = object
        .get("mode")
        .and_then(Value::as_str)
        .ok_or(FixtureError("invalid_request"))?;
    Ok(Request {
        token: token.to_owned(),
        sql: sql.to_owned(),
        mode: mode.to_owned(),
    })
}

#[derive(Debug)]
struct Request {
    token: String,
    sql: String,
    mode: String,
}

#[derive(Debug)]
struct FixtureError(&'static str);

impl FixtureError {
    fn response(&self) -> Value {
        json!({"ok": false, "error": self.0})
    }
}

fn validate_bind(value: &str) -> Result<SocketAddr, &'static str> {
    let address: SocketAddr = value.parse().map_err(|_| "invalid_bind")?;
    if address.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) || address.port() == 0 {
        return Err("invalid_bind");
    }
    Ok(address)
}

fn validate_token(value: &str) -> Result<(), &'static str> {
    if value.len() < 32 || !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
        return Err("invalid_token");
    }
    Ok(())
}

fn authenticated(provided: &str, expected: &str) -> bool {
    let a = provided.as_bytes();
    let b = expected.as_bytes();
    let mut difference = a.len() ^ b.len();
    for index in 0..a.len().max(b.len()) {
        difference |=
            usize::from(a.get(index).copied().unwrap_or(0) ^ b.get(index).copied().unwrap_or(0));
    }
    difference == 0
}

async fn execute(
    store: &Store,
    expected_token: &str,
    request: Request,
) -> Result<Value, FixtureError> {
    if !authenticated(&request.token, expected_token) {
        return Err(FixtureError("unauthorized"));
    }
    if request.sql.is_empty() || request.sql.len() > MAX_SQL_BYTES {
        return Err(FixtureError("invalid_request"));
    }
    let database = store.database();
    let connection = database
        .connection()
        .await
        .map_err(|_| FixtureError("database_error"))?;
    match request.mode.as_str() {
        "batch" => {
            connection
                .execute_batch(&request.sql)
                .await
                .map_err(|_| FixtureError("sql_error"))?;
            Ok(json!({"ok": true, "rows": []}))
        }
        "query" => {
            let mut cursor = connection
                .query(&request.sql, ())
                .await
                .map_err(|_| FixtureError("sql_error"))?;
            let mut rows = Vec::new();
            let mut response_bytes = 32usize;
            while let Some(row) = cursor.next().await.map_err(|_| FixtureError("sql_error"))? {
                if rows.len() >= MAX_ROWS || row.column_count() > MAX_COLUMNS {
                    return Err(FixtureError("result_too_large"));
                }
                let mut cells = Vec::with_capacity(row.column_count());
                for index in 0..row.column_count() {
                    let cell = match row
                        .get_value(index)
                        .map_err(|_| FixtureError("sql_error"))?
                    {
                        turso::Value::Null => Value::Null,
                        turso::Value::Integer(value) => Value::String(value.to_string()),
                        turso::Value::Real(value) if value.is_finite() => json!(value),
                        turso::Value::Real(_) => return Err(FixtureError("unsupported_value")),
                        turso::Value::Text(value) => Value::String(value),
                        turso::Value::Blob(_) => return Err(FixtureError("unsupported_value")),
                    };
                    let encoded_size = serde_json::to_vec(&cell)
                        .map_err(|_| FixtureError("unsupported_value"))?
                        .len();
                    response_bytes = response_bytes
                        .checked_add(encoded_size + 1)
                        .ok_or(FixtureError("result_too_large"))?;
                    if response_bytes > MAX_RESPONSE_BYTES {
                        return Err(FixtureError("result_too_large"));
                    }
                    cells.push(cell);
                }
                rows.push(cells);
            }
            Ok(json!({"ok": true, "rows": rows}))
        }
        _ => Err(FixtureError("invalid_request")),
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<Vec<u8>, FixtureError> {
    let reader = BufReader::new(stream);
    let mut line = Vec::new();
    timeout(
        REQUEST_TIMEOUT,
        reader
            .take((MAX_REQUEST_BYTES + 1) as u64)
            .read_until(b'\n', &mut line),
    )
    .await
    .map_err(|_| FixtureError("timeout"))?
    .map_err(|_| FixtureError("invalid_request"))?;
    if line.is_empty() || line.len() > MAX_REQUEST_BYTES || line.last() != Some(&b'\n') {
        return Err(FixtureError("invalid_request"));
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(line)
}

async fn handle_connection(mut stream: TcpStream, store: Arc<Store>, token: Arc<String>) {
    let response = match timeout(REQUEST_TIMEOUT, async {
        let bytes = read_request(&mut stream).await?;
        let request = parse_request(&bytes)?;
        execute(&store, &token, request).await
    })
    .await
    {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => error.response(),
        Err(_) => FixtureError("timeout").response(),
    };
    let mut encoded = serde_json::to_vec(&response)
        .unwrap_or_else(|_| b"{\"ok\":false,\"error\":\"internal_error\"}".to_vec());
    if encoded.len() > MAX_RESPONSE_BYTES {
        encoded = b"{\"ok\":false,\"error\":\"result_too_large\"}".to_vec();
    }
    if encoded.len() <= MAX_RESPONSE_BYTES {
        let _ = timeout(REQUEST_TIMEOUT, async {
            stream.write_all(&encoded).await?;
            stream.write_all(b"\n").await?;
            stream.shutdown().await
        })
        .await;
    }
}

pub async fn bind_from_env() -> Result<TcpListener, &'static str> {
    let bind = std::env::var("YOINK_FIXTURE_BIND").map_err(|_| "missing_bind")?;
    let address = validate_bind(&bind)?;
    TcpListener::bind(address).await.map_err(|_| "bind_failed")
}

pub fn token_from_env() -> Result<String, &'static str> {
    let token = std::env::var("YOINK_FIXTURE_TOKEN").map_err(|_| "missing_token")?;
    validate_token(&token)?;
    Ok(token)
}

pub async fn serve(
    listener: TcpListener,
    store: Arc<Store>,
    token: String,
    shutdown: CancellationToken,
) -> io::Result<()> {
    let semaphore = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            completed = tasks.join_next(), if !tasks.is_empty() => {
                if completed.is_some_and(|result| result.is_err()) {
                    tasks.abort_all();
                    while tasks.join_next().await.is_some() {}
                    return Err(io::Error::other("fixture connection task failed"));
                }
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        let _ = stream.try_write(b"{\"ok\":false,\"error\":\"busy\"}\n");
                        continue;
                    }
                };
                let store = store.clone();
                let token = Arc::new(token.clone());
                tasks.spawn(async move {
                    let _permit = permit;
                    handle_connection(stream, store, token).await;
                });
            }
        }
    }
    let drain = timeout(REQUEST_TIMEOUT, async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    if drain.is_err() {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
    Ok(())
}

pub async fn offline_request(store: &Store, token: &str, line: &[u8]) -> Value {
    let result = async {
        if line.len() > MAX_REQUEST_BYTES {
            return Err(FixtureError("invalid_request"));
        }
        let request = parse_request(line)?;
        execute(store, token, request).await
    }
    .await;
    result.unwrap_or_else(|error| error.response())
}

pub fn configured_path(path: &Path, store: &Store) -> bool {
    let expected = path
        .parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok())
        .and_then(|parent| path.file_name().map(|name| parent.join(name)));
    expected.as_deref() == Some(store.database().path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDb {
        directory: std::path::PathBuf,
        store: Store,
    }

    impl TempDb {
        async fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory =
                std::env::temp_dir().join(format!("yoink-fixture-{}-{nonce}", std::process::id()));
            fs::create_dir(&directory).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let store = Store::open(&directory.join("test.db"), true, Duration::from_secs(3))
                .await
                .unwrap();
            store.migrate().await.unwrap();
            Self { directory, store }
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    const TOKEN: &str = "fixture-test-token-that-is-at-least-32-chars";

    fn request(sql: &str, mode: &str) -> Request {
        Request {
            token: TOKEN.to_owned(),
            sql: sql.to_owned(),
            mode: mode.to_owned(),
        }
    }

    #[tokio::test]
    async fn auth_rejection_does_not_mutate_and_server_recovers() {
        let db = TempDb::new().await;
        let mut bad = request("CREATE TABLE forbidden(value)", "batch");
        bad.token = "wrong".into();
        assert_eq!(
            execute(&db.store, TOKEN, bad).await.unwrap_err().0,
            "unauthorized"
        );
        let result = execute(
            &db.store,
            TOKEN,
            request(
                "SELECT name FROM sqlite_schema WHERE name='forbidden'",
                "query",
            ),
        )
        .await
        .unwrap();
        assert_eq!(result["rows"], json!([]));
        assert_eq!(
            execute(&db.store, TOKEN, request("INVALID SQL", "query"))
                .await
                .unwrap_err()
                .0,
            "sql_error"
        );
        let result = execute(&db.store, TOKEN, request("SELECT 7", "query"))
            .await
            .unwrap();
        assert_eq!(result["rows"], json!([["7"]]));
    }

    #[tokio::test]
    async fn bounded_and_invalid_requests_are_rejected() {
        assert!(validate_bind("0.0.0.0:1234").is_err());
        assert!(validate_bind("127.0.0.1:0").is_err());
        assert!(validate_token("short").is_err());
        assert!(validate_token(&"x".repeat(MAX_REQUEST_BYTES + 1)).is_ok());
        let db = TempDb::new().await;
        assert_eq!(
            execute(&db.store, TOKEN, request("", "query"))
                .await
                .unwrap_err()
                .0,
            "invalid_request"
        );
        assert_eq!(
            execute(&db.store, TOKEN, request("SELECT 1", "mystery"))
                .await
                .unwrap_err()
                .0,
            "invalid_request"
        );
        let oversized = vec![b'x'; MAX_REQUEST_BYTES + 1];
        assert_eq!(
            offline_request(&db.store, TOKEN, &oversized).await["error"],
            "invalid_request"
        );
    }

    #[tokio::test]
    async fn query_values_and_batch_use_shared_database() {
        let db = TempDb::new().await;
        let response = execute(
            &db.store,
            TOKEN,
            request("SELECT 9007199254740993, 1.25, 'hello', NULL", "query"),
        )
        .await
        .unwrap();
        assert_eq!(
            response["rows"],
            json!([["9007199254740993", 1.25, "hello", null]])
        );
        let result = execute(
            &db.store,
            TOKEN,
            request(
                "CREATE TABLE fixture_data(value TEXT); INSERT INTO fixture_data VALUES ('ok')",
                "batch",
            ),
        )
        .await
        .unwrap();
        assert_eq!(result, json!({"ok": true, "rows": []}));
        let result = execute(
            &db.store,
            TOKEN,
            request("SELECT value FROM fixture_data", "query"),
        )
        .await
        .unwrap();
        assert_eq!(result["rows"], json!([["ok"]]));
    }

    #[tokio::test]
    async fn blobs_are_rejected_without_poisoning_connection() {
        let db = TempDb::new().await;
        assert_eq!(
            execute(&db.store, TOKEN, request("SELECT x'0102'", "query"))
                .await
                .unwrap_err()
                .0,
            "unsupported_value"
        );
        assert_eq!(
            execute(&db.store, TOKEN, request("SELECT 1", "query"))
                .await
                .unwrap()["rows"],
            json!([["1"]])
        );
    }
}
