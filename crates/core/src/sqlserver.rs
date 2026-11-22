//! Driver SQL Server. O servidor não tem transação somente leitura: a leitura confia
//! na análise do SQL e roda numa transação sempre desfeita. Cancelamento por `KILL`
//! da sessão em outra conexão.

use crate::exec::{Column, Credential, Failure, Options, Outcome, ResultSet};
use crate::session::{self, ConnectError, Killer, Session};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};
use futures_util::TryStreamExt;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tiberius::{AuthMethod, ColumnData, ColumnType, Config, EncryptionLevel, FromSql, QueryItem};
use tokio::runtime::Runtime;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

type Client = tiberius::Client<Compat<tokio::net::TcpStream>>;

const LOGIN_FAILED: u32 = 18456;
const LOCK_TIMEOUT_MS: u32 = 10_000;

fn config(host: &str, credential: &Credential) -> Config {
    let mut config = Config::new();
    config.host(host);
    config.port(credential.port);
    config.authentication(AuthMethod::sql_server(
        &credential.username,
        &credential.password,
    ));
    if !credential.database.is_empty() {
        config.database(&credential.database);
    }
    config.application_name("SelectAll");
    if credential.tls {
        config.encryption(EncryptionLevel::Required);
    } else {
        // Sem TLS no perfil, só o login vai cifrado. O certificado do SQL Server de loja
        // costuma ser o autoassinado da instalação, então não é conferido.
        config.encryption(EncryptionLevel::Off);
        config.trust_cert();
    }
    config
}

/// Traduz os erros mais comuns; o original segue em `detail`.
pub(crate) fn describe(error: &tiberius::error::Error) -> Failure {
    use tiberius::error::Error;
    let message = match error {
        Error::Server(token) => match token.code() {
            LOGIN_FAILED => "Usuário ou senha recusados.".to_owned(),
            4060 => {
                "O banco de dados informado no perfil não existe ou o usuário não tem acesso a ele."
                    .to_owned()
            }
            208 => session::with_name("Tabela não encontrada", token.message(), '\''),
            207 => session::with_name("Coluna não encontrada", token.message(), '\''),
            102 | 156 | 170 => "Erro de sintaxe no SQL.".to_owned(),
            229 | 230 | 262 | 297 | 300 => session::DENIED.to_owned(),
            1222 => "Linha travada por outra transação por mais de 10 s.".to_owned(),
            1205 => "Conflito de travas com outra transação (deadlock).".to_owned(),
            3980 | 596 => session::INTERRUPTED.to_owned(),
            _ => token.message().to_owned(),
        },
        Error::Io { .. } => session::NETWORK.to_owned(),
        Error::Tls(_) => "Falha ao negociar TLS com o servidor. SQL Server 2008 e 2012 sem atualização só oferecem TLS 1.0, que o SelectAll não aceita; atualize o SQL Server ou habilite TLS 1.2 nele.".to_owned(),
        _ => "Erro de comunicação com o servidor.".to_owned(),
    };
    Failure {
        message,
        detail: Some(error.to_string()),
    }
}

fn network(kind: ErrorKind, detail: String) -> ConnectError {
    ConnectError::Network(
        Some(kind),
        Failure {
            message: session::NETWORK.to_owned(),
            detail: Some(detail),
        },
    )
}

/// Tenta cada endereço do nome: `localhost` pode resolver para `::1` antes de `127.0.0.1`.
fn tcp(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, ConnectError> {
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|error| network(ErrorKind::NotFound, error.to_string()))?;
    let mut last = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => last = Some(error),
        }
    }
    let error = last.unwrap_or_else(|| ErrorKind::NotFound.into());
    Err(network(error.kind(), error.to_string()))
}

/// Conecta e autentica. Devolve também uma cópia do soquete, para encerrar a conexão
/// de fora quando o `KILL` não é permitido.
fn login(
    runtime: &Runtime,
    host: &str,
    credential: &Credential,
    timeout: Duration,
) -> Result<(Client, TcpStream), ConnectError> {
    let stream = tcp(host, credential.port, timeout)?;
    let copy = stream.try_clone();
    let prepared = stream
        .set_nodelay(true)
        .and_then(|()| stream.set_nonblocking(true))
        .and(copy);
    let copy = prepared.map_err(|error| network(error.kind(), error.to_string()))?;
    let config = config(host, credential);
    let login = runtime.block_on(async {
        let stream = tokio::net::TcpStream::from_std(stream)?;
        let login = tiberius::Client::connect(config, stream.compat_write());
        Ok::<_, std::io::Error>(
            Box::pin(tokio::time::timeout(
                timeout.max(Duration::from_secs(10)),
                login,
            ))
            .await,
        )
    });
    match login {
        Err(error) => Err(network(error.kind(), error.to_string())),
        Ok(Err(_)) => Err(network(ErrorKind::TimedOut, "login sem resposta".into())),
        Ok(Ok(Ok(client))) => Ok((client, copy)),
        Ok(Ok(Err(tiberius::error::Error::Server(token)))) if token.code() == LOGIN_FAILED => {
            Err(ConnectError::Refused)
        }
        Ok(Ok(Err(error))) => Err(ConnectError::Other(describe(&error))),
    }
}

fn runtime() -> Result<Runtime, Failure> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| Failure {
            message: "Não foi possível iniciar o driver do SQL Server.".into(),
            detail: Some(error.to_string()),
        })
}

pub(crate) fn open(
    host: &str,
    credential: &Credential,
    timeout: Duration,
) -> Result<Box<dyn Session>, ConnectError> {
    let runtime = runtime().map_err(ConnectError::Other)?;
    let (mut client, socket) = login(&runtime, host, credential, timeout)?;
    let spid = runtime
        .block_on(async {
            let row = client
                .simple_query("SELECT @@SPID")
                .await?
                .into_row()
                .await?;
            Ok::<_, tiberius::error::Error>(row.and_then(|row| row.get::<i16, _>(0)))
        })
        .map_err(|error| ConnectError::Other(describe(&error)))?
        .ok_or_else(|| {
            ConnectError::Other(Failure::new("O servidor não informou a sessão (@@SPID)."))
        })?;
    Ok(Box::new(SqlserverSession {
        runtime,
        client,
        socket: Arc::new(socket),
        host: host.to_owned(),
        credential: credential.clone(),
        spid,
    }))
}

struct SqlserverSession {
    runtime: Runtime,
    client: Client,
    socket: Arc<TcpStream>,
    host: String,
    credential: Credential,
    spid: i16,
}

impl SqlserverSession {
    /// Executa um lote de controle (`SET`, `BEGIN TRAN`) e descarta o que voltar.
    fn batch(&mut self, sql: &str) -> Result<(), tiberius::error::Error> {
        let client = &mut self.client;
        self.runtime.block_on(async {
            client.simple_query(sql).await?.into_results().await?;
            Ok(())
        })
    }
}

/// `KILL` exige `ALTER ANY CONNECTION`. Sem essa permissão, encerrar o soquete faz a
/// mesma coisa para quem espera: a instrução para de responder aqui e o servidor desfaz
/// a transação ao perceber a conexão perdida.
fn kill(
    host: &str,
    credential: &Credential,
    spid: i16,
    socket: &TcpStream,
    live: &AtomicBool,
) -> Result<(), String> {
    let killed = runtime()
        .map_err(|failure| failure.message)
        .and_then(|runtime| {
            let (mut client, _) = login(&runtime, host, credential, Duration::from_secs(5))
                .map_err(|error| match error {
                    ConnectError::Network(_, failure) | ConnectError::Other(failure) => {
                        failure.message
                    }
                    ConnectError::Refused => "Usuário ou senha recusados.".to_owned(),
                })?;
            if !live.load(Ordering::SeqCst) {
                return Ok(());
            }
            runtime
                .block_on(async {
                    client
                        .simple_query(format!("KILL {spid}"))
                        .await?
                        .into_results()
                        .await
                })
                .map(drop)
                .map_err(|error| describe(&error).message)
        });
    match killed {
        Ok(()) => Ok(()),
        Err(error) => socket.shutdown(Shutdown::Both).map_err(|shutdown| {
            format!("{error} Não foi possível encerrar a conexão: {shutdown}.")
        }),
    }
}

fn is_numeric(kind: ColumnType) -> bool {
    use ColumnType::{
        Decimaln, Float4, Float8, Floatn, Int1, Int2, Int4, Int8, Intn, Money, Money4, Numericn,
    };
    matches!(
        kind,
        Int1 | Int2
            | Int4
            | Int8
            | Intn
            | Float4
            | Float8
            | Floatn
            | Decimaln
            | Numericn
            | Money
            | Money4
    )
}

fn chrono_text<'a, T: FromSql<'a>>(
    data: &'a ColumnData<'static>,
    format: impl Fn(T) -> String,
) -> Option<String> {
    T::from_sql(data).ok().flatten().map(format)
}

/// Datas no mesmo formato do MySQL: `DATE` sem hora, frações só quando existem.
fn format_value(data: &ColumnData<'static>) -> Option<String> {
    match data {
        ColumnData::U8(value) => value.map(|v| v.to_string()),
        ColumnData::I16(value) => value.map(|v| v.to_string()),
        ColumnData::I32(value) => value.map(|v| v.to_string()),
        ColumnData::I64(value) => value.map(|v| v.to_string()),
        ColumnData::F32(value) => value.map(|v| v.to_string()),
        ColumnData::F64(value) => value.map(|v| v.to_string()),
        ColumnData::Bit(value) => value.map(|v| if v { "1" } else { "0" }.to_owned()),
        ColumnData::String(value) => value.as_ref().map(ToString::to_string),
        ColumnData::Guid(value) => value.map(|v| v.to_string().to_uppercase()),
        ColumnData::Binary(value) => value.as_ref().map(|bytes| {
            bytes.iter().fold(String::from("0x"), |mut hex, byte| {
                use std::fmt::Write;
                let _ = write!(hex, "{byte:02X}");
                hex
            })
        }),
        ColumnData::Numeric(value) => value.map(|v| v.to_string()),
        ColumnData::Xml(value) => value.as_ref().map(ToString::to_string),
        ColumnData::Date(_) => {
            chrono_text(data, |date: NaiveDate| date.format("%Y-%m-%d").to_string())
        }
        ColumnData::Time(_) => chrono_text(data, |time: NaiveTime| {
            time.format("%H:%M:%S%.f").to_string()
        }),
        // `datetime` guarda 1/300 s: em milissegundos, como o próprio SQL Server mostra.
        ColumnData::DateTime(_) | ColumnData::SmallDateTime(_) => {
            chrono_text(data, |moment: NaiveDateTime| {
                moment
                    .format("%Y-%m-%d %H:%M:%S%.3f")
                    .to_string()
                    .trim_end_matches(".000")
                    .to_owned()
            })
        }
        ColumnData::DateTime2(_) => chrono_text(data, |moment: NaiveDateTime| {
            moment.format("%Y-%m-%d %H:%M:%S%.f").to_string()
        }),
        ColumnData::DateTimeOffset(_) => chrono_text(data, |moment: DateTime<FixedOffset>| {
            moment.format("%Y-%m-%d %H:%M:%S%.f %:z").to_string()
        }),
    }
}

impl Session for SqlserverSession {
    fn killer(&self) -> Killer {
        let (host, credential, spid) = (self.host.clone(), self.credential.clone(), self.spid);
        let socket = self.socket.clone();
        Arc::new(move |live: &AtomicBool| {
            if !live.load(Ordering::SeqCst) {
                return Ok(());
            }
            kill(&host, &credential, spid, &socket, live)
        })
    }

    fn begin_read(&mut self, settings: &Options) -> Result<(), Failure> {
        let rows = settings.row_limit.saturating_add(1);
        self.batch(&format!(
            "SET ROWCOUNT {rows}; SET XACT_ABORT ON; BEGIN TRANSACTION"
        ))
        .map_err(|error| describe(&error))
    }

    fn read(
        &mut self,
        statement: &str,
        outcome: &mut Outcome,
        settings: &Options,
        bytes: &mut usize,
    ) {
        let client = &mut self.client;
        let result = self.runtime.block_on(async {
            let mut stream = client.simple_query(statement).await?;
            while let Some(item) = stream.try_next().await? {
                match item {
                    QueryItem::Metadata(metadata) => {
                        let columns: Vec<Column> = metadata
                            .columns()
                            .iter()
                            .map(|column| Column {
                                name: column.name().to_owned(),
                                numeric: is_numeric(column.column_type()),
                            })
                            .collect();
                        *bytes += columns.iter().map(|c| c.name.len() + 32).sum::<usize>();
                        outcome.sets.push(ResultSet {
                            columns,
                            rows: Vec::new(),
                            affected_rows: 0,
                            warnings: 0,
                            truncated: false,
                        });
                    }
                    QueryItem::Row(row) => {
                        let Some(set) = outcome.sets.last_mut() else {
                            continue;
                        };
                        if set.rows.len() >= settings.row_limit {
                            set.truncated = true;
                            continue;
                        }
                        let cells: Vec<Option<String>> =
                            row.cells().map(|(_, data)| format_value(data)).collect();
                        let row_bytes = cells
                            .iter()
                            .map(|cell| cell.as_ref().map_or(0, String::len) + 24)
                            .sum::<usize>();
                        if bytes.saturating_add(row_bytes) > settings.byte_limit {
                            set.truncated = true;
                            continue;
                        }
                        *bytes += row_bytes;
                        set.rows.push(cells);
                    }
                }
            }
            Ok::<_, tiberius::error::Error>(())
        });
        if let Err(error) = result {
            session::record_failure(outcome, describe(&error));
        }
    }

    fn begin_write(&mut self, _words: &HashSet<String>) -> Result<(), Failure> {
        // `XACT_ABORT` desfaz a transação inteira em qualquer erro, inclusive trava esgotada.
        self.batch(&format!(
            "SET XACT_ABORT ON; SET LOCK_TIMEOUT {LOCK_TIMEOUT_MS}; BEGIN TRANSACTION"
        ))
        .map_err(|error| describe(&error))
    }

    fn write(&mut self, statement: &str) -> Result<u64, Failure> {
        let client = &mut self.client;
        self.runtime
            .block_on(async { client.execute(statement, &[]).await })
            .map(|result| result.rows_affected().iter().sum())
            .map_err(|error| describe(&error))
    }

    fn finish(&mut self, commit: bool) -> Result<(), String> {
        self.batch(if commit {
            "COMMIT TRANSACTION"
        } else {
            "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION"
        })
        .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_follow_the_mysql_format() {
        assert_eq!(
            format_value(&ColumnData::I32(Some(7))).as_deref(),
            Some("7")
        );
        assert_eq!(format_value(&ColumnData::I32(None)), None);
        // 2026-11-21 08:14:03.8533 em `datetime`: dias desde 1900 e 1/300 s desde meia-noite.
        let legacy = ColumnData::DateTime(Some(tiberius::time::DateTime::new(46_345, 8_893_156)));
        assert_eq!(
            format_value(&legacy).as_deref(),
            Some("2026-11-21 08:14:03.853")
        );
        let whole = ColumnData::DateTime(Some(tiberius::time::DateTime::new(46_345, 0)));
        assert_eq!(format_value(&whole).as_deref(), Some("2026-11-21 00:00:00"));
        assert_eq!(
            format_value(&ColumnData::Bit(Some(true))).as_deref(),
            Some("1")
        );
        assert_eq!(
            format_value(&ColumnData::Binary(Some(vec![0xff, 0x00].into()))).as_deref(),
            Some("0xFF00")
        );
    }
}
