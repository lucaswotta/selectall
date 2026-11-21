//! Driver PostgreSQL: leitura em `START TRANSACTION READ ONLY` com os valores em
//! texto (`simple_query`), correção em transação, cancelamento pelo pedido de
//! cancelamento do protocolo.

use crate::exec::{Column, Credential, Failure, Options, Outcome, ResultSet};
use crate::model::Engine;
use crate::session::{self, ConnectError, Killer, Session};
use postgres::config::SslMode;
use postgres::types::Type;
use postgres::{CancelToken, Client, Config, NoTls, SimpleQueryMessage};
use std::collections::HashSet;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio_postgres_rustls::MakeRustlsConnect;

/// Linhas trazidas do cursor por vez: limita a memória mesmo sem `LIMIT` no SQL.
const FETCH_BATCH: usize = 500;

fn config(host: &str, credential: &Credential, timeout: Duration) -> Config {
    let mut config = Config::new();
    config
        .host(host)
        .port(credential.port)
        .user(&credential.username)
        // Senha vazia vai ao servidor, que recusa como senha errada; sem ela o driver
        // falharia antes, sem passar ao próximo perfil.
        .password(&credential.password)
        .connect_timeout(timeout)
        .keepalives(true)
        .keepalives_idle(Duration::from_secs(30))
        .application_name("SelectAll")
        .ssl_mode(if credential.tls {
            SslMode::Require
        } else {
            SslMode::Disable
        });
    if !credential.database.is_empty() {
        config.dbname(&credential.database);
    }
    config
}

/// Certificado conferido pelas raízes públicas, como no MySQL com TLS.
fn tls() -> Result<MakeRustlsConnect, String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| error.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(MakeRustlsConnect::new(config))
}

fn io_kind(error: &postgres::Error) -> Option<std::io::ErrorKind> {
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        if let Some(io) = inner.downcast_ref::<std::io::Error>() {
            return Some(io.kind());
        }
        source = inner.source();
    }
    None
}

fn refused(error: &postgres::Error) -> bool {
    error
        .as_db_error()
        .is_some_and(|db| matches!(db.code().code(), "28P01" | "28000"))
}

/// Traduz os erros mais comuns; o original segue em `detail`.
pub(crate) fn describe(error: &postgres::Error) -> Failure {
    let message = match error.as_db_error() {
        Some(db) => match db.code().code() {
            "28P01" | "28000" => "Usuário ou senha recusados.".to_owned(),
            "3D000" => "O banco de dados informado no perfil não existe.".to_owned(),
            "42P01" => session::with_name("Tabela não encontrada", db.message(), '"'),
            "42703" => session::with_name("Coluna não encontrada", db.message(), '"'),
            "42601" => "Erro de sintaxe no SQL.".to_owned(),
            "42501" => session::DENIED.to_owned(),
            "25006" => session::READ_ONLY.to_owned(),
            "57014" => session::INTERRUPTED.to_owned(),
            "55P03" => "Linha travada por outra transação por mais de 10 s.".to_owned(),
            "40P01" => "Conflito de travas com outra transação (deadlock).".to_owned(),
            _ => db.message().to_owned(),
        },
        None if io_kind(error).is_some() || error.is_closed() => session::NETWORK.to_owned(),
        None => "Erro de comunicação com o servidor.".to_owned(),
    };
    Failure {
        message,
        detail: Some(error.to_string()),
    }
}

pub(crate) fn open(
    host: &str,
    credential: &Credential,
    timeout: Duration,
) -> Result<Box<dyn Session>, ConnectError> {
    let config = config(host, credential, timeout);
    let connected = if credential.tls {
        let connector = tls().map_err(|detail| {
            ConnectError::Other(Failure {
                message: "Falha ao preparar o TLS.".into(),
                detail: Some(detail),
            })
        })?;
        config.connect(connector)
    } else {
        config.connect(NoTls)
    };
    match connected {
        Ok(client) => Ok(Box::new(PostgresSession {
            cancel: client.cancel_token(),
            client,
            tls: credential.tls,
        })),
        Err(error) if refused(&error) => Err(ConnectError::Refused),
        Err(error) if error.as_db_error().is_none() && io_kind(&error).is_some() => {
            Err(ConnectError::Network(io_kind(&error), describe(&error)))
        }
        Err(error) => Err(ConnectError::Other(describe(&error))),
    }
}

struct PostgresSession {
    client: Client,
    cancel: CancelToken,
    tls: bool,
}

fn is_numeric(kind: &Type) -> bool {
    [
        Type::INT2,
        Type::INT4,
        Type::INT8,
        Type::OID,
        Type::FLOAT4,
        Type::FLOAT8,
        Type::NUMERIC,
    ]
    .contains(kind)
}

/// Converte o texto do servidor: booleano sai como `true`/`false` em vez de `t`/`f`.
fn cell(value: Option<&str>, boolean: bool) -> Option<String> {
    value.map(|text| match (boolean, text) {
        (true, "t") => "true".to_owned(),
        (true, "f") => "false".to_owned(),
        _ => text.to_owned(),
    })
}

/// Guarda as linhas até o limite de linhas e de bytes. Devolve falso quando não cabe mais nada.
fn keep(
    captured: &mut ResultSet,
    booleans: &[bool],
    rows: impl IntoIterator<Item = postgres::SimpleQueryRow>,
    settings: &Options,
    bytes: &mut usize,
) -> bool {
    for row in rows {
        if captured.rows.len() >= settings.row_limit {
            captured.truncated = true;
            return false;
        }
        let cells: Vec<Option<String>> = (0..row.len())
            .map(|index| {
                cell(
                    row.get(index),
                    booleans.get(index).copied().unwrap_or(false),
                )
            })
            .collect();
        let row_bytes = cells
            .iter()
            .map(|cell| cell.as_ref().map_or(0, String::len) + 24)
            .sum::<usize>();
        if bytes.saturating_add(row_bytes) > settings.byte_limit {
            captured.truncated = true;
            return false;
        }
        *bytes += row_bytes;
        captured.rows.push(cells);
    }
    true
}

fn rows(messages: Vec<SimpleQueryMessage>) -> Vec<postgres::SimpleQueryRow> {
    messages
        .into_iter()
        .filter_map(|message| match message {
            SimpleQueryMessage::Row(row) => Some(row),
            _ => None,
        })
        .collect()
}

impl PostgresSession {
    /// `SELECT` passa por um cursor e chega em lotes; `SHOW` e `EXPLAIN` não aceitam
    /// cursor e voltam de uma vez, sempre curtos.
    fn fetch(
        &mut self,
        statement: &str,
        captured: &mut ResultSet,
        booleans: &[bool],
        settings: &Options,
        bytes: &mut usize,
    ) -> Result<(), postgres::Error> {
        if !matches!(
            crate::sql::leading_word(statement, Engine::Postgres).as_deref(),
            Some("SELECT" | "VALUES" | "TABLE")
        ) {
            let messages = self.client.simple_query(statement)?;
            keep(captured, booleans, rows(messages), settings, bytes);
            return Ok(());
        }
        self.client.batch_execute(&format!(
            "DECLARE selectall_leitura NO SCROLL CURSOR FOR {statement}"
        ))?;
        loop {
            let messages = self
                .client
                .simple_query(&format!("FETCH {FETCH_BATCH} FROM selectall_leitura"))?;
            let batch = rows(messages);
            let complete = batch.len() < FETCH_BATCH;
            if !keep(captured, booleans, batch, settings, bytes) || complete {
                break;
            }
        }
        self.client.batch_execute("CLOSE selectall_leitura")
    }
}

impl Session for PostgresSession {
    fn killer(&self) -> Killer {
        let (token, secure) = (self.cancel.clone(), self.tls);
        Arc::new(move |live: &AtomicBool| {
            if !live.load(Ordering::SeqCst) {
                return Ok(());
            }
            let sent = if secure {
                token.cancel_query(tls()?)
            } else {
                token.cancel_query(NoTls)
            };
            sent.map_err(|error| describe(&error).message)
        })
    }

    fn begin_read(&mut self, _settings: &Options) -> Result<(), Failure> {
        // `standard_conforming_strings` garante a barra invertida literal que a análise supõe.
        self.client
            .batch_execute(
                "SET standard_conforming_strings = on; SET DateStyle = ISO; \
                 START TRANSACTION READ ONLY",
            )
            .map_err(|error| describe(&error))
    }

    fn read(
        &mut self,
        statement: &str,
        outcome: &mut Outcome,
        settings: &Options,
        bytes: &mut usize,
    ) {
        // Preparar dá os tipos das colunas e recusa mais de uma instrução no mesmo texto.
        let prepared = match self.client.prepare(statement) {
            Ok(prepared) => prepared,
            Err(error) => return session::record_failure(outcome, describe(&error)),
        };
        let columns = prepared.columns();
        let booleans: Vec<bool> = columns.iter().map(|c| *c.type_() == Type::BOOL).collect();
        let mut captured = ResultSet {
            columns: columns
                .iter()
                .map(|column| Column {
                    name: column.name().to_owned(),
                    numeric: is_numeric(column.type_()),
                })
                .collect(),
            rows: Vec::new(),
            affected_rows: 0,
            warnings: 0,
            truncated: false,
        };
        *bytes += captured
            .columns
            .iter()
            .map(|c| c.name.len() + 32)
            .sum::<usize>();
        let fetched = if captured.columns.is_empty() {
            self.client.batch_execute(statement)
        } else {
            self.fetch(statement, &mut captured, &booleans, settings, bytes)
        };
        match fetched {
            Ok(()) => outcome.sets.push(captured),
            Err(error) => {
                if !captured.rows.is_empty() {
                    outcome.sets.push(captured);
                }
                session::record_failure(outcome, describe(&error));
            }
        }
    }

    fn begin_write(&mut self, _words: &HashSet<String>) -> Result<(), Failure> {
        self.client
            .batch_execute(
                "SET standard_conforming_strings = on; BEGIN; SET LOCAL lock_timeout = '10s'",
            )
            .map_err(|error| describe(&error))
    }

    fn write(&mut self, statement: &str) -> Result<u64, Failure> {
        // `execute` usa o protocolo estendido, que recusa mais de uma instrução no texto.
        self.client
            .execute(statement, &[])
            .map_err(|error| describe(&error))
    }

    fn finish(&mut self, commit: bool) -> Result<(), String> {
        self.client
            .batch_execute(if commit { "COMMIT" } else { "ROLLBACK" })
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn booleans_are_spelled_out() {
        assert_eq!(cell(Some("t"), true).as_deref(), Some("true"));
        assert_eq!(cell(Some("f"), true).as_deref(), Some("false"));
        assert_eq!(cell(Some("t"), false).as_deref(), Some("t"));
        assert_eq!(cell(None, true), None);
    }

    #[test]
    fn tls_config_builds_with_the_bundled_provider() {
        assert!(tls().is_ok());
    }
}
