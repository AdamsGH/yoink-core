use crate::{ImportError, Result, catalog::ValidatedCatalog};

#[derive(Clone, Debug)]
pub(super) struct Column {
    pub(super) name: String,
    pub(super) udt: String,
    pub(super) sequence: Option<String>,
}
#[derive(Clone, Debug)]
pub(super) struct Table {
    pub(super) name: String,
    pub(super) columns: Vec<Column>,
    pub(super) primary_key: Vec<String>,
    pub(super) foreign_keys: Vec<(Vec<String>, String, Vec<String>)>,
}

pub(super) fn ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub(super) fn parse_tables(catalog: &ValidatedCatalog) -> Result<Vec<Table>> {
    let entries = catalog.document["tables"]
        .as_array()
        .ok_or(ImportError::Catalog)?;
    let mut tables = Vec::new();
    for entry in entries {
        let name = entry["name"].as_str().ok_or(ImportError::Catalog)?;
        if name == "alembic_version" {
            continue;
        }
        let name = identifier(name)?;
        let mut columns = Vec::new();
        for column in entry["columns"].as_array().ok_or(ImportError::Catalog)? {
            let column_name = identifier(column["name"].as_str().ok_or(ImportError::Catalog)?)?;
            if name == "stats_messages" && column_name == "text_search" {
                continue;
            }
            let udt = column["udt"].as_str().ok_or(ImportError::Catalog)?;
            if !matches!(
                udt,
                "int4"
                    | "int8"
                    | "float8"
                    | "bool"
                    | "timestamptz"
                    | "json"
                    | "jsonb"
                    | "text"
                    | "varchar"
                    | "userrole"
            ) {
                return Err(ImportError::Catalog);
            }
            columns.push(Column {
                name: column_name,
                udt: udt.to_owned(),
                sequence: column["sequence"].as_str().map(str::to_owned),
            });
        }
        let mut primary_key = Vec::new();
        let mut foreign_keys = Vec::new();
        for constraint in entry["constraints"]
            .as_array()
            .ok_or(ImportError::Catalog)?
        {
            let definition = constraint["definition"]
                .as_str()
                .ok_or(ImportError::Catalog)?;
            match constraint["kind"].as_str() {
                Some("p") => {
                    primary_key = columns_in(
                        definition
                            .strip_prefix("PRIMARY KEY ")
                            .ok_or(ImportError::Catalog)?,
                    )?
                }
                Some("f") => foreign_keys.push(parse_fk(definition)?),
                _ => (),
            }
        }
        if primary_key.is_empty()
            || primary_key
                .iter()
                .any(|key| !columns.iter().any(|c| &c.name == key))
        {
            return Err(ImportError::Catalog);
        }
        tables.push(Table {
            name,
            columns,
            primary_key,
            foreign_keys,
        });
    }
    if !matches!(tables.len(), 41 | 42)
        || (tables.len() != catalog.application_tables.len()
            && !catalog.application_tables.is_empty())
    {
        return Err(ImportError::Catalog);
    }
    for table in &tables {
        for (children, parent, keys) in &table.foreign_keys {
            let parent = tables
                .iter()
                .find(|t| &t.name == parent)
                .ok_or(ImportError::Catalog)?;
            if children.len() != keys.len()
                || children.is_empty()
                || children
                    .iter()
                    .any(|k| !table.columns.iter().any(|c| &c.name == k))
                || keys
                    .iter()
                    .any(|k| !parent.columns.iter().any(|c| &c.name == k))
            {
                return Err(ImportError::Catalog);
            }
        }
    }
    Ok(tables)
}

fn identifier(token: &str) -> Result<String> {
    let token = token.trim();
    let name = if token.starts_with('"') {
        token
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .ok_or(ImportError::Catalog)?
    } else {
        token
    };
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        || !chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    {
        return Err(ImportError::Catalog);
    }
    Ok(name.to_owned())
}
fn columns_in(definition: &str) -> Result<Vec<String>> {
    definition
        .trim()
        .strip_prefix('(')
        .and_then(|v| v.strip_suffix(')'))
        .ok_or(ImportError::Catalog)?
        .split(',')
        .map(identifier)
        .collect()
}
fn parse_fk(definition: &str) -> Result<(Vec<String>, String, Vec<String>)> {
    let (children, parent) = definition
        .strip_prefix("FOREIGN KEY ")
        .ok_or(ImportError::Catalog)?
        .split_once(" REFERENCES ")
        .ok_or(ImportError::Catalog)?;
    let (parent, keys) = parent.split_once('(').ok_or(ImportError::Catalog)?;
    let parent = parent
        .trim()
        .strip_prefix("public.")
        .unwrap_or(parent.trim());
    let (keys, _) = keys.split_once(')').ok_or(ImportError::Catalog)?;
    Ok((
        columns_in(children)?,
        identifier(parent)?,
        keys.split(',').map(identifier).collect::<Result<_>>()?,
    ))
}

pub(super) fn projection(table: &Table) -> String {
    let fields = table
        .columns
        .iter()
        .map(|column| {
            let name = ident(&column.name);
            let expression = match column.udt.as_str() {
                "json" | "jsonb" | "float8" => format!("{name}::text"),
                "timestamptz" => format!("trunc(extract(epoch FROM {name}) * 1000000)::text"),
                _ => name,
            };
            format!("{expression} AS {}", ident(&column.name))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let order = table
        .primary_key
        .iter()
        .map(|key| {
            let text = table.columns.iter().any(|column| {
                &column.name == key && matches!(column.udt.as_str(), "text" | "varchar")
            });
            format!("{}{}", ident(key), if text { " COLLATE \"C\"" } else { "" })
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT row_to_json(projected)::text FROM (SELECT {fields} FROM public.{}) AS projected ORDER BY {order}",
        ident(&table.name)
    )
}
pub(super) fn insert_sql(table: &Table) -> String {
    format!(
        "INSERT INTO {} ({}) VALUES ({})",
        ident(&table.name),
        table
            .columns
            .iter()
            .map(|column| ident(&column.name))
            .collect::<Vec<_>>()
            .join(", "),
        (1..=table.columns.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_actual_canonical_constraints_and_keeps_normalized_column() {
        let document =
            serde_json::from_str(include_str!("../../../../schema/legacy-postgres.json")).unwrap();
        let tables = parse_tables(&ValidatedCatalog {
            document,
            application_tables: Vec::new(),
        })
        .unwrap();
        assert_eq!(tables.len(), 41);
        assert_eq!(
            tables
                .iter()
                .find(|t| t.name == "rate_limits")
                .unwrap()
                .primary_key,
            ["user_id", "window"]
        );
        assert!(
            !tables
                .iter()
                .find(|t| t.name == "stats_messages")
                .unwrap()
                .columns
                .iter()
                .any(|c| c.name == "text_search")
        );
        assert!(
            tables
                .iter()
                .find(|t| t.name == "inbox_categories")
                .unwrap()
                .columns
                .iter()
                .any(|c| c.name == "normalized_name")
        );
        assert!(
            tables
                .iter()
                .find(|t| t.name == "insight_user_prompts")
                .map(projection)
                .unwrap()
                .contains("\"command\" COLLATE \"C\"")
        );
    }

    #[test]
    fn parses_quoted_keywords_and_rejects_noncanonical_identifiers() {
        assert_eq!(
            columns_in("(user_id, \"window\")").unwrap(),
            ["user_id", "window"]
        );
        assert_eq!(
            parse_fk("FOREIGN KEY (owner_id) REFERENCES public.users(id) ON DELETE CASCADE")
                .unwrap(),
            (vec!["owner_id".into()], "users".into(), vec!["id".into()])
        );
        assert!(identifier("id); DROP TABLE users").is_err());
    }
}
