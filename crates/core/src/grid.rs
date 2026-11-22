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

/// Coluna que pode entrar na comparação. `temporal`: todo valor é data ou hora,
/// que muda a cada execução e fica de fora por padrão.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CompareColumn {
    pub name: String,
    pub chosen: bool,
    pub temporal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Member {
    pub store: String,
    pub device: String,
    pub host: String,
}

/// Quantos equipamentos da loja caíram no grupo, de quantos comparados.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StoreShare {
    pub store: String,
    pub count: usize,
    pub total: usize,
}

/// Equipamentos com o mesmo resultado nas colunas escolhidas.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Group {
    /// Linhas distintas do resultado, só nas colunas escolhidas, até `SAMPLE_ROWS`.
    pub rows: Rows,
    pub row_count: usize,
    pub members: Vec<Member>,
    pub stores: Vec<StoreShare>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Comparison {
    pub columns: Vec<CompareColumn>,
    /// Do maior para o menor; empate mantém a ordem da grade.
    pub groups: Vec<Group>,
    /// O primeiro grupo é maior que todos os outros.
    pub majority: bool,
    /// Equipamentos que responderam e entraram na comparação.
    pub compared: usize,
}

const SAMPLE_ROWS: usize = 20;

type Rows = Vec<Vec<Option<String>>>;

/// Data (`2026-11-21`, com ou sem hora) ou hora (`09:41`, `09:41:02.5`).
fn is_temporal(value: &str) -> bool {
    let bytes = value.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        bytes
            .get(range)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
    };
    let date = digits(0..4)
        && bytes.get(4) == Some(&b'-')
        && digits(5..7)
        && bytes.get(7) == Some(&b'-')
        && digits(8..10);
    let time = digits(0..2) && bytes.get(2) == Some(&b':') && digits(3..5);
    date || time
}

/// Agrupa os equipamentos que responderam pelo resultado do conjunto `set` nas
/// colunas `chosen` (padrão: todas menos as de data e hora). A ordem das linhas
/// não conta: cada resultado é comparado com as linhas ordenadas.
pub fn compare<S: std::hash::BuildHasher>(
    outcomes: &[Outcome],
    origins: &HashMap<i64, Origin, S>,
    set: usize,
    chosen: Option<&[String]>,
) -> Comparison {
    use crate::exec::Status;
    let mut responded: Vec<&Outcome> = outcomes
        .iter()
        .filter(|o| matches!(o.status, Status::Success | Status::Partial))
        .collect();
    responded.sort_by_key(|o| {
        origins
            .get(&o.device_id)
            .map_or(usize::MAX, |origin| origin.order)
    });
    let owned: Vec<Outcome> = responded.iter().map(|o| (*o).clone()).collect();
    let grid = consolidate(&owned, origins, set);
    let (columns, picked) = choose_columns(&grid, chosen);

    // Linhas de cada equipamento: a grade vem na ordem da origem, então ficam juntas.
    let mut by_device: HashMap<[&str; 3], Rows> = HashMap::new();
    for row in &grid.rows {
        let key = [0, 1, 2].map(|index| row[index].as_deref().unwrap_or(""));
        by_device
            .entry(key)
            .or_default()
            .push(picked.iter().map(|&index| row[index].clone()).collect());
    }
    let mut groups: Vec<(Rows, Vec<Member>)> = Vec::new();
    for outcome in &responded {
        let Some(origin) = origins.get(&outcome.device_id) else {
            continue;
        };
        let mut rows = by_device
            .remove(&[
                origin.store.as_str(),
                origin.device.as_str(),
                origin.host.as_str(),
            ])
            .unwrap_or_default();
        rows.sort();
        let member = Member {
            store: origin.store.clone(),
            device: origin.device.clone(),
            host: origin.host.clone(),
        };
        if let Some((_, members)) = groups.iter_mut().find(|(key, _)| *key == rows) {
            members.push(member);
        } else {
            groups.push((rows, vec![member]));
        }
    }
    groups.sort_by_key(|(_, members)| std::cmp::Reverse(members.len()));
    summarize(columns, &groups)
}

/// Colunas de dados da grade e os índices das escolhidas.
fn choose_columns(grid: &Grid, chosen: Option<&[String]>) -> (Vec<CompareColumn>, Vec<usize>) {
    let data: Vec<usize> = (FIXED_COLUMNS.len()..grid.columns.len()).collect();
    let temporal: Vec<bool> = data
        .iter()
        .map(|&index| {
            let mut values = grid
                .rows
                .iter()
                .filter_map(|row| row[index].as_deref())
                .peekable();
            values.peek().is_some() && values.all(is_temporal)
        })
        .collect();
    let all_temporal = temporal.iter().all(|t| *t);
    let columns: Vec<CompareColumn> = data
        .iter()
        .zip(&temporal)
        .map(|(&index, &temporal)| {
            let name = grid.columns[index].name.clone();
            let picked = chosen.map_or(!temporal || all_temporal, |names| names.contains(&name));
            CompareColumn {
                name,
                chosen: picked,
                temporal,
            }
        })
        .collect();
    let picked: Vec<usize> = data
        .iter()
        .zip(&columns)
        .filter(|(_, column)| column.chosen)
        .map(|(&index, _)| index)
        .collect();
    (columns, picked)
}

fn summarize(columns: Vec<CompareColumn>, groups: &[(Rows, Vec<Member>)]) -> Comparison {
    let mut totals: HashMap<&str, usize> = HashMap::new();
    for (_, members) in groups {
        for member in members {
            *totals.entry(member.store.as_str()).or_default() += 1;
        }
    }
    let compared = groups.iter().map(|(_, members)| members.len()).sum();
    let majority = match groups {
        [] => false,
        [_] => true,
        [first, second, ..] => first.1.len() > second.1.len(),
    };
    let groups = groups
        .iter()
        .map(|(rows, members)| {
            let mut distinct = rows.clone();
            distinct.dedup();
            let mut stores: Vec<StoreShare> = Vec::new();
            for member in members {
                if let Some(share) = stores.iter_mut().find(|s| s.store == member.store) {
                    share.count += 1;
                } else {
                    stores.push(StoreShare {
                        store: member.store.clone(),
                        count: 1,
                        total: totals.get(member.store.as_str()).copied().unwrap_or(0),
                    });
                }
            }
            Group {
                row_count: rows.len(),
                rows: distinct.into_iter().take(SAMPLE_ROWS).collect(),
                members: members.clone(),
                stores,
            }
        })
        .collect();
    Comparison {
        columns,
        groups,
        majority,
        compared,
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

    fn five_origins() -> HashMap<i64, Origin> {
        [
            (1, "101", "PDV 01"),
            (2, "101", "PDV 02"),
            (3, "102", "PDV 01"),
            (4, "102", "PDV 02"),
            (5, "102", "PDV 03"),
        ]
        .into_iter()
        .enumerate()
        .map(|(order, (id, store, device))| {
            (
                id,
                Origin {
                    order,
                    store: store.into(),
                    device: device.into(),
                    host: format!("10.0.0.{id}"),
                },
            )
        })
        .collect()
    }

    #[test]
    fn compare_groups_devices_ignoring_dates_and_row_order() {
        let columns = || vec![column("versao", false), column("carga", false)];
        let origins = five_origins();
        let outcomes = vec![
            outcome(
                1,
                vec![set(
                    columns(),
                    vec![
                        vec![Some("4.2"), Some("2026-11-21 09:00:00")],
                        vec![Some("a"), None],
                    ],
                )],
            ),
            outcome(
                2,
                vec![set(
                    columns(),
                    vec![
                        vec![Some("a"), None],
                        vec![Some("4.2"), Some("2026-11-20 08:00:00")],
                    ],
                )],
            ),
            outcome(
                3,
                vec![set(
                    columns(),
                    vec![
                        vec![Some("4.1"), Some("2026-11-21 07:00:00")],
                        vec![Some("a"), None],
                    ],
                )],
            ),
            outcome(
                4,
                vec![set(
                    columns(),
                    vec![
                        vec![Some("4.2"), Some("2026-11-19 07:00:00")],
                        vec![Some("a"), None],
                    ],
                )],
            ),
            Outcome {
                status: Status::Offline,
                ..outcome(5, Vec::new())
            },
        ];
        let comparison = compare(&outcomes, &origins, 0, None);
        assert_eq!(
            comparison
                .columns
                .iter()
                .map(|c| (c.name.as_str(), c.chosen, c.temporal))
                .collect::<Vec<_>>(),
            [("versao", true, false), ("carga", false, true)]
        );
        assert!(comparison.majority);
        assert_eq!(comparison.compared, 4);
        assert_eq!(comparison.groups.len(), 2);
        assert_eq!(comparison.groups[0].members.len(), 3);
        let minor = &comparison.groups[1];
        assert_eq!(minor.members[0].store, "102");
        assert_eq!(
            minor.rows,
            [vec![Some("4.1".to_owned())], vec![Some("a".to_owned())]]
        );
        assert_eq!(
            minor.stores,
            [StoreShare {
                store: "102".into(),
                count: 1,
                total: 2
            }]
        );

        // Com a data escolhida, cada equipamento fica sozinho e não há maioria.
        let all = compare(
            &outcomes,
            &origins,
            0,
            Some(&["versao".into(), "carga".into()]),
        );
        assert_eq!(all.groups.len(), 4);
        assert!(!all.majority);
    }

    #[test]
    fn temporal_values_are_dates_or_times() {
        for value in [
            "2026-11-21",
            "2026-11-21 09:41:02",
            "2026-11-21T09:41:02+00",
            "09:41",
            "23:59:59.123",
        ] {
            assert!(is_temporal(value), "{value}");
        }
        for value in ["4.2.1", "2026", "1:2", "abc", "20261121"] {
            assert!(!is_temporal(value), "{value}");
        }
    }
}
