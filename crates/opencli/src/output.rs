use serde::Serialize;

use crate::cli::OutputFormat;
use crate::error::CliError;

const COLUMN_GAP: &str = "  ";

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
}
