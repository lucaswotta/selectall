//! Comandos do modo Corrigir e da Auditoria. Piloto e lote só seguem o ensaio
//! guardado na sessão, com o mesmo SQL.

use crate::query::{Kind, start};
use crate::{AppState, CommandResult, explain, lock};
use selectall_core::correction::{Correction, Summary};
use selectall_core::exec::Step;
use selectall_core::params::{Script, Values};
use selectall_core::store::{AuditDevice, AuditEntry};
use tauri::{AppHandle, State};

const AUDIT_LIMIT: usize = 500;

fn with_correction<T>(
    state: &State<'_, AppState>,
    use_correction: impl FnOnce(&Correction) -> CommandResult<T>,
) -> CommandResult<T> {
    let query = state
        .query
        .lock()
        .map_err(|_| "A consulta ficou indisponível. Reinicie o aplicativo.".to_owned())?;
    let correction = query
        .correction
        .as_ref()
        .ok_or("Faça o ensaio antes de aplicar.")?;
    use_correction(correction)
}

/// Ensaio em todos os alvos: roda em transação e desfaz. Começa uma correção nova.
#[tauri::command]
pub fn rehearse_correction(
    app: AppHandle,
    state: State<'_, AppState>,
    chain_id: i64,
    device_ids: Vec<i64>,
    sql: String,
    values: Values,
) -> CommandResult<u64> {
    start(
        &app,
        &state,
        chain_id,
        &device_ids,
        Script::new(sql, values),
        None,
        Kind::Write(Step::Rehearsal),
    )
}

/// Pula o ensaio por escolha de quem corrige: piloto e lote valem para os
/// equipamentos escolhidos, sem prévia de linhas. Nada roda aqui; o SQL é conferido
/// contra os motores dos perfis desses equipamentos.
#[tauri::command]
pub fn skip_rehearsal(
    state: State<'_, AppState>,
    chain_id: i64,
    device_ids: Vec<i64>,
    sql: String,
    values: Values,
) -> CommandResult<()> {
    if device_ids.is_empty() {
        return Err("Escolha ao menos um equipamento.".into());
    }
    let engines = {
        let db = lock(&state)?;
        let read = |error| explain(&error, "");
        let profiles = db.profiles(chain_id).map_err(read)?;
        db.devices(chain_id)
            .map_err(read)?
            .iter()
            .filter(|device| device_ids.contains(&device.id))
            .flat_map(|device| device.profile_ids.iter())
            .filter_map(|id| profiles.iter().find(|profile| profile.id == *id))
            .map(|profile| profile.engine)
            .collect::<Vec<_>>()
    };
    let script = Script::new(sql, values);
    selectall_core::exec::check_write_engines(&engines, script.clone())?;
    let mut query = state
        .query
        .lock()
        .map_err(|_| "A consulta ficou indisponível. Reinicie o aplicativo.".to_owned())?;
    if query.running() {
        return Err("Já há uma consulta em andamento.".into());
    }
    query.correction =
        Some(Correction::without_rehearsal(script.sql, &device_ids).with_values(script.values));
    Ok(())
}

#[tauri::command]
pub fn apply_pilot(
    app: AppHandle,
    state: State<'_, AppState>,
    chain_id: i64,
    device_ids: Vec<i64>,
) -> CommandResult<u64> {
    let script = with_correction(&state, |correction| {
        correction.check_pilot(&device_ids)?;
        Ok(correction.script())
    })?;
    start(
        &app,
        &state,
        chain_id,
        &device_ids,
        script,
        None,
        Kind::Write(Step::Pilot),
    )
}

/// Lote no restante do ensaio. `confirmation` é o número de equipamentos digitado na janela.
#[tauri::command]
pub fn apply_rollout(
    app: AppHandle,
    state: State<'_, AppState>,
    chain_id: i64,
    excluded: Vec<i64>,
    confirmation: usize,
) -> CommandResult<u64> {
    let (script, ids) = with_correction(&state, |correction| {
        Ok((correction.script(), correction.rollout(&excluded)?))
    })?;
    if confirmation != ids.len() {
        return Err(format!(
            "Confirmação não confere: o lote tem {} equipamentos.",
            ids.len()
        ));
    }
    start(
        &app,
        &state,
        chain_id,
        &ids,
        script,
        None,
        Kind::Write(Step::Rollout),
    )
}

#[tauri::command]
pub fn correction_summary(state: State<'_, AppState>) -> CommandResult<Option<Summary>> {
    let query = state
        .query
        .lock()
        .map_err(|_| "A consulta ficou indisponível. Reinicie o aplicativo.".to_owned())?;
    Ok(query.correction.as_ref().map(Correction::summary))
}

/// Parar depois do piloto: esquece o ensaio, e o próximo passo volta a ser um ensaio.
#[tauri::command]
pub fn abandon_correction(state: State<'_, AppState>) {
    if let Ok(mut query) = state.query.lock() {
        query.correction = None;
    }
}

#[tauri::command]
pub fn audit_log(state: State<'_, AppState>) -> CommandResult<Vec<AuditEntry>> {
    lock(&state)?
        .audit_log(AUDIT_LIMIT)
        .map_err(|error| explain(&error, ""))
}

#[tauri::command]
pub fn audit_devices(state: State<'_, AppState>, audit_id: i64) -> CommandResult<Vec<AuditDevice>> {
    lock(&state)?
        .audit_devices(audit_id)
        .map_err(|error| explain(&error, ""))
}

/// Grava a auditoria inteira em Downloads, em CSV ou XLSX, e devolve o caminho.
#[tauri::command]
pub fn export_audit(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    format: &str,
    stem: &str,
) -> CommandResult<String> {
    crate::query::check_stem(stem)?;
    let grid = lock(&state)?
        .audit_grid()
        .map_err(|error| explain(&error, ""))?;
    if grid.rows.is_empty() {
        return Err("A auditoria está vazia.".into());
    }
    let bytes = match format {
        "csv" => selectall_core::grid::to_csv(&grid)?,
        "xlsx" => selectall_core::grid::to_xlsx(&grid)?,
        _ => return Err("Formato desconhecido.".into()),
    };
    crate::query::save_download(&app, stem, format, &bytes)
}
