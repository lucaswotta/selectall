//! Testes contra um SQL Server real. Rode com `scripts/test-sqlserver.sh`, que cria o
//! servidor descartável, o usuário somente leitura, o usuário de escrita e as tabelas.

mod real;

use real::{Server, cancel_after_start, closed_port, run, write};
use selectall_core::correction::Correction;
use selectall_core::exec::{Options, Run, Status, Step};
use selectall_core::model::Engine;
use std::time::{Duration, Instant};

const MS: Server = Server {
    engine: Engine::Sqlserver,
    prefix: "SQLSERVER",
    database: "pdv",
};

/// Bilhões de combinações. `CHECKSUM` das três tabelas impede o otimizador de
/// calcular o resultado sem percorrê-las, como faria com `COUNT(*)`.
const SLOW: &str = "SELECT MAX(CHECKSUM(a.name, b.name, c.name)) FROM sys.all_objects a CROSS JOIN sys.all_objects b CROSS JOIN sys.all_objects c";

#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn reads_typed_values_and_several_result_sets() {
    let outcomes = run(
        vec![MS.good(1)],
        "SELECT * FROM parametros ORDER BY qtd_max_itens DESC; SELECT COUNT(*) AS total FROM parametros",
        &Options::default(),
    );
    let outcome = &outcomes[0];
    assert_eq!(outcome.status, Status::Success, "{:?}", outcome.failure);
    assert_eq!(outcome.profile.as_deref(), Some("Leitura"));
    assert_eq!(outcome.sets.len(), 2);
    let set = &outcome.sets[0];
    let numeric: Vec<bool> = set.columns.iter().map(|c| c.numeric).collect();
    assert_eq!(
        numeric,
        [true, false, true, true, false, false, false, false]
    );
    assert_eq!(
        set.rows[0],
        [
            Some("101".into()),
            Some("7.12.4".into()),
            Some("999".into()),
            Some("12.50".into()),
            Some("2026-11-21".into()),
            Some("2026-11-21 08:14:03.000120".into()),
            None,
            Some("1".into()),
        ]
    );
    assert_eq!(set.rows[1][5].as_deref(), Some("2026-11-20 23:59:59"));
    assert_eq!(set.rows[1][6].as_deref(), Some("ação"));
    assert_eq!(set.rows[1][7].as_deref(), Some("0"));
    assert_eq!(outcome.sets[1].rows[0][0].as_deref(), Some("2"));
}

#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn falls_back_to_the_next_profile_when_the_password_is_refused() {
    let password = MS.env("PASSWORD");
    let outcomes = run(
        vec![MS.target(
            1,
            vec![
                MS.credential("Consinco", "errada"),
                MS.credential("Leitura", &password),
            ],
        )],
        "SELECT 1",
        &Options::default(),
    );
    assert_eq!(
        outcomes[0].status,
        Status::Success,
        "{:?}",
        outcomes[0].failure
    );
    assert_eq!(outcomes[0].profile.as_deref(), Some("Leitura"));

    let refused = run(
        vec![MS.target(
            2,
            vec![
                MS.credential("Consinco", "errada"),
                MS.credential("Leitura", ""),
            ],
        )],
        "SELECT 1",
        &Options::default(),
    );
    assert_eq!(refused[0].status, Status::Failed);
    assert_eq!(
        refused[0].failure.as_ref().unwrap().message,
        "Usuário ou senha recusados pelos perfis Consinco e Leitura."
    );
}

#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn writes_hidden_in_a_read_are_refused_before_connecting() {
    for sql in [
        "DELETE FROM parametros",
        "SELECT 1 DELETE FROM parametros",
        "SELECT * INTO copia FROM parametros",
        "SELECT 1 EXEC sp_who",
    ] {
        assert!(
            Run::start(vec![MS.good(1)], sql, &Options::default()).is_err(),
            "{sql}"
        );
    }
    let count = run(
        vec![MS.good(1)],
        "SELECT COUNT(*) FROM parametros",
        &Options::default(),
    );
    assert_eq!(count[0].sets[0].rows[0][0].as_deref(), Some("2"));
}

#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn truncates_at_the_row_limit() {
    let outcomes = run(
        vec![MS.good(1)],
        "SELECT name FROM sys.all_objects",
        &Options {
            row_limit: 5,
            ..Options::default()
        },
    );
    let set = &outcomes[0].sets[0];
    assert_eq!(set.rows.len(), 5);
    assert!(set.truncated);
}

#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn missing_objects_are_named() {
    let outcomes = run(
        vec![MS.good(1)],
        "SELECT * FROM nao_existe",
        &Options::default(),
    );
    assert_eq!(
        outcomes[0].failure.as_ref().unwrap().message,
        "Tabela não encontrada: nao_existe."
    );
}

#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn deadline_interrupts_a_slow_query() {
    let start = Instant::now();
    let outcomes = run(
        vec![MS.good(1)],
        SLOW,
        &Options {
            deadline: Duration::from_secs(1),
            ..Options::default()
        },
    );
    assert_eq!(
        outcomes[0].status,
        Status::TimedOut,
        "{:?}",
        outcomes[0].failure
    );
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

/// O usuário de leitura não tem `ALTER ANY CONNECTION`: o `KILL` é recusado e a
/// conexão é encerrada deste lado.
#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn cancel_stops_running_and_queued_devices() {
    let run = Run::start(
        (1..=3).map(|id| MS.good(id)).collect(),
        SLOW,
        &Options {
            concurrency: 1,
            ..Options::default()
        },
    )
    .unwrap();
    let start = Instant::now();
    let (outcomes, skipped) = cancel_after_start(&run, Duration::from_millis(300));
    let statuses: Vec<Status> = outcomes.iter().map(|o| o.status).collect();
    assert_eq!(statuses, [Status::Cancelled]);
    assert_eq!(skipped, 2);
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
}

#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn offline_device_does_not_hold_the_others() {
    let mut offline = MS.credential("Leitura", "x");
    offline.port = closed_port();
    let outcomes = run(
        vec![MS.target(1, vec![offline]), MS.good(2), MS.good(3)],
        "SELECT versao_pdv FROM parametros",
        &Options::default(),
    );
    let status = |id| outcomes.iter().find(|o| o.device_id == id).unwrap().status;
    assert_eq!(status(1), Status::Offline);
    assert_eq!(status(2), Status::Success);
    assert_eq!(status(3), Status::Success);
}

#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn scan_reports_sqlserver_version_without_credentials() {
    let hosts = vec![MS.env("HOST")];
    let hits = selectall_core::scan::scan(&hosts, MS.port(), Duration::from_secs(2), 4);
    assert!(hits[0].open);
    assert_eq!(hits[0].engine, Some(Engine::Sqlserver));
    let version = hits[0].version.as_deref().unwrap_or_default();
    assert!(version.starts_with("16."), "versão anunciada: {version:?}");
}

#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn rehearsal_counts_rows_per_device_and_writes_nothing() {
    let outcomes = write(
        vec![MS.writer(1, "loja1"), MS.writer(2, "loja2")],
        "UPDATE produto SET bloqueado = 1 WHERE cod IN (1, 2); DELETE FROM produto WHERE cod = 5",
        Step::Rehearsal,
    );
    for outcome in &outcomes {
        assert_eq!(outcome.status, Status::Rehearsed, "{:?}", outcome.failure);
        assert_eq!(outcome.affected_rows, 3);
    }
    for database in ["loja1", "loja2"] {
        assert_eq!(MS.blocked(database, 1), "0");
        assert_eq!(MS.value(database, "SELECT COUNT(*) FROM produto"), "5");
    }
}

#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn pilot_writes_only_the_chosen_device_and_rollout_the_rest() {
    let sql = "UPDATE produto SET bloqueado = 1 WHERE cod = 3";
    let mut correction = Correction::new(sql);
    for outcome in write(
        vec![MS.writer(1, "loja1"), MS.writer(2, "loja2")],
        sql,
        Step::Rehearsal,
    ) {
        correction.record(Step::Rehearsal, &outcome);
    }
    correction.check_pilot(&[1]).unwrap();
    let pilot = write(vec![MS.writer(1, "loja1")], sql, Step::Pilot);
    assert_eq!(pilot[0].status, Status::Applied, "{:?}", pilot[0].failure);
    assert_eq!(pilot[0].affected_rows, 1);
    correction.record(Step::Pilot, &pilot[0]);
    assert_eq!(MS.blocked("loja1", 3), "1");
    assert_eq!(MS.blocked("loja2", 3), "0");

    assert_eq!(correction.rollout(&[]).unwrap(), [2]);
    let rollout = write(vec![MS.writer(2, "loja2")], sql, Step::Rollout);
    assert_eq!(
        rollout[0].status,
        Status::Applied,
        "{:?}",
        rollout[0].failure
    );
    assert_eq!(MS.blocked("loja2", 3), "1");
}

#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn a_failing_statement_undoes_the_whole_device() {
    let outcomes = write(
        vec![MS.writer(1, "loja1")],
        "UPDATE produto SET bloqueado = 1 WHERE cod = 4; UPDATE nao_existe SET a = 1 WHERE b = 1",
        Step::Pilot,
    );
    assert_eq!(outcomes[0].status, Status::Failed);
    let message = &outcomes[0].failure.as_ref().unwrap().message;
    assert!(
        message.contains("nao_existe") && message.contains("desfeita"),
        "{message}"
    );
    assert_eq!(MS.blocked("loja1", 4), "0");
}

#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn unsafe_corrections_are_blocked_before_connecting() {
    for sql in [
        "UPDATE produto SET bloqueado = 1",
        "DELETE FROM produto WHERE 1 = 1",
        "DELETE FROM produto WHERE N'a' = N'a'",
        "DROP TABLE produto",
        "UPDATE produto SET bloqueado = 1 WHERE cod = 1 DELETE FROM produto",
        "UPDATE produto SET bloqueado = 1 WHERE cod = 1\nGO",
    ] {
        assert!(
            Run::start_write(
                vec![MS.writer(1, "loja1")],
                sql,
                Step::Rehearsal,
                &Options::default()
            )
            .is_err(),
            "{sql}"
        );
    }
    assert_eq!(MS.value("loja1", "SELECT COUNT(*) FROM produto"), "5");
}

/// O usuário de escrita tem `ALTER ANY CONNECTION`: o cancelamento usa `KILL`.
#[test]
#[ignore = "precisa de SQL Server: scripts/test-sqlserver.sh"]
fn cancelling_a_pilot_undoes_its_transaction() {
    let run = Run::start_write(
        vec![MS.writer(2, "loja2")],
        &format!("UPDATE produto SET bloqueado = 1 WHERE cod = 5 AND ({SLOW}) IS NOT NULL"),
        Step::Pilot,
        &Options::default(),
    )
    .unwrap();
    let started = Instant::now();
    let (outcomes, _) = cancel_after_start(&run, Duration::from_millis(500));
    assert_eq!(
        outcomes[0].status,
        Status::Cancelled,
        "{:?}",
        outcomes[0].failure
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(MS.blocked("loja2", 5), "0");
}
