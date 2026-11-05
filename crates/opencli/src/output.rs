use serde::Serialize;
use serde_json::{Map, Value};

use crate::cli::OutputFormat;
use crate::error::CliError;

const COLUMN_GAP: &str = "  ";
/// Wider tables stop being readable in a terminal; `--output json` has the rest.
const MAX_COLUMNS: usize = 8;
const MAX_CELL_CHARACTERS: usize = 60;
const ELLIPSIS: char = '…';

/// Prints `value` as JSON, or runs `table` to print the human-readable form.
pub fn emit<T: Serialize>(
    format: OutputFormat,
    value: &T,
    table: impl FnOnce(&T) -> String,
) -> Result<(), CliError> {
    let rendered = match format {
        OutputFormat::Json => serde_json::to_string_pretty(value)?,
        OutputFormat::Table => table(value),
    };
    println!("{}", rendered.trim_end());
    Ok(())
}

/// Left-aligned columns sized to their widest cell.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|header| header.chars().count())
        .collect();
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let header_cells: Vec<String> = headers.iter().map(|&header| header.to_owned()).collect();
    std::iter::once(&header_cells)
        .chain(rows)
        .map(|row| format_row(row, &widths))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Human-readable rendering of an API response.
///
/// Arrays of objects become tables of their scalar fields; objects become
/// key/value tables; anything else is pretty-printed JSON.
pub fn json_table(value: &Value) -> String {
    match value {
        Value::Array(items) if items.is_empty() => "No results.".to_owned(),
        Value::Array(items) if items.iter().all(Value::is_object) => object_rows(items),
        Value::Object(fields) => key_values(fields),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn object_rows(items: &[Value]) -> String {
    let mut columns: Vec<&str> = Vec::new();
    for fields in items.iter().filter_map(Value::as_object) {
        for (key, value) in fields {
            if is_scalar(value) && !columns.contains(&key.as_str()) && columns.len() < MAX_COLUMNS {
                columns.push(key);
            }
        }
    }
    let headers: Vec<String> = columns
        .iter()
        .map(|column| column.to_ascii_uppercase())
        .collect();
    let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            columns
                .iter()
                .map(|column| item.get(*column).map(cell_text).unwrap_or_default())
                .collect()
        })
        .collect();
    table(&header_refs, &rows)
}

fn key_values(fields: &Map<String, Value>) -> String {
    let rows: Vec<Vec<String>> = fields
        .iter()
        .map(|(key, value)| vec![key.clone(), cell_text(value)])
        .collect();
    table(&["FIELD", "VALUE"], &rows)
}

const fn is_scalar(value: &Value) -> bool {
    !matches!(value, Value::Object(_) | Value::Array(_))
}

fn cell_text(value: &Value) -> String {
    let text = match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    truncate(&text.replace(['\n', '\r', '\t'], " "))
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_CELL_CHARACTERS {
        return text.to_owned();
    }
    let mut kept: String = text.chars().take(MAX_CELL_CHARACTERS - 1).collect();
    kept.push(ELLIPSIS);
    kept
}

fn format_row(cells: &[String], widths: &[usize]) -> String {
    cells
        .iter()
        .zip(widths)
        .map(|(cell, &width)| format!("{cell:<width$}"))
        .collect::<Vec<_>>()
        .join(COLUMN_GAP)
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn table_pads_columns_to_widest_cell() {
        let rendered = table(
            &["NAME", "OPS"],
            &[
                vec!["petstore".into(), "4".into()],
                vec!["crm".into(), "120".into()],
            ],
        );
        assert_eq!(rendered, "NAME      OPS\npetstore  4\ncrm       120");
    }

    #[test]
    fn table_with_no_rows_prints_headers() {
        assert_eq!(table(&["NAME"], &[]), "NAME");
    }

    #[test]
    fn json_table_renders_arrays_of_objects_by_scalar_fields() {
        let rendered = json_table(&json!([
            {"id": 1, "name": "Rex", "owner": {"id": 9}},
            {"id": 2, "name": "Tom", "tag": null}
        ]));
        assert_eq!(rendered, "ID  NAME  TAG\n1   Rex\n2   Tom");
    }

    #[test]
    fn json_table_renders_objects_as_key_values_with_compact_nesting() {
        let rendered = json_table(&json!({"id": 1, "tags": ["a", "b"], "note": "line1\nline2"}));
        assert_eq!(
            rendered,
            "FIELD  VALUE\nid     1\ntags   [\"a\",\"b\"]\nnote   line1 line2"
        );
    }

    #[test]
    fn json_table_handles_empty_arrays_and_scalars() {
        assert_eq!(json_table(&json!([])), "No results.");
        assert_eq!(json_table(&json!("ok")), "\"ok\"");
        assert_eq!(json_table(&json!([1, 2])), "[\n  1,\n  2\n]");
    }

    #[test]
    fn json_table_truncates_long_cells_and_caps_columns() {
        let long = "x".repeat(100);
        let rendered = json_table(&json!({"long": long}));
        assert!(
            rendered.ends_with(&format!("{}…", "x".repeat(59))),
            "{rendered}"
        );

        let wide: serde_json::Map<String, Value> = (0..12)
            .map(|index| (format!("c{index}"), json!(index)))
            .collect();
        let header = json_table(&Value::Array(vec![Value::Object(wide)]))
            .lines()
            .next()
            .unwrap()
            .to_owned();
        assert_eq!(header.split_whitespace().count(), MAX_COLUMNS);
    }
}
