//! Comandos da Frota: perfis com senha no cofre, lojas, equipamentos, regra,
//! prévia e inclusão em lote, varredura.

use crate::{AppState, CommandResult, explain, lock};
use selectall_core::fleet::{self, Batch, Fleet, Plan};
use selectall_core::model::{AddressRule, Chain, Device, Profile, Store};
use selectall_core::scan::{self, ScanHit};
use selectall_core::store::{Applied, Db};
use selectall_core::{import, vault};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tauri::State;

const SCAN_TIMEOUT: Duration = Duration::from_millis(800);
const SCAN_CONCURRENCY: usize = 64;
const DEFAULT_PORT: u16 = 3306;

#[derive(Serialize)]
pub struct ProfileView {
    #[serde(flatten)]
    profile: Profile,
    has_password: bool,
}

#[derive(Serialize)]
pub struct FleetView {
    stores: Vec<Store>,
    devices: Vec<Device>,
    profiles: Vec<ProfileView>,
    rules: Vec<AddressRule>,
}

fn install_id(db: &Db) -> CommandResult<String> {
    db.setting("install_id")
        .map_err(|error| explain(&error, ""))?
        .ok_or_else(|| "Instalação sem identificador; reinicie o aplicativo.".into())
}

#[tauri::command]
pub fn load_fleet(state: State<'_, AppState>, chain_id: i64) -> CommandResult<FleetView> {
    let db = lock(&state)?;
    let install = install_id(&db)?;
    let read = |error| explain(&error, "");
    let profiles = db
        .profiles(chain_id)
        .map_err(read)?
        .into_iter()
        .map(|profile| ProfileView {
            // Cofre bloqueado ou ausente aparece como "sem senha"; o erro real surge ao salvar.
            has_password: vault::read(&vault::profile_key(&install, profile.id))
                .is_ok_and(|password| password.is_some()),
            profile,
        })
        .collect();
    Ok(FleetView {
        stores: db.stores(chain_id).map_err(read)?,
        devices: db.devices(chain_id).map_err(read)?,
        profiles,
        rules: db.rules(chain_id).map_err(read)?,
    })
}

#[tauri::command]
pub fn rename_chain(state: State<'_, AppState>, id: i64, name: &str) -> CommandResult<Chain> {
    lock(&state)?
        .rename_chain(id, name)
        .map_err(|error| explain(&error, "Já existe uma rede com esse nome."))
}

#[tauri::command]
pub fn delete_chain(state: State<'_, AppState>, id: i64) -> CommandResult<()> {
    let db = lock(&state)?;
    let install = install_id(&db)?;
    let profiles = db.profiles(id).map_err(|error| explain(&error, ""))?;
    db.delete_chain(id).map_err(|error| explain(&error, ""))?;
    forget_passwords(&install, profiles.iter().map(|profile| profile.id))
}

fn forget_passwords(install: &str, ids: impl Iterator<Item = i64>) -> CommandResult<()> {
    let failures: Vec<String> = ids
        .filter_map(|id| vault::delete(&vault::profile_key(install, id)).err())
        .collect();
    match failures.first() {
        None => Ok(()),
        Some(first) => Err(format!("Excluído, mas {first}")),
    }
}

/// `password` ausente mantém a senha guardada; presente (mesmo vazia) substitui.
#[tauri::command]
pub fn save_profile(
    state: State<'_, AppState>,
    profile: Profile,
    password: Option<String>,
) -> CommandResult<Profile> {
    let db = lock(&state)?;
    let install = install_id(&db)?;
    let duplicate = "Já existe um perfil com esse nome nesta rede.";
    let mut saved = profile;
    let created = saved.id == 0;
    if created {
        saved.id = db
            .add_profile(&saved)
            .map_err(|error| explain(&error, duplicate))?;
    } else {
        db.update_profile(&saved)
            .map_err(|error| explain(&error, duplicate))?;
    }
    if let Some(password) = password
        && let Err(message) = vault::write(&vault::profile_key(&install, saved.id), &password)
    {
        // Perfil novo sem senha guardada não serve; desfaz para a pessoa tentar de novo.
        if created {
            let _ = db.delete_profile(saved.id);
        }
        return Err(message);
    }
    db.profile(saved.id).map_err(|error| explain(&error, ""))
}

#[tauri::command]
pub fn delete_profile(state: State<'_, AppState>, id: i64) -> CommandResult<()> {
    let db = lock(&state)?;
    let install = install_id(&db)?;
    db.delete_profile(id).map_err(|error| explain(&error, ""))?;
    forget_passwords(&install, std::iter::once(id))
}

#[tauri::command]
pub fn save_store(state: State<'_, AppState>, store: Store) -> CommandResult<Store> {
    let db = lock(&state)?;
    let duplicate = "Já existe uma loja com esse código nesta rede.";
    if store.id == 0 {
        db.add_store(store.chain_id, &store.code, &store.name, &store.subnet)
            .map_err(|error| explain(&error, duplicate))
    } else {
        db.update_store(&store)
            .map_err(|error| explain(&error, duplicate))?;
        Ok(store)
    }
}

#[tauri::command]
pub fn delete_store(state: State<'_, AppState>, id: i64) -> CommandResult<()> {
    lock(&state)?
        .delete_store(id)
        .map_err(|error| explain(&error, ""))
}

#[tauri::command]
pub fn save_device(state: State<'_, AppState>, device: Device) -> CommandResult<Device> {
    let mut db = lock(&state)?;
    let duplicate = "Esse endereço já está cadastrado nesta loja.";
    let mut saved = device;
    if saved.id == 0 {
        saved.id = db
            .add_device(&saved)
            .map_err(|error| explain(&error, duplicate))?;
    } else {
        db.update_device(&saved)
            .map_err(|error| explain(&error, duplicate))?;
    }
    Ok(saved)
}

#[tauri::command]
pub fn delete_device(state: State<'_, AppState>, id: i64) -> CommandResult<()> {
    lock(&state)?
        .delete_device(id)
        .map_err(|error| explain(&error, ""))
}

#[tauri::command]
pub fn save_rules(
    state: State<'_, AppState>,
    chain_id: i64,
    rules: Vec<AddressRule>,
) -> CommandResult<()> {
    for rule in &rules {
        if rule.kind.trim().is_empty() || rule.template.trim().is_empty() {
            return Err("Cada linha da regra precisa de tipo e endereço.".into());
        }
        fleet::expand_template(&rule.template, "1", "1")
            .map_err(|message| format!("Regra {}: {message}", rule.kind))?;
    }
    lock(&state)?
        .set_rules(chain_id, &rules)
        .map_err(|error| explain(&error, ""))
}

/// De onde vem o lote da janela "Adicionar lojas".
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    Rule {
        rules: Vec<AddressRule>,
        stores: String,
        /// Mantém só os endereços que respondem na porta do perfil.
        scan: bool,
    },
    Paste {
        text: String,
    },
    File {
        name: String,
        bytes: Vec<u8>,
    },
    Batch {
        batch: Batch,
    },
}

#[tauri::command]
pub async fn preview_batch(
    state: State<'_, AppState>,
    chain_id: i64,
    source: Source,
) -> CommandResult<Plan> {
    let (rules_scan, mut batch) = match source {
        Source::Rule {
            rules,
            stores,
            scan,
        } => (scan, fleet::generate(&rules, &stores)),
        Source::Paste { text } => (false, fleet::parse_paste(&text)),
        Source::File { name, bytes } => (false, import::read(&name, &bytes)?),
        Source::Batch { batch } => (false, batch),
    };
    if rules_scan {
        let ports = {
            let db = lock(&state)?;
            profile_ports(&db, chain_id)?
        };
        batch = tauri::async_runtime::spawn_blocking(move || keep_responding(batch, &ports))
            .await
            .map_err(|error| format!("A varredura foi interrompida: {error}"))?;
    }
    let db = lock(&state)?;
    let read = |error| explain(&error, "");
    let (stores, devices, rules, profiles) = (
        db.stores(chain_id).map_err(read)?,
        db.devices(chain_id).map_err(read)?,
        db.rules(chain_id).map_err(read)?,
        db.profiles(chain_id).map_err(read)?,
    );
    Ok(fleet::plan(
        batch,
        &Fleet {
            stores: &stores,
            devices: &devices,
            rules: &rules,
            profiles: &profiles,
        },
    ))
}

fn profile_ports(db: &Db, chain_id: i64) -> CommandResult<HashMap<i64, u16>> {
    Ok(db
        .profiles(chain_id)
        .map_err(|error| explain(&error, ""))?
        .into_iter()
        .map(|profile| (profile.id, profile.port))
        .collect())
}

/// Varre cada entrada na porta do seu perfil e descarta quem não respondeu.
fn keep_responding(mut batch: Batch, ports: &HashMap<i64, u16>) -> Batch {
    let port_of = |profile_id: Option<i64>| {
        profile_id
            .and_then(|id| ports.get(&id).copied())
            .unwrap_or(DEFAULT_PORT)
    };
    let mut by_port: HashMap<u16, Vec<String>> = HashMap::new();
    for entry in &batch.entries {
        by_port
            .entry(port_of(entry.profile_id))
            .or_default()
            .push(entry.host.clone());
    }
    let open: std::collections::HashSet<(u16, String)> = by_port
        .into_iter()
        .flat_map(|(port, hosts)| {
            scan::scan(&hosts, port, SCAN_TIMEOUT, SCAN_CONCURRENCY)
                .into_iter()
                .filter(|hit| hit.open)
                .map(move |hit| (port, hit.host))
        })
        .collect();
    let total = batch.entries.len();
    batch
        .entries
        .retain(|entry| open.contains(&(port_of(entry.profile_id), entry.host.clone())));
    batch.notices.push(format!(
        "A varredura encontrou {} de {total} endereços respondendo.",
        batch.entries.len()
    ));
    batch
}

#[tauri::command]
pub fn apply_batch(
    state: State<'_, AppState>,
    chain_id: i64,
    plan: Plan,
) -> CommandResult<Applied> {
    lock(&state)?.apply_plan(chain_id, &plan).map_err(|error| {
        explain(
            &error,
            "Algum endereço já foi cadastrado; gere a prévia de novo.",
        )
    })
}

#[tauri::command]
pub async fn scan_range(range: String, port: u16) -> CommandResult<Vec<ScanHit>> {
    let hosts = fleet::expand_range(&range)?;
    if port == 0 {
        return Err("Informe uma porta entre 1 e 65535.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        scan::scan(&hosts, port, SCAN_TIMEOUT, SCAN_CONCURRENCY)
    })
    .await
    .map_err(|error| format!("A varredura foi interrompida: {error}"))
}

/// Varre os equipamentos de uma loja na porta do primeiro perfil de cada um.
#[tauri::command]
pub async fn scan_store(
    state: State<'_, AppState>,
    chain_id: i64,
    store_id: i64,
) -> CommandResult<Vec<ScanHit>> {
    let (targets, ports) = {
        let db = lock(&state)?;
        let devices = db.devices(chain_id).map_err(|error| explain(&error, ""))?;
        (
            devices
                .into_iter()
                .filter(|device| device.store_id == store_id)
                .collect::<Vec<_>>(),
            profile_ports(&db, chain_id)?,
        )
    };
    tauri::async_runtime::spawn_blocking(move || {
        let mut by_port: HashMap<u16, Vec<String>> = HashMap::new();
        for device in &targets {
            let port = device
                .profile_ids
                .first()
                .and_then(|id| ports.get(id).copied())
                .unwrap_or(DEFAULT_PORT);
            by_port.entry(port).or_default().push(device.host.clone());
        }
        by_port
            .into_iter()
            .flat_map(|(port, hosts)| scan::scan(&hosts, port, SCAN_TIMEOUT, SCAN_CONCURRENCY))
            .collect()
    })
    .await
    .map_err(|error| format!("A varredura foi interrompida: {error}"))
}
