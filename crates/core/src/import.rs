//! Importação de planilha CSV ou XLSX para um [`Batch`]. Colunas de senha são
//! ignoradas: credenciais só entram pelo perfil, no cofre.

use crate::fleet::{Batch, Entry, LineError, StoreSpec};
use calamine::{Reader, open_workbook_auto_from_rs};
use std::collections::HashSet;
use std::io::Cursor;

/// Lê o arquivo pelo conteúdo; a extensão do nome decide entre CSV e planilha.
pub fn read(file_name: &str, bytes: &[u8]) -> Result<Batch, String> {
    let extension = file_name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    let (headers, rows) = match extension.as_str() {
        "csv" | "txt" => read_csv(bytes)?,
        "xlsx" | "xlsm" | "xls" | "ods" => read_workbook(bytes)?,
        _ => return Err("Formato não suportado. Escolha CSV ou XLSX.".into()),
    };
    parse_rows(&headers, rows)
}

type Rows = Vec<Result<Vec<String>, String>>;

fn read_csv(bytes: &[u8]) -> Result<(Vec<String>, Rows), String> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(detect_delimiter(bytes))
        .flexible(true)
        .from_reader(bytes);
    let headers = reader
        .headers()
        .map_err(|error| format!("Cabeçalho CSV inválido: {error}"))?
        .iter()
        .map(str::to_owned)
        .collect();
    let rows = reader
        .records()
        .map(|row| {
            row.map(|record| record.iter().map(str::to_owned).collect())
                .map_err(|error| format!("linha CSV inválida: {error}"))
        })
        .collect();
    Ok((headers, rows))
}

/// Separador mais frequente fora de aspas na primeira linha; vírgula se nenhum aparecer.
fn detect_delimiter(bytes: &[u8]) -> u8 {
    let first_line = bytes
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    let mut counts = [(b',', 0usize), (b';', 0), (b'\t', 0), (b'|', 0)];
    let mut quoted = false;
    for byte in first_line {
        if *byte == b'"' {
            quoted = !quoted;
        } else if !quoted
            && let Some((_, count)) = counts.iter_mut().find(|(delimiter, _)| delimiter == byte)
        {
            *count += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .max_by_key(|(_, count)| *count)
        .map_or(b',', |(delimiter, _)| delimiter)
}

fn read_workbook(bytes: &[u8]) -> Result<(Vec<String>, Rows), String> {
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|error| format!("Não foi possível abrir a planilha: {error}"))?;
    let range = workbook
        .worksheet_range_at(0)
        .ok_or("A planilha não tem abas.")?
        .map_err(|error| format!("Não foi possível ler a primeira aba: {error}"))?;
    let mut rows = range.rows();
    let headers = rows
        .next()
        .ok_or("A planilha está vazia.")?
        .iter()
        .map(ToString::to_string)
        .collect();
    let rows = rows
        .map(|row| Ok(row.iter().map(ToString::to_string).collect()))
        .collect();
    Ok((headers, rows))
}

struct Columns {
    host: usize,
    store: Option<usize>,
    store_name: Option<usize>,
    subnet: Option<usize>,
    device: Option<usize>,
    kind: Option<usize>,
    profile: Option<usize>,
    password: bool,
}

impl Columns {
    fn find(headers: &[String]) -> Result<Self, String> {
        let normalized: Vec<String> = headers.iter().map(|header| normalize(header)).collect();
        if normalized.iter().all(String::is_empty) {
            return Err("O arquivo não tem cabeçalho.".into());
        }
        let mut seen = HashSet::new();
        if let Some(duplicate) = normalized
            .iter()
            .find(|header| !header.is_empty() && !seen.insert(*header))
        {
            return Err(format!("O cabeçalho repete a coluna {duplicate}."));
        }
        let find = |aliases: &[&str]| {
            aliases
                .iter()
                .find_map(|alias| normalized.iter().position(|header| header == alias))
        };
        Ok(Self {
            host: find(&["ip", "host", "endereco", "enderecoip", "hostname"])
                .ok_or("Falta a coluna ip (ou host, endereço).")?,
            store: find(&["loja", "codigo", "codloja", "codigoloja", "store"]),
            store_name: find(&["nome", "nomeloja", "descricao"]),
            subnet: find(&["subrede", "sub", "subnet"]),
            device: find(&["equipamento", "nomeequipamento", "maquina", "pdv", "caixa"]),
            kind: find(&["tipo", "kind"]),
            profile: find(&["perfil", "sistema", "profile"]),
            password: find(&["senha", "password", "pass"]).is_some(),
        })
    }
}

fn parse_rows(headers: &[String], rows: Rows) -> Result<Batch, String> {
    let columns = Columns::find(headers)?;
    let mut batch = Batch::default();
    if columns.password {
        batch
            .notices
            .push("A coluna de senha foi ignorada; cadastre a senha no perfil de sistema.".into());
    }
    let mut stores = HashSet::new();
    for (index, row) in rows.into_iter().enumerate() {
        // Linha 1 é o cabeçalho.
        let line = index + 2;
        let row = match row {
            Ok(row) => row,
            Err(message) => {
                batch.errors.push(LineError { line, message });
                continue;
            }
        };
        let value = |column: Option<usize>| {
            column
                .and_then(|index| row.get(index))
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
        };
        if row.iter().all(|cell| cell.trim().is_empty()) {
            continue;
        }
        let Some(host) = value(Some(columns.host)) else {
            batch.errors.push(LineError {
                line,
                message: "Falta o IP.".into(),
            });
            continue;
        };
        let store_code = value(columns.store);
        if let Some(code) = &store_code
            && stores.insert(code.to_lowercase())
        {
            batch.stores.push(StoreSpec {
                line,
                code: code.clone(),
                name: value(columns.store_name).unwrap_or_default(),
                subnet: value(columns.subnet).unwrap_or_default(),
            });
        }
        batch.entries.push(Entry {
            line,
            store_code,
            host,
            kind: value(columns.kind),
            name: value(columns.device),
            profile_id: None,
            profile_name: value(columns.profile),
        });
    }
    Ok(batch)
}

fn normalize(value: &str) -> String {
    value
        .trim_matches(['\u{feff}', ' ', '\t'])
        .to_lowercase()
        .chars()
        .filter_map(|character| match character {
            'á' | 'à' | 'ã' | 'â' => Some('a'),
            'é' | 'ê' => Some('e'),
            'í' => Some('i'),
            'ó' | 'ô' | 'õ' => Some('o'),
            'ú' => Some('u'),
            'ç' => Some('c'),
            ' ' | '-' | '_' | '.' => None,
            other => Some(other),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_csv_with_semicolons_bom_and_accents() {
        let csv = "\u{feff}Loja;Nome;IP;Tipo;Perfil;Senha\n101;Centro;10.101.3.1;PDV;Econect;segredo\n101;Centro;10.101.3.2;;;\n;;;;;\n102;\"Jardim; Sul\";;PDV;;\n";
        let batch = read("frota.CSV", csv.as_bytes()).unwrap();
        assert_eq!(batch.entries.len(), 2);
        assert_eq!(batch.entries[0].profile_name.as_deref(), Some("Econect"));
        assert_eq!(batch.entries[1].kind, None);
        assert_eq!(batch.stores.len(), 1);
        assert_eq!(batch.stores[0].name, "Centro");
        assert_eq!(
            batch.errors,
            [LineError {
                line: 5,
                message: "Falta o IP.".into()
            }]
        );
        assert_eq!(batch.notices.len(), 1);
        assert!(!format!("{batch:?}").contains("segredo"));
    }

    #[test]
    fn requires_ip_column_and_unique_headers() {
        assert!(read("a.csv", b"loja,nome\n1,x").unwrap_err().contains("ip"));
        assert!(read("a.csv", b"ip,IP\n1,2").unwrap_err().contains("repete"));
        assert!(read("a.pdf", b"").unwrap_err().contains("Formato"));
    }

    #[test]
    fn detects_delimiter_outside_quotes() {
        assert_eq!(detect_delimiter(b"\"a,b,c\";ip;loja\n"), b';');
        assert_eq!(detect_delimiter(b"ip\n"), b',');
        assert_eq!(detect_delimiter(b"ip\tloja\n"), b'\t');
    }

    #[test]
    fn reads_first_sheet_of_xlsx() {
        let bytes = include_bytes!("../tests/fixtures/frota.xlsx");
        let batch = read("frota.xlsx", bytes).unwrap();
        let hosts: Vec<_> = batch
            .entries
            .iter()
            .map(|entry| entry.host.as_str())
            .collect();
        assert_eq!(hosts, ["10.101.3.1", "10.102.3.200"]);
        assert_eq!(batch.entries[1].kind.as_deref(), Some("Concentrador"));
        assert_eq!(batch.stores[1].name, "Jardim");
        assert_eq!(batch.notices.len(), 1);
    }

    #[test]
    fn rejects_broken_workbook() {
        assert!(
            read("frota.xlsx", b"isto nao e zip")
                .unwrap_err()
                .starts_with("Não foi possível abrir")
        );
    }
}
