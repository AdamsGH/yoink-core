mod error;
mod files;
pub mod publication;

pub use error::Error;

use std::fmt;
use std::fs::{File, OpenOptions};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde_json::Value as JsonValue;
use tokio::sync::{Mutex, MutexGuard};
use turso::{Database as EngineDatabase, Value};

struct State {
    connection: Option<turso::Connection>,
    engine: Option<EngineDatabase>,
    lock: File,
}

struct Shared {
    state: Mutex<State>,
    path: PathBuf,
    acquire_timeout: Duration,
    file_identity: files::FileIdentity,
}

#[derive(Clone)]
pub struct Database(Arc<Shared>);

pub struct ConnectionGuard<'a>(MutexGuard<'a, State>);

impl Deref for ConnectionGuard<'_> {
    type Target = turso::Connection;
    fn deref(&self) -> &Self::Target {
        self.0
            .connection
            .as_ref()
            .expect("guard only exists while open")
    }
}

impl DerefMut for ConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
            .connection
            .as_mut()
            .expect("guard only exists while open")
    }
}

impl Database {
    pub async fn open(path: &Path, create: bool, acquire_timeout: Duration) -> Result<Self, Error> {
        if acquire_timeout.is_zero() || acquire_timeout > Duration::from_secs(300) {
            return Err(Error::InvalidPath);
        }
        if path.as_os_str().is_empty() {
            return Err(Error::InvalidPath);
        }
        let input_parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = std::fs::canonicalize(input_parent).map_err(|_| Error::InvalidPath)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::metadata(&parent)
                .map_err(|_| Error::InvalidPath)?
                .permissions()
                .mode();
            if permissions & 0o077 != 0 {
                return Err(Error::InsecureDirectory);
            }
        }
        let name = path.file_name().ok_or(Error::InvalidPath)?;
        let path = parent.join(name);
        let lock_path = sidecar_path(&path);
        reject_unsafe_path(&path, true)?;
        reject_unsafe_path(&lock_path, false)?;

        let mut lock_options = OpenOptions::new();
        lock_options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            lock_options.mode(0o600);
        }
        let lock = lock_options
            .open(&lock_path)
            .map_err(|_| Error::InvalidPath)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            lock.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| Error::InvalidPath)?;
            if lock.metadata().map_err(|_| Error::InvalidPath)?.nlink() != 1 {
                return Err(Error::InvalidPath);
            }
        }
        match lock.try_lock_exclusive() {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(Error::AlreadyOwned);
            }
            Err(_) => return Err(Error::InvalidPath),
        }

        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_file() {
                    let _ = FileExt::unlock(&lock);
                    return Err(Error::InvalidPath);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if metadata.nlink() != 1 {
                        let _ = FileExt::unlock(&lock);
                        return Err(Error::InvalidPath);
                    }
                }
                if create {
                    let _ = FileExt::unlock(&lock);
                    return Err(Error::InvalidPath);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => {
                let _ = FileExt::unlock(&lock);
                return Err(Error::InvalidPath);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                #[cfg(unix)]
                let created = {
                    use std::os::unix::fs::OpenOptionsExt;
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&path)
                };
                #[cfg(not(unix))]
                let created = OpenOptions::new().write(true).create_new(true).open(&path);
                if created.is_err() {
                    let _ = FileExt::unlock(&lock);
                    return Err(Error::InvalidPath);
                }
            }
            Err(_) => {
                let _ = FileExt::unlock(&lock);
                return Err(Error::InvalidPath);
            }
        }

        let database_path = match path.to_str() {
            Some(path) => path,
            None => {
                let _ = FileExt::unlock(&lock);
                return Err(Error::InvalidPath);
            }
        };
        let engine = match turso::Builder::new_local(database_path).build().await {
            Ok(db) => db,
            Err(error) => {
                let _ = FileExt::unlock(&lock);
                return Err(error.into());
            }
        };
        let connection = match engine.connect() {
            Ok(connection) => connection,
            Err(error) => {
                drop(engine);
                let _ = FileExt::unlock(&lock);
                return Err(error.into());
            }
        };
        if let Err(error) = configure(&connection).await {
            drop(connection);
            drop(engine);
            let _ = FileExt::unlock(&lock);
            return Err(error);
        }
        let file_identity = files::FileIdentity::read(&path)?;
        Ok(Self(Arc::new(Shared {
            state: Mutex::new(State {
                connection: Some(connection),
                engine: Some(engine),
                lock,
            }),
            path,
            acquire_timeout,
            file_identity,
        })))
    }

    pub async fn connection(&self) -> Result<ConnectionGuard<'_>, Error> {
        let guard = tokio::time::timeout(self.0.acquire_timeout, self.0.state.lock())
            .await
            .map_err(|_| Error::Timeout)?;
        if guard.connection.is_none() {
            return Err(Error::Closed);
        }
        Ok(ConnectionGuard(guard))
    }

    /// Detect removed/replaced files and permission drift before reporting health.
    /// On Unix, both database and ownership-lock inode identities are checked.
    pub async fn verify_files(&self) -> Result<(), Error> {
        let guard = self.connection().await?;
        let lock_metadata = guard.0.lock.metadata().map_err(|_| Error::InvalidPath)?;
        let lock_identity = files::FileIdentity::from_metadata(&lock_metadata);
        let file_identity = self.0.file_identity;
        let path = self.0.path.clone();
        tokio::task::spawn_blocking(move || files::verify(&path, file_identity, lock_identity))
            .await
            .map_err(|_| Error::Task)?
    }

    pub async fn close(&self) -> Result<(), Error> {
        let mut state = self.0.state.lock().await;
        if let Some(connection) = state.connection.take() {
            drop(connection);
            drop(state.engine.take());
            FileExt::unlock(&state.lock).map_err(|_| Error::InvalidPath)?;
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.0.path
    }
}

fn reject_unsafe_path(path: &Path, database: bool) -> Result<(), Error> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || (database && !metadata.file_type().is_file()) {
                return Err(Error::InvalidPath);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() != 1 {
                    return Err(Error::InvalidPath);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err(Error::InvalidPath),
    }
    Ok(())
}

fn sidecar_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".yoink-lock");
    PathBuf::from(name)
}

async fn configure(connection: &turso::Connection) -> Result<(), Error> {
    connection
        .execute_batch(include_str!("../sql/configure.sql"))
        .await?;
    Ok(())
}

#[derive(Clone)]
pub enum BindValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl fmt::Debug for BindValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Null => "Null",
            Self::Integer(_) => "Integer(..)",
            Self::Real(_) => "Real(..)",
            Self::Text(_) => "Text(..)",
            Self::Blob(_) => "Blob(..)",
        })
    }
}

pub trait ToBindValue {
    fn to_bind_value(&self) -> Result<BindValue, Error>;
}

macro_rules! signed_bind {
    ($($ty:ty),*) => {$(
        impl ToBindValue for $ty {
            fn to_bind_value(&self) -> Result<BindValue, Error> { Ok(BindValue::Integer(*self as i64)) }
        }
    )*};
}
signed_bind!(i8, i16, i32, i64, isize);
impl ToBindValue for f32 {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        if self.is_finite() {
            Ok(BindValue::Real(*self as f64))
        } else {
            Err(Error::Conversion)
        }
    }
}
impl ToBindValue for f64 {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        if self.is_finite() {
            Ok(BindValue::Real(*self))
        } else {
            Err(Error::Conversion)
        }
    }
}

macro_rules! unsigned_bind {
    ($($ty:ty),*) => {$(
        impl ToBindValue for $ty {
            fn to_bind_value(&self) -> Result<BindValue, Error> {
                i64::try_from(*self).map(BindValue::Integer).map_err(|_| Error::Conversion)
            }
        }
    )*};
}
unsigned_bind!(u8, u16, u32, u64, usize);

impl ToBindValue for bool {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        Ok(BindValue::Integer(i64::from(*self)))
    }
}
impl ToBindValue for str {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        Ok(BindValue::Text(self.to_owned()))
    }
}
impl ToBindValue for String {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        self.as_str().to_bind_value()
    }
}
impl ToBindValue for Vec<u8> {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        Ok(BindValue::Blob(self.clone()))
    }
}
impl ToBindValue for [u8] {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        Ok(BindValue::Blob(self.to_vec()))
    }
}
impl ToBindValue for JsonValue {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        serde_json::to_string(self)
            .map(BindValue::Text)
            .map_err(|_| Error::Conversion)
    }
}
impl ToBindValue for Vec<i32> {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        serde_json::to_string(self)
            .map(BindValue::Text)
            .map_err(|_| Error::Conversion)
    }
}
impl ToBindValue for uuid::Uuid {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        Ok(BindValue::Text(self.to_string()))
    }
}
impl ToBindValue for DateTime<Utc> {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        Ok(BindValue::Integer(self.timestamp_micros()))
    }
}
impl<T: ToBindValue> ToBindValue for Option<T> {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        self.as_ref()
            .map_or(Ok(BindValue::Null), ToBindValue::to_bind_value)
    }
}
impl<T: ToBindValue + ?Sized> ToBindValue for &T {
    fn to_bind_value(&self) -> Result<BindValue, Error> {
        (**self).to_bind_value()
    }
}

pub struct Query {
    sql: String,
    values: Vec<Result<BindValue, Error>>,
}
impl fmt::Debug for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Query")
            .field("bind_count", &self.values.len())
            .field(
                "bind_types",
                &self
                    .values
                    .iter()
                    .map(|v| match v {
                        Ok(BindValue::Null) => "Null",
                        Ok(BindValue::Integer(_)) => "Integer",
                        Ok(BindValue::Real(_)) => "Real",
                        Ok(BindValue::Text(_)) => "Text",
                        Ok(BindValue::Blob(_)) => "Blob",
                        Err(_) => "Invalid",
                    })
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}
pub fn query(sql: &str) -> Query {
    Query {
        sql: sql.to_owned(),
        values: Vec::new(),
    }
}
impl Query {
    pub fn bind(mut self, value: impl ToBindValue) -> Self {
        self.values.push(value.to_bind_value());
        self
    }
    fn params(&self) -> Result<Vec<Value>, Error> {
        self.values
            .iter()
            .map(|value| {
                Ok(match value.as_ref().map_err(|_| Error::Conversion)? {
                    BindValue::Null => Value::Null,
                    BindValue::Integer(value) => Value::Integer(*value),
                    BindValue::Real(value) => Value::Real(*value),
                    BindValue::Text(value) => Value::Text(value.clone()),
                    BindValue::Blob(value) => Value::Blob(value.clone()),
                })
            })
            .collect::<Result<Vec<_>, Error>>()
    }
    pub async fn fetch_all<E: Executor>(self, executor: &E) -> Result<Vec<Row>, Error> {
        executor.fetch(self).await
    }
    pub async fn fetch_optional<E: Executor>(self, executor: &E) -> Result<Option<Row>, Error> {
        Ok(executor.fetch(self).await?.into_iter().next())
    }
    pub async fn fetch_one<E: Executor>(self, executor: &E) -> Result<Row, Error> {
        self.fetch_optional(executor).await?.ok_or(Error::NoRows)
    }
    pub async fn execute<E: Executor>(self, executor: &E) -> Result<ExecuteResult, Error> {
        executor.run(self).await
    }
}

pub trait Executor {
    fn fetch<'a>(
        &'a self,
        query: Query,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Vec<Row>, Error>> + Send + 'a>>;
    fn run<'a>(
        &'a self,
        query: Query,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ExecuteResult, Error>> + Send + 'a>>;
    fn batch<'a>(
        &'a self,
        sql: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>>;
}
impl Executor for Database {
    fn fetch<'a>(
        &'a self,
        query: Query,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Vec<Row>, Error>> + Send + 'a>> {
        Box::pin(async move {
            let conn = self.connection().await?;
            execute_query(&conn, query).await
        })
    }
    fn run<'a>(
        &'a self,
        query: Query,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ExecuteResult, Error>> + Send + 'a>> {
        Box::pin(async move {
            let conn = self.connection().await?;
            execute_statement(&conn, query).await
        })
    }
    fn batch<'a>(
        &'a self,
        sql: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let conn = self.connection().await?;
            conn.execute_batch(sql).await?;
            Ok(())
        })
    }
}
impl Executor for turso::Connection {
    fn fetch<'a>(
        &'a self,
        query: Query,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Vec<Row>, Error>> + Send + 'a>> {
        Box::pin(async move { execute_query(self, query).await })
    }
    fn run<'a>(
        &'a self,
        query: Query,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ExecuteResult, Error>> + Send + 'a>> {
        Box::pin(async move { execute_statement(self, query).await })
    }
    fn batch<'a>(
        &'a self,
        sql: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            self.execute_batch(sql).await?;
            Ok(())
        })
    }
}
async fn execute_query(connection: &turso::Connection, query: Query) -> Result<Vec<Row>, Error> {
    let mut cursor = connection.query(&query.sql, query.params()?).await?;
    let names = cursor.column_names().to_vec();
    let mut rows = Vec::new();
    while let Some(row) = cursor.next().await? {
        rows.push(Row {
            row,
            names: names.clone(),
        });
    }
    Ok(rows)
}
async fn execute_statement(
    connection: &turso::Connection,
    query: Query,
) -> Result<ExecuteResult, Error> {
    let result = connection.execute(&query.sql, query.params()?).await?;
    Ok(ExecuteResult {
        rows_affected: result,
    })
}

pub struct Row {
    row: turso::Row,
    names: Vec<String>,
}
impl fmt::Debug for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Row")
            .field("column_count", &self.names.len())
            .finish()
    }
}
impl Row {
    pub fn try_get<T: FromColumn, I: ColumnIndex>(&self, index: I) -> Result<T, Error> {
        T::from_value(self.row.get_value(index.index(&self.names)?)?)
    }
}
pub trait ColumnIndex {
    fn index(self, names: &[String]) -> Result<usize, Error>;
}
impl ColumnIndex for usize {
    fn index(self, _names: &[String]) -> Result<usize, Error> {
        Ok(self)
    }
}
impl ColumnIndex for &str {
    fn index(self, names: &[String]) -> Result<usize, Error> {
        names
            .iter()
            .position(|name| name == self)
            .ok_or(Error::Conversion)
    }
}
impl ColumnIndex for String {
    fn index(self, names: &[String]) -> Result<usize, Error> {
        names
            .iter()
            .position(|name| name == &self)
            .ok_or(Error::Conversion)
    }
}
pub trait FromColumn: Sized {
    fn from_value(value: Value) -> Result<Self, Error>;
}

impl FromColumn for bool {
    fn from_value(value: Value) -> Result<Self, Error> {
        match value {
            Value::Integer(0) => Ok(false),
            Value::Integer(1) => Ok(true),
            _ => Err(Error::Conversion),
        }
    }
}
macro_rules! integer_column {
    ($($ty:ty),*) => {$(
        impl FromColumn for $ty {
            fn from_value(value: Value) -> Result<Self, Error> {
                match value { Value::Integer(value) => <$ty>::try_from(value).map_err(|_| Error::Conversion), _ => Err(Error::Conversion) }
            }
        }
    )*};
}
integer_column!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);
impl FromColumn for f32 {
    fn from_value(value: Value) -> Result<Self, Error> {
        match value {
            Value::Real(v) => Ok(v as f32),
            Value::Integer(v) => Ok(v as f32),
            _ => Err(Error::Conversion),
        }
    }
}
impl FromColumn for f64 {
    fn from_value(value: Value) -> Result<Self, Error> {
        match value {
            Value::Real(v) => Ok(v),
            Value::Integer(v) => Ok(v as f64),
            _ => Err(Error::Conversion),
        }
    }
}
impl FromColumn for String {
    fn from_value(value: Value) -> Result<Self, Error> {
        match value {
            Value::Text(value) => Ok(value),
            _ => Err(Error::Conversion),
        }
    }
}
impl FromColumn for Vec<u8> {
    fn from_value(value: Value) -> Result<Self, Error> {
        match value {
            Value::Blob(value) => Ok(value),
            _ => Err(Error::Conversion),
        }
    }
}
impl FromColumn for JsonValue {
    fn from_value(value: Value) -> Result<Self, Error> {
        match value {
            Value::Text(value) => serde_json::from_str(&value).map_err(|_| Error::Conversion),
            _ => Err(Error::Conversion),
        }
    }
}
impl FromColumn for Vec<i32> {
    fn from_value(value: Value) -> Result<Self, Error> {
        match value {
            Value::Text(value) => serde_json::from_str(&value).map_err(|_| Error::Conversion),
            _ => Err(Error::Conversion),
        }
    }
}
impl FromColumn for uuid::Uuid {
    fn from_value(value: Value) -> Result<Self, Error> {
        match value {
            Value::Text(value) => uuid::Uuid::parse_str(&value).map_err(|_| Error::Conversion),
            _ => Err(Error::Conversion),
        }
    }
}
impl FromColumn for DateTime<Utc> {
    fn from_value(value: Value) -> Result<Self, Error> {
        match value {
            Value::Integer(value) => {
                DateTime::from_timestamp_micros(value).ok_or(Error::Conversion)
            }
            _ => Err(Error::Conversion),
        }
    }
}
impl<T: FromColumn> FromColumn for Option<T> {
    fn from_value(value: Value) -> Result<Self, Error> {
        if matches!(value, Value::Null) {
            Ok(None)
        } else {
            T::from_value(value).map(Some)
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecuteResult {
    rows_affected: u64,
}
impl ExecuteResult {
    pub fn rows_affected(self) -> u64 {
        self.rows_affected
    }
}

pub struct RawSql<'a> {
    sql: &'a str,
}
pub fn raw_sql(sql: &str) -> RawSql<'_> {
    RawSql { sql }
}
impl RawSql<'_> {
    pub async fn execute<E: Executor>(self, executor: &E) -> Result<(), Error> {
        executor.batch(self.sql).await
    }
}
