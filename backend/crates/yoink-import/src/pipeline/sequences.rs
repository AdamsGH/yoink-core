use tokio_postgres::Client;
use turso::{Connection, Value};

use super::{Table, ident};
use crate::{ImportError, Result};

pub(super) async fn preserve(
    client: &Client,
    connection: &Connection,
    tables: &[Table],
) -> Result<()> {
    for table in tables {
        for column in &table.columns {
            let Some(sequence) = column.sequence.as_deref() else {
                continue;
            };
            if table.name == "users" || table.name == "groups" {
                continue;
            }
            let sequence_name = validate_sequence(sequence)?;
            let expected_maximum = match column.udt.as_str() {
                "int4" => i64::from(i32::MAX),
                "int8" => i64::MAX,
                _ => return Err(ImportError::Catalog),
            };
            let kind_row = client.query_opt(
                "SELECT seqtypid = to_regtype($2), seqincrement, seqmin, seqstart, seqmax, seqcycle \
                 FROM pg_sequence WHERE seqrelid = to_regclass($1)",
                &[&sequence, &column.udt],
            ).await.map_err(|_| ImportError::Source)?.ok_or(ImportError::Catalog)?;
            let type_matches: bool = kind_row.try_get(0).map_err(|_| ImportError::Source)?;
            let increment: i64 = kind_row.try_get(1).map_err(|_| ImportError::Source)?;
            let minimum: i64 = kind_row.try_get(2).map_err(|_| ImportError::Source)?;
            let start: i64 = kind_row.try_get(3).map_err(|_| ImportError::Source)?;
            let maximum: i64 = kind_row.try_get(4).map_err(|_| ImportError::Source)?;
            let cycles: bool = kind_row.try_get(5).map_err(|_| ImportError::Source)?;
            if !type_matches
                || increment != 1
                || minimum != 1
                || start != 1
                || maximum != expected_maximum
                || cycles
            {
                return Err(ImportError::Catalog);
            }

            let sequence_sql = format!("SELECT last_value::text, is_called FROM {sequence_name}");
            let sequence_row = client
                .query_one(&sequence_sql, &[])
                .await
                .map_err(|_| ImportError::Source)?;
            let last_text: String = sequence_row.try_get(0).map_err(|_| ImportError::Source)?;
            let is_called: bool = sequence_row.try_get(1).map_err(|_| ImportError::Source)?;
            let last_value = last_text
                .parse::<i64>()
                .map_err(|_| ImportError::Conversion)?;
            if last_value < minimum || last_value > maximum || last_value <= 0 {
                return Err(ImportError::Catalog);
            }
            let sequence_high = if is_called {
                last_value
            } else {
                last_value.checked_sub(1).ok_or(ImportError::Conversion)?
            };

            let maximum_sql = format!(
                "SELECT max({}) FROM {}",
                ident(&column.name),
                ident(&table.name)
            );
            let mut source_rows = connection
                .query(&maximum_sql, ())
                .await
                .map_err(|_| ImportError::Storage)?;
            let source_row = source_rows
                .next()
                .await
                .map_err(|_| ImportError::Storage)?
                .ok_or(ImportError::Storage)?;
            let native_high = match source_row.get_value(0).map_err(|_| ImportError::Storage)? {
                Value::Null => 0,
                Value::Integer(value) => value,
                _ => return Err(ImportError::Storage),
            };
            if source_rows
                .next()
                .await
                .map_err(|_| ImportError::Storage)?
                .is_some()
            {
                return Err(ImportError::Storage);
            }
            drop(source_rows);
            let high_water = native_high.max(sequence_high).max(0);
            if high_water > maximum {
                return Err(ImportError::Catalog);
            }

            let parameters = vec![Value::Integer(high_water), Value::Text(table.name.clone())];
            connection
                .execute(
                    "UPDATE sqlite_sequence SET seq = ?1 WHERE name = ?2",
                    parameters.clone(),
                )
                .await
                .map_err(|_| ImportError::Storage)?;
            let lookup = vec![Value::Text(table.name.clone())];
            let exists = {
                let mut rows = connection
                    .query(
                        "SELECT seq FROM sqlite_sequence WHERE name = ?1",
                        lookup.clone(),
                    )
                    .await
                    .map_err(|_| ImportError::Storage)?;
                let exists = rows
                    .next()
                    .await
                    .map_err(|_| ImportError::Storage)?
                    .is_some();
                if rows
                    .next()
                    .await
                    .map_err(|_| ImportError::Storage)?
                    .is_some()
                {
                    return Err(ImportError::Storage);
                }
                exists
            };
            if !exists {
                connection
                    .execute(
                        "INSERT INTO sqlite_sequence(name, seq) VALUES (?1, ?2)",
                        vec![Value::Text(table.name.clone()), Value::Integer(high_water)],
                    )
                    .await
                    .map_err(|_| ImportError::Storage)?;
            }

            let mut confirm = connection
                .query("SELECT seq FROM sqlite_sequence WHERE name = ?1", lookup)
                .await
                .map_err(|_| ImportError::Storage)?;
            let stored = confirm
                .next()
                .await
                .map_err(|_| ImportError::Storage)?
                .ok_or(ImportError::Storage)?;
            if stored.get_value(0).map_err(|_| ImportError::Storage)? != Value::Integer(high_water)
                || confirm
                    .next()
                    .await
                    .map_err(|_| ImportError::Storage)?
                    .is_some()
            {
                return Err(ImportError::Storage);
            }
        }
    }
    Ok(())
}

fn validate_sequence(name: &str) -> Result<String> {
    let mut parts = name.split('.');
    let schema = parts.next().ok_or(ImportError::Catalog)?;
    let relation = parts.next().ok_or(ImportError::Catalog)?;
    if parts.next().is_some() || schema != "public" || !valid_identifier(relation) {
        return Err(ImportError::Catalog);
    }
    Ok(format!("{}.{}", ident(schema), ident(relation)))
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
}
