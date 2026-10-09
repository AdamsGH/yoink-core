use serde_json::Value;
use tokio_postgres::Client;

use crate::{ImportError, Result};

const CANONICAL_CATALOG: &str = include_str!("../../../schema/legacy-postgres.json");
const MUSIC_CATALOG: &str = include_str!("../../../schema/music-resolve-log.json");
const LIVE_PROFILE: &str = include_str!("../../../schema/legacy-live-profile.json");
const CATALOG_SQL: &str = include_str!("../../../scripts/sql/legacy-catalog.sql");

#[derive(Clone, Debug)]
pub struct ValidatedCatalog {
    pub document: Value,
    pub application_tables: Vec<String>,
}

/// Lock the fixed baseline before inspection; the supported optional Music table
/// is locked before final validation as well. No source identifier enters SQL.
pub async fn lock_expected_tables(client: &Client) -> Result<()> {
    let expected: Value =
        serde_json::from_str(CANONICAL_CATALOG).map_err(|_| ImportError::Catalog)?;
    for table in expected["tables"].as_array().ok_or(ImportError::Catalog)? {
        let name = table["name"].as_str().ok_or(ImportError::Catalog)?;
        client
            .batch_execute(&format!(
                "LOCK TABLE public.\"{name}\" IN ACCESS SHARE MODE"
            ))
            .await
            .map_err(|_| ImportError::Source.at_table(name, "catalog locking"))?;
    }
    let present: bool = client
        .query_one(
            "SELECT to_regclass('public.music_resolve_log') IS NOT NULL",
            &[],
        )
        .await
        .map_err(|_| ImportError::Source)?
        .get(0);
    if present {
        client
            .batch_execute("LOCK TABLE public.music_resolve_log IN ACCESS SHARE MODE")
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
    validate_document(row.try_get(0).map_err(|_| ImportError::Catalog)?)
}

/// Named columns and constraints are independent of physical ordinal and catalog
/// enumeration order. All other fields remain part of the reviewed contract.
fn normalize(value: &Value, key: &str) -> Result<Value> {
    match value {
        Value::Array(items) if matches!(key, "tables" | "columns" | "constraints") => {
            let mut named = serde_json::Map::new();
            for item in items {
                let name = item["name"].as_str().ok_or(ImportError::Catalog)?;
                if named.insert(name.into(), normalize(item, "")?).is_some() {
                    return Err(ImportError::Catalog);
                }
            }
            Ok(Value::Object(named))
        }
        Value::Array(items) => {
            let mut items = items
                .iter()
                .map(|v| normalize(v, ""))
                .collect::<Result<Vec<_>>>()?;
            // Enum label ordering is meaningful, unlike OID-based index ordering.
            if key != "userrole" {
                items.sort_by_key(Value::to_string);
            }
            Ok(Value::Array(items))
        }
        Value::Object(fields) => {
            let mut result = serde_json::Map::new();
            for (name, value) in fields {
                if name == "ordinal" && fields.contains_key("udt") {
                    continue;
                }
                result.insert(name.clone(), normalize(value, name)?);
            }
            Ok(Value::Object(result))
        }
        _ => Ok(value.clone()),
    }
}

fn contracts() -> Result<Vec<Value>> {
    let baseline: Value =
        serde_json::from_str(CANONICAL_CATALOG).map_err(|_| ImportError::Catalog)?;
    let mut music = baseline.clone();
    music["tables"]
        .as_array_mut()
        .ok_or(ImportError::Catalog)?
        .push(serde_json::from_str(MUSIC_CATALOG).map_err(|_| ImportError::Catalog)?);
    let baseline = normalize(&baseline, "")?;
    let music = normalize(&music, "")?;
    let mut live = music.clone();
    let changes: Vec<Value> =
        serde_json::from_str(LIVE_PROFILE).map_err(|_| ImportError::Catalog)?;
    for change in changes {
        let path = change["path"].as_str().ok_or(ImportError::Catalog)?;
        if change["op"] != "add" && live.pointer(path) != Some(&change["expected"]) {
            return Err(ImportError::Catalog);
        }
        let (parent, field) = path.rsplit_once('/').ok_or(ImportError::Catalog)?;
        let fields = live
            .pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .ok_or(ImportError::Catalog)?;
        if change["op"] == "remove" {
            fields.remove(field);
        } else {
            fields.insert(field.into(), change["value"].clone());
        }
    }
    Ok(vec![baseline, music, live])
}

fn differences(expected: &Value, actual: &Value, path: &str, output: &mut Vec<String>) {
    if expected == actual {
        return;
    }
    if let (Some(e), Some(a)) = (expected.as_object(), actual.as_object()) {
        let keys: std::collections::BTreeSet<_> = e.keys().chain(a.keys()).collect();
        for key in keys {
            let field = format!("{path}/{key}");
            match (e.get(key), a.get(key)) {
                (Some(expected), Some(actual)) => differences(expected, actual, &field, output),
                _ => output.push(field),
            }
        }
    } else {
        // Paths only. Never disclose source expressions or row/credential values.
        output.push(path.to_owned());
    }
}

pub fn validate_document(document: Value) -> Result<ValidatedCatalog> {
    let actual = normalize(&document, "")?;
    let contracts = contracts()?;
    if !contracts.contains(&actual) {
        let mut candidates = Vec::new();
        for expected in &contracts {
            let mut paths = Vec::new();
            differences(expected, &actual, "", &mut paths);
            candidates.push(paths);
        }
        let paths = candidates
            .into_iter()
            .min_by_key(Vec::len)
            .ok_or(ImportError::Catalog)?;
        // Source identifiers are untrusted. Escape them and bound diagnostics.
        let fields = paths
            .iter()
            .take(12)
            .map(|p| format!("{:?}", p.chars().take(240).collect::<String>()))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(ImportError::CatalogFields { fields });
    }
    let application_tables = document["tables"]
        .as_array()
        .ok_or(ImportError::Catalog)?
        .iter()
        .filter_map(|table| {
            table["name"]
                .as_str()
                .filter(|n| *n != "alembic_version")
                .map(str::to_owned)
        })
        .collect();
    Ok(ValidatedCatalog {
        document,
        application_tables,
    })
}

/// These constraints are absent only in the exact reviewed live profile.
/// Check data in the same read-only snapshot before opening/migrating a target.
pub async fn preflight(client: &Client, catalog: &ValidatedCatalog) -> Result<()> {
    let live = contracts()?.pop().ok_or(ImportError::Catalog)?;
    if normalize(&catalog.document, "")? != live {
        return Ok(());
    }
    for (field, sql) in PREFLIGHTS {
        let invalid: bool = client
            .query_one(*sql, &[])
            .await
            .map_err(|_| ImportError::Source)?
            .get(0);
        if invalid {
            return Err(ImportError::CatalogFields {
                fields: (*field).into(),
            });
        }
    }
    Ok(())
}

const PREFLIGHTS: &[(&str, &str)] = &[
    (
        "tables/cookies/indexes/cookies_personal_unique: duplicate (user_id, domain) WHERE is_pool=false",
        "SELECT EXISTS(SELECT 1 FROM public.cookies WHERE is_pool=false AND user_id IS NOT NULL AND domain IS NOT NULL GROUP BY user_id,domain HAVING count(*)>1)",
    ),
    (
        "tables/insight_summary_cache/constraints/uq_insight_cache_key: duplicate (content_key, lang, command)",
        "SELECT EXISTS(SELECT 1 FROM public.insight_summary_cache WHERE content_key IS NOT NULL AND lang IS NOT NULL AND command IS NOT NULL GROUP BY content_key,lang,command HAVING count(*)>1)",
    ),
    (
        "tables/insight_tldr_aliases/constraints/ck_insight_tldr_alias_shape: invalid alias shape",
        "SELECT EXISTS(SELECT 1 FROM public.insight_tldr_aliases WHERE NOT ((aliases IS NOT NULL AND prompt IS NOT NULL) OR target_alias IS NOT NULL OR domains IS NOT NULL))",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewed_profiles_accept_only_named_semantics() {
        for contract in contracts().unwrap() {
            // Contracts are normalized maps; reconstruct named arrays for validator.
            let mut document = contract;
            let tables = document["tables"]
                .as_object()
                .unwrap()
                .values()
                .cloned()
                .collect::<Vec<_>>();
            document["tables"] = Value::Array(tables);
            for table in document["tables"].as_array_mut().unwrap() {
                for field in ["columns", "constraints"] {
                    if let Some(map) = table[field].as_object() {
                        table[field] = Value::Array(map.values().cloned().collect());
                    }
                }
            }
            assert!(validate_document(document.clone()).is_ok());
            document["tables"].as_array_mut().unwrap().reverse();
            for table in document["tables"].as_array_mut().unwrap() {
                table["columns"].as_array_mut().unwrap().reverse();
                for (index, column) in table["columns"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .enumerate()
                {
                    column["ordinal"] = Value::from(index + 1);
                }
                if let Some(constraints) = table["constraints"].as_array_mut() {
                    constraints.reverse();
                }
                if let Some(indexes) = table["indexes"].as_array_mut() {
                    indexes.reverse();
                }
            }
            assert!(validate_document(document.clone()).is_ok());
            let column = &mut document["tables"][0]["columns"][0];
            column["not_null"] = Value::Bool(!column["not_null"].as_bool().unwrap());
            assert!(matches!(
                validate_document(document),
                Err(ImportError::CatalogFields { .. })
            ));
        }
    }

    #[test]
    fn actual_revision_0054_live_catalog_has_explicit_music_and_no_unreviewed_drift() {
        let document: Value =
            serde_json::from_str(include_str!("../fixtures/live-catalog.json")).unwrap();
        let catalog = validate_document(document.clone()).unwrap();
        assert_eq!(catalog.application_tables.len(), 42);
        assert!(
            catalog
                .application_tables
                .iter()
                .any(|t| t == "music_resolve_log")
        );
        assert_eq!(
            normalize(&document, "").unwrap(),
            contracts().unwrap().pop().unwrap()
        );
        for (table, field, value) in [
            ("music_resolve_log", "type", Value::String("bigint".into())),
            (
                "api_keys",
                "default_value",
                Value::String("'{}'::json".into()),
            ),
        ] {
            let mut drift = document.clone();
            let table = drift["tables"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|t| t["name"] == table)
                .unwrap();
            table["columns"][0][field] = value;
            assert!(validate_document(drift).is_err());
        }
        let mut drift = document.clone();
        drift["triggers"] = serde_json::json!([{"name":"unexpected"}]);
        assert!(validate_document(drift).is_err());
        let mut drift = document;
        let music = drift["tables"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|t| t["name"] == "music_resolve_log")
            .unwrap();
        music["constraints"].as_array_mut().unwrap().pop();
        assert!(
            validate_document(drift)
                .unwrap_err()
                .to_string()
                .contains("music_resolve_log_user_id_fkey")
        );
    }

    #[test]
    fn unsafe_drift_and_unknown_entities_have_field_diagnostics() {
        let baseline: Value = serde_json::from_str(CANONICAL_CATALOG).unwrap();
        for (field, value) in [
            ("default_value", Value::String("unsafe()".into())),
            ("type", Value::String("text".into())),
            ("generated", Value::String("s".into())),
            ("sequence", Value::String("public.wrong".into())),
        ] {
            let mut document = baseline.clone();
            document["tables"][1]["columns"][0][field] = value;
            let error = validate_document(document).unwrap_err().to_string();
            assert!(error.contains(field), "{error}");
        }
        let mut document = baseline.clone();
        document["enums"]["userrole"]
            .as_array_mut()
            .unwrap()
            .reverse();
        assert!(
            validate_document(document)
                .unwrap_err()
                .to_string()
                .contains("userrole")
        );
        let mut document = baseline.clone();
        document["tables"][0]["columns"][0]
            .as_object_mut()
            .unwrap()
            .remove("default_value");
        assert!(
            validate_document(document)
                .unwrap_err()
                .to_string()
                .contains("default_value")
        );
        let mut document = baseline.clone();
        document["tables"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"name":"unknown","columns":[],"constraints":[],"indexes":[]}));
        assert!(
            validate_document(document)
                .unwrap_err()
                .to_string()
                .contains("unknown")
        );
        let mut document = baseline;
        let duplicate = document["tables"][0].clone();
        document["tables"].as_array_mut().unwrap().push(duplicate);
        assert_eq!(
            validate_document(document).unwrap_err(),
            ImportError::Catalog
        );
    }

    #[tokio::test]
    async fn live_profile_preflight_fixtures_reject_unsafe_constraint_data() {
        let directory = tempfile::tempdir().unwrap();
        let database =
            turso::Builder::new_local(directory.path().join("preflight.db").to_str().unwrap())
                .build()
                .await
                .unwrap();
        let connection = database.connect().unwrap();
        for sql in [
            "CREATE TABLE cookies(user_id INTEGER,domain TEXT,is_pool INTEGER)",
            "CREATE TABLE insight_summary_cache(content_key TEXT,lang TEXT,command TEXT)",
            "CREATE TABLE insight_tldr_aliases(aliases TEXT,prompt TEXT,target_alias TEXT,domains TEXT)",
        ] {
            connection.execute(sql, ()).await.unwrap();
        }
        let fixtures = [
            (
                "INSERT INTO cookies VALUES(1,'youtube',0),(1,'youtube',1),(NULL,'youtube',0),(NULL,'youtube',0)",
                "INSERT INTO cookies VALUES(1,'youtube',0)",
            ),
            (
                "INSERT INTO insight_summary_cache VALUES('a','en','summary'),('a','ru','summary'),('a','en',NULL),('a','en',NULL)",
                "INSERT INTO insight_summary_cache VALUES('a','en','summary')",
            ),
            (
                "INSERT INTO insight_tldr_aliases VALUES('a','prompt',NULL,NULL),(NULL,NULL,'alias',NULL),(NULL,NULL,NULL,'domain')",
                "INSERT INTO insight_tldr_aliases VALUES(NULL,NULL,NULL,NULL)",
            ),
        ];
        for ((_, sql), (valid, invalid)) in PREFLIGHTS.iter().zip(fixtures) {
            connection.execute(valid, ()).await.unwrap();
            let sql = sql.replace("public.", "");
            let mut rows = connection.query(&sql, ()).await.unwrap();
            assert_eq!(
                rows.next().await.unwrap().unwrap().get_value(0).unwrap(),
                turso::Value::Integer(0)
            );
            drop(rows);
            connection.execute(invalid, ()).await.unwrap();
            let mut rows = connection.query(&sql, ()).await.unwrap();
            assert_eq!(
                rows.next().await.unwrap().unwrap().get_value(0).unwrap(),
                turso::Value::Integer(1)
            );
        }
    }
}
