//! Comandos das Consultas salvas e do Histórico.

use crate::{AppState, CommandResult, explain, lock};
use selectall_core::grid;
use selectall_core::history::{HistoryEntry, Limits, Usage};
use selectall_core::library::{self, SavedQuery, SavedView};
use tauri::{AppHandle, Manager, State};

const DUPLICATE_NAME: &str = "Já existe uma consulta com esse nome nesta rede.";

#[tauri::command]
pub fn list_saved(state: State<'_, AppState>, chain_id: i64) -> CommandResult<Vec<SavedView>> {
    let db = lock(&state)?;
    let read = |error| explain(&error, "");
    let profiles = db.profiles(chain_id).map_err(read)?;
    Ok(db
        .saved_queries(chain_id)
        .map_err(read)?
        .into_iter()
        .map(|query| library::describe(query, &profiles))
        .collect())
}

/// Grava uma consulta nova (`id` 0) ou altera a existente.
#[tauri::command]
pub fn save_saved(state: State<'_, AppState>, query: SavedQuery) -> CommandResult<SavedView> {
    let db = lock(&state)?;
    let profiles = db
        .profiles(query.chain_id)
        .map_err(|error| explain(&error, ""))?;
    if query
        .profile_id
        .is_some_and(|id| !profiles.iter().any(|p| p.id == id))
    {
        return Err("O perfil escolhido não existe mais nesta rede.".into());
    }
    library::validate(&query, &profiles)?;
    let id = db
        .save_query(&query)
        .map_err(|error| explain(&error, DUPLICATE_NAME))?;
    let saved = SavedQuery {
        id,
        name: query.name.trim().to_owned(),
        description: query.description.trim().to_owned(),
        ..query
    };
    Ok(library::describe(saved, &profiles))
}

#[tauri::command]
pub fn delete_saved(state: State<'_, AppState>, id: i64) -> CommandResult<()> {
    lock(&state)?
        .delete_query(id)
        .map_err(|error| explain(&error, ""))
}

/// Grava a biblioteca da rede em Downloads e devolve o caminho.
#[tauri::command]
pub fn export_library(
    app: AppHandle,
    state: State<'_, AppState>,
    chain_id: i64,
    stem: &str,
) -> CommandResult<String> {
    if stem.is_empty()
        || !stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Nome de arquivo inválido.".into());
    }
    let text = {
        let db = lock(&state)?;
        let read = |error| explain(&error, "");
        let queries = db.saved_queries(chain_id).map_err(read)?;
        if queries.is_empty() {
            return Err("A biblioteca desta rede está vazia.".into());
        }
        library::to_json(&queries, &db.profiles(chain_id).map_err(read)?)?
    };
    let dir = app
        .path()
        .download_dir()
        .map_err(|_| "Não foi encontrada a pasta Downloads deste usuário.".to_owned())?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Não foi possível criar {}: {error}", dir.display()))?;
    let path = grid::free_path(&dir, stem, "json");
    std::fs::write(&path, text)
        .map_err(|error| format!("Não foi possível gravar {}: {error}", path.display()))?;
    Ok(path.display().to_string())
}

/// Quantas consultas entraram novas e quantas substituíram uma de mesmo nome.
#[derive(serde::Serialize)]
pub struct Imported {
    added: usize,
    replaced: usize,
}

/// Importa o arquivo exportado por outra instalação: tudo ou nada.
#[tauri::command]
pub fn import_library(
    state: State<'_, AppState>,
    chain_id: i64,
    text: &str,
) -> CommandResult<Imported> {
    let mut db = lock(&state)?;
    let profiles = db.profiles(chain_id).map_err(|error| explain(&error, ""))?;
    let queries = library::from_json(text, chain_id, &profiles)?;
    if queries.is_empty() {
        return Err("O arquivo não tem consultas.".into());
    }
    let mut names: Vec<&str> = queries.iter().map(|q| q.name.as_str()).collect();
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("O arquivo tem duas consultas com o mesmo nome.".into());
    }
    let (added, replaced) = db
        .import_queries(&queries)
        .map_err(|error| explain(&error, DUPLICATE_NAME))?;
    Ok(Imported { added, replaced })
}

#[tauri::command]
pub fn list_history(state: State<'_, AppState>, chain_id: i64) -> CommandResult<Vec<HistoryEntry>> {
    lock(&state)?
        .history(chain_id)
        .map_err(|error| explain(&error, ""))
}

#[tauri::command]
pub fn history_usage(state: State<'_, AppState>) -> CommandResult<Usage> {
    lock(&state)?
        .history_usage()
        .map_err(|error| explain(&error, ""))
}

/// Muda os limites e já apaga o que passou deles.
#[tauri::command]
pub fn set_history_limits(state: State<'_, AppState>, limits: Limits) -> CommandResult<Usage> {
    if limits.days == 0 || limits.megabytes == 0 {
        return Err("Os limites precisam ser maiores que zero.".into());
    }
    let db = lock(&state)?;
    let read = |error| explain(&error, "");
    db.set_history_limits(limits).map_err(read)?;
    db.history_usage().map_err(read)
}

#[tauri::command]
pub fn per_store_limit(state: State<'_, AppState>) -> CommandResult<usize> {
    lock(&state)?
        .per_store_limit()
        .map_err(|error| explain(&error, ""))
}

#[tauri::command]
pub fn set_per_store_limit(state: State<'_, AppState>, limit: usize) -> CommandResult<usize> {
    let db = lock(&state)?;
    let read = |error| explain(&error, "");
    db.set_per_store_limit(limit).map_err(read)?;
    db.per_store_limit().map_err(read)
}

#[tauri::command]
pub fn delete_history(state: State<'_, AppState>, id: i64) -> CommandResult<()> {
    lock(&state)?
        .delete_history(id)
        .map_err(|error| explain(&error, ""))
}
