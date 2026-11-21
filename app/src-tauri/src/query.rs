//! Comandos da Consulta: execução em massa sobre a frota, cancelamento, grade
//! consolidada e exportação para a pasta Downloads.

use crate::{AppState, CommandResult, explain, lock};
use selectall_core::exec::{
    Canceller, Credential, Event, Failure, Options, Outcome, Run, Status, Target,
};
use selectall_core::grid::{self, Grid, Origin};
use selectall_core::model::{Device, Profile, Store};
use selectall_core::vault;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Emitter, Manager, State};

const PROGRESS_EVENT: &str = "query-progress";

#[derive(Default)]
pub struct QueryState {
    current: Option<Execution>,
    last_id: u64,
}

/// A execução mais recente. Fica guardada para a grade e a exportação.
struct Execution {
    id: u64,
    sql: String,
    canceller: Canceller,
    running: bool,
    outcomes: Vec<Outcome>,
    origins: HashMap<i64, Origin>,
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

fn start(
    app: &AppHandle,
    state: &State<'_, AppState>,
    chain_id: i64,
    device_ids: &[i64],
    sql: String,
    retry: bool,
) -> CommandResult<u64> {
    if device_ids.is_empty() {
        return Err("Escolha ao menos um equipamento.".into());
    }
    let ids: HashSet<i64> = device_ids.iter().copied().collect();
    let (chosen, credentials) = {
        let db = lock(state)?;
        let read = |error| explain(&error, "");
        let install = db
            .setting("install_id")
            .map_err(read)?
            .ok_or("Instalação sem identificador; reinicie o aplicativo.")?;
        let stores = db.stores(chain_id).map_err(read)?;
        let chosen = select(&stores, db.devices(chain_id).map_err(read)?, &ids);
        let credentials = credentials(&install, &db.profiles(chain_id).map_err(read)?, &chosen)?;
        (chosen, credentials)
    };
    if chosen.is_empty() {
        return Err("Os equipamentos escolhidos não existem mais. Recarregue a frota.".into());
    }
    let targets = chosen
        .iter()
        .map(|(device, _)| Target {
            device_id: device.id,
            host: device.host.clone(),
            credentials: device
                .profile_ids
                .iter()
                .filter_map(|id| credentials.get(id).cloned())
                .collect(),
        })
        .collect();

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
    let run = Run::start(targets, &sql, &Options::default())?;
    query.last_id += 1;
    let id = query.last_id;
    let fresh: HashMap<i64, Origin> = chosen
        .into_iter()
        .map(|(d, origin)| (d.id, origin))
        .collect();
    let (outcomes, origins) = match query.current.take() {
        // Repetir mantém o resultado de quem já respondeu e a ordem original das linhas.
        Some(previous) if retry => (
            previous
                .outcomes
                .into_iter()
                .filter(|outcome| !ids.contains(&outcome.device_id))
                .collect(),
            previous.origins,
        ),
        _ => (Vec::new(), fresh),
    };
    query.current = Some(Execution {
        id,
        sql,
        canceller: run.canceller(),
        running: true,
        outcomes,
        origins,
    });
    drop(query);

    let app = app.clone();
    std::thread::spawn(move || forward(&app, id, &run));
    Ok(id)
}

/// Repassa os eventos à janela e guarda cada resultado na execução corrente.
fn forward(app: &AppHandle, id: u64, run: &Run) {
    let state = app.state::<AppState>();
    let emit = |progress| {
        let _ = app.emit(PROGRESS_EVENT, Envelope { run: id, progress });
    };
    for event in &run.events {
        if let Event::Finished(outcome) = &event
            && let Ok(mut query) = state.query.lock()
            && let Some(current) = query.current.as_mut().filter(|c| c.id == id)
        {
            current.outcomes.push(outcome.clone());
        }
        emit(progress(event));
    }
    if let Ok(mut query) = state.query.lock()
        && let Some(current) = query.current.as_mut().filter(|c| c.id == id)
    {
        current.running = false;
    }
    emit(Progress::Done);
}

/// Inicia a consulta e devolve o número da execução, que acompanha cada evento `query-progress`.
#[tauri::command]
pub fn run_query(
    app: AppHandle,
    state: State<'_, AppState>,
    chain_id: i64,
    device_ids: Vec<i64>,
    sql: String,
) -> CommandResult<u64> {
    start(&app, &state, chain_id, &device_ids, sql, false)
}

/// Repete o SQL da última execução só nestes equipamentos, mantendo os demais resultados.
#[tauri::command]
pub fn retry_query(
    app: AppHandle,
    state: State<'_, AppState>,
    chain_id: i64,
    device_ids: Vec<i64>,
) -> CommandResult<u64> {
    let sql = state
        .query
        .lock()
        .ok()
        .and_then(|query| query.current.as_ref().map(|c| c.sql.clone()))
        .ok_or("Não há execução anterior para repetir.")?;
    start(&app, &state, chain_id, &device_ids, sql, true)
}

#[tauri::command]
pub fn cancel_query(state: State<'_, AppState>) {
    if let Ok(query) = state.query.lock()
        && let Some(current) = query.current.as_ref().filter(|c| c.running)
    {
        current.canceller.cancel();
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
    if stem.is_empty()
        || !stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Nome de arquivo inválido.".into());
    }
    let bytes = match format {
        "csv" => with_grid(&state, set, |grid| grid::to_csv(&grid))??,
        "xlsx" => with_grid(&state, set, |grid| grid::to_xlsx(&grid))??,
        _ => return Err("Formato desconhecido.".into()),
    };
    let dir = app
        .path()
        .download_dir()
        .map_err(|_| "Não foi encontrada a pasta Downloads deste usuário.".to_owned())?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Não foi possível criar {}: {error}", dir.display()))?;
    let path = grid::free_path(&dir, stem, format);
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
            }
        );
    }

    #[test]
    fn store_codes_sort_as_numbers_when_possible() {
        assert_eq!(compare_codes("9", "10"), Ordering::Less);
        assert_eq!(compare_codes("A1", "B0"), Ordering::Less);
    }
}
