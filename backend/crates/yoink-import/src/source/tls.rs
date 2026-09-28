use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use rustls::{ClientConfig, RootCertStore};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::{ImportError, Result};

/// Synchronous certificate-store I/O, called only on the blocking pool.
pub(super) fn connector(ca: Option<&Path>) -> Result<MakeRustlsConnect> {
    let certificates = if let Some(path) = ca {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| ImportError::Tls)?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ImportError::Tls)?;
        if bytes.len() > 1024 * 1024 {
            return Err(ImportError::Tls);
        }
        rustls_pemfile::certs(&mut BufReader::new(bytes.as_slice()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| ImportError::Tls)?
    } else {
        rustls_native_certs::load_native_certs().certs
    };
    let mut roots = RootCertStore::empty();
    let (accepted, _) = roots.add_parsable_certificates(certificates);
    if accepted == 0 {
        return Err(ImportError::Tls);
    }
    let config =
        ClientConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
            .with_safe_default_protocol_versions()
            .map_err(|_| ImportError::Tls)?
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(MakeRustlsConnect::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rejects_empty_invalid_or_missing_custom_trust_store() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        assert!(connector(Some(file.path())).is_err());
        file.write_all(b"not a certificate").unwrap();
        assert!(connector(Some(file.path())).is_err());
        let missing = file.path().with_extension("missing");
        assert!(connector(Some(&missing)).is_err());
    }
}
