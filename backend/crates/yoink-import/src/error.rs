use thiserror::Error;

pub type Result<T> = std::result::Result<T, ImportError>;

/// Errors deliberately contain no driver detail, SQL, credentials, or row values.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum ImportError {
    #[error("invalid importer configuration")]
    Configuration,
    #[error("set YOINK_IMPORT_SOURCE_URL explicitly")]
    MissingSource,
    #[error("source connection or snapshot operation failed")]
    Source,
    #[error("source TLS verification failed; check the certificate, hostname and trust store")]
    Tls,
    #[error(
        "plaintext requires exclusively explicit numeric loopback addresses or absolute Unix sockets, without a CA override"
    )]
    PlaintextEndpoint,
    #[error("import interrupted")]
    Interrupted,
    #[error("source catalog does not match the supported legacy schema")]
    Catalog,
    #[error("source data cannot be represented safely")]
    Conversion,
    #[error("destination path or ownership checks failed")]
    Destination,
    #[error("destination storage operation failed")]
    Storage,
    #[error("import deadline exceeded")]
    Deadline,
    #[error("native engine lacks required import constraints")]
    EngineCapability,
    #[error("import publication completed, but parent durability could not be confirmed")]
    PublishedDurabilityUncertain,
    #[error("{stage} failed for table {table}: {source}")]
    AtTable {
        table: String,
        stage: &'static str,
        source: Box<ImportError>,
    },
}

impl ImportError {
    pub(crate) fn at_table(self, table: &str, stage: &'static str) -> Self {
        Self::AtTable {
            table: table.to_owned(),
            stage,
            source: Box::new(self),
        }
    }
}
