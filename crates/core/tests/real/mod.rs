//! Apoio aos testes contra servidores reais criados por `scripts/test-<motor>.sh`.
//! As variáveis seguem `SELECTALL_TEST_<MOTOR>_{HOST,PORT,USER,PASSWORD,WRITE_USER,WRITE_PASSWORD}`.

#![allow(dead_code)]

use selectall_core::exec::{Credential, Event, Options, Outcome, Run, Status, Step, Target};
use selectall_core::model::Engine;

pub struct Server {
    pub engine: Engine,
    pub prefix: &'static str,
    /// Banco do usuário de leitura.
    pub database: &'static str,
}

impl Server {
    pub fn env(&self, name: &str) -> String {
        let key = format!("SELECTALL_TEST_{}_{name}", self.prefix);
        std::env::var(&key).unwrap_or_else(|_| {
            panic!(
                "{key} não definido; use scripts/test-{}.sh",
                self.engine.as_str()
            )
        })
    }

    pub fn port(&self) -> u16 {
        self.env("PORT").parse().unwrap()
    }

    pub fn credential(&self, name: &str, password: &str) -> Credential {
        Credential {
            profile_id: 1,
            profile_name: name.into(),
            engine: self.engine,
            port: self.port(),
            database: self.database.into(),
            username: self.env("USER"),
            password: password.into(),
            tls: false,
        }
    }

    pub fn target(&self, device_id: i64, credentials: Vec<Credential>) -> Target {
        Target {
            device_id,
            store_id: 1,
            host: self.env("HOST"),
            credentials,
        }
    }

    pub fn good(&self, device_id: i64) -> Target {
        self.target(
            device_id,
            vec![self.credential("Leitura", &self.env("PASSWORD"))],
        )
    }

    /// Cada equipamento simulado é um banco (`loja1`, `loja2`) no mesmo servidor.
    pub fn writer(&self, device_id: i64, database: &str) -> Target {
        self.target(
            device_id,
            vec![Credential {
                database: database.into(),
                username: self.env("WRITE_USER"),
                ..self.credential("Escrita", &self.env("WRITE_PASSWORD"))
            }],
        )
    }

    pub fn value(&self, database: &str, sql: &str) -> String {
        let outcome = run(vec![self.writer(1, database)], sql, &Options::default()).remove(0);
        assert_eq!(outcome.status, Status::Success, "{:?}", outcome.failure);
        outcome.sets[0].rows[0][0].clone().unwrap_or_default()
    }

    pub fn blocked(&self, database: &str, cod: i64) -> String {
        self.value(
            database,
            &format!("SELECT bloqueado FROM produto WHERE cod = {cod}"),
        )
    }
}

pub fn finished(run: &Run) -> Vec<Outcome> {
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

pub fn run(targets: Vec<Target>, sql: &str, options: &Options) -> Vec<Outcome> {
    finished(&Run::start(targets, sql, options).unwrap())
}

pub fn write(targets: Vec<Target>, sql: &str, step: Step) -> Vec<Outcome> {
    finished(&Run::start_write(targets, sql, step, &Options::default()).unwrap())
}

/// Porta local livre e fechada: o equipamento simulado está desligado.
pub fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Cancela assim que o primeiro equipamento começa; devolve os status e quantos foram pulados.
pub fn cancel_after_start(run: &Run, wait: std::time::Duration) -> (Vec<Outcome>, usize) {
    let mut cancelled = false;
    let mut outcomes = Vec::new();
    let mut skipped = 0;
    for event in &run.events {
        match event {
            Event::Running { .. } if !cancelled => {
                std::thread::sleep(wait);
                run.cancel();
                cancelled = true;
            }
            Event::Finished(outcome) => outcomes.push(outcome),
            Event::Skipped { .. } => skipped += 1,
            _ => {}
        }
    }
    (outcomes, skipped)
}

/// Valor hostil de parâmetro: precisa voltar idêntico, fora e dentro de aspas,
/// sem fechar o texto nem virar comando.
pub fn parameter_values_stay_literal(server: &Server) {
    let value = r"O'Brien \' ; SELECT 1 -- ção \";
    let values = [("v".to_owned(), value.to_owned())].into_iter().collect();
    let script = selectall_core::params::Script::new("SELECT {{v}} AS v, 'x{{v}}' AS w", values);
    let outcomes =
        finished(&Run::start(vec![server.good(1)], script, &Options::default()).unwrap());
    let outcome = &outcomes[0];
    assert_eq!(outcome.status, Status::Success, "{:?}", outcome.failure);
    assert_eq!(
        outcome.sets[0].rows,
        [vec![Some(value.to_owned()), Some(format!("x{value}"))]]
    );
}
