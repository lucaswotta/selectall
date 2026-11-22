//! Execução de um SQL em vários equipamentos, com concorrência limitada,
//! cancelamento e um evento por mudança de estado.

use crate::model::Engine;
use crate::params::{self, Script};
use crate::session::{self, ActiveQueries};
use crate::sql;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;

pub const MAX_CONCURRENCY: usize = 32;
pub const DEFAULT_PER_STORE: usize = 4;

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
    /// Equipamentos da mesma loja dividem o link; ver `Options::per_store`.
    pub store_id: i64,
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
    /// Equipamentos da mesma loja executando ao mesmo tempo, para não saturar o link da loja.
    pub per_store: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            row_limit: 1000,
            byte_limit: 4 * 1024 * 1024,
            connect_timeout: Duration::from_secs(3),
            deadline: Duration::from_secs(30),
            concurrency: 16,
            per_store: DEFAULT_PER_STORE,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub numeric: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultSet {
    pub columns: Vec<Column>,
    /// `None` representa NULL.
    pub rows: Vec<Vec<Option<String>>>,
    pub affected_rows: u64,
    pub warnings: u16,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Ensaio: as instruções rodaram e a transação foi desfeita.
    Rehearsed,
    /// Correção confirmada pelo `COMMIT`.
    Applied,
    /// A conexão caiu depois do `COMMIT` ser enviado: só o equipamento sabe se gravou.
    Unknown,
}

/// Etapa de uma correção. Só o ensaio desfaz a transação.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Rehearsal,
    Pilot,
    Rollout,
}

impl Step {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rehearsal => "rehearsal",
            Self::Pilot => "pilot",
            Self::Rollout => "rollout",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Rehearsal, Self::Pilot, Self::Rollout]
            .into_iter()
            .find(|step| step.as_str() == text)
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Read,
    Write { commit: bool },
}

/// Erro para a pessoa (`message`) e o texto original do driver (`detail`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub device_id: i64,
    pub profile: Option<String>,
    pub sets: Vec<ResultSet>,
    pub status: Status,
    pub failure: Option<Failure>,
    pub elapsed_ms: u64,
    /// Linhas alteradas, somadas entre as instruções. Zero em leitura.
    pub affected_rows: u64,
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

/// As instruções de cada motor presente na execução: aspas e comentários mudam
/// entre os dialetos, então a divisão também pode mudar.
pub(crate) struct Plans {
    statements: HashMap<Engine, Vec<String>>,
    /// Palavras do SQL, para cruzar com nomes de tabela sem transação no MySQL.
    words: HashSet<String>,
}

impl Plans {
    pub(crate) fn statements(&self, engine: Engine) -> &[String] {
        self.statements.get(&engine).map_or(&[], Vec::as_slice)
    }
}

/// Analisa o SQL no dialeto de cada motor dos equipamentos; sem perfis, no do MySQL.
fn analyze(targets: &[Target], script: &Script) -> Result<Vec<sql::SqlPlan>, String> {
    let engines = targets
        .iter()
        .flat_map(|target| target.credentials.iter().map(|c| c.engine))
        .collect();
    analyze_engines(engines, script)
}

/// Os parâmetros são preenchidos no dialeto de cada motor antes da análise.
fn analyze_engines(mut engines: Vec<Engine>, script: &Script) -> Result<Vec<sql::SqlPlan>, String> {
    engines.sort_by_key(|engine| engine.as_str());
    engines.dedup();
    if engines.is_empty() {
        engines.push(Engine::Mysql);
    }
    engines
        .into_iter()
        .map(|engine| sql::analyze(&params::fill(&script.sql, &script.values, engine)?, engine))
        .collect()
}

/// Recusa a correção insegura em qualquer um dos motores dos equipamentos.
pub fn check_write(targets: &[Target], script: impl Into<Script>) -> Result<(), String> {
    analyze(targets, &script.into())?
        .iter()
        .try_for_each(sql::check_write)
}

/// O mesmo que `check_write`, a partir só dos motores, antes de ler senhas do cofre.
pub fn check_write_engines(engines: &[Engine], script: impl Into<Script>) -> Result<(), String> {
    analyze_engines(engines.to_vec(), &script.into())?
        .iter()
        .try_for_each(sql::check_write)
}

fn plans(analyzed: Vec<sql::SqlPlan>) -> Plans {
    let words = analyzed
        .iter()
        .find(|plan| plan.engine == Engine::Mysql)
        .map_or_default(sql::words);
    Plans {
        statements: analyzed
            .into_iter()
            .map(|plan| (plan.engine, plan.statements))
            .collect(),
        words,
    }
}

/// `events` termina quando o último equipamento é concluído ou pulado.
pub struct Run {
    pub events: mpsc::Receiver<Event>,
    canceller: Canceller,
}

/// Cancela uma execução a partir de outra thread, enquanto `Run` segue lendo os eventos.
#[derive(Clone)]
pub struct Canceller {
    cancelled: Arc<AtomicBool>,
    active: ActiveQueries,
    /// Usado só para avisar falhas do cancelamento. O último worker esvazia o
    /// slot; sem isso o canal nunca fecharia e `events` não terminaria.
    sender: Arc<Mutex<Option<mpsc::SyncSender<Event>>>>,
}

impl Run {
    /// Recusa SQL que não seja de leitura: correções passam por outro fluxo.
    pub fn start(
        targets: Vec<Target>,
        script: impl Into<Script>,
        options: &Options,
    ) -> Result<Self, String> {
        let analyzed = analyze(&targets, &script.into())?;
        if analyzed.iter().any(|plan| !plan.read_only_candidate) {
            return Err("Este SQL pode alterar dados. Use o modo Corrigir.".into());
        }
        Ok(Self::spawn(targets, plans(analyzed), Mode::Read, options))
    }

    /// Correção: cada equipamento na sua transação. O ensaio desfaz; piloto e lote confirmam.
    pub fn start_write(
        targets: Vec<Target>,
        script: impl Into<Script>,
        step: Step,
        options: &Options,
    ) -> Result<Self, String> {
        let analyzed = analyze(&targets, &script.into())?;
        analyzed.iter().try_for_each(sql::check_write)?;
        let commit = step != Step::Rehearsal;
        Ok(Self::spawn(
            targets,
            plans(analyzed),
            Mode::Write { commit },
            options,
        ))
    }

    fn spawn(targets: Vec<Target>, plans: Plans, mode: Mode, options: &Options) -> Self {
        let workers = options
            .concurrency
            .clamp(1, MAX_CONCURRENCY)
            .min(targets.len());
        let jobs = Arc::new(Jobs::new(targets, options.per_store));
        let active: ActiveQueries = Arc::new(Mutex::new(Vec::new()));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, events) = mpsc::sync_channel(64);
        let plans = Arc::new(plans);
        let running = Arc::new(AtomicUsize::new(workers));
        let slot = Arc::new(Mutex::new((workers > 0).then(|| sender.clone())));
        for _ in 0..workers {
            let (jobs, active, cancelled, sender, plans, options, running, slot) = (
                jobs.clone(),
                active.clone(),
                cancelled.clone(),
                sender.clone(),
                plans.clone(),
                options.clone(),
                running.clone(),
                slot.clone(),
            );
            thread::spawn(move || {
                while let Some(target) = jobs.next() {
                    let device_id = target.device_id;
                    let event = if cancelled.load(Ordering::SeqCst) {
                        Event::Skipped { device_id }
                    } else {
                        if sender.send(Event::Connecting { device_id }).is_err() {
                            break;
                        }
                        Event::Finished(execute(
                            &target,
                            &plans,
                            mode,
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
                    jobs.release(target.store_id);
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
        Self {
            events,
            canceller: Canceller {
                cancelled,
                active,
                sender: slot,
            },
        }
    }

    pub fn cancel(&self) {
        self.canceller.cancel();
    }

    pub fn canceller(&self) -> Canceller {
        self.canceller.clone()
    }
}

/// Fila dos equipamentos com o limite por loja: um worker livre pega o primeiro
/// equipamento cuja loja ainda tem vaga e espera se nenhuma tiver.
struct Jobs {
    state: Mutex<(VecDeque<Target>, HashMap<i64, usize>)>,
    freed: Condvar,
    per_store: usize,
}

impl Jobs {
    fn new(targets: Vec<Target>, per_store: usize) -> Self {
        Self {
            state: Mutex::new((VecDeque::from(targets), HashMap::new())),
            freed: Condvar::new(),
            per_store: per_store.max(1),
        }
    }

    fn next(&self) -> Option<Target> {
        let mut state = self.state.lock().ok()?;
        loop {
            let (queue, running) = &mut *state;
            if queue.is_empty() {
                return None;
            }
            let free = queue.iter().position(|target| {
                running.get(&target.store_id).copied().unwrap_or(0) < self.per_store
            });
            if let Some(target) = free.and_then(|index| queue.remove(index)) {
                *running.entry(target.store_id).or_default() += 1;
                return Some(target);
            }
            state = self.freed.wait(state).ok()?;
        }
    }

    fn release(&self, store_id: i64) {
        if let Ok(mut state) = self.state.lock()
            && let Some(count) = state.1.get_mut(&store_id)
        {
            *count = count.saturating_sub(1);
        }
        self.freed.notify_all();
    }
}

impl Canceller {
    /// Para de iniciar equipamentos e interrompe os que estão executando.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let active = self.active.clone();
        let sender = self.sender.lock().ok().and_then(|slot| slot.clone());
        thread::spawn(move || {
            let errors = session::cancel_active(&active);
            if let Some(sender) = sender.filter(|_| !errors.is_empty()) {
                let _ = sender.send(Event::CancelErrors { errors });
            }
        });
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.canceller.cancelled.store(true, Ordering::SeqCst);
    }
}

fn execute(
    target: &Target,
    plans: &Plans,
    mode: Mode,
    options: &Options,
    cancelled: &AtomicBool,
    active: &ActiveQueries,
    on_running: impl FnOnce(&str),
) -> Outcome {
    match mode {
        Mode::Read => session::execute_read(target, plans, options, cancelled, active, on_running),
        Mode::Write { commit } => session::execute_write(
            target,
            plans,
            &plans.words,
            commit,
            options,
            cancelled,
            active,
            on_running,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(device_id: i64, store_id: i64) -> Target {
        Target {
            device_id,
            store_id,
            host: String::new(),
            credentials: Vec::new(),
        }
    }

    #[test]
    fn jobs_respect_the_limit_per_store() {
        let jobs = Arc::new(Jobs::new(
            vec![target(1, 10), target(2, 10), target(3, 20)],
            1,
        ));
        assert_eq!(jobs.next().map(|t| t.device_id), Some(1));
        // A loja 10 está cheia: passa à frente o equipamento da loja 20.
        assert_eq!(jobs.next().map(|t| t.device_id), Some(3));
        let waiting = {
            let jobs = jobs.clone();
            thread::spawn(move || jobs.next().map(|t| t.device_id))
        };
        thread::sleep(Duration::from_millis(50));
        assert!(
            !waiting.is_finished(),
            "não deveria pegar o 2 com a loja cheia"
        );
        jobs.release(10);
        assert_eq!(waiting.join().unwrap(), Some(2));
        assert!(jobs.next().is_none());
    }
}
