//! Grade consolidada de uma execução: uma linha por linha de resultado, com a
//! origem (loja, equipamento, IP, perfil) à frente. Exporta em CSV e XLSX.

use crate::exec::Outcome;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

pub const FIXED_COLUMNS: [&str; 4] = ["Loja", "Equipamento", "IP", "Perfil"];

/// De onde veio um resultado. `order` define a ordem das linhas na grade.
#[derive(Clone, Debug)]
pub struct Origin {
    pub order: usize,
    pub store: String,
    pub device: String,
    pub host: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GridColumn {
    pub name: String,
    pub numeric: bool,
    pub fixed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Grid {
    pub columns: Vec<GridColumn>,
    pub rows: Vec<Vec<Option<String>>>,
    /// Maior quantidade de conjuntos de resultado entre os equipamentos.
    pub sets: usize,
}

/// Junta o conjunto `set` de cada equipamento. Colunas de mesmo nome se fundem na
/// ordem em que aparecem; nome repetido no mesmo resultado vira outra coluna.
pub fn consolidate<S: std::hash::BuildHasher>(
    outcomes: &[Outcome],
    origins: &HashMap<i64, Origin, S>,
    set: usize,
) -> Grid {
    let mut ordered: Vec<&Outcome> = outcomes.iter().collect();
    ordered.sort_by_key(|outcome| {
        origins
            .get(&outcome.device_id)
            .map_or(usize::MAX, |origin| origin.order)
    });

    let mut keys: Vec<(String, usize)> = Vec::new();
    let mut numeric: Vec<bool> = Vec::new();
    let mut positions: Vec<Vec<usize>> = Vec::new();
    for outcome in &ordered {
        let Some(result) = outcome.sets.get(set) else {
            positions.push(Vec::new());
            continue;
        };
        let mut seen: HashMap<&str, usize> = HashMap::new();
        let mut mapping = Vec::with_capacity(result.columns.len());
        for column in &result.columns {
            let occurrence = seen.entry(column.name.as_str()).or_insert(0);
            let key = (column.name.clone(), *occurrence);
            *occurrence += 1;
            let index = if let Some(index) = keys.iter().position(|existing| *existing == key) {
                numeric[index] &= column.numeric;
                index
            } else {
                keys.push(key);
                numeric.push(column.numeric);
                keys.len() - 1
            };
            mapping.push(index);
        }
        positions.push(mapping);
    }

    let mut rows = Vec::new();
    for (outcome, mapping) in ordered.iter().zip(&positions) {
        let Some(result) = outcome.sets.get(set) else {
            continue;
        };
        let origin = origins.get(&outcome.device_id);
        let fixed = [
            origin.map(|o| o.store.clone()),
            origin.map_or_else(
                || Some(outcome.device_id.to_string()),
                |o| Some(o.device.clone()),
            ),
            origin.map(|o| o.host.clone()),
            outcome.profile.clone(),
        ];
        for values in &result.rows {
            let mut row: Vec<Option<String>> = fixed.to_vec();
            row.resize(FIXED_COLUMNS.len() + keys.len(), None);
            for (value, index) in values.iter().zip(mapping) {
                row[FIXED_COLUMNS.len() + index].clone_from(value);
            }
            rows.push(row);
        }
    }

    let columns = FIXED_COLUMNS
        .iter()
        .map(|name| GridColumn {
            name: (*name).to_owned(),
            numeric: false,
            fixed: true,
        })
        .chain(
            keys.into_iter()
                .zip(numeric)
                .map(|((name, _), numeric)| GridColumn {
                    name,
                    numeric,
                    fixed: false,
                }),
        )
        .collect();
    Grid {
        columns,
        rows,
        sets: outcomes.iter().map(|o| o.sets.len()).max().unwrap_or(0),
    }
}

/// CSV com `;` e BOM, como o Excel em português abre sem assistente. NULL sai vazio.
pub fn to_csv(grid: &Grid) -> Result<Vec<u8>, String> {
    let mut writer = csv::WriterBuilder::new()
        .delimiter(b';')
        .from_writer(b"\xEF\xBB\xBF".to_vec());
    let failed = |error: csv::Error| format!("Não foi possível montar o CSV: {error}");
    writer
        .write_record(grid.columns.iter().map(|c| c.name.as_str()))
        .map_err(failed)?;
    for row in &grid.rows {
        writer
            .write_record(row.iter().map(|v| v.as_deref().unwrap_or("")))
            .map_err(failed)?;
    }
    writer
        .into_inner()
        .map_err(|error| format!("Não foi possível montar o CSV: {error}"))
}

/// Planilha de uma aba, cabeçalho congelado, números como número quando a coluna é numérica.
pub fn to_xlsx(grid: &Grid) -> Result<Vec<u8>, String> {
    let mut sheet = String::from(concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
        r#"<sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews>"#,
        "<sheetData>"
    ));
    let header: Vec<Option<&str>> = grid.columns.iter().map(|c| Some(c.name.as_str())).collect();
    write_row(&mut sheet, 1, &header, &[]);
    let numeric: Vec<bool> = grid.columns.iter().map(|c| c.numeric).collect();
    for (index, row) in grid.rows.iter().enumerate() {
        let values: Vec<Option<&str>> = row.iter().map(Option::as_deref).collect();
        write_row(&mut sheet, index + 2, &values, &numeric);
    }
    sheet.push_str("</sheetData></worksheet>");

    let parts: [(&str, &str); 4] = [
        (
            "[Content_Types].xml",
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
                r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
                r#"<Default Extension="xml" ContentType="application/xml"/>"#,
                r#"<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#,
                r#"<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#,
                "</Types>"
            ),
        ),
        (
            "_rels/.rels",
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
                r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>"#,
                "</Relationships>"
            ),
        ),
        (
            "xl/workbook.xml",
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
                r#"<sheets><sheet name="Resultado" sheetId="1" r:id="rId1"/></sheets></workbook>"#
            ),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
                r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>"#,
                "</Relationships>"
            ),
        ),
    ];

    let failed = |error: &dyn std::fmt::Display| format!("Não foi possível montar o XLSX: {error}");
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in parts
        .iter()
        .map(|(name, body)| (*name, body.as_bytes()))
        .chain(std::iter::once((
            "xl/worksheets/sheet1.xml",
            sheet.as_bytes(),
        )))
    {
        zip.start_file(name, options).map_err(|e| failed(&e))?;
        zip.write_all(body).map_err(|e| failed(&e))?;
    }
    Ok(zip.finish().map_err(|e| failed(&e))?.into_inner())
}

fn write_row(sheet: &mut String, number: usize, values: &[Option<&str>], numeric: &[bool]) {
    use std::fmt::Write as _;
    let _ = write!(sheet, r#"<row r="{number}">"#);
    for (index, value) in values.iter().enumerate() {
        let Some(value) = value else { continue };
        let reference = format!("{}{number}", column_letters(index));
        if numeric.get(index).copied().unwrap_or(false) && is_safe_number(value) {
            let _ = write!(sheet, r#"<c r="{reference}"><v>{value}</v></c>"#);
        } else {
            let _ = write!(
                sheet,
                r#"<c r="{reference}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,
                escape_xml(value)
            );
        }
    }
    sheet.push_str("</row>");
}

/// Número que o Excel guarda sem perder dígitos: até 15 algarismos significativos.
fn is_safe_number(value: &str) -> bool {
    let digits = value.chars().filter(char::is_ascii_digit).count();
    digits > 0 && digits <= 15 && value.parse::<f64>().is_ok_and(f64::is_finite)
}

fn column_letters(mut index: usize) -> String {
    let mut letters = Vec::new();
    loop {
        letters.push(b'A' + u8::try_from(index % 26).unwrap_or(0));
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    letters.reverse();
    String::from_utf8(letters).unwrap_or_default()
}

/// Escapa o XML e descarta caracteres de controle que o formato não aceita.
fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\t' | '\n' | '\r' => escaped.push(character),
            control if control < ' ' => {}
            other => escaped.push(other),
        }
    }
    escaped
}

/// `dir/stem.ext`, ou `dir/stem-2.ext` e seguintes se o nome já existir.
pub fn free_path(dir: &Path, stem: &str, extension: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{extension}"));
    if !first.exists() {
        return first;
    }
    (2..u32::MAX)
        .map(|n| dir.join(format!("{stem}-{n}.{extension}")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::{Column, ResultSet, Status};
    use calamine::{Data, Reader, Xlsx};

    fn column(name: &str, numeric: bool) -> Column {
        Column {
            name: name.into(),
            numeric,
        }
    }

    fn outcome(device_id: i64, sets: Vec<ResultSet>) -> Outcome {
        Outcome {
            device_id,
            profile: Some("Econect".into()),
            sets,
            status: Status::Success,
            failure: None,
            elapsed_ms: 1,
            affected_rows: 0,
        }
    }

    fn set(columns: Vec<Column>, rows: Vec<Vec<Option<&str>>>) -> ResultSet {
        ResultSet {
            columns,
            rows: rows
                .into_iter()
                .map(|row| row.into_iter().map(|v| v.map(str::to_owned)).collect())
                .collect(),
            affected_rows: 0,
            warnings: 0,
            truncated: false,
        }
    }

    fn origins() -> HashMap<i64, Origin> {
        [(1, "PDV 01", "10.1.3.1", 1), (2, "PDV 02", "10.1.3.2", 0)]
            .into_iter()
            .map(|(id, device, host, order)| {
                (
                    id,
                    Origin {
                        order,
                        store: "101".into(),
                        device: device.into(),
                        host: host.into(),
                    },
                )
            })
            .collect()
    }

    fn sample() -> Grid {
        let outcomes = vec![
            outcome(
                1,
                vec![set(
                    vec![column("versao", false), column("qtd", true)],
                    vec![vec![Some("7.12.4"), Some("999")]],
                )],
            ),
            outcome(
                2,
                vec![
                    set(
                        vec![column("qtd", true), column("extra", false)],
                        vec![vec![Some("500"), None], vec![Some("12.5"), Some("a;b")]],
                    ),
                    set(vec![column("total", true)], vec![vec![Some("2")]]),
                ],
            ),
            Outcome {
                status: Status::Offline,
                sets: Vec::new(),
                profile: None,
                ..outcome(3, Vec::new())
            },
        ];
        consolidate(&outcomes, &origins(), 0)
    }

    #[test]
    fn merges_columns_by_name_and_orders_rows_by_origin() {
        let grid = sample();
        let names: Vec<&str> = grid.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Loja",
                "Equipamento",
                "IP",
                "Perfil",
                "qtd",
                "extra",
                "versao"
            ]
        );
        assert!(grid.columns[4].numeric && !grid.columns[6].numeric);
        assert_eq!(grid.sets, 2);
        let cell = |row: usize, col: usize| grid.rows[row][col].as_deref();
        assert_eq!(grid.rows.len(), 3);
        assert_eq!(cell(0, 1), Some("PDV 02"));
        assert_eq!(cell(0, 4), Some("500"));
        assert_eq!(cell(0, 5), None);
        assert_eq!(cell(2, 1), Some("PDV 01"));
        assert_eq!(cell(2, 2), Some("10.1.3.1"));
        assert_eq!(cell(2, 3), Some("Econect"));
        assert_eq!(cell(2, 4), Some("999"));
        assert_eq!(cell(2, 6), Some("7.12.4"));
    }

    #[test]
    fn repeated_name_in_one_result_keeps_both_columns() {
        let outcomes = vec![outcome(
            1,
            vec![set(
                vec![column("a", true), column("a", true)],
                vec![vec![Some("1"), Some("2")]],
            )],
        )];
        let grid = consolidate(&outcomes, &origins(), 0);
        assert_eq!(grid.columns.len(), 6);
        assert_eq!(grid.rows[0][4..], [Some("1".into()), Some("2".into())]);
    }

    #[test]
    fn later_result_sets_are_separate_grids() {
        let outcomes = vec![outcome(
            2,
            vec![
                set(vec![column("x", false)], vec![vec![Some("1")]]),
                set(vec![column("total", true)], vec![vec![Some("2")]]),
            ],
        )];
        let grid = consolidate(&outcomes, &origins(), 1);
        assert_eq!(grid.columns[4].name, "total");
        assert_eq!(
            grid.rows,
            [vec![
                Some("101".into()),
                Some("PDV 02".into()),
                Some("10.1.3.2".into()),
                Some("Econect".into()),
                Some("2".into())
            ]]
        );
    }

    #[test]
    fn csv_uses_semicolon_bom_and_quotes() {
        let csv = String::from_utf8(to_csv(&sample()).unwrap()).unwrap();
        let mut lines = csv.lines();
        assert_eq!(
            lines.next(),
            Some("\u{feff}Loja;Equipamento;IP;Perfil;qtd;extra;versao")
        );
        assert_eq!(lines.next(), Some("101;PDV 02;10.1.3.2;Econect;500;;"));
        assert_eq!(
            lines.next(),
            Some("101;PDV 02;10.1.3.2;Econect;12.5;\"a;b\";")
        );
    }

    #[test]
    fn xlsx_opens_with_numbers_and_text() {
        let bytes = to_xlsx(&sample()).unwrap();
        let mut book: Xlsx<_> = calamine::open_workbook_from_rs(Cursor::new(bytes)).unwrap();
        let range = book.worksheet_range("Resultado").unwrap();
        assert_eq!(range.get((0, 6)), Some(&Data::String("versao".into())));
        assert_eq!(range.get((1, 4)), Some(&Data::Float(500.0)));
        assert_eq!(range.get((2, 4)), Some(&Data::Float(12.5)));
        assert_eq!(range.get((2, 5)), Some(&Data::String("a;b".into())));
        assert_eq!(range.get((3, 6)), Some(&Data::String("7.12.4".into())));
    }

    #[test]
    fn xlsx_helpers() {
        assert_eq!(column_letters(0), "A");
        assert_eq!(column_letters(25), "Z");
        assert_eq!(column_letters(26), "AA");
        assert_eq!(column_letters(701), "ZZ");
        assert_eq!(column_letters(702), "AAA");
        assert_eq!(escape_xml("a<b>&\"\u{1}c"), "a&lt;b&gt;&amp;&quot;c");
        assert!(is_safe_number("-12.50"));
        assert!(!is_safe_number("1234567890123456789"));
        assert!(!is_safe_number("abc"));
    }

    #[test]
    fn free_path_skips_existing_names() {
        let dir = std::env::temp_dir().join(format!("selectall-grid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = free_path(&dir, "resultado", "csv");
        assert_eq!(first, dir.join("resultado.csv"));
        std::fs::write(&first, "").unwrap();
        assert_eq!(
            free_path(&dir, "resultado", "csv"),
            dir.join("resultado-2.csv")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
