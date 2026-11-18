//! A relation's properties and dependencies (SpelaionDB's SDB-393), from DuckDB's catalog
//! functions. DuckDB has no table partitions (`Connection::table_partitions` keeps its default).
//!
//! Dependencies are what DuckDB records: foreign keys, both ways (`duckdb_constraints()`). A
//! view is bound each time it runs, so which tables a view reads, and which views read a table,
//! is not recorded, nor what a macro's body uses: views and functions are untracked kinds.

use duckdb::{Connection, params};
use spelaion_driver_api::{
    Dependencies, DependencyInfo, DependencyKind, DriverError, ObjectProperty, Result, property,
};

use crate::exec::map_error;
use crate::introspect::locate;

fn missing(schema: &str, name: &str) -> DriverError {
    DriverError::Other(format!("there is no table or view {schema}.{name}"))
}

/// Whether `name` is a table (`Some(false)`), a view (`Some(true)`) or neither, and whether it
/// is temporary.
fn relation(
    conn: &Connection,
    database: &str,
    schema: &str,
    name: &str,
) -> Result<Option<(bool, bool)>> {
    let mut statement = conn
        .prepare(
            "select false, temporary from duckdb_tables() \
               where database_name = ?1 and schema_name = ?2 and table_name = ?3 \
             union all \
             select true, temporary from duckdb_views() \
               where database_name = ?1 and schema_name = ?2 and view_name = ?3 and not internal",
        )
        .map_err(map_error)?;
    let mut rows = statement
        .query_map(params![database, schema, name], |r| {
            Ok((r.get::<_, bool>(0)?, r.get::<_, bool>(1)?))
        })
        .map_err(map_error)?;
    rows.next().transpose().map_err(map_error)
}

pub fn properties(conn: &Connection, schema: &str, name: &str) -> Result<Vec<ObjectProperty>> {
    let (database, schema_name) = locate(conn, schema)?;
    let (_, temporary) =
        relation(conn, &database, &schema_name, name)?.ok_or_else(|| missing(schema, name))?;
    let mut out = vec![ObjectProperty {
        name: property::PERSISTENCE.into(),
        value: if temporary { "temporary" } else { "permanent" }.into(),
    }];
    // The file the table is in, when its database is one.
    let path: Option<String> = conn
        .query_row(
            "select path from duckdb_databases() where database_name = ?",
            params![database],
            |r| r.get(0),
        )
        .map_err(map_error)?;
    if let Some(path) = path.filter(|p| !p.is_empty()) {
        out.push(ObjectProperty {
            name: "file".into(),
            value: path,
        });
    }
    Ok(out)
}

/// The foreign keys of `name` (`outgoing`) or that point to it, as dependencies.
fn keys(
    conn: &Connection,
    database: &str,
    schema: &str,
    shown_schema: &str,
    name: &str,
    outgoing: bool,
) -> Result<Vec<DependencyInfo>> {
    let filter = if outgoing {
        "table_name = ?3"
    } else {
        "lower(referenced_table) = lower(?3)"
    };
    let mut statement = conn
        .prepare(&format!(
            "select table_name, referenced_table, coalesce(constraint_name, ''), \
                    array_to_string(constraint_column_names, ', '), \
                    array_to_string(referenced_column_names, ', ') \
             from duckdb_constraints() \
             where database_name = ?1 and schema_name = ?2 and constraint_type = 'FOREIGN KEY' \
               and {filter} \
             order by table_name, constraint_name"
        ))
        .map_err(map_error)?;
    let rows = statement
        .query_map(params![database, schema, name], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(map_error)?;
    let mut out = Vec::new();
    for row in rows {
        let (table, referenced, key, from, to) = row.map_err(map_error)?;
        let columns = format!(
            "({}) → ({})",
            from.unwrap_or_default(),
            to.unwrap_or_default()
        );
        out.push(DependencyInfo {
            schema: shown_schema.to_string(),
            name: if outgoing { referenced } else { table },
            kind: if outgoing {
                DependencyKind::Table
            } else {
                DependencyKind::ForeignKey
            },
            detail: Some(if key.is_empty() {
                columns
            } else {
                format!("{key} {columns}")
            }),
        });
    }
    Ok(out)
}

pub fn dependencies(conn: &Connection, schema: &str, name: &str) -> Result<Option<Dependencies>> {
    let (database, schema_name) = locate(conn, schema)?;
    let (is_view, _) =
        relation(conn, &database, &schema_name, name)?.ok_or_else(|| missing(schema, name))?;
    let mut untracked = vec![DependencyKind::View, DependencyKind::Function];
    if is_view {
        // What a view reads is bound when it runs.
        untracked.insert(0, DependencyKind::Table);
        return Ok(Some(Dependencies {
            uses: Vec::new(),
            used_by: Vec::new(),
            untracked,
        }));
    }
    Ok(Some(Dependencies {
        uses: keys(conn, &database, &schema_name, schema, name, true)?,
        used_by: keys(conn, &database, &schema_name, schema, name, false)?,
        untracked,
    }))
}
