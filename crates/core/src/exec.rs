//! Execução de um SQL em vários equipamentos, com concorrência limitada,
//! cancelamento e um evento por mudança de estado.

use crate::model::Engine;
use crate::{mysql, sql};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

pub const MAX_CONCURRENCY: usize = 32;

#[derive(Clone)]
pub struct Credential {
    pub profile_id: i64,
    pub profile_name: String,
    pub engine: Engine,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
    pub tls: bool,
}

/// Um equipamento pronto para executar: endereço e perfis em ordem de tentativa.
#[derive(Clone)]
pub struct Target {
    pub device_id: i64,
    pub host: String,
    pub credentials: Vec<Credential>,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub row_limit: usize,
    pub byte_limit: usize,
    pub connect_timeout: Duration,
    pub deadline: Duration,
    pub concurrency: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            row_limit: 1000,
            byte_limit: 4 * 1024 * 1024,
            connect_timeout: Duration::from_secs(3),
            deadline: Duration::from_secs(30),
            concurrency: 16,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Column {
    pub name: String,
    pub numeric: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResultSet {
    pub columns: Vec<Column>,
    /// `None` representa NULL.
    pub rows: Vec<Vec<Option<String>>>,
    pub affected_rows: u64,
    pub warnings: u16,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Success,
    /// Parte dos resultados chegou antes de um erro.
    Partial,
    Failed,
    /// Não respondeu na rede: comum em PDV desligado.
    Offline,
    Cancelled,
    TimedOut,
}

/// Erro para a pessoa (`message`) e o texto original do driver (`detail`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Failure {
    pub message: String,
    pub detail: Option<String>,
}

impl Failure {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub device_id: i64,
    pub profile: Option<String>,
    pub sets: Vec<ResultSet>,
    pub status: Status,
    pub failure: Option<Failure>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Connecting { device_id: i64 },
    Running { device_id: i64, profile: String },
    Finished(Outcome),
    Skipped { device_id: i64 },
    CancelErrors { errors: Vec<String> },
}

pub(crate) type ActiveQueries = Arc<Mutex<Vec<mysql::ActiveQuery>>>;

/// `events` termina quando o último equipamento é concluído ou pulado.
pub struct Run {
    pub events: mpsc::Receiver<Event>,
    cancelled: Arc<AtomicBool>,
    active: ActiveQueries,
    /// Usado só para avisar falhas do cancelamento. O último worker esvazia o
    /// slot; sem isso o canal nunca fecharia e `events` não terminaria.
    sender: Arc<Mutex<Option<mpsc::SyncSender<Event>>>>,
}

impl Run {
    /// Recusa SQL que não seja de leitura: correções passam por outro fluxo.
    pub fn start(targets: Vec<Target>, sql_text: &str, options: &Options) -> Result<Self, String> {
        let plan = sql::analyze(sql_text)?;
        if !plan.read_only_candidate {
            return Err("Este SQL pode alterar dados. Use o modo Corrigir.".into());
        }
        let workers = options
            .concurrency
            .clamp(1, MAX_CONCURRENCY)
            .min(targets.len());
        let jobs = Arc::new(Mutex::new(VecDeque::from(targets)));
        let active: ActiveQueries = Arc::new(Mutex::new(Vec::new()));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, events) = mpsc::sync_channel(64);
        let statements = Arc::new(plan.statements);
        let running = Arc::new(AtomicUsize::new(workers));
        let slot = Arc::new(Mutex::new((workers > 0).then(|| sender.clone())));
        for _ in 0..workers {
            let (jobs, active, cancelled, sender, statements, options, running, slot) = (
                jobs.clone(),
                active.clone(),
                cancelled.clone(),
                sender.clone(),
                statements.clone(),
                options.clone(),
                running.clone(),
                slot.clone(),
            );
            thread::spawn(move || {
                while let Some(target) = jobs.lock().ok().and_then(|mut queue| queue.pop_front()) {
                    let device_id = target.device_id;
                    let event = if cancelled.load(Ordering::SeqCst) {
                        Event::Skipped { device_id }
                    } else {
                        if sender.send(Event::Connecting { device_id }).is_err() {
                            break;
                        }
                        Event::Finished(execute(
                            &target,
                            &statements,
                            &options,
                            &cancelled,
                            &active,
                            |profile| {
                                let _ = sender.send(Event::Running {
                                    device_id,
                                    profile: profile.to_owned(),
                                });
                            },
                        ))
                    };
                    if sender.send(event).is_err() {
                        break;
                    }
                }
                if running.fetch_sub(1, Ordering::SeqCst) == 1
                    && let Ok(mut slot) = slot.lock()
                {
                    slot.take();
                }
            });
        }
        Ok(Self {
            events,
            cancelled,
            active,
            sender: slot,
        })
    }

    /// Para de iniciar equipamentos e pede `KILL QUERY` aos que estão executando.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let active = self.active.clone();
        let sender = self.sender.lock().ok().and_then(|slot| slot.clone());
        thread::spawn(move || {
            let errors = mysql::cancel_active(&active);
            if let Some(sender) = sender.filter(|_| !errors.is_empty()) {
                let _ = sender.send(Event::CancelErrors { errors });
            }
        });
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

fn execute(
    target: &Target,
    statements: &[String],
    options: &Options,
    cancelled: &AtomicBool,
    active: &ActiveQueries,
    on_running: impl FnOnce(&str),
) -> Outcome {
    let unsupported = target
        .credentials
        .iter()
        .find(|credential| credential.engine != Engine::Mysql);
    if let Some(credential) = unsupported {
        return Outcome {
            device_id: target.device_id,
            profile: Some(credential.profile_name.clone()),
            sets: Vec::new(),
            status: Status::Failed,
            failure: Some(Failure::new(format!(
                "O perfil {} usa {}, que ainda não é suportado.",
                credential.profile_name,
                credential.engine.as_str()
            ))),
            elapsed_ms: 0,
        };
    }
    mysql::execute_read(target, statements, options, cancelled, active, on_running)
}
