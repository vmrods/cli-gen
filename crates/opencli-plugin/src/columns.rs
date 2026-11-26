use serde_json::Value;

use crate::Format;

/// Headers and rows for a response, using a plugin's chosen columns. Arrays
/// give one row per item; anything else gives one row.
#[must_use]
pub fn render_columns(value: &Value, format: &Format) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = format.columns.iter().map(crate::Column::header).collect();
    let row = |item: &Value| -> Vec<String> {
        format
            .columns
            .iter()
            .map(|column| lookup(item, column.path()).map_or_default(text))
            .collect()
    };
    let rows = match value {
        Value::Array(items) => items.iter().map(row).collect(),
        other => vec![row(other)],
    };
    (headers, rows)
}

/// Follows `a.b.0.c` through objects and arrays.
fn lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(value, |current, segment| match current {
            Value::Object(fields) => fields.get(segment),
            Value::Array(items) => segment
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get(index)),
            _ => None,
        })
}

fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Column;

    fn format(columns: &[&str]) -> Format {
        Format {
            columns: columns
                .iter()
                .map(|&path| Column::Path(path.to_owned()))
                .collect(),
        }
    }

    #[test]
    fn render_columns_follows_nested_paths_per_item() {
        let value = json!([
            {"key": "OC-1", "fields": {"status": {"name": "Open"}, "labels": ["a", "b"]}},
            {"key": "OC-2", "fields": {}}
        ]);
        let (headers, rows) = render_columns(
            &value,
            &format(&["key", "fields.status.name", "fields.labels.1"]),
        );
        assert_eq!(headers, ["KEY", "NAME", "1"]);
        assert_eq!(rows, [vec!["OC-1", "Open", "b"], vec!["OC-2", "", ""]]);
    }

    #[test]
    fn render_columns_treats_objects_as_one_row() {
        let (_, rows) = render_columns(
            &json!({"id": 7, "ok": true}),
            &format(&["id", "ok", "missing"]),
        );
        assert_eq!(rows, [vec!["7", "true", ""]]);
    }
}
