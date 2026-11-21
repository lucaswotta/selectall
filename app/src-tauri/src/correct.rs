//! Comandos do modo Corrigir e da Auditoria. Piloto e lote só seguem o ensaio
//! guardado na sessão, com o mesmo SQL.

use crate::query::{Kind, start};
use crate::{AppState, CommandResult, explain, lock};
use selectall_core::correction::{Correction, Summary};
use selectall_core::exec::Step;
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
) -> CommandResult<u64> {
    start(
        &app,
        &state,
        chain_id,
        &device_ids,
        sql,
        Kind::Write(Step::Rehearsal),
    )
}

#[tauri::command]
pub fn apply_pilot(
    app: AppHandle,
    state: State<'_, AppState>,
    chain_id: i64,
    device_ids: Vec<i64>,
) -> CommandResult<u64> {
    let sql = with_correction(&state, |correction| {
        correction.check_pilot(&device_ids)?;
        Ok(correction.sql.clone())
    })?;
    start(
        &app,
        &state,
        chain_id,
        &device_ids,
        sql,
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
    let (sql, ids) = with_correction(&state, |correction| {
        Ok((correction.sql.clone(), correction.rollout(&excluded)?))
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
        sql,
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
