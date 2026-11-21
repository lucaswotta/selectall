//! Fluxo comum aos motores: alternativa entre perfis, prazo, cancelamento e a
//! transação de cada equipamento. Cada driver só implementa `Session`.

use crate::exec::{Credential, Failure, Options, Outcome, Plans, Status, Target};
use crate::model::Engine;
use crate::{mysql, postgres, sqlserver};
use std::collections::HashSet;
use std::io::ErrorKind;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

/// Interrompe a instrução em andamento a partir de outra thread. Recebe o sinal
/// `live`, que fica falso quando a instrução já terminou e o pedido deve ser ignorado.
pub(crate) type Killer = Arc<dyn Fn(&AtomicBool) -> Result<(), String> + Send + Sync>;

pub(crate) trait Session {
    fn killer(&self) -> Killer;
    /// Abre a transação somente leitura (ou sempre desfeita) e limita as linhas.
    fn begin_read(&mut self, settings: &Options) -> Result<(), Failure>;
    /// Executa uma instrução e acrescenta os conjuntos de resultado a `outcome`.
    fn read(
        &mut self,
        statement: &str,
        outcome: &mut Outcome,
        settings: &Options,
        bytes: &mut usize,
    );
    /// Recusa o que não pode ser desfeito e abre a transação da correção.
    fn begin_write(&mut self, words: &HashSet<String>) -> Result<(), Failure>;
    /// Linhas alteradas pela instrução.
    fn write(&mut self, statement: &str) -> Result<u64, Failure>;
    fn finish(&mut self, commit: bool) -> Result<(), String>;
}

pub(crate) enum ConnectError {
    /// Sem resposta na porta: o próximo perfil na mesma porta não é tentado.
    Network(Option<ErrorKind>, Failure),
    /// Usuário ou senha recusados: passa ao próximo perfil.
    Refused,
    Other(Failure),
}

fn open(
    host: &str,
    credential: &Credential,
    timeout: Duration,
) -> Result<Box<dyn Session>, ConnectError> {
    match credential.engine {
        Engine::Mysql => mysql::open(host, credential, timeout),
        Engine::Postgres => postgres::open(host, credential, timeout),
        Engine::Sqlserver => sqlserver::open(host, credential, timeout),
    }
}

#[derive(Clone)]
pub(crate) struct ActiveQuery {
    host: String,
    kill: Killer,
    live: Arc<AtomicBool>,
}

pub(crate) type ActiveQueries = Arc<Mutex<Vec<ActiveQuery>>>;

pub(crate) fn offline_message(kind: Option<ErrorKind>, ports: &str, timeout: Duration) -> String {
    use ErrorKind::{ConnectionRefused, HostUnreachable, NetworkUnreachable};
    match kind {
        Some(ConnectionRefused) => {
            format!("O equipamento está na rede, mas a porta {ports} recusou a conexão.")
        }
        Some(HostUnreachable | NetworkUnreachable) => {
            format!("Sem rota de rede até o equipamento (porta {ports}).")
        }
        _ => format!("Não respondeu na porta {ports} em {} s.", timeout.as_secs()),
    }
}

/// Primeiro trecho entre `delimiter` na mensagem do servidor, como a tabela em
/// "Table 'pdv.parametros' doesn't exist" ou `relation "parametros" does not exist`.
pub(crate) fn quoted(message: &str, delimiter: char) -> Option<&str> {
    let (_, rest) = message.split_once(delimiter)?;
    let (name, _) = rest.split_once(delimiter)?;
    (!name.is_empty()).then_some(name)
}

pub(crate) fn with_name(text: &str, message: &str, delimiter: char) -> String {
    match quoted(message, delimiter) {
        Some(name) => format!("{text}: {name}."),
        None => format!("{text}."),
    }
}

pub(crate) const DENIED: &str = "O usuário do perfil não tem permissão para esta consulta.";
pub(crate) const READ_ONLY: &str = "Bloqueado: a consulta tentou alterar dados no modo leitura.";
pub(crate) const INTERRUPTED: &str = "Consulta interrompida pelo servidor.";
pub(crate) const NETWORK: &str = "Sem resposta do equipamento na rede.";

/// Primeira falha numa leitura: com resultados já recebidos, o equipamento fica parcial.
pub(crate) fn record_failure(outcome: &mut Outcome, failure: Failure) {
    outcome.status = if outcome.sets.is_empty() {
        Status::Failed
    } else {
        Status::Partial
    };
    outcome.failure = Some(failure);
}

enum Connected {
    Ready(Box<dyn Session>, Credential),
    Failed(Status, Failure),
}

/// Tenta os perfis em ordem. Endereço sem resposta numa porta não é testado de novo
/// na mesma porta; senha recusada passa ao próximo perfil.
fn connect(target: &Target, timeout: Duration) -> Connected {
    let mut unreachable_ports = Vec::new();
    let mut refused = Vec::new();
    let mut last_failure = None;
    let mut last_network = None;
    for credential in &target.credentials {
        if unreachable_ports.contains(&credential.port) {
            continue;
        }
        match open(&target.host, credential, timeout) {
            Ok(session) => return Connected::Ready(session, credential.clone()),
            Err(ConnectError::Network(kind, failure)) => {
                unreachable_ports.push(credential.port);
                last_network = kind;
                last_failure = Some(failure);
            }
            Err(ConnectError::Refused) => refused.push(credential.profile_name.clone()),
            Err(ConnectError::Other(failure)) => last_failure = Some(failure),
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
                    message: offline_message(last_network, &ports, timeout),
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

fn blank(target: &Target, status: Status) -> Outcome {
    Outcome {
        device_id: target.device_id,
        profile: None,
        sets: Vec::new(),
        status,
        failure: None,
        elapsed_ms: 0,
        affected_rows: 0,
    }
}

fn elapsed(start: Instant, mut outcome: Outcome) -> Outcome {
    outcome.elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    outcome
}

pub(crate) fn execute_read(
    target: &Target,
    plans: &Plans,
    settings: &Options,
    cancelled: &AtomicBool,
    active: &ActiveQueries,
    on_running: impl FnOnce(&str),
) -> Outcome {
    let start = Instant::now();
    let mut outcome = blank(target, Status::Success);
    let (mut session, credential) = match connect(target, settings.connect_timeout) {
        Connected::Ready(session, credential) => (session, credential),
        Connected::Failed(status, failure) => {
            outcome.status = status;
            outcome.failure = Some(failure);
            return elapsed(start, outcome);
        }
    };
    outcome.profile = Some(credential.profile_name.clone());
    if let Err(failure) = session.begin_read(settings) {
        outcome.status = Status::Failed;
        outcome.failure = Some(failure);
        return elapsed(start, outcome);
    }

    let watch = Watch::start(&target.host, session.killer(), settings.deadline, active);
    on_running(&credential.profile_name);
    let mut bytes = 0;
    for statement in plans.statements(credential.engine) {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        session.read(statement, &mut outcome, settings, &mut bytes);
        if outcome.failure.is_some() {
            break;
        }
    }

    watch.stop();
    if watch.timed_out() {
        outcome.status = Status::TimedOut;
        outcome.failure = Some(Failure::new(format!(
            "Prazo de {} s esgotado. Os resultados recebidos até aqui foram mantidos.",
            settings.deadline.as_secs()
        )));
    } else if cancelled.load(Ordering::SeqCst) {
        // O MySQL devolve o `SLEEP` interrompido sem erro; o PostgreSQL e o SQL Server
        // devolvem erro da instrução cancelada. Nos dois casos o motivo é o cancelamento.
        outcome.status = Status::Cancelled;
        outcome.failure = Some(Failure::new(
            "Cancelado. Os resultados recebidos até aqui foram mantidos.",
        ));
    }
    let _ = session.finish(false);
    elapsed(start, outcome)
}

/// Registra a conexão para o cancelamento e interrompe a instrução quando o prazo esgota.
struct Watch {
    live: Arc<AtomicBool>,
    timed_out: Arc<AtomicBool>,
    done: mpsc::Sender<()>,
    active: ActiveQueries,
}

impl Watch {
    fn start(host: &str, kill: Killer, deadline: Duration, active: &ActiveQueries) -> Self {
        let live = Arc::new(AtomicBool::new(true));
        let query = ActiveQuery {
            host: host.to_owned(),
            kill,
            live: live.clone(),
        };
        if let Ok(mut queries) = active.lock() {
            queries.push(query.clone());
        }
        let timed_out = Arc::new(AtomicBool::new(false));
        let (done, wait) = mpsc::channel::<()>();
        {
            let timed_out = timed_out.clone();
            std::thread::spawn(move || {
                if matches!(
                    wait.recv_timeout(deadline),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) && query.live.load(Ordering::SeqCst)
                {
                    timed_out.store(true, Ordering::SeqCst);
                    let _ = (query.kill)(&query.live);
                }
            });
        }
        Self {
            live,
            timed_out,
            done,
            active: active.clone(),
        }
    }

    /// Depois disto nem o cancelamento nem o prazo interrompem esta conexão.
    fn stop(&self) {
        self.live.store(false, Ordering::SeqCst);
        let _ = self.done.send(());
        if let Ok(mut queries) = self.active.lock() {
            queries.retain(|query| !Arc::ptr_eq(&query.live, &self.live));
        }
    }

    fn timed_out(&self) -> bool {
        self.timed_out.load(Ordering::SeqCst)
    }
}

/// O `COMMIT` foi enviado: sem a resposta, não dá para saber se gravou.
pub(crate) fn commit_status(result: Result<(), String>) -> (Status, Option<Failure>) {
    match result {
        Ok(()) => (Status::Applied, None),
        Err(detail) => (
            Status::Unknown,
            Some(Failure {
                message: "Resultado desconhecido: a conexão caiu depois do COMMIT ser enviado. Confira no equipamento se a alteração foi gravada.".into(),
                detail: Some(detail),
            }),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_write(
    target: &Target,
    plans: &Plans,
    words: &HashSet<String>,
    commit: bool,
    settings: &Options,
    cancelled: &AtomicBool,
    active: &ActiveQueries,
    on_running: impl FnOnce(&str),
) -> Outcome {
    let start = Instant::now();
    let mut outcome = blank(target, Status::Failed);
    let (mut session, credential) = match connect(target, settings.connect_timeout) {
        Connected::Ready(session, credential) => (session, credential),
        Connected::Failed(status, failure) => {
            outcome.status = status;
            outcome.failure = Some(failure);
            return elapsed(start, outcome);
        }
    };
    outcome.profile = Some(credential.profile_name.clone());
    if let Err(failure) = session.begin_write(words) {
        outcome.failure = Some(failure);
        return elapsed(start, outcome);
    }

    let watch = Watch::start(&target.host, session.killer(), settings.deadline, active);
    on_running(&credential.profile_name);
    let mut affected = 0u64;
    let mut failure = None;
    for statement in plans.statements(credential.engine) {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        match session.write(statement) {
            Ok(rows) => affected = affected.saturating_add(rows),
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    // O COMMIT não pode ser interrompido: o resultado ficaria incerto.
    watch.stop();

    // Se a conexão caiu, o servidor já desfez a transação ao perdê-la.
    let undo = |session: &mut Box<dyn Session>| {
        let _ = session.finish(false);
    };
    if watch.timed_out() {
        undo(&mut session);
        outcome.status = Status::TimedOut;
        outcome.failure = Some(Failure::new(format!(
            "Prazo de {} s esgotado. A transação foi desfeita.",
            settings.deadline.as_secs()
        )));
    } else if cancelled.load(Ordering::SeqCst) {
        undo(&mut session);
        outcome.status = Status::Cancelled;
        outcome.failure = Some(Failure::new("Cancelado. A transação foi desfeita."));
    } else if let Some(mut failure) = failure {
        undo(&mut session);
        failure.message = format!("{} A transação foi desfeita.", failure.message);
        outcome.failure = Some(failure);
    } else if !commit {
        match session.finish(false) {
            Ok(()) => {
                outcome.status = Status::Rehearsed;
                outcome.affected_rows = affected;
            }
            Err(detail) => {
                outcome.failure = Some(Failure {
                    message: "O ROLLBACK do ensaio não foi confirmado. O servidor desfaz a transação ao perder a conexão; refaça o ensaio.".into(),
                    detail: Some(detail),
                });
            }
        }
    } else {
        let (status, failure) = commit_status(session.finish(true));
        outcome.status = status;
        outcome.failure = failure;
        if status == Status::Applied {
            outcome.affected_rows = affected;
        }
    }
    elapsed(start, outcome)
}

pub(crate) fn cancel_active(active: &ActiveQueries) -> Vec<String> {
    let queries = match active.lock() {
        Ok(queries) => queries.clone(),
        Err(_) => return vec!["Não foi possível acessar as consultas em andamento.".into()],
    };
    queries
        .iter()
        .filter_map(|query| {
            if !query.live.load(Ordering::SeqCst) {
                return None;
            }
            (query.kill)(&query.live)
                .err()
                .map(|error| format!("{}: {error}", query.host))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_message_follows_the_network_failure() {
        let timeout = Duration::from_secs(3);
        assert_eq!(
            offline_message(Some(ErrorKind::HostUnreachable), "3306", timeout),
            "Sem rota de rede até o equipamento (porta 3306)."
        );
        assert!(
            offline_message(Some(ErrorKind::ConnectionRefused), "5432", timeout)
                .contains("recusou")
        );
        assert_eq!(
            offline_message(Some(ErrorKind::TimedOut), "1433", timeout),
            "Não respondeu na porta 1433 em 3 s."
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
    fn quoted_names_follow_each_server() {
        assert_eq!(quoted("Table 'pdv.x' doesn't exist", '\''), Some("pdv.x"));
        assert_eq!(quoted(r#"relation "x" does not exist"#, '"'), Some("x"));
        assert_eq!(quoted("sem aspas", '\''), None);
        assert_eq!(quoted("vazio ''", '\''), None);
    }

    #[test]
    fn closed_port_is_offline_without_retrying_the_same_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        for engine in [Engine::Mysql, Engine::Postgres, Engine::Sqlserver] {
            let credential = |name: &str| Credential {
                profile_id: 1,
                profile_name: name.into(),
                engine,
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
            let Connected::Failed(status, failure) = connect(&target, Duration::from_secs(1))
            else {
                panic!("a porta fechada não deveria aceitar conexão");
            };
            assert_eq!(status, Status::Offline, "{engine:?}: {failure:?}");
            assert!(
                failure.message.contains("recusou") && failure.message.contains(&port.to_string()),
                "{engine:?}: {}",
                failure.message
            );
        }
    }

    #[test]
    fn a_lost_commit_is_unknown_not_failed() {
        assert_eq!(commit_status(Ok(())), (Status::Applied, None));
        let (status, failure) = commit_status(Err("connection reset".into()));
        assert_eq!(status, Status::Unknown);
        assert!(
            failure
                .unwrap()
                .message
                .starts_with("Resultado desconhecido")
        );
    }
}
