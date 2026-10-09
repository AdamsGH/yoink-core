use sha2::{Digest, Sha256};
use turso::{Connection, Value};

use super::{Column, Table, ident};
use crate::{ImportError, Result};

const DIGEST_DOMAIN: &[u8] = b"yoink-import-row-v1";

fn hash_field(hash: &mut Sha256, tag: u8, bytes: &[u8]) -> Result<()> {
    let length = u64::try_from(bytes.len()).map_err(|_| ImportError::Conversion)?;
    hash.update([tag]);
    hash.update(length.to_be_bytes());
    hash.update(bytes);
    Ok(())
}

fn value_for_column(value: Value, column: &Column) -> Result<Value> {
    match (column.udt.as_str(), value) {
        (_, Value::Null) => Ok(Value::Null),
        ("float4" | "float8", Value::Integer(value)) => Ok(Value::Real(value as f64)),
        ("float4" | "float8", Value::Real(value)) if value.is_finite() => {
            Ok(Value::Real(if value == 0.0 { 0.0 } else { value }))
        }
        (_, value) => Ok(value),
    }
}

pub(super) fn digest_values(hash: &mut Sha256, values: &[Value]) -> Result<()> {
    let columns = u64::try_from(values.len()).map_err(|_| ImportError::Conversion)?;
    hash.update(DIGEST_DOMAIN);
    hash.update(columns.to_be_bytes());
    for value in values {
        match value {
            Value::Null => hash_field(hash, 0, &[])?,
            Value::Text(value) => hash_field(hash, 1, value.as_bytes())?,
            Value::Integer(value) => hash_field(hash, 2, &value.to_be_bytes())?,
            Value::Real(value) => {
                if !value.is_finite() {
                    return Err(ImportError::Conversion);
                }
                let normalized = if *value == 0.0 { 0.0 } else { *value };
                hash_field(hash, 3, &normalized.to_bits().to_be_bytes())?;
            }
            Value::Blob(value) => hash_field(hash, 4, value)?,
        }
    }
    hash.update([0xff]);
    Ok(())
}

pub(super) async fn verify_content(
    connection: &Connection,
    table: &Table,
    expected: &[u8; 32],
    count: u64,
) -> Result<()> {
    let projection = table
        .columns
        .iter()
        .map(|column| ident(&column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let ordering = table
        .primary_key
        .iter()
        .map(|column| ident(column))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {projection} FROM {} ORDER BY {ordering}",
        ident(&table.name)
    );
    let mut rows = connection
        .query(&sql, ())
        .await
        .map_err(|_| ImportError::Storage)?;
    let mut digest = Sha256::new();
    let mut actual = 0u64;
    while let Some(row) = rows.next().await.map_err(|_| ImportError::Storage)? {
        let mut values = Vec::with_capacity(table.columns.len());
        for (index, column) in table.columns.iter().enumerate() {
            let value = row.get_value(index).map_err(|_| ImportError::Storage)?;
            values.push(value_for_column(value, column)?);
        }
        digest_values(&mut digest, &values)?;
        actual = actual.checked_add(1).ok_or(ImportError::Conversion)?;
    }
    if actual != count || digest.finalize().as_slice() != expected {
        return Err(ImportError::Storage);
    }
    Ok(())
}

pub(super) async fn verify_integrity(connection: &Connection) -> Result<()> {
    let mut rows = connection
        .query("PRAGMA integrity_check", ())
        .await
        .map_err(|_| ImportError::EngineCapability)?;
    let first = rows
        .next()
        .await
        .map_err(|_| ImportError::EngineCapability)?
        .ok_or(ImportError::EngineCapability)?;
    if first
        .get_value(0)
        .map_err(|_| ImportError::EngineCapability)?
        != Value::Text("ok".to_owned())
    {
        return Err(ImportError::EngineCapability);
    }
    if rows
        .next()
        .await
        .map_err(|_| ImportError::EngineCapability)?
        .is_some()
    {
        return Err(ImportError::EngineCapability);
    }
    Ok(())
}

pub(super) async fn checkpoint(connection: &Connection) -> Result<()> {
    let mut rows = connection
        .query("PRAGMA wal_checkpoint(TRUNCATE)", ())
        .await
        .map_err(|_| ImportError::EngineCapability)?;
    let row = rows
        .next()
        .await
        .map_err(|_| ImportError::EngineCapability)?
        .ok_or(ImportError::EngineCapability)?;
    let busy = row
        .get_value(0)
        .map_err(|_| ImportError::EngineCapability)?;
    let log = row
        .get_value(1)
        .map_err(|_| ImportError::EngineCapability)?;
    let checkpointed = row
        .get_value(2)
        .map_err(|_| ImportError::EngineCapability)?;
    if rows
        .next()
        .await
        .map_err(|_| ImportError::EngineCapability)?
        .is_some()
    {
        return Err(ImportError::EngineCapability);
    }
    match (busy, log, checkpointed) {
        (Value::Integer(busy), Value::Integer(log), Value::Integer(checkpointed))
            if busy == 0 && log >= 0 && checkpointed >= 0 && log == checkpointed =>
        {
            Ok(())
        }
        _ => Err(ImportError::EngineCapability),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(values: &[Value]) -> [u8; 32] {
        let mut hash = Sha256::new();
        digest_values(&mut hash, values).unwrap();
        hash.finalize().into()
    }

    #[test]
    fn digest_encodes_types_and_row_shapes_unambiguously() {
        assert_ne!(
            digest(&[Value::Integer(1)]),
            digest(&[Value::Text("1".into())])
        );
        assert_ne!(
            digest(&[Value::Null]),
            digest(&[Value::Text("null".into())])
        );
        assert_ne!(
            digest(&[Value::Text("a".into()), Value::Text("bc".into())]),
            digest(&[Value::Text("ab".into()), Value::Text("c".into())])
        );
        assert_ne!(
            digest(&[Value::Integer(i64::MAX)]),
            digest(&[Value::Integer(i64::MIN)])
        );
    }

    #[test]
    fn digest_normalizes_float_negative_zero() {
        assert_eq!(digest(&[Value::Real(-0.0)]), digest(&[Value::Real(0.0)]));
    }

    #[tokio::test]
    async fn music_import_fixture_preserves_named_values_and_detects_drift() {
        use super::super::{
            table::{insert_sql, parse_tables, projection},
            value::row_values,
        };
        use crate::catalog::validate_document;
        use yoink_db::{query, raw_sql};
        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../../schema/legacy-postgres.json")).unwrap();
        document["tables"].as_array_mut().unwrap().push(
            serde_json::from_str(include_str!("../../../../schema/music-resolve-log.json"))
                .unwrap(),
        );
        let mut tables = parse_tables(&validate_document(document).unwrap()).unwrap();
        let mut table = tables.remove(
            tables
                .iter()
                .position(|t| t.name == "music_resolve_log")
                .unwrap(),
        );
        table.columns.reverse();
        assert!(
            projection(&table)
                .contains("trunc(extract(epoch FROM \"created_at\") * 1000000)::text")
        );
        assert_eq!(
            table.foreign_keys,
            vec![(vec!["user_id".into()], "users".into(), vec!["id".into()])]
        );
        assert_eq!(
            table
                .columns
                .iter()
                .find(|c| c.name == "id")
                .unwrap()
                .sequence
                .as_deref(),
            Some("public.music_resolve_log_id_seq")
        );
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let store = yoink_storage::Store::open(
            &directory.path().join("music-import.db"),
            true,
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap();
        store.migrate().await.unwrap();
        query("INSERT INTO users(id,created_at,updated_at) VALUES(?1,1,1)")
            .bind(9_007_199_254_740_993_i64)
            .execute(store.database())
            .await
            .unwrap();
        let connection = store.database().connection().await.unwrap();
        let mut hash = Sha256::new();
        for encoded in [
            r#"{"id":40,"user_id":9007199254740993,"source_platform":"spotify","artist":"Исполнитель","title":null,"platforms_found":3,"created_at":"-1"}"#,
            r#"{"id":41,"user_id":9007199254740993,"source_platform":"youtube","artist":null,"title":"English title","platforms_found":0,"created_at":"1728000000000001"}"#,
        ] {
            let values = row_values(encoded, &table, 4096).unwrap();
            digest_values(&mut hash, &values).unwrap();
            connection
                .execute(&insert_sql(&table), values)
                .await
                .unwrap();
        }
        let expected: [u8; 32] = hash.finalize().into();
        verify_content(&connection, &table, &expected, 2)
            .await
            .unwrap();
        raw_sql("UPDATE music_resolve_log SET title='' WHERE id=40")
            .execute(&*connection)
            .await
            .unwrap();
        assert_eq!(
            verify_content(&connection, &table, &expected, 2).await,
            Err(ImportError::Storage)
        );
        raw_sql("UPDATE music_resolve_log SET title=NULL WHERE id=40")
            .execute(&*connection)
            .await
            .unwrap();
        verify_content(&connection, &table, &expected, 2)
            .await
            .unwrap();
        drop(connection);
        let id = store
            .insert_music_resolve_log(&yoink_storage::NewMusicResolveLog {
                user_id: 9_007_199_254_740_993,
                source_platform: "spotify".into(),
                artist: None,
                title: None,
                platforms_found: 1,
                created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(id, 42);
    }

    #[test]
    fn digest_rejects_non_finite_float() {
        let mut hash = Sha256::new();
        assert_eq!(
            digest_values(&mut hash, &[Value::Real(f64::NAN)]),
            Err(ImportError::Conversion)
        );
    }
}
