//! Driver MySQL/MariaDB: leitura em transação `READ ONLY`, correção em transação
//! com recusa de tabelas sem transação, cancelamento por `KILL QUERY` em outra conexão.

use crate::exec::{Column, Credential, Failure, Options, Outcome, ResultSet};
use crate::session::{self, ConnectError, Killer, Session};
use mysql::consts::ColumnType;
use mysql::prelude::Queryable;
use mysql::{Conn, OptsBuilder, SslOpts, Value};
use std::collections::HashSet;
use std::net::ToSocketAddrs;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

const ACCESS_DENIED: u16 = 1045;

fn options(host: &str, credential: &Credential, timeout: Duration) -> OptsBuilder {
    let mut options = OptsBuilder::default()
        .ip_or_hostname(Some(host.to_owned()))
        .tcp_port(credential.port)
        .user(Some(credential.username.clone()))
        .pass(Some(credential.password.clone()))
        .tcp_connect_timeout(Some(timeout))
        .read_timeout(Some(timeout.max(Duration::from_secs(30))))
        .write_timeout(Some(timeout.max(Duration::from_secs(30))));
    if !credential.database.is_empty() {
        options = options.db_name(Some(credential.database.clone()));
    }
    if credential.tls {
        options = options.ssl_opts(Some(SslOpts::default()));
    }
    options
}

fn is_network(error: &mysql::Error) -> bool {
    matches!(
        error,
        mysql::Error::IoError(_)
            | mysql::Error::DriverError(
                mysql::DriverError::CouldNotConnect(_) | mysql::DriverError::ConnectTimeout
            )
    )
}

/// Motivo da falha de rede: porta fechada, sem rota e prazo esgotado pedem ações diferentes.
fn network_kind(error: &mysql::Error) -> Option<std::io::ErrorKind> {
    match error {
        mysql::Error::IoError(io) => Some(io.kind()),
        mysql::Error::DriverError(mysql::DriverError::CouldNotConnect(Some((_, _, kind)))) => {
            Some(*kind)
        }
        mysql::Error::DriverError(mysql::DriverError::ConnectTimeout) => {
            Some(std::io::ErrorKind::TimedOut)
        }
        _ => None,
    }
}

/// Traduz os erros mais comuns; o original segue em `detail`.
pub(crate) fn describe(error: &mysql::Error) -> Failure {
    let message = match error {
        mysql::Error::MySqlError(server) => match server.code {
            ACCESS_DENIED => "Usuário ou senha recusados.".to_owned(),
            1049 => "O banco de dados informado no perfil não existe.".to_owned(),
            1146 => session::with_name("Tabela não encontrada", &server.message, '\''),
            1054 => session::with_name("Coluna não encontrada", &server.message, '\''),
            1064 => "Erro de sintaxe no SQL.".to_owned(),
            1142 | 1143 | 1044 => session::DENIED.to_owned(),
            1792 => session::READ_ONLY.to_owned(),
            1317 | 3024 => session::INTERRUPTED.to_owned(),
            1205 => "Linha travada por outra transação por mais de 10 s.".to_owned(),
            _ => server.message.clone(),
        },
        error if is_network(error) => session::NETWORK.to_owned(),
        mysql::Error::TlsError(_) => "Falha ao negociar TLS com o servidor.".to_owned(),
        _ => "Erro de comunicação com o servidor.".to_owned(),
    };
    Failure {
        message,
        detail: Some(error.to_string()),
    }
}

/// Endereços a tentar: o driver só usa o primeiro que o nome resolve, e `localhost`
/// costuma resolver para `::1` antes de `127.0.0.1`. Com TLS fica o nome, que o
/// certificado confere.
fn addresses(host: &str, credential: &Credential) -> Vec<String> {
    if credential.tls {
        return vec![host.to_owned()];
    }
    let mut found: Vec<String> = (host, credential.port)
        .to_socket_addrs()
        .map(|list| list.map(|address| address.ip().to_string()).collect())
        .unwrap_or_default();
    found.dedup();
    if found.is_empty() {
        found.push(host.to_owned());
    }
    found
}

pub(crate) fn open(
    host: &str,
    credential: &Credential,
    timeout: Duration,
) -> Result<Box<dyn Session>, ConnectError> {
    let mut unreachable = None;
    for address in addresses(host, credential) {
        match Conn::new(options(&address, credential, timeout)) {
            Ok(conn) => {
                return Ok(Box::new(MysqlSession {
                    conn,
                    host: address,
                    credential: credential.clone(),
                }));
            }
            Err(error) if is_network(&error) => {
                unreachable = Some(ConnectError::Network(
                    network_kind(&error),
                    describe(&error),
                ));
            }
            Err(mysql::Error::MySqlError(ref server)) if server.code == ACCESS_DENIED => {
                return Err(ConnectError::Refused);
            }
            Err(error) => return Err(ConnectError::Other(describe(&error))),
        }
    }
    Err(unreachable.unwrap_or_else(|| {
        ConnectError::Other(Failure::new("O endereço do equipamento não resolve."))
    }))
}

struct MysqlSession {
    conn: Conn,
    host: String,
    credential: Credential,
}

impl Session for MysqlSession {
    fn killer(&self) -> Killer {
        let (host, credential) = (self.host.clone(), self.credential.clone());
        let connection_id = self.conn.connection_id();
        Arc::new(move |live: &AtomicBool| kill(&host, &credential, connection_id, live))
    }

    fn begin_read(&mut self, settings: &Options) -> Result<(), Failure> {
        self.conn
            .query_drop(format!(
                "SET SESSION SQL_SELECT_LIMIT = {}",
                settings.row_limit.saturating_add(1)
            ))
            .and_then(|()| self.conn.query_drop("START TRANSACTION READ ONLY"))
            .map_err(|error| describe(&error))
    }

    fn read(
        &mut self,
        statement: &str,
        outcome: &mut Outcome,
        settings: &Options,
        bytes: &mut usize,
    ) {
        match self.conn.exec_iter(statement, ()) {
            Ok(result) => collect(result, outcome, settings, bytes),
            Err(error) => session::record_failure(outcome, describe(&error)),
        }
    }

    fn begin_write(&mut self, words: &HashSet<String>) -> Result<(), Failure> {
        let setup = self
            .conn
            .query_drop(format!(
                "SET SESSION innodb_lock_wait_timeout = {LOCK_WAIT_SECONDS}"
            ))
            .and_then(|()| non_transactional(&mut self.conn, words));
        match setup {
            Ok(None) => {}
            Ok(Some(failure)) => return Err(failure),
            Err(error) => return Err(describe(&error)),
        }
        self.conn
            .query_drop("START TRANSACTION")
            .map_err(|error| describe(&error))
    }

    fn write(&mut self, statement: &str) -> Result<u64, Failure> {
        self.conn
            .query_drop(statement)
            .map(|()| self.conn.affected_rows())
            .map_err(|error| describe(&error))
    }

    fn finish(&mut self, commit: bool) -> Result<(), String> {
        self.conn
            .query_drop(if commit { "COMMIT" } else { "ROLLBACK" })
            .map_err(|error| error.to_string())
    }
}

/// Espera por trava de linha limitada: um PDV vendendo não deve segurar a correção.
const LOCK_WAIT_SECONDS: u32 = 10;

/// Tabelas fora do `InnoDB` não desfazem alterações; o ensaio nelas gravaria de verdade.
const NON_TRANSACTIONAL: &str = "SELECT TABLE_SCHEMA, TABLE_NAME, ENGINE FROM information_schema.TABLES \
     WHERE TABLE_TYPE = 'BASE TABLE' AND ENGINE IS NOT NULL AND ENGINE <> 'InnoDB' \
     AND TABLE_SCHEMA NOT IN ('mysql', 'information_schema', 'performance_schema', 'sys')";

/// Recusa o SQL que cita, pelo nome, uma tabela sem transação. Pode recusar a
/// mais (uma coluna com o nome da tabela), nunca a menos.
fn non_transactional(
    conn: &mut Conn,
    words: &HashSet<String>,
) -> Result<Option<Failure>, mysql::Error> {
    let tables: Vec<(String, String, String)> = conn.query(NON_TRANSACTIONAL)?;
    Ok(tables
        .into_iter()
        .find(|(_, name, _)| words.contains(&name.to_uppercase()))
        .map(|(schema, name, engine)| {
            Failure::new(format!(
                "A tabela {schema}.{name} usa {engine}, que não desfaz alterações. Corrigir exige tabelas InnoDB."
            ))
        }))
}

fn collect<T: mysql::prelude::Protocol>(
    mut result: mysql::QueryResult<'_, '_, '_, T>,
    outcome: &mut Outcome,
    settings: &Options,
    bytes: &mut usize,
) {
    while let Some(mut set) = result.iter() {
        let columns = set.columns();
        let kinds: Vec<ColumnType> = columns
            .as_ref()
            .iter()
            .map(mysql::Column::column_type)
            .collect();
        let mut captured = ResultSet {
            columns: columns
                .as_ref()
                .iter()
                .map(|column| Column {
                    name: column.name_str().into_owned(),
                    numeric: is_numeric(column.column_type()),
                })
                .collect(),
            rows: Vec::new(),
            affected_rows: set.affected_rows(),
            warnings: set.warnings(),
            truncated: false,
        };
        *bytes += captured
            .columns
            .iter()
            .map(|c| c.name.len() + 32)
            .sum::<usize>();
        for row in &mut set {
            match row {
                Ok(row) => {
                    if captured.rows.len() >= settings.row_limit {
                        captured.truncated = true;
                        continue;
                    }
                    let cells: Vec<Option<String>> = (0..row.len())
                        .map(|index| {
                            format_value(row.as_ref(index).unwrap_or(&Value::NULL), kinds[index])
                        })
                        .collect();
                    let row_bytes = cells
                        .iter()
                        .map(|cell| cell.as_ref().map_or(0, String::len) + 24)
                        .sum::<usize>();
                    if bytes.saturating_add(row_bytes) > settings.byte_limit {
                        captured.truncated = true;
                        continue;
                    }
                    *bytes += row_bytes;
                    captured.rows.push(cells);
                }
                Err(error) => session::record_failure(outcome, describe(&error)),
            }
        }
        if outcome.failure.is_none() || !captured.columns.is_empty() {
            outcome.sets.push(captured);
        }
    }
}

fn is_numeric(kind: ColumnType) -> bool {
    use ColumnType::{
        MYSQL_TYPE_DECIMAL, MYSQL_TYPE_DOUBLE, MYSQL_TYPE_FLOAT, MYSQL_TYPE_INT24, MYSQL_TYPE_LONG,
        MYSQL_TYPE_LONGLONG, MYSQL_TYPE_NEWDECIMAL, MYSQL_TYPE_SHORT, MYSQL_TYPE_TINY,
        MYSQL_TYPE_YEAR,
    };
    matches!(
        kind,
        MYSQL_TYPE_DECIMAL
            | MYSQL_TYPE_NEWDECIMAL
            | MYSQL_TYPE_TINY
            | MYSQL_TYPE_SHORT
            | MYSQL_TYPE_LONG
            | MYSQL_TYPE_LONGLONG
            | MYSQL_TYPE_INT24
            | MYSQL_TYPE_FLOAT
            | MYSQL_TYPE_DOUBLE
            | MYSQL_TYPE_YEAR
    )
}

/// Datas seguem o tipo da coluna: `DATE` sem hora, frações só quando existem.
pub(crate) fn format_value(value: &Value, kind: ColumnType) -> Option<String> {
    let fraction = |micros: u32| {
        if micros == 0 {
            String::new()
        } else {
            format!(".{micros:06}")
        }
    };
    Some(match value {
        Value::NULL => return None,
        Value::Bytes(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => text.to_owned(),
            Err(_) => bytes.iter().fold(String::from("0x"), |mut hex, byte| {
                use std::fmt::Write;
                let _ = write!(hex, "{byte:02X}");
                hex
            }),
        },
        Value::Int(value) => value.to_string(),
        Value::UInt(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::Double(value) => value.to_string(),
        Value::Date(year, month, day, ..)
            if matches!(
                kind,
                ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE
            ) =>
        {
            format!("{year:04}-{month:02}-{day:02}")
        }
        Value::Date(year, month, day, hour, minute, second, micros) => format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}{}",
            fraction(*micros)
        ),
        Value::Time(negative, days, hour, minute, second, micros) => format!(
            "{}{:02}:{minute:02}:{second:02}{}",
            if *negative { "-" } else { "" },
            days * 24 + u32::from(*hour),
            fraction(*micros)
        ),
    })
}

fn kill(
    host: &str,
    credential: &Credential,
    connection_id: u32,
    live: &AtomicBool,
) -> Result<(), String> {
    let mut conn = Conn::new(options(host, credential, Duration::from_secs(5)))
        .map_err(|error| describe(&error).message)?;
    if live.load(Ordering::SeqCst) {
        conn.query_drop(format!("KILL QUERY {connection_id}"))
            .map_err(|error| describe(&error).message)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_follow_the_column_type() {
        let date = Value::Date(2026, 11, 21, 0, 0, 0, 0);
        assert_eq!(
            format_value(&date, ColumnType::MYSQL_TYPE_DATE).unwrap(),
            "2026-11-21"
        );
        assert_eq!(
            format_value(&date, ColumnType::MYSQL_TYPE_DATETIME).unwrap(),
            "2026-11-21 00:00:00"
        );
        let precise = Value::Date(2026, 11, 21, 8, 14, 3, 120);
        assert_eq!(
            format_value(&precise, ColumnType::MYSQL_TYPE_TIMESTAMP).unwrap(),
            "2026-11-21 08:14:03.000120"
        );
        let time = Value::Time(true, 1, 2, 3, 4, 0);
        assert_eq!(
            format_value(&time, ColumnType::MYSQL_TYPE_TIME).unwrap(),
            "-26:03:04"
        );
        assert_eq!(
            format_value(&Value::NULL, ColumnType::MYSQL_TYPE_LONG),
            None
        );
        assert_eq!(
            format_value(&Value::Bytes(vec![0xff, 0x00]), ColumnType::MYSQL_TYPE_BLOB).unwrap(),
            "0xFF00"
        );
    }

    #[test]
    fn server_errors_name_the_missing_object() {
        let server = |code, message: &str| mysql::MySqlError {
            state: "42S02".into(),
            message: message.into(),
            code,
        };
        let table = server(1146, "Table 'pdv.parametros' doesn't exist");
        assert_eq!(
            describe(&mysql::Error::MySqlError(table)).message,
            "Tabela não encontrada: pdv.parametros."
        );
        let column = server(1054, "Unknown column 'versao_pdv' in 'field list'");
        assert_eq!(
            describe(&mysql::Error::MySqlError(column)).message,
            "Coluna não encontrada: versao_pdv."
        );
    }

    #[test]
    fn network_kind_reads_the_driver_error() {
        use std::io::{Error, ErrorKind};
        let no_route = mysql::Error::DriverError(mysql::DriverError::CouldNotConnect(Some((
            "192.0.2.10:3306".into(),
            "No route to host".into(),
            ErrorKind::HostUnreachable,
        ))));
        assert_eq!(network_kind(&no_route), Some(ErrorKind::HostUnreachable));
        let refused = mysql::Error::IoError(Error::from(ErrorKind::ConnectionRefused));
        assert_eq!(network_kind(&refused), Some(ErrorKind::ConnectionRefused));
        let slow = mysql::Error::DriverError(mysql::DriverError::ConnectTimeout);
        assert_eq!(network_kind(&slow), Some(ErrorKind::TimedOut));
    }
}
