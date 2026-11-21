// Sem janela de console no Windows em release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Tauri injeta `State` por valor nos comandos.
#![allow(clippy::needless_pass_by_value)]

mod fleet;

use selectall_core::model::Chain;
use selectall_core::store::{self, Db};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{Manager, State};

pub(crate) struct AppState {
    db: Mutex<Db>,
    data_path: String,
}

pub(crate) type CommandResult<T> = Result<T, String>;

/// Converte erros do SQLite em mensagens para a pessoa, sem expor SQL.
pub(crate) fn explain(error: &store::Error, duplicate: &str) -> String {
    match error {
        store::Error::QueryReturnedNoRows => {
            "O item não existe mais. Feche e abra a tela para recarregar.".to_owned()
        }
        store::Error::SqliteFailure(failure, Some(text))
            if failure.code == store::ErrorCode::ConstraintViolation && text.contains("UNIQUE") =>
        {
            duplicate.to_owned()
        }
        store::Error::SqliteFailure(failure, Some(text))
            if failure.code == store::ErrorCode::ConstraintViolation && text.contains("CHECK") =>
        {
            "Preencha os campos obrigatórios.".to_owned()
        }
        other => format!("Não foi possível gravar no banco local: {other}"),
    }
}

pub(crate) fn lock<'a>(
    state: &'a State<'_, AppState>,
) -> CommandResult<std::sync::MutexGuard<'a, Db>> {
    state
        .db
        .lock()
        .map_err(|_| "O banco local ficou indisponível. Reinicie o aplicativo.".to_owned())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: &'static str,
    data_path: String,
    schema_version: usize,
}

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> CommandResult<AppInfo> {
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        data_path: state.data_path.clone(),
        schema_version: lock(&state)?
            .schema_version()
            .map_err(|error| explain(&error, ""))?,
    })
}

#[tauri::command]
fn list_chains(state: State<'_, AppState>) -> CommandResult<Vec<Chain>> {
    lock(&state)?.chains().map_err(|error| explain(&error, ""))
}

#[tauri::command]
fn add_chain(state: State<'_, AppState>, name: &str) -> CommandResult<Chain> {
    lock(&state)?
        .add_chain(name)
        .map_err(|error| explain(&error, "Já existe uma rede com esse nome."))
}

#[tauri::command]
fn get_theme(state: State<'_, AppState>) -> CommandResult<String> {
    Ok(lock(&state)?
        .setting("theme")
        .map_err(|error| explain(&error, ""))?
        .unwrap_or_else(|| "system".into()))
}

#[tauri::command]
fn set_theme(state: State<'_, AppState>, theme: &str) -> CommandResult<()> {
    if !matches!(theme, "system" | "light" | "dark") {
        return Err("Tema desconhecido.".into());
    }
    lock(&state)?
        .set_setting("theme", theme)
        .map_err(|error| explain(&error, ""))
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let path = dir.join("selectall.db");
            let db = Db::open(&path)?;
            if db.setting("install_id")?.is_none() {
                db.set_setting("install_id", &install_id())?;
            }
            app.manage(AppState {
                db: Mutex::new(db),
                data_path: path.display().to_string(),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            list_chains,
            add_chain,
            get_theme,
            set_theme,
            fleet::load_fleet,
            fleet::rename_chain,
            fleet::delete_chain,
            fleet::save_profile,
            fleet::delete_profile,
            fleet::save_store,
            fleet::delete_store,
            fleet::save_device,
            fleet::delete_device,
            fleet::save_rules,
            fleet::preview_batch,
            fleet::apply_batch,
            fleet::scan_range,
            fleet::scan_store
        ])
        .run(tauri::generate_context!())
        .expect("falha ao iniciar a janela do SelectAll");
}

/// Identifica esta instalação no cofre do sistema; não precisa ser secreto, só único.
fn install_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    hasher.write_u32(std::process::id());
    format!("{:016x}", hasher.finish())
}
