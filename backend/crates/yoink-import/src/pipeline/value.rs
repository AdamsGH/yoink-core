use serde_json::Value as Json;
use turso::Value;

use super::{Column, Table};
use crate::{ImportError, Result};

pub(super) fn row_values(encoded: &str, table: &Table, bound: usize) -> Result<Vec<Value>> {
    if encoded.len() > bound {
        return Err(ImportError::Conversion);
    }
    let row: Json = serde_json::from_str(encoded).map_err(|_| ImportError::Conversion)?;
    let row = row.as_object().ok_or(ImportError::Conversion)?;
    if row.len() != table.columns.len() {
        return Err(ImportError::Conversion);
    }
    table
        .columns
        .iter()
        .map(|column| {
            convert(
                row.get(&column.name).ok_or(ImportError::Conversion)?,
                column,
                bound,
            )
        })
        .collect()
}

fn convert(value: &Json, column: &Column, bound: usize) -> Result<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    Ok(match column.udt.as_str() {
        "int4" | "int8" => {
            let integer = value.as_i64().ok_or(ImportError::Conversion)?;
            if column.udt == "int4" && i32::try_from(integer).is_err() {
                return Err(ImportError::Conversion);
            }
            Value::Integer(integer)
        }
        "float8" => {
            // PostgreSQL extra_float_digits=3 supplies a round-trippable string.
            // Parsing directly avoids workspace-wide serde_json feature changes.
            let number = value
                .as_str()
                .ok_or(ImportError::Conversion)?
                .parse::<f64>()
                .map_err(|_| ImportError::Conversion)?;
            if !number.is_finite() {
                return Err(ImportError::Conversion);
            }
            Value::Real(if number == 0.0 { 0.0 } else { number })
        }
        "bool" => Value::Integer(i64::from(value.as_bool().ok_or(ImportError::Conversion)?)),
        "timestamptz" => Value::Integer(
            value
                .as_str()
                .ok_or(ImportError::Conversion)?
                .parse::<i64>()
                .map_err(|_| ImportError::Conversion)?,
        ),
        "json" | "jsonb" | "varchar" | "text" | "userrole" => {
            let text = value.as_str().ok_or(ImportError::Conversion)?;
            if text.len() > bound {
                return Err(ImportError::Conversion);
            }
            Value::Text(text.to_owned())
        }
        _ => return Err(ImportError::Conversion),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(udt: &str) -> Column {
        Column {
            name: "value".into(),
            udt: udt.into(),
            sequence: None,
        }
    }

    #[test]
    fn preserves_sql_null_json_literal_null_and_wide_numbers() {
        assert!(matches!(
            convert(&Json::Null, &column("json"), 100).unwrap(),
            Value::Null
        ));
        assert!(
            matches!(convert(&Json::String("null".into()), &column("json"), 100).unwrap(), Value::Text(text) if text == "null")
        );
        let raw = "{\"number\":9007199254740993}";
        assert!(
            matches!(convert(&Json::String(raw.into()), &column("jsonb"), 100).unwrap(), Value::Text(text) if text == raw)
        );
        let number: Json = serde_json::from_str("9007199254740993").unwrap();
        assert!(matches!(
            convert(&number, &column("int8"), 100).unwrap(),
            Value::Integer(9_007_199_254_740_993)
        ));
        assert!(convert(&number, &column("int4"), 100).is_err());
    }

    #[test]
    fn finite_floats_round_trip_without_changing_json_features() {
        for number in [
            1.25_f64,
            9.953947048118591e-11,
            f64::MAX,
            f64::from_bits(1),
            -0.0,
        ] {
            let actual =
                convert(&Json::String(number.to_string()), &column("float8"), 1000).unwrap();
            let Value::Real(actual) = actual else {
                panic!("expected native real")
            };
            assert_eq!(
                actual.to_bits(),
                (if number == 0.0 { 0.0 } else { number }).to_bits()
            );
        }
        for text in ["NaN", "Infinity", "-Infinity", "1e400"] {
            assert!(convert(&Json::String(text.into()), &column("float8"), 100).is_err());
        }
    }

    #[test]
    fn row_shape_bounds_and_integer_extrema_are_checked_on_the_copy_path() {
        let table = Table {
            name: "fixture".into(),
            columns: vec![column("int8")],
            primary_key: vec!["value".into()],
            foreign_keys: Vec::new(),
        };
        for number in [i64::MIN, i64::MAX] {
            assert_eq!(
                row_values(&format!("{{\"value\":{number}}}"), &table, 100).unwrap(),
                vec![Value::Integer(number)]
            );
        }
        for encoded in [
            "{}",
            "{\"unknown\":1}",
            "{\"value\":9223372036854775808}",
            "{\"value\":1.5}",
            "[]",
        ] {
            assert!(row_values(encoded, &table, 100).is_err());
        }
        assert!(row_values("{\"value\":1}", &table, 1).is_err());
        assert_eq!(
            convert(&Json::Bool(false), &column("bool"), 10).unwrap(),
            Value::Integer(0)
        );
        assert_eq!(
            convert(&Json::String("雪".into()), &column("text"), 3).unwrap(),
            Value::Text("雪".into())
        );
        assert!(convert(&Json::String("雪".into()), &column("text"), 2).is_err());
    }

    #[test]
    fn exact_timestamp_conversion_rejects_infinity_overflow_and_fraction() {
        assert!(matches!(
            convert(&Json::String("-1".into()), &column("timestamptz"), 100).unwrap(),
            Value::Integer(-1)
        ));
        for text in ["infinity", "9223372036854775808", "1.5"] {
            assert!(convert(&Json::String(text.into()), &column("timestamptz"), 100).is_err());
        }
    }
}
