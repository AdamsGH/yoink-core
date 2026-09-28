use serde_json::Value;
use tokio_postgres::Client;

use crate::{ImportError, Result};

const CANONICAL_CATALOG: &str = include_str!("../../../schema/legacy-postgres.json");
const CATALOG_SQL: &str = include_str!("../../../scripts/sql/legacy-catalog.sql");

#[derive(Clone, Debug)]
pub struct ValidatedCatalog {
    pub document: Value,
    pub application_tables: Vec<String>,
}

/// Acquire ACCESS SHARE locks in canonical order before catalog reads. Since
/// identifiers come only from the embedded revision contract, no source-provided
/// identifier enters generated SQL.
pub async fn lock_expected_tables(client: &Client) -> Result<()> {
    let expected: Value =
        serde_json::from_str(CANONICAL_CATALOG).map_err(|_| ImportError::Catalog)?;
    let mut names: Vec<String> = expected["tables"]
        .as_array()
        .ok_or(ImportError::Catalog)?
        .iter()
        .filter_map(|table| table["name"].as_str().map(str::to_owned))
        .collect();
    names.sort();
    if names.len() != 42 {
        return Err(ImportError::Catalog);
    }
    for table in names {
        let quoted = table.replace('"', "\"\"");
        client
            .batch_execute(&format!(
                "LOCK TABLE public.\"{quoted}\" IN ACCESS SHARE MODE"
            ))
            .await
            .map_err(|_| ImportError::Source)?;
    }
    Ok(())
}

pub async fn validate(client: &Client) -> Result<ValidatedCatalog> {
    let unsupported: bool = client
        .query_one(include_str!("../sql/source-unsupported.sql"), &[])
        .await
        .map_err(|_| ImportError::Source)?
        .get(0);
    if unsupported {
        return Err(ImportError::Catalog);
    }
    let row = client
        .query_one(CATALOG_SQL, &[])
        .await
        .map_err(|_| ImportError::Source)?;
    let actual: Value = row.try_get(0).map_err(|_| ImportError::Catalog)?;
    let expected: Value =
        serde_json::from_str(CANONICAL_CATALOG).map_err(|_| ImportError::Catalog)?;
    if actual != expected {
        return Err(ImportError::Catalog);
    }
    let tables = actual["tables"].as_array().ok_or(ImportError::Catalog)?;
    if tables.len() != 42 {
        return Err(ImportError::Catalog);
    }
    let names: Vec<String> = tables
        .iter()
        .filter_map(|table| {
            table["name"]
                .as_str()
                .filter(|name| *name != "alembic_version")
                .map(str::to_owned)
        })
        .collect();
    if names.len() != 41 {
        return Err(ImportError::Catalog);
    }
    Ok(ValidatedCatalog {
        document: actual,
        application_tables: names,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_contract_has_expected_revision_and_table_count() {
        let value: Value = serde_json::from_str(CANONICAL_CATALOG).unwrap();
        assert_eq!(value["revision"], "0054_restore_stats_latest_view");
        assert_eq!(value["tables"].as_array().unwrap().len(), 42);
    }
}
