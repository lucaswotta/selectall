//! Testes contra um MySQL real. Rode com `scripts/test-mysql.sh`, que cria o
//! servidor descartável, o usuário somente `SELECT`, o usuário de escrita e as tabelas.

use selectall_core::correction::Correction;
use selectall_core::exec::{Credential, Event, Options, Outcome, Run, Status, Step, Target};
use selectall_core::model::Engine;
use std::time::{Duration, Instant};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} não definido; use scripts/test-mysql.sh"))
}

fn credential(name: &str, password: &str) -> Credential {
    Credential {
        profile_id: 1,
        profile_name: name.into(),
        engine: Engine::Mysql,
        port: env("SELECTALL_TEST_MYSQL_PORT").parse().unwrap(),
        database: "pdv".into(),
        username: env("SELECTALL_TEST_MYSQL_USER"),
        password: password.into(),
        tls: false,
    }
}

fn target(device_id: i64, credentials: Vec<Credential>) -> Target {
    Target {
        device_id,
        host: env("SELECTALL_TEST_MYSQL_HOST"),
        credentials,
    }
}

fn good(device_id: i64) -> Target {
    target(
        device_id,
        vec![credential("Econect", &env("SELECTALL_TEST_MYSQL_PASSWORD"))],
    )
}

fn run(targets: Vec<Target>, sql: &str, options: &Options) -> Vec<Outcome> {
    let run = Run::start(targets, sql, options).unwrap();
    run.events
        .iter()
        .filter_map(|event| match event {
            Event::Finished(outcome) => Some(outcome),
            _ => None,
        })
        .collect()
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn reads_typed_values_and_several_result_sets() {
    let outcomes = run(
        vec![good(1)],
        "SELECT * FROM parametros ORDER BY qtd_max_itens DESC; SELECT COUNT(*) AS total FROM parametros",
        &Options::default(),
    );
    let outcome = &outcomes[0];
    assert_eq!(outcome.status, Status::Success, "{:?}", outcome.failure);
    assert_eq!(outcome.profile.as_deref(), Some("Econect"));
    assert_eq!(outcome.sets.len(), 2);
    let set = &outcome.sets[0];
    let numeric: Vec<bool> = set.columns.iter().map(|c| c.numeric).collect();
    assert_eq!(numeric, [true, false, true, true, false, false, false]);
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
        ]
    );
    assert_eq!(set.rows[1][5].as_deref(), Some("2026-11-20 23:59:59"));
    assert_eq!(set.rows[1][6].as_deref(), Some("ação"));
    assert_eq!(outcome.sets[1].rows[0][0].as_deref(), Some("2"));
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn falls_back_to_the_next_profile_when_the_password_is_refused() {
    let password = env("SELECTALL_TEST_MYSQL_PASSWORD");
    let outcomes = run(
        vec![target(
            1,
            vec![
                credential("Consinco", "errada"),
                credential("Econect", &password),
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
    assert_eq!(outcomes[0].profile.as_deref(), Some("Econect"));

    let refused = run(
        vec![target(
            2,
            vec![
                credential("Consinco", "errada"),
                credential("Econect", "outra"),
            ],
        )],
        "SELECT 1",
        &Options::default(),
    );
    assert_eq!(refused[0].status, Status::Failed);
    assert_eq!(
        refused[0].failure.as_ref().unwrap().message,
        "Usuário ou senha recusados pelos perfis Consinco e Econect."
    );
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn server_rejects_writes_hidden_in_a_read() {
    assert!(Run::start(vec![good(1)], "DELETE FROM parametros", &Options::default()).is_err());
    // A análise local aceita o SELECT; a função grava e o servidor recusa.
    let outcomes = run(vec![good(1)], "SELECT marca()", &Options::default());
    assert_eq!(outcomes[0].status, Status::Failed);
    assert_eq!(
        outcomes[0].failure.as_ref().unwrap().message,
        "Bloqueado: a consulta tentou alterar dados no modo leitura."
    );
    let count = run(
        vec![good(1)],
        "SELECT COUNT(*) FROM marcas",
        &Options::default(),
    );
    assert_eq!(count[0].sets[0].rows[0][0].as_deref(), Some("0"));
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn truncates_at_the_row_limit() {
    let outcomes = run(
        vec![good(1)],
        "SELECT table_name FROM information_schema.columns",
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
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn deadline_interrupts_a_slow_query() {
    let start = Instant::now();
    let outcomes = run(
        vec![good(1)],
        "SELECT SLEEP(10)",
        &Options {
            deadline: Duration::from_secs(1),
            ..Options::default()
        },
    );
    assert_eq!(outcomes[0].status, Status::TimedOut);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn cancel_stops_running_and_queued_devices() {
    let targets = (1..=3).map(good).collect();
    let run = Run::start(
        targets,
        "SELECT SLEEP(10)",
        &Options {
            concurrency: 1,
            ..Options::default()
        },
    )
    .unwrap();
    let start = Instant::now();
    let mut cancelled = false;
    let mut statuses = Vec::new();
    let mut skipped = 0;
    for event in &run.events {
        match event {
            Event::Running { .. } if !cancelled => {
                std::thread::sleep(Duration::from_millis(300));
                run.cancel();
                cancelled = true;
            }
            Event::Finished(outcome) => statuses.push(outcome.status),
            Event::Skipped { .. } => skipped += 1,
            _ => {}
        }
    }
    assert_eq!(statuses, [Status::Cancelled]);
    assert_eq!(skipped, 2);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn canceller_works_from_another_thread_while_events_are_read() {
    let run = Run::start(
        (1..=2).map(good).collect(),
        "SELECT SLEEP(10)",
        &Options {
            concurrency: 1,
            ..Options::default()
        },
    )
    .unwrap();
    let canceller = run.canceller();
    let start = Instant::now();
    let reader = std::thread::spawn(move || {
        run.events
            .iter()
            .filter_map(|event| match event {
                Event::Finished(outcome) => Some(outcome.status),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    std::thread::sleep(Duration::from_millis(800));
    canceller.cancel();
    assert_eq!(reader.join().unwrap(), [Status::Cancelled]);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn offline_device_does_not_hold_the_others() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = listener.local_addr().unwrap().port();
    drop(listener);
    let mut offline = credential("Econect", "x");
    offline.port = closed;
    let outcomes = run(
        vec![target(1, vec![offline]), good(2), good(3)],
        "SELECT versao_pdv FROM parametros",
        &Options::default(),
    );
    let status = |id| outcomes.iter().find(|o| o.device_id == id).unwrap().status;
    assert_eq!(status(1), Status::Offline);
    assert_eq!(status(2), Status::Success);
    assert_eq!(status(3), Status::Success);
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn scan_reports_server_version_without_credentials() {
    let port: u16 = env("SELECTALL_TEST_MYSQL_PORT").parse().unwrap();
    let hosts = vec![env("SELECTALL_TEST_MYSQL_HOST")];
    let hits = selectall_core::scan::scan(&hosts, port, Duration::from_secs(2), 4);
    assert!(hits[0].open);
    let version = hits[0].version.as_deref().unwrap_or_default();
    assert!(version.starts_with('8'), "versão anunciada: {version:?}");
}

/// Cada equipamento simulado é um banco (`loja1`, `loja2`) no mesmo servidor.
fn writer(device_id: i64, database: &str) -> Target {
    target(
        device_id,
        vec![Credential {
            database: database.into(),
            username: env("SELECTALL_TEST_MYSQL_WRITE_USER"),
            ..credential("Escrita", &env("SELECTALL_TEST_MYSQL_WRITE_PASSWORD"))
        }],
    )
}

fn write(targets: Vec<Target>, sql: &str, step: Step) -> Vec<Outcome> {
    let run = Run::start_write(targets, sql, step, &Options::default()).unwrap();
    let mut outcomes: Vec<Outcome> = run
        .events
        .iter()
        .filter_map(|event| match event {
            Event::Finished(outcome) => Some(outcome),
            _ => None,
        })
        .collect();
    outcomes.sort_by_key(|outcome| outcome.device_id);
    outcomes
}

fn value(database: &str, sql: &str) -> String {
    let outcome = run(vec![writer(1, database)], sql, &Options::default()).remove(0);
    assert_eq!(outcome.status, Status::Success, "{:?}", outcome.failure);
    outcome.sets[0].rows[0][0].clone().unwrap_or_default()
}

fn blocked(database: &str, cod: i64) -> String {
    value(
        database,
        &format!("SELECT bloqueado FROM produto WHERE cod = {cod}"),
    )
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn rehearsal_counts_rows_per_device_and_writes_nothing() {
    let outcomes = write(
        vec![writer(1, "loja1"), writer(2, "loja2")],
        "UPDATE produto SET bloqueado = 1 WHERE cod IN (1, 2); DELETE FROM produto WHERE cod = 5",
        Step::Rehearsal,
    );
    for outcome in &outcomes {
        assert_eq!(outcome.status, Status::Rehearsed, "{:?}", outcome.failure);
        assert_eq!(outcome.affected_rows, 3);
    }
    for database in ["loja1", "loja2"] {
        assert_eq!(blocked(database, 1), "0");
        assert_eq!(blocked(database, 2), "0");
        assert_eq!(value(database, "SELECT COUNT(*) FROM produto"), "5");
    }
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn pilot_writes_only_the_chosen_device_and_rollout_the_rest() {
    let sql = "UPDATE produto SET bloqueado = 1 WHERE cod = 3";
    let mut correction = Correction::new(sql);
    for outcome in write(
        vec![writer(1, "loja1"), writer(2, "loja2")],
        sql,
        Step::Rehearsal,
    ) {
        correction.record(Step::Rehearsal, &outcome);
    }
    correction.check_pilot(&[1]).unwrap();
    let pilot = write(vec![writer(1, "loja1")], sql, Step::Pilot);
    assert_eq!(pilot[0].status, Status::Applied, "{:?}", pilot[0].failure);
    assert_eq!(pilot[0].affected_rows, 1);
    correction.record(Step::Pilot, &pilot[0]);
    assert_eq!(blocked("loja1", 3), "1");
    assert_eq!(blocked("loja2", 3), "0");

    assert_eq!(correction.rollout(&[]).unwrap(), [2]);
    let rollout = write(vec![writer(2, "loja2")], sql, Step::Rollout);
    assert_eq!(
        rollout[0].status,
        Status::Applied,
        "{:?}",
        rollout[0].failure
    );
    assert_eq!(blocked("loja2", 3), "1");
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn a_failing_statement_undoes_the_whole_device() {
    let outcomes = write(
        vec![writer(1, "loja1")],
        "UPDATE produto SET bloqueado = 1 WHERE cod = 4; UPDATE nao_existe SET a = 1 WHERE b = 1",
        Step::Pilot,
    );
    assert_eq!(outcomes[0].status, Status::Failed);
    let message = &outcomes[0].failure.as_ref().unwrap().message;
    assert!(
        message.contains("nao_existe") && message.contains("desfeita"),
        "{message}"
    );
    assert_eq!(blocked("loja1", 4), "0");
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn tables_without_transactions_are_refused_before_any_write() {
    let outcomes = write(
        vec![writer(1, "loja1")],
        "UPDATE contador SET n = n + 1 WHERE id = 1",
        Step::Rehearsal,
    );
    assert_eq!(outcomes[0].status, Status::Failed);
    let message = &outcomes[0].failure.as_ref().unwrap().message;
    assert!(message.contains("loja1.contador usa MyISAM"), "{message}");
    assert_eq!(value("loja1", "SELECT n FROM contador WHERE id = 1"), "0");
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn unsafe_corrections_are_blocked_before_connecting() {
    for sql in [
        "UPDATE produto SET bloqueado = 1",
        "DELETE FROM produto WHERE 1 = 1",
        "DROP TABLE produto",
        "ALTER TABLE produto ADD c INT",
    ] {
        assert!(
            Run::start_write(
                vec![writer(1, "loja1")],
                sql,
                Step::Rehearsal,
                &Options::default()
            )
            .is_err(),
            "{sql}"
        );
    }
    assert_eq!(value("loja1", "SELECT COUNT(*) FROM produto"), "5");
}

#[test]
#[ignore = "precisa de MySQL: scripts/test-mysql.sh"]
fn cancelling_a_pilot_undoes_its_transaction() {
    let run = Run::start_write(
        vec![writer(2, "loja2")],
        "UPDATE produto SET bloqueado = 1 + SLEEP(5) * 0 WHERE cod = 5",
        Step::Pilot,
        &Options::default(),
    )
    .unwrap();
    let canceller = run.canceller();
    let started = Instant::now();
    let mut outcome = None;
    for event in &run.events {
        match event {
            Event::Running { .. } => {
                std::thread::sleep(Duration::from_millis(500));
                canceller.cancel();
            }
            Event::Finished(finished) => outcome = Some(finished),
            _ => {}
        }
    }
    let outcome = outcome.unwrap();
    assert_eq!(outcome.status, Status::Cancelled, "{:?}", outcome.failure);
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(blocked("loja2", 5), "0");
}
