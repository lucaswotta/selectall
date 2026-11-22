//! Histórico de leituras: cada execução com os dados de cada equipamento, dentro
//! de um limite de idade e de tamanho. SQL com termo de autenticação não é guardado.

use crate::exec::{Outcome, Status};
use crate::grid::Origin;
use crate::params::{Script, Values};
use crate::sql;
use serde::Serialize;

pub const DEFAULT_DAYS: u32 = 30;
pub const DEFAULT_MEGABYTES: u32 = 500;
const LABEL_CHARS: usize = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Limits {
    pub days: u32,
    pub megabytes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            days: DEFAULT_DAYS,
            megabytes: DEFAULT_MEGABYTES,
        }
    }
}

impl Limits {
    pub fn bytes(self) -> i64 {
        i64::from(self.megabytes) * 1024 * 1024
    }
}

/// Uma leitura concluída, pronta para gravar.
#[derive(Clone, Debug)]
pub struct NewRun {
    pub chain_id: i64,
    /// Nome da consulta salva, quando veio de uma.
    pub label: Option<String>,
    pub script: Option<Script>,
    pub targets: usize,
    pub elapsed_ms: u64,
    pub devices: Vec<(Origin, Outcome)>,
}

/// Uma execução na lista do histórico.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HistoryEntry {
    pub id: i64,
    /// UTC, `AAAA-MM-DDTHH:MM:SSZ`.
    pub at: String,
    pub label: String,
    /// `None` quando o SQL tinha termo de autenticação.
    pub sql: Option<String>,
    pub values: Values,
    pub targets: i64,
    pub responded: i64,
    pub offline: i64,
    pub failed: i64,
    pub rows: i64,
    pub elapsed_ms: i64,
    pub bytes: i64,
    /// Códigos das lojas alcançadas, preenchido na lista da rede.
    pub stores: Vec<String>,
}

/// Uso do histórico frente ao limite configurado.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub limits: Limits,
    pub bytes: i64,
    pub runs: i64,
}

/// O SQL e os valores somem quando qualquer um cita senha, token ou segredo.
pub fn redact(script: Option<&Script>) -> Option<&Script> {
    script.filter(|script| {
        !sql::may_contain_credentials(&script.sql)
            && !script
                .values
                .values()
                .any(|v| sql::may_contain_credentials(v))
    })
}

/// Rótulo da execução: o nome da consulta salva ou a primeira linha com código.
pub fn label(run: &NewRun) -> String {
    if let Some(label) = run
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        return label.chars().take(LABEL_CHARS).collect();
    }
    let Some(script) = redact(run.script.as_ref()) else {
        return "SQL não guardado: cita senha, token ou segredo".into();
    };
    let line = script
        .sql
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("--"))
        .unwrap_or("");
    let mut text: String = line.chars().take(LABEL_CHARS).collect();
    if line.chars().count() > LABEL_CHARS || script.sql.trim() != line {
        text.push('…');
    }
    text
}

/// Equipamentos que responderam, ficaram offline e falharam (o resto).
pub fn counts(devices: &[(Origin, Outcome)]) -> (i64, i64, i64) {
    let count = |wanted: &dyn Fn(Status) -> bool| {
        i64::try_from(devices.iter().filter(|(_, o)| wanted(o.status)).count()).unwrap_or(0)
    };
    let responded = count(&|s| matches!(s, Status::Success | Status::Partial));
    let offline = count(&|s| s == Status::Offline);
    let total = i64::try_from(devices.len()).unwrap_or(0);
    (responded, offline, total - responded - offline)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(label: Option<&str>, sql: &str, values: &[(&str, &str)]) -> NewRun {
        NewRun {
            chain_id: 1,
            label: label.map(str::to_owned),
            script: Some(Script::new(
                sql,
                values
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect(),
            )),
            targets: 0,
            elapsed_ms: 0,
            devices: Vec::new(),
        }
    }

    #[test]
    fn credentials_in_sql_or_values_hide_the_script() {
        assert!(redact(run(None, "SELECT 1", &[]).script.as_ref()).is_some());
        for (sql, values) in [
            ("SELECT password FROM users", vec![]),
            ("ALTER USER x IDENTIFIED BY 'a'", vec![]),
            ("SELECT {{a}}", vec![("a", "meu_token")]),
            ("SELECT senha FROM operadores", vec![]),
        ] {
            let run = run(None, sql, &values);
            assert!(redact(run.script.as_ref()).is_none(), "{sql}");
            assert!(label(&run).starts_with("SQL não guardado"));
        }
    }

    #[test]
    fn label_prefers_the_saved_name_then_the_first_line() {
        assert_eq!(label(&run(Some(" Versão "), "SELECT 1", &[])), "Versão");
        assert_eq!(
            label(&run(None, "-- versão\nSELECT 1\nFROM t", &[])),
            "SELECT 1…"
        );
        assert_eq!(label(&run(None, "SELECT 1", &[])), "SELECT 1");
    }
}
