use std::fmt;

pub enum Error {
    Engine(turso::Error),
    Closed,
    AlreadyOwned,
    InvalidPath,
    InsecureDirectory,
    Timeout,
    Conversion,
    NoRows,
    Task,
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(error) => f
                .debug_tuple("Engine")
                .field(&engine_category(error))
                .finish(),
            other => f.write_str(match other {
                Self::Closed => "Closed",
                Self::AlreadyOwned => "AlreadyOwned",
                Self::InvalidPath => "InvalidPath",
                Self::InsecureDirectory => "InsecureDirectory",
                Self::Timeout => "Timeout",
                Self::Conversion => "Conversion",
                Self::NoRows => "NoRows",
                Self::Task => "Task",
                Self::Engine(_) => unreachable!(),
            }),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Engine(_) => "database operation failed",
            Self::Closed => "database is closed",
            Self::AlreadyOwned => "database is already owned by another process",
            Self::InvalidPath => "database path is invalid or unavailable",
            Self::InsecureDirectory => "database parent directory permissions are insecure",
            Self::Timeout => "database operation timed out",
            Self::Conversion => "database value conversion failed",
            Self::NoRows => "query returned no rows",
            Self::Task => "database task failed",
        })
    }
}

impl std::error::Error for Error {}

impl From<turso::Error> for Error {
    #[track_caller]
    fn from(error: turso::Error) -> Self {
        let caller = std::panic::Location::caller();
        // Engine messages can contain SQL, bound values and credential paths.
        // Record only a closed classification and the Rust source location.
        tracing::warn!(
            category = engine_category(&error),
            source_file = caller.file(),
            source_line = caller.line(),
            "native database operation failed"
        );
        Self::Engine(error)
    }
}

fn engine_category(error: &turso::Error) -> &'static str {
    use std::io::ErrorKind;
    match error {
        turso::Error::ToSqlConversionFailure(_) => "parameter_conversion",
        turso::Error::QueryReturnedNoRows => "no_rows",
        turso::Error::ConversionFailure(_) => "value_conversion",
        turso::Error::Busy(_) => "busy",
        turso::Error::BusySnapshot(_) => "busy_snapshot",
        turso::Error::Interrupt(_) => "interrupted",
        turso::Error::Error(_) => "sql",
        turso::Error::Misuse(_) => "misuse",
        turso::Error::Constraint(_) => "constraint",
        turso::Error::Readonly(_) => "read_only",
        turso::Error::DatabaseFull(_) => "database_full",
        turso::Error::NotAdb(_) => "not_a_database",
        turso::Error::Corrupt(_) => "corrupt",
        turso::Error::IoError(kind, _) => match kind {
            ErrorKind::PermissionDenied => "io_permission_denied",
            ErrorKind::NotFound => "io_not_found",
            ErrorKind::StorageFull => "io_storage_full",
            ErrorKind::ReadOnlyFilesystem => "io_read_only_filesystem",
            _ => "io",
        },
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn engine_diagnostics_keep_categories_without_sql_values_or_paths() {
        let secret = "fake-secret-SQL-cookie-proxy-path";
        let cases = [
            (turso::Error::Error(secret.into()), "sql"),
            (turso::Error::Busy(secret.into()), "busy"),
            (turso::Error::BusySnapshot(secret.into()), "busy_snapshot"),
            (turso::Error::Constraint(secret.into()), "constraint"),
            (turso::Error::Readonly(secret.into()), "read_only"),
            (turso::Error::DatabaseFull(secret.into()), "database_full"),
            (turso::Error::Corrupt(secret.into()), "corrupt"),
            (turso::Error::NotAdb(secret.into()), "not_a_database"),
            (turso::Error::Misuse(secret.into()), "misuse"),
            (turso::Error::Interrupt(secret.into()), "interrupted"),
            (
                turso::Error::ConversionFailure(secret.into()),
                "value_conversion",
            ),
            (
                turso::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(secret))),
                "parameter_conversion",
            ),
            (turso::Error::QueryReturnedNoRows, "no_rows"),
            (
                turso::Error::IoError(std::io::ErrorKind::PermissionDenied, secret),
                "io_permission_denied",
            ),
        ];
        let capture = Capture(Arc::new(Mutex::new(Vec::new())));
        let writer = capture.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            for (error, category) in cases {
                let safe = Error::from(error);
                assert!(format!("{safe:?}").contains(category));
                assert!(!format!("{safe:?} {safe}").contains(secret));
                assert!(std::error::Error::source(&safe).is_none());
            }
        });
        let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert!(output.contains("io_permission_denied"));
        assert!(output.contains("source_line="));
        assert!(!output.contains(secret));
    }
}
