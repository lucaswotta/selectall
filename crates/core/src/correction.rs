//! Regras do fluxo Corrigir: só vai ao piloto quem passou pelo ensaio, e só vai
//! ao lote depois de um piloto aplicado.

use crate::exec::{Outcome, Status, Step};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Uma correção em andamento, presa ao SQL do ensaio.
#[derive(Clone, Debug)]
pub struct Correction {
    pub sql: String,
    /// Linhas que cada equipamento alteraria, segundo o ensaio.
    rehearsed: BTreeMap<i64, u64>,
    applied: BTreeSet<i64>,
    unknown: BTreeSet<i64>,
    pilot_applied: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub sql: String,
    pub rehearsed: Vec<(i64, u64)>,
    pub median: u64,
    pub outliers: Vec<i64>,
    pub applied: Vec<i64>,
    pub unknown: Vec<i64>,
    pub pilot_applied: bool,
}

impl Correction {
    pub fn new(sql: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            rehearsed: BTreeMap::new(),
            applied: BTreeSet::new(),
            unknown: BTreeSet::new(),
            pilot_applied: false,
        }
    }

    pub fn record(&mut self, step: Step, outcome: &Outcome) {
        let id = outcome.device_id;
        match (step, outcome.status) {
            (Step::Rehearsal, Status::Rehearsed) => {
                self.rehearsed.insert(id, outcome.affected_rows);
            }
            (Step::Pilot | Step::Rollout, Status::Applied) => {
                self.applied.insert(id);
                self.unknown.remove(&id);
                self.pilot_applied |= step == Step::Pilot;
            }
            (Step::Pilot | Step::Rollout, Status::Unknown) => {
                self.unknown.insert(id);
            }
            _ => {}
        }
    }

    /// Equipamentos que ainda podem receber a correção: ensaiados, sem aplicação
    /// confirmada e sem resultado desconhecido (esses pedem conferência manual).
    fn pending(&self) -> impl Iterator<Item = i64> + '_ {
        self.rehearsed
            .keys()
            .copied()
            .filter(|id| !self.applied.contains(id) && !self.unknown.contains(id))
    }

    pub fn check_pilot(&self, ids: &[i64]) -> Result<(), String> {
        if ids.is_empty() {
            return Err("Escolha o equipamento piloto.".into());
        }
        if self.pilot_applied {
            return Err("O piloto já foi aplicado. Siga para o lote.".into());
        }
        let pending: BTreeSet<i64> = self.pending().collect();
        if ids.iter().all(|id| pending.contains(id)) {
            Ok(())
        } else {
            Err("O piloto precisa ser um equipamento que passou pelo ensaio.".into())
        }
    }

    /// O lote: o restante do ensaio, menos os tirados à mão.
    pub fn rollout(&self, excluded: &[i64]) -> Result<Vec<i64>, String> {
        if !self.pilot_applied {
            return Err("Aplique e confira o piloto antes do lote.".into());
        }
        let ids: Vec<i64> = self.pending().filter(|id| !excluded.contains(id)).collect();
        if ids.is_empty() {
            return Err("Não há equipamentos restantes para o lote.".into());
        }
        Ok(ids)
    }

    pub fn summary(&self) -> Summary {
        let median = median(self.rehearsed.values().copied());
        Summary {
            sql: self.sql.clone(),
            rehearsed: self
                .rehearsed
                .iter()
                .map(|(id, rows)| (*id, *rows))
                .collect(),
            median,
            outliers: self
                .rehearsed
                .iter()
                .filter(|(_, rows)| self.rehearsed.len() >= 3 && is_outlier(**rows, median))
                .map(|(id, _)| *id)
                .collect(),
            applied: self.applied.iter().copied().collect(),
            unknown: self.unknown.iter().copied().collect(),
            pilot_applied: self.pilot_applied,
        }
    }
}

fn median(values: impl Iterator<Item = u64>) -> u64 {
    let mut values: Vec<u64> = values.collect();
    values.sort_unstable();
    values.get(values.len() / 2).copied().unwrap_or(0)
}

/// Menos da metade da mediana, ou mais que o dobro com folga de 5 linhas.
fn is_outlier(rows: u64, median: u64) -> bool {
    rows.saturating_mul(2) < median || rows > median.saturating_mul(2).saturating_add(5)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(device_id: i64, status: Status, affected_rows: u64) -> Outcome {
        Outcome {
            device_id,
            profile: None,
            sets: Vec::new(),
            status,
            failure: None,
            elapsed_ms: 0,
            affected_rows,
        }
    }

    fn rehearsed(rows: &[(i64, Status, u64)]) -> Correction {
        let mut correction = Correction::new("UPDATE t SET a = 1 WHERE b = 2");
        for (id, status, affected) in rows {
            correction.record(Step::Rehearsal, &outcome(*id, *status, *affected));
        }
        correction
    }

    #[test]
    fn only_rehearsed_devices_go_to_pilot_and_rollout() {
        let mut correction = rehearsed(&[
            (1, Status::Rehearsed, 6),
            (2, Status::Rehearsed, 6),
            (3, Status::Offline, 0),
            (4, Status::Rehearsed, 7),
        ]);
        assert!(correction.check_pilot(&[3]).is_err());
        assert!(correction.check_pilot(&[]).is_err());
        assert_eq!(
            correction.rollout(&[]).unwrap_err(),
            "Aplique e confira o piloto antes do lote."
        );
        assert_eq!(correction.check_pilot(&[1]), Ok(()));
        correction.record(Step::Pilot, &outcome(1, Status::Applied, 6));
        assert!(correction.check_pilot(&[2]).is_err());
        assert_eq!(correction.rollout(&[4]).unwrap(), [2]);
        assert_eq!(correction.rollout(&[]).unwrap(), [2, 4]);

        correction.record(Step::Rollout, &outcome(2, Status::Failed, 0));
        correction.record(Step::Rollout, &outcome(4, Status::Unknown, 0));
        // Falha volta ao lote; resultado desconhecido não, até ser conferido.
        assert_eq!(correction.rollout(&[]).unwrap(), [2]);
        correction.record(Step::Rollout, &outcome(2, Status::Applied, 6));
        assert!(correction.rollout(&[]).is_err());
        let summary = correction.summary();
        assert_eq!(summary.applied, [1, 2]);
        assert_eq!(summary.unknown, [4]);
    }

    #[test]
    fn counts_far_from_the_median_are_outliers() {
        let correction = rehearsed(&[
            (1, Status::Rehearsed, 6),
            (2, Status::Rehearsed, 6),
            (3, Status::Rehearsed, 7),
            (4, Status::Rehearsed, 412),
            (5, Status::Rehearsed, 0),
        ]);
        let summary = correction.summary();
        assert_eq!(summary.median, 6);
        assert_eq!(summary.outliers, [4, 5]);
        let few = rehearsed(&[(1, Status::Rehearsed, 1), (2, Status::Rehearsed, 900)]);
        assert_eq!(few.summary().outliers, Vec::<i64>::new());
    }
}
