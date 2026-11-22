//! Comandos da Consulta: execução em massa sobre a frota, cancelamento, grade
//! consolidada e exportação para a pasta Downloads. As etapas de correção usam a
//! mesma execução e o mesmo evento de progresso.

use crate::{AppState, CommandResult, explain, lock};
use selectall_core::correction::Correction;
use selectall_core::exec::{
    Canceller, Credential, Event, Failure, Options, Outcome, Run, Status, Step, Target,
};
use selectall_core::grid::{self, Comparison, Grid, Origin};
use selectall_core::history::{HistoryEntry, NewRun};
use selectall_core::model::{Device, Profile, Store};
use selectall_core::params::{Script, Values};
use selectall_core::store::{AuditDevice, AuditStart};
use selectall_core::vault;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, State};

const PROGRESS_EVENT: &str = "query-progress";

#[derive(Default)]
pub struct QueryState {
    current: Option<Execution>,
    last_id: u64,
    /// A correção do último ensaio; piloto e lote usam o SQL guardado aqui.
    pub(crate) correction: Option<Correction>,
}

impl QueryState {
    pub(crate) fn running(&self) -> bool {
        self.current.as_ref().is_some_and(|current| current.running)
    }
}

/// A execução mais recente, ou a reaberta do histórico. Fica guardada para a
/// grade, a comparação e a exportação.
struct Execution {
    id: u64,
    chain_id: i64,
    /// `None` quando reaberta do histórico sem o SQL (tinha termo de autenticação).
    script: Option<Script>,
    /// Nome da consulta salva de onde veio, para o histórico.
    label: Option<String>,
    /// `None` quando reaberta do histórico: não há o que cancelar.
    canceller: Option<Canceller>,
    running: bool,
    started: Instant,
    outcomes: Vec<Outcome>,
    origins: HashMap<i64, Origin>,
    /// Etapa e registro de auditoria quando a execução é uma correção.
    step: Option<(Step, i64)>,
    /// Registro no histórico; a repetição das falhas o substitui.
    history_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Progress {
    Connecting {
        device_id: i64,
    },
    Running {
        device_id: i64,
        profile: String,
    },
    /// Sem as linhas: a grade é pedida à parte, em `query_grid`.
    Finished {
        device_id: i64,
        profile: Option<String>,
        status: Status,
        failure: Option<Failure>,
        elapsed_ms: u64,
        rows: usize,
        sets: usize,
        truncated: bool,
        warnings: u32,
        affected_rows: u64,
    },
    Skipped {
        device_id: i64,
    },
    CancelErrors {
        errors: Vec<String>,
    },
    Done,
}

#[derive(Clone, Serialize)]
struct Envelope {
    run: u64,
    #[serde(flatten)]
    progress: Progress,
}

fn progress(event: Event) -> Progress {
    match event {
        Event::Connecting { device_id } => Progress::Connecting { device_id },
        Event::Running { device_id, profile } => Progress::Running { device_id, profile },
        Event::Finished(outcome) => Progress::Finished {
            device_id: outcome.device_id,
            profile: outcome.profile,
            status: outcome.status,
            failure: outcome.failure,
            elapsed_ms: outcome.elapsed_ms,
            rows: outcome.sets.iter().map(|set| set.rows.len()).sum(),
            sets: outcome.sets.len(),
            truncated: outcome.sets.iter().any(|set| set.truncated),
            warnings: outcome.sets.iter().map(|set| u32::from(set.warnings)).sum(),
            affected_rows: outcome.affected_rows,
        },
        Event::Skipped { device_id } => Progress::Skipped { device_id },
        Event::CancelErrors { errors } => Progress::CancelErrors { errors },
    }
}

/// Códigos numéricos em ordem de número ("9" antes de "10"); o resto em ordem alfabética.
fn compare_codes(left: &str, right: &str) -> Ordering {
    match (left.parse::<u64>(), right.parse::<u64>()) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => left.cmp(right),
    }
}

/// Equipamentos escolhidos, em ordem de loja e nome, com a origem de cada um.
fn select(stores: &[Store], devices: Vec<Device>, ids: &HashSet<i64>) -> Vec<(Device, Origin)> {
    let code: HashMap<i64, &str> = stores.iter().map(|s| (s.id, s.code.as_str())).collect();
    let mut chosen: Vec<Device> = devices
        .into_iter()
        .filter(|d| ids.contains(&d.id))
        .collect();
    chosen.sort_by(|a, b| {
        let (ca, cb) = (
            code.get(&a.store_id).copied().unwrap_or(""),
            code.get(&b.store_id).copied().unwrap_or(""),
        );
        compare_codes(ca, cb)
            .then_with(|| a.name.cmp(&b.name))
            .then(a.id.cmp(&b.id))
    });
    chosen
        .into_iter()
        .enumerate()
        .map(|(order, device)| {
            let origin = Origin {
                order,
                store: code.get(&device.store_id).copied().unwrap_or("").to_owned(),
                device: device.name.clone(),
                host: device.host.clone(),
            };
            (device, origin)
        })
        .collect()
}

fn targets(chosen: &[(Device, Origin)], credentials: &HashMap<i64, Credential>) -> Vec<Target> {
    chosen
        .iter()
        .map(|(device, _)| Target {
            device_id: device.id,
            store_id: device.store_id,
            host: device.host.clone(),
            credentials: device
                .profile_ids
                .iter()
                .filter_map(|id| credentials.get(id).cloned())
                .collect(),
        })
        .collect()
}

/// Lê do cofre a senha de cada perfil usado. Perfil sem senha guardada tenta senha vazia.
fn credentials(
    install: &str,
    profiles: &[Profile],
    devices: &[(Device, Origin)],
) -> CommandResult<HashMap<i64, Credential>> {
    let used: HashSet<i64> = devices
        .iter()
        .flat_map(|(d, _)| d.profile_ids.iter().copied())
        .collect();
    profiles
        .iter()
        .filter(|profile| used.contains(&profile.id))
        .map(|profile| {
            let password =
                vault::read(&vault::profile_key(install, profile.id))?.unwrap_or_default();
            Ok((
                profile.id,
                Credential {
                    profile_id: profile.id,
                    profile_name: profile.name.clone(),
                    engine: profile.engine,
                    port: profile.port,
                    database: profile.database.clone(),
                    username: profile.username.clone(),
                    password,
                    tls: profile.tls,
                },
            ))
        })
        .collect()
}

/// Como a execução começa: leitura nova, repetição de leitura ou etapa de correção.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Read,
    Retry,
    Write(Step),
}

fn user_name() -> String {
    ["USER", "USERNAME"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|name| !name.is_empty()))
        .unwrap_or_else(|| "desconhecido".into())
}

/// Em correção, recusa o SQL bloqueado e grava o cabeçalho da auditoria antes de
/// qualquer equipamento começar: SQL recusado não chega a nenhum equipamento.
fn begin_audit(
    state: &State<'_, AppState>,
    kind: Kind,
    script: &Script,
    targets: &[Target],
    chain: (i64, String),
    planned: usize,
    rehearsal_skipped: bool,
) -> CommandResult<Option<(Step, i64)>> {
    let Kind::Write(step) = kind else {
        return Ok(None);
    };
    selectall_core::exec::check_write(targets, script.clone())?;
    let audit = lock(state)?
        .begin_audit(&AuditStart {
            user: user_name(),
            chain_id: chain.0,
            chain: chain.1,
            step: step.as_str().into(),
            sql: audit_sql(script),
            planned,
            rehearsal_skipped,
        })
        .map_err(|error| explain(&error, ""))?;
    Ok(Some((step, audit)))
}

/// SQL da auditoria: com parâmetros, os valores vão junto, um por linha de comentário.
fn audit_sql(script: &Script) -> String {
    use std::fmt::Write as _;
    let mut text = script.sql.clone();
    for (name, value) in &script.values {
        let value = value.replace(['\n', '\r'], " ");
        let _ = write!(text, "\n-- {{{{{name}}}}} = {value}");
    }
    text
}

pub(crate) fn start(
    app: &AppHandle,
    state: &State<'_, AppState>,
    chain_id: i64,
    device_ids: &[i64],
    script: Script,
    label: Option<String>,
    kind: Kind,
) -> CommandResult<u64> {
    let retry = kind == Kind::Retry;
    if device_ids.is_empty() {
        return Err("Escolha ao menos um equipamento.".into());
    }
    let ids: HashSet<i64> = device_ids.iter().copied().collect();
    let (chosen, credentials, chain, options) = {
        let db = lock(state)?;
        let read = |error| explain(&error, "");
        let install = db
            .setting("install_id")
            .map_err(read)?
            .ok_or("Instalação sem identificador; reinicie o aplicativo.")?;
        let stores = db.stores(chain_id).map_err(read)?;
        let chosen = select(&stores, db.devices(chain_id).map_err(read)?, &ids);
        let credentials = credentials(&install, &db.profiles(chain_id).map_err(read)?, &chosen)?;
        let chain = db
            .chains()
            .map_err(read)?
            .into_iter()
            .find(|chain| chain.id == chain_id)
            .map_or_default(|chain| chain.name);
        let options = Options {
            per_store: db.per_store_limit().map_err(read)?,
            ..Options::default()
        };
        (chosen, credentials, chain, options)
    };
    if chosen.is_empty() {
        return Err("Os equipamentos escolhidos não existem mais. Recarregue a frota.".into());
    }
    let targets = targets(&chosen, &credentials);

    let mut query = state
        .query
        .lock()
        .map_err(|_| "A consulta ficou indisponível. Reinicie o aplicativo.".to_owned())?;
    if query
        .current
        .as_ref()
        .is_some_and(|current| current.running)
    {
        return Err("Já há uma consulta em andamento.".into());
    }
    let skipped = kind != Kind::Write(Step::Rehearsal)
        && query
            .correction
            .as_ref()
            .is_some_and(Correction::rehearsal_skipped);
    let step = begin_audit(
        state,
        kind,
        &script,
        &targets,
        (chain_id, chain),
        chosen.len(),
        skipped,
    )?;
    let run = match step {
        Some((step, _)) => Run::start_write(targets, script.clone(), step, &options)?,
        None => Run::start(targets, script.clone(), &options)?,
    };
    if let Some((Step::Rehearsal, _)) = step {
        query.correction =
            Some(Correction::new(script.sql.clone()).with_values(script.values.clone()));
    }
    query.last_id += 1;
    let id = query.last_id;
    let fresh: HashMap<i64, Origin> = chosen
        .into_iter()
        .map(|(d, origin)| (d.id, origin))
        .collect();
    let (outcomes, origins, history_id) = match query.current.take() {
        // Repetir mantém o resultado de quem já respondeu e a ordem original das linhas.
        Some(previous) if retry => (
            kept_on_retry(previous.outcomes, &fresh),
            previous.origins,
            previous.history_id,
        ),
        _ => (Vec::new(), fresh, None),
    };
    query.current = Some(Execution {
        id,
        chain_id,
        script: Some(script),
        label,
        canceller: Some(run.canceller()),
        running: true,
        started: Instant::now(),
        outcomes,
        origins,
        step,
        history_id,
    });
    drop(query);

    let app = app.clone();
    std::thread::spawn(move || forward(&app, id, &run));
    Ok(id)
}

fn skipped(device_id: i64) -> Outcome {
    Outcome {
        device_id,
        profile: None,
        sets: Vec::new(),
        status: Status::Cancelled,
        failure: Some(Failure {
            message: "Não iniciado: a execução foi cancelada.".into(),
            detail: None,
        }),
        elapsed_ms: 0,
        affected_rows: 0,
    }
}

/// Resultados que a repetição mantém: os de quem não vai rodar de novo. Equipamento
/// excluído da frota não roda e fica com o resultado antigo.
fn kept_on_retry(outcomes: Vec<Outcome>, rerun: &HashMap<i64, Origin>) -> Vec<Outcome> {
    outcomes
        .into_iter()
        .filter(|outcome| !rerun.contains_key(&outcome.device_id))
        .collect()
}

/// Repassa os eventos à janela e guarda cada resultado na execução corrente.
fn forward(app: &AppHandle, id: u64, run: &Run) {
    let state = app.state::<AppState>();
    let emit = |progress| {
        let _ = app.emit(PROGRESS_EVENT, Envelope { run: id, progress });
    };
    for event in &run.events {
        match &event {
            Event::Finished(outcome) => record(&state, id, outcome),
            // Pulado pelo cancelamento: entra no histórico e na auditoria como cancelado,
            // para reabrir com ele e para "Repetir só estes" alcançá-lo.
            Event::Skipped { device_id } => record(&state, id, &skipped(*device_id)),
            _ => {}
        }
        emit(progress(event));
    }
    let reading = if let Ok(mut query) = state.query.lock()
        && let Some(current) = query.current.as_mut().filter(|c| c.id == id)
    {
        current.running = false;
        current.step.is_none().then(|| {
            let mut devices: Vec<(Origin, Outcome)> = current
                .outcomes
                .iter()
                .filter_map(|o| {
                    current
                        .origins
                        .get(&o.device_id)
                        .map(|origin| (origin.clone(), o.clone()))
                })
                .collect();
            devices.sort_by_key(|(origin, _)| origin.order);
            (
                NewRun {
                    chain_id: current.chain_id,
                    label: current.label.clone(),
                    script: current.script.clone(),
                    targets: current.origins.len(),
                    elapsed_ms: u64::try_from(current.started.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                    devices,
                },
                current.history_id,
            )
        })
    } else {
        None
    };
    if let Some((run, replaces)) = reading {
        save_history(&state, id, &run, replaces);
    }
    emit(Progress::Done);
}

/// Grava a leitura no histórico. Uma falha aqui não apaga o resultado da tela.
fn save_history(state: &State<'_, AppState>, id: u64, run: &NewRun, replaces: Option<i64>) {
    let saved = state
        .db
        .lock()
        .ok()
        .and_then(|mut db| db.add_history(run, replaces).ok());
    if let Some(history_id) = saved
        && let Ok(mut query) = state.query.lock()
        && let Some(current) = query.current.as_mut().filter(|c| c.id == id)
    {
        current.history_id = Some(history_id);
    }
}

/// Guarda o resultado na execução; em correção, também na auditoria e no estado da correção.
fn record(state: &State<'_, AppState>, id: u64, outcome: &Outcome) {
    let audit = {
        let Ok(mut guard) = state.query.lock() else {
            return;
        };
        let query = &mut *guard;
        let Some(current) = query.current.as_mut().filter(|c| c.id == id) else {
            return;
        };
        current.outcomes.push(outcome.clone());
        let Some((step, audit_id)) = current.step else {
            return;
        };
        if let Some(correction) = query.correction.as_mut() {
            correction.record(step, outcome);
        }
        let origin = current.origins.get(&outcome.device_id);
        (
            audit_id,
            AuditDevice {
                store: origin.map_or_default(|o| o.store.clone()),
                device: origin.map_or_default(|o| o.device.clone()),
                host: origin.map_or_default(|o| o.host.clone()),
                profile: outcome.profile.clone(),
                status: status_name(outcome.status).into(),
                affected: i64::try_from(outcome.affected_rows).unwrap_or(i64::MAX),
                message: outcome.failure.as_ref().map(|f| f.message.clone()),
            },
        )
    };
    if let Ok(db) = state.db.lock() {
        // A janela já mostra o resultado; uma falha aqui fica visível na Auditoria incompleta.
        let _ = db.append_audit(audit.0, &audit.1);
    }
}

fn status_name(status: Status) -> &'static str {
    match status {
        Status::Success => "success",
        Status::Partial => "partial",
        Status::Failed => "failed",
        Status::Offline => "offline",
        Status::Cancelled => "cancelled",
        Status::TimedOut => "timed_out",
        Status::Rehearsed => "rehearsed",
        Status::Applied => "applied",
        Status::Unknown => "unknown",
    }
}

/// Inicia a consulta e devolve o número da execução, que acompanha cada evento `query-progress`.
#[tauri::command]
pub fn run_query(
    app: AppHandle,
    state: State<'_, AppState>,
    chain_id: i64,
    device_ids: Vec<i64>,
    sql: String,
    values: Values,
    label: Option<String>,
) -> CommandResult<u64> {
    start(
        &app,
        &state,
        chain_id,
        &device_ids,
        Script::new(sql, values),
        label,
        Kind::Read,
    )
}

/// Repete o SQL da última execução só nestes equipamentos, mantendo os demais resultados.
#[tauri::command]
pub fn retry_query(
    app: AppHandle,
    state: State<'_, AppState>,
    chain_id: i64,
    device_ids: Vec<i64>,
) -> CommandResult<u64> {
    let (script, label) = state
        .query
        .lock()
        .ok()
        .and_then(|query| {
            query
                .current
                .as_ref()
                .map(|c| (c.script.clone(), c.label.clone()))
        })
        .ok_or("Não há execução anterior para repetir.")?;
    let script = script.ok_or(
        "O SQL desta execução não foi guardado porque cita senha, token ou segredo. Execute de novo pelo editor.",
    )?;
    start(
        &app,
        &state,
        chain_id,
        &device_ids,
        script,
        label,
        Kind::Retry,
    )
}

#[tauri::command]
pub fn cancel_query(state: State<'_, AppState>) {
    if let Ok(query) = state.query.lock()
        && let Some(canceller) = query
            .current
            .as_ref()
            .filter(|c| c.running)
            .and_then(|c| c.canceller.as_ref())
    {
        canceller.cancel();
    }
}

fn with_grid<T>(
    state: &State<'_, AppState>,
    set: usize,
    use_grid: impl FnOnce(Grid) -> T,
) -> CommandResult<T> {
    let query = state
        .query
        .lock()
        .map_err(|_| "A consulta ficou indisponível. Reinicie o aplicativo.".to_owned())?;
    let current = query
        .current
        .as_ref()
        .ok_or("Execute uma consulta primeiro.")?;
    Ok(use_grid(grid::consolidate(
        &current.outcomes,
        &current.origins,
        set,
    )))
}

/// Agrupa os equipamentos que responderam pelo resultado nas colunas escolhidas.
#[tauri::command]
pub fn query_compare(
    state: State<'_, AppState>,
    set: usize,
    columns: Option<Vec<String>>,
) -> CommandResult<Comparison> {
    let query = state
        .query
        .lock()
        .map_err(|_| "A consulta ficou indisponível. Reinicie o aplicativo.".to_owned())?;
    let current = query
        .current
        .as_ref()
        .ok_or("Execute uma consulta primeiro.")?;
    Ok(grid::compare(
        &current.outcomes,
        &current.origins,
        set,
        columns.as_deref(),
    ))
}

/// Um equipamento da execução reaberta, no formato do evento de conclusão.
#[derive(Serialize)]
pub struct ReopenedDevice {
    store: String,
    device: String,
    host: String,
    #[serde(flatten)]
    progress: Progress,
}

#[derive(Serialize)]
pub struct Reopened {
    entry: HistoryEntry,
    devices: Vec<ReopenedDevice>,
}

/// Reabre uma execução do histórico como a execução corrente: a grade, a comparação,
/// a exportação e a repetição das falhas passam a valer para ela.
#[tauri::command]
pub fn reopen_history(state: State<'_, AppState>, id: i64) -> CommandResult<Reopened> {
    let (entry, devices) = lock(&state)?
        .history_run(id)
        .map_err(|error| explain(&error, ""))?;
    let chain_id = lock(&state)?
        .history_chain(id)
        .map_err(|error| explain(&error, ""))?;
    let mut query = state
        .query
        .lock()
        .map_err(|_| "A consulta ficou indisponível. Reinicie o aplicativo.".to_owned())?;
    if query.running() {
        return Err("Espere a consulta em andamento terminar.".into());
    }
    query.last_id += 1;
    let listed = devices
        .iter()
        .map(|(origin, outcome)| ReopenedDevice {
            store: origin.store.clone(),
            device: origin.device.clone(),
            host: origin.host.clone(),
            progress: progress(Event::Finished(outcome.clone())),
        })
        .collect();
    query.current = Some(Execution {
        id: query.last_id,
        chain_id,
        script: entry
            .sql
            .clone()
            .map(|sql| Script::new(sql, entry.values.clone())),
        label: Some(entry.label.clone()),
        canceller: None,
        running: false,
        started: Instant::now(),
        origins: devices
            .iter()
            .map(|(origin, outcome)| (outcome.device_id, origin.clone()))
            .collect(),
        outcomes: devices.into_iter().map(|(_, outcome)| outcome).collect(),
        step: None,
        history_id: Some(id),
    });
    Ok(Reopened {
        entry,
        devices: listed,
    })
}

#[tauri::command]
pub fn query_grid(state: State<'_, AppState>, set: usize) -> CommandResult<Grid> {
    with_grid(&state, set, |grid| grid)
}

/// Grava a grade em Downloads e devolve o caminho. `stem` vem da janela, com data e hora locais.
#[tauri::command]
pub fn export_grid(
    app: AppHandle,
    state: State<'_, AppState>,
    set: usize,
    format: &str,
    stem: &str,
) -> CommandResult<String> {
    check_stem(stem)?;
    let bytes = match format {
        "csv" => with_grid(&state, set, |grid| grid::to_csv(&grid))??,
        "xlsx" => with_grid(&state, set, |grid| grid::to_xlsx(&grid))??,
        _ => return Err("Formato desconhecido.".into()),
    };
    save_download(&app, stem, format, &bytes)
}

pub(crate) fn check_stem(stem: &str) -> CommandResult<()> {
    if stem.is_empty()
        || !stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Nome de arquivo inválido.".into());
    }
    Ok(())
}

/// Grava em Downloads sem sobrescrever arquivo existente e devolve o caminho.
pub(crate) fn save_download(
    app: &AppHandle,
    stem: &str,
    extension: &str,
    bytes: &[u8],
) -> CommandResult<String> {
    let dir = app
        .path()
        .download_dir()
        .map_err(|_| "Não foi encontrada a pasta Downloads deste usuário.".to_owned())?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Não foi possível criar {}: {error}", dir.display()))?;
    let path = grid::free_path(&dir, stem, extension);
    std::fs::write(&path, bytes)
        .map_err(|error| format!("Não foi possível gravar {}: {error}", path.display()))?;
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use selectall_core::exec::{Column, ResultSet};

    fn device(id: i64, store_id: i64, name: &str) -> Device {
        Device {
            id,
            store_id,
            name: name.into(),
            kind: "PDV".into(),
            host: format!("10.0.0.{id}"),
            profile_ids: vec![1],
        }
    }

    fn store(id: i64, code: &str) -> Store {
        Store {
            id,
            chain_id: 1,
            code: code.into(),
            name: String::new(),
            subnet: String::new(),
        }
    }

    #[test]
    fn select_keeps_chosen_devices_in_store_and_name_order() {
        let stores = [store(1, "10"), store(2, "9")];
        let devices = vec![
            device(1, 1, "PDV 02"),
            device(2, 1, "PDV 01"),
            device(3, 2, "PDV 05"),
            device(4, 2, "PDV 06"),
        ];
        let chosen = select(&stores, devices, &HashSet::from([1, 2, 3]));
        let order: Vec<(i64, &str, usize)> = chosen
            .iter()
            .map(|(d, o)| (d.id, o.store.as_str(), o.order))
            .collect();
        assert_eq!(order, [(3, "9", 0), (2, "10", 1), (1, "10", 2)]);
    }

    #[test]
    fn finished_progress_summarises_without_rows() {
        let set = |rows: usize, truncated| ResultSet {
            columns: vec![Column {
                name: "a".into(),
                numeric: false,
            }],
            rows: vec![vec![None]; rows],
            affected_rows: 0,
            warnings: 1,
            truncated,
        };
        let event = Event::Finished(Outcome {
            device_id: 7,
            profile: Some("Econect".into()),
            sets: vec![set(2, false), set(3, true)],
            status: Status::Partial,
            failure: None,
            elapsed_ms: 40,
            affected_rows: 0,
        });
        assert_eq!(
            progress(event),
            Progress::Finished {
                device_id: 7,
                profile: Some("Econect".into()),
                status: Status::Partial,
                failure: None,
                elapsed_ms: 40,
                rows: 5,
                sets: 2,
                truncated: true,
                warnings: 2,
                affected_rows: 0,
            }
        );
    }

    #[test]
    fn store_codes_sort_as_numbers_when_possible() {
        assert_eq!(compare_codes("9", "10"), Ordering::Less);
        assert_eq!(compare_codes("A1", "B0"), Ordering::Less);
    }

    #[test]
    fn retry_keeps_results_of_devices_removed_from_the_fleet() {
        let outcome = |device_id| Outcome {
            device_id,
            profile: None,
            sets: Vec::new(),
            status: Status::Offline,
            failure: None,
            elapsed_ms: 0,
            affected_rows: 0,
        };
        let origin = Origin {
            order: 0,
            store: "101".into(),
            device: "PDV 1".into(),
            host: "10.0.0.1".into(),
        };
        // Pedido de repetir 1 e 2; o 2 saiu da frota e não está entre os que rodam.
        let rerun = HashMap::from([(1, origin)]);
        let kept: Vec<i64> = kept_on_retry(vec![outcome(1), outcome(2), outcome(3)], &rerun)
            .iter()
            .map(|o| o.device_id)
            .collect();
        assert_eq!(kept, [2, 3]);
    }
}
