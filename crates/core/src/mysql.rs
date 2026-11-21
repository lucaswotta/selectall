//! Driver MySQL/MariaDB: leitura em transação `READ ONLY`, alternativa entre
//! perfis, prazo e cancelamento por `KILL QUERY` em outra conexão.

use crate::exec::{
    ActiveQueries, Column, Credential, Failure, Options, Outcome, ResultSet, Status, Target,
};
use mysql::consts::ColumnType;
use mysql::prelude::Queryable;
use mysql::{Conn, OptsBuilder, SslOpts, Value};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

const ACCESS_DENIED: u16 = 1045;

#[derive(Clone)]
pub(crate) struct ActiveQuery {
    host: String,
    credential: Credential,
    connection_id: u32,
    live: Arc<AtomicBool>,
}

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

/// Traduz os erros mais comuns; o original segue em `detail`.
pub(crate) fn describe(error: &mysql::Error) -> Failure {
    let message = match error {
        mysql::Error::MySqlError(server) => match server.code {
            ACCESS_DENIED => "Usuário ou senha recusados.".to_owned(),
            1049 => "O banco de dados informado no perfil não existe.".to_owned(),
            1146 => "Tabela não encontrada.".to_owned(),
            1054 => "Coluna não encontrada.".to_owned(),
            1064 => "Erro de sintaxe no SQL.".to_owned(),
            1142 | 1143 | 1044 => {
                "O usuário do perfil não tem permissão para esta consulta.".to_owned()
            }
            1792 => "Bloqueado: a consulta tentou alterar dados no modo leitura.".to_owned(),
            1317 | 3024 => "Consulta interrompida pelo servidor.".to_owned(),
            _ => server.message.clone(),
        },
        error if is_network(error) => "Sem resposta do equipamento na rede.".to_owned(),
        mysql::Error::TlsError(_) => "Falha ao negociar TLS com o servidor.".to_owned(),
        _ => "Erro de comunicação com o servidor.".to_owned(),
    };
    Failure {
        message,
        detail: Some(error.to_string()),
    }
}

enum Connected {
    Ready(Conn, Credential),
    Failed(Status, Failure),
}

/// Tenta os perfis em ordem. Endereço sem resposta numa porta não é testado de novo
/// na mesma porta; senha recusada passa ao próximo perfil.
fn connect(target: &Target, timeout: Duration) -> Connected {
    let mut unreachable_ports = Vec::new();
    let mut refused = Vec::new();
    let mut last_failure = None;
    for credential in &target.credentials {
        if unreachable_ports.contains(&credential.port) {
            continue;
        }
        match Conn::new(options(&target.host, credential, timeout)) {
            Ok(conn) => return Connected::Ready(conn, credential.clone()),
            Err(error) if is_network(&error) => {
                unreachable_ports.push(credential.port);
                last_failure = Some(describe(&error));
            }
            Err(mysql::Error::MySqlError(ref server)) if server.code == ACCESS_DENIED => {
                refused.push(credential.profile_name.clone());
            }
            Err(error) => last_failure = Some(describe(&error)),
        }
    }
    if !refused.is_empty() {
        let names = join_names(&refused);
        let message = if refused.len() == 1 {
            format!("Usuário ou senha recusados pelo perfil {names}.")
        } else {
            format!("Usuário ou senha recusados pelos perfis {names}.")
        };
        return Connected::Failed(Status::Failed, Failure::new(message));
    }
    match last_failure {
        Some(failure) if unreachable_ports.len() == distinct_ports(target) => {
            let ports = unreachable_ports
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            Connected::Failed(
                Status::Offline,
                Failure {
                    message: format!("Não respondeu na porta {ports} em {} s.", timeout.as_secs()),
                    detail: failure.detail,
                },
            )
        }
        Some(failure) => Connected::Failed(Status::Failed, failure),
        None => Connected::Failed(
            Status::Failed,
            Failure::new("Nenhum perfil configurado para este equipamento."),
        ),
    }
}

fn distinct_ports(target: &Target) -> usize {
    let mut ports: Vec<u16> = target.credentials.iter().map(|c| c.port).collect();
    ports.sort_unstable();
    ports.dedup();
    ports.len()
}

fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} e {last}", rest.join(", ")),
    }
}

pub(crate) fn execute_read(
    target: &Target,
    statements: &[String],
    settings: &Options,
    cancelled: &AtomicBool,
    active: &ActiveQueries,
    on_running: impl FnOnce(&str),
) -> Outcome {
    let start = Instant::now();
    let mut outcome = Outcome {
        device_id: target.device_id,
        profile: None,
        sets: Vec::new(),
        status: Status::Success,
        failure: None,
        elapsed_ms: 0,
    };
    let finish = |mut outcome: Outcome| {
        outcome.elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        outcome
    };
    let (mut conn, credential) = match connect(target, settings.connect_timeout) {
        Connected::Ready(conn, credential) => (conn, credential),
        Connected::Failed(status, failure) => {
            outcome.status = status;
            outcome.failure = Some(failure);
            return finish(outcome);
        }
    };
    outcome.profile = Some(credential.profile_name.clone());
    let setup = conn
        .query_drop(format!(
            "SET SESSION SQL_SELECT_LIMIT = {}",
            settings.row_limit.saturating_add(1)
        ))
        .and_then(|()| conn.query_drop("START TRANSACTION READ ONLY"));
    if let Err(error) = setup {
        outcome.status = Status::Failed;
        outcome.failure = Some(describe(&error));
        return finish(outcome);
    }

    let live = Arc::new(AtomicBool::new(true));
    let query = ActiveQuery {
        host: target.host.clone(),
        credential: credential.clone(),
        connection_id: conn.connection_id(),
        live: live.clone(),
    };
    if let Ok(mut queries) = active.lock() {
        queries.push(query.clone());
    }
    let timed_out = Arc::new(AtomicBool::new(false));
    let (done, wait) = mpsc::channel::<()>();
    {
        let (timed_out, deadline) = (timed_out.clone(), settings.deadline);
        std::thread::spawn(move || {
            if matches!(
                wait.recv_timeout(deadline),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) && query.live.load(Ordering::SeqCst)
            {
                timed_out.store(true, Ordering::SeqCst);
                let _ = kill(&query);
            }
        });
    }

    on_running(&credential.profile_name);
    let mut bytes = 0;
    for statement in statements {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        match conn.exec_iter(statement.as_str(), ()) {
            Ok(result) => collect(result, &mut outcome, settings, &mut bytes),
            Err(error) => record_error(&mut outcome, &error),
        }
        if outcome.failure.is_some() {
            break;
        }
    }

    live.store(false, Ordering::SeqCst);
    let _ = done.send(());
    if let Ok(mut queries) = active.lock() {
        queries.retain(|query| !Arc::ptr_eq(&query.live, &live));
    }
    if timed_out.load(Ordering::SeqCst) {
        outcome.status = Status::TimedOut;
        outcome.failure = Some(Failure::new(format!(
            "Prazo de {} s esgotado. Os resultados recebidos até aqui foram mantidos.",
            settings.deadline.as_secs()
        )));
    } else if cancelled.load(Ordering::SeqCst) && outcome.status == Status::Success {
        outcome.status = Status::Cancelled;
        outcome.failure = Some(Failure::new(
            "Cancelado. Os resultados recebidos até aqui foram mantidos.",
        ));
    }
    let _ = conn.query_drop("ROLLBACK");
    finish(outcome)
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
                Err(error) => record_error(outcome, &error),
            }
        }
        if outcome.failure.is_none() || !captured.columns.is_empty() {
            outcome.sets.push(captured);
        }
    }
}

fn record_error(outcome: &mut Outcome, error: &mysql::Error) {
    outcome.status = if outcome.sets.is_empty() {
        Status::Failed
    } else {
        Status::Partial
    };
    outcome.failure = Some(describe(error));
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

fn kill(query: &ActiveQuery) -> Result<(), String> {
    if !query.live.load(Ordering::SeqCst) {
        return Ok(());
    }
    let mut conn = Conn::new(options(
        &query.host,
        &query.credential,
        Duration::from_secs(5),
    ))
    .map_err(|error| describe(&error).message)?;
    if query.live.load(Ordering::SeqCst) {
        conn.query_drop(format!("KILL QUERY {}", query.connection_id))
            .map_err(|error| describe(&error).message)?;
    }
    Ok(())
}

pub(crate) fn cancel_active(active: &ActiveQueries) -> Vec<String> {
    let queries = match active.lock() {
        Ok(queries) => queries.clone(),
        Err(_) => return vec!["Não foi possível acessar as consultas em andamento.".into()],
    };
    queries
        .iter()
        .filter_map(|query| {
            kill(query)
                .err()
                .map(|error| format!("{}: {error}", query.host))
        })
        .collect()
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
    fn names_are_joined_in_portuguese() {
        let names =
            |list: &[&str]| join_names(&list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert_eq!(names(&["Econect"]), "Econect");
        assert_eq!(names(&["Econect", "Consinco"]), "Econect e Consinco");
        assert_eq!(names(&["A", "B", "C"]), "A, B e C");
    }

    #[test]
    fn closed_port_is_offline_without_retrying_the_same_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let credential = |name: &str| Credential {
            profile_id: 1,
            profile_name: name.into(),
            engine: crate::model::Engine::Mysql,
            port,
            database: String::new(),
            username: "u".into(),
            password: String::new(),
            tls: false,
        };
        let target = Target {
            device_id: 1,
            host: "127.0.0.1".into(),
            credentials: vec![credential("A"), credential("B")],
        };
        let Connected::Failed(status, failure) = connect(&target, Duration::from_secs(1)) else {
            panic!("a porta fechada não deveria aceitar conexão");
        };
        assert_eq!(status, Status::Offline);
        assert!(
            failure.message.contains(&port.to_string()),
            "{}",
            failure.message
        );
    }
}
