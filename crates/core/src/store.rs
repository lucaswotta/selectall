//! Armazenamento local em SQLite: frota, perfis, configuração, consultas salvas,
//! histórico e auditoria. Sem senhas.

use crate::exec::Outcome;
use crate::fleet::{DeviceStatus, Plan};
use crate::grid::Origin;
use crate::history::{self, HistoryEntry, Limits, NewRun, Usage};
use crate::library::SavedQuery;
use crate::model::{AddressRule, Chain, Device, Engine, Profile, Store};
use crate::params::Values;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::path::Path;

pub use rusqlite::{Error, ErrorCode};
pub type Result<T> = rusqlite::Result<T>;

/// Cada posição é uma migração; o índice + 1 vira o `user_version` do banco.
const MIGRATIONS: &[&str] = &[
    r"
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE chains (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK (trim(name) <> '')
    );
    CREATE TABLE profiles (
        id INTEGER PRIMARY KEY,
        chain_id INTEGER NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
        name TEXT NOT NULL CHECK (trim(name) <> ''),
        engine TEXT NOT NULL CHECK (engine IN ('mysql', 'postgres', 'sqlserver')),
        port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
        database TEXT NOT NULL,
        username TEXT NOT NULL,
        tls INTEGER NOT NULL DEFAULT 0,
        UNIQUE (chain_id, name)
    );
    CREATE TABLE stores (
        id INTEGER PRIMARY KEY,
        chain_id INTEGER NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
        code TEXT NOT NULL CHECK (trim(code) <> ''),
        name TEXT NOT NULL,
        UNIQUE (chain_id, code)
    );
    CREATE TABLE devices (
        id INTEGER PRIMARY KEY,
        store_id INTEGER NOT NULL REFERENCES stores(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        kind TEXT NOT NULL,
        host TEXT NOT NULL CHECK (trim(host) <> ''),
        UNIQUE (store_id, host)
    );
    CREATE TABLE device_profiles (
        device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
        profile_id INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        PRIMARY KEY (device_id, profile_id)
    );
",
    r"
    ALTER TABLE stores ADD COLUMN subnet TEXT NOT NULL DEFAULT '';
    CREATE TABLE address_rules (
        id INTEGER PRIMARY KEY,
        chain_id INTEGER NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        kind TEXT NOT NULL CHECK (trim(kind) <> ''),
        template TEXT NOT NULL CHECK (trim(template) <> ''),
        profile_id INTEGER REFERENCES profiles(id) ON DELETE SET NULL
    );
    CREATE INDEX address_rules_chain ON address_rules (chain_id, position);
",
    // Auditoria sem chave estrangeira: apagar a rede ou o equipamento não apaga o registro.
    r"
    CREATE TABLE audit (
        id INTEGER PRIMARY KEY,
        at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
        user TEXT NOT NULL,
        chain TEXT NOT NULL,
        step TEXT NOT NULL CHECK (step IN ('rehearsal', 'pilot', 'rollout')),
        sql TEXT NOT NULL,
        planned INTEGER NOT NULL
    );
    CREATE TABLE audit_devices (
        id INTEGER PRIMARY KEY,
        audit_id INTEGER NOT NULL,
        at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
        store TEXT NOT NULL,
        device TEXT NOT NULL,
        host TEXT NOT NULL,
        profile TEXT,
        status TEXT NOT NULL,
        affected INTEGER NOT NULL,
        message TEXT
    );
    CREATE INDEX audit_devices_audit ON audit_devices (audit_id);
    CREATE TRIGGER audit_no_update BEFORE UPDATE ON audit
        BEGIN SELECT RAISE(ABORT, 'auditoria só aceita acréscimo'); END;
    CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit
        BEGIN SELECT RAISE(ABORT, 'auditoria só aceita acréscimo'); END;
    CREATE TRIGGER audit_devices_no_update BEFORE UPDATE ON audit_devices
        BEGIN SELECT RAISE(ABORT, 'auditoria só aceita acréscimo'); END;
    CREATE TRIGGER audit_devices_no_delete BEFORE DELETE ON audit_devices
        BEGIN SELECT RAISE(ABORT, 'auditoria só aceita acréscimo'); END;
",
    // Piloto e lote feitos com o ensaio pulado ficam marcados.
    r"
    ALTER TABLE audit ADD COLUMN rehearsal_skipped INTEGER NOT NULL DEFAULT 0;
",
    // Consultas salvas por rede; histórico de leituras com o resultado de cada equipamento.
    r"
    CREATE TABLE saved_queries (
        id INTEGER PRIMARY KEY,
        chain_id INTEGER NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
        name TEXT NOT NULL CHECK (trim(name) <> ''),
        description TEXT NOT NULL DEFAULT '',
        profile_id INTEGER REFERENCES profiles(id) ON DELETE SET NULL,
        sql TEXT NOT NULL CHECK (trim(sql) <> ''),
        UNIQUE (chain_id, name)
    );
    CREATE TABLE history (
        id INTEGER PRIMARY KEY,
        at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
        chain_id INTEGER NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
        label TEXT NOT NULL,
        sql TEXT,
        params TEXT NOT NULL DEFAULT '{}',
        targets INTEGER NOT NULL,
        responded INTEGER NOT NULL,
        offline INTEGER NOT NULL,
        failed INTEGER NOT NULL,
        rows INTEGER NOT NULL,
        elapsed_ms INTEGER NOT NULL,
        bytes INTEGER NOT NULL
    );
    CREATE INDEX history_chain ON history (chain_id, id);
    CREATE TABLE history_devices (
        history_id INTEGER NOT NULL REFERENCES history(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        store TEXT NOT NULL,
        device TEXT NOT NULL,
        host TEXT NOT NULL,
        outcome TEXT NOT NULL,
        PRIMARY KEY (history_id, position)
    );
",
    // A auditoria passa a guardar a rede pelo id, para a tela filtrar pela rede aberta
    // mesmo depois de um renome; registros anteriores casam pelo nome. Sem chave
    // estrangeira, como o resto da auditoria.
    r"
    ALTER TABLE audit ADD COLUMN chain_id INTEGER;
    CREATE INDEX audit_chain ON audit (chain_id, id);
",
];

/// Cabeçalho de uma etapa de correção, gravado antes do primeiro equipamento.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditStart {
    pub user: String,
    pub chain_id: i64,
    pub chain: String,
    pub step: String,
    pub sql: String,
    pub planned: usize,
    pub rehearsal_skipped: bool,
}

/// Resultado de um equipamento, acrescentado assim que ele termina.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AuditDevice {
    pub store: String,
    pub device: String,
    pub host: String,
    pub profile: Option<String>,
    pub status: String,
    pub affected: i64,
    pub message: Option<String>,
}

/// Uma etapa com os resultados somados. `statuses` conta equipamentos por status.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub at: String,
    pub user: String,
    pub chain: String,
    pub step: String,
    pub sql: String,
    pub planned: i64,
    pub rehearsal_skipped: bool,
    pub finished: i64,
    pub affected: i64,
    pub statuses: Vec<(String, i64)>,
    /// Códigos das lojas que tiveram equipamento nesta etapa.
    pub stores: Vec<String>,
}

fn audit_step_label(step: &str) -> &str {
    match step {
        "rehearsal" => "Ensaio",
        "pilot" => "Piloto",
        "rollout" => "Lote",
        other => other,
    }
}

fn audit_status_label(status: &str) -> &str {
    match status {
        "rehearsed" => "Ensaio desfeito",
        "applied" => "Aplicado",
        "unknown" => "Resultado desconhecido",
        "failed" => "Falhou",
        "offline" => "Offline",
        "cancelled" => "Cancelado",
        "timed_out" => "Prazo esgotado",
        "success" => "Respondeu",
        "partial" => "Parcial",
        other => other,
    }
}

/// Quantidades gravadas por [`Db::apply_plan`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Applied {
    pub stores: usize,
    pub devices: usize,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let mut db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<()> {
        let current = self.schema_version()?;
        if current > MIGRATIONS.len() {
            return Err(Error::InvalidQuery);
        }
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(current) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(
                None,
                "user_version",
                i64::try_from(index + 1).unwrap_or(i64::MAX),
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<usize> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok(usize::try_from(version).unwrap_or(usize::MAX))
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    pub fn chains(&self) -> Result<Vec<Chain>> {
        let mut statement = self
            .conn
            .prepare("SELECT id, name FROM chains ORDER BY name COLLATE NOCASE")?;
        statement
            .query_map([], |row| {
                Ok(Chain {
                    id: row.get(0)?,
                    name: row.get(1)?,
                })
            })?
            .collect()
    }

    pub fn add_chain(&self, name: &str) -> Result<Chain> {
        let name = name.trim();
        self.conn
            .execute("INSERT INTO chains (name) VALUES (?1)", [name])?;
        Ok(Chain {
            id: self.conn.last_insert_rowid(),
            name: name.to_owned(),
        })
    }

    pub fn rename_chain(&self, id: i64, name: &str) -> Result<Chain> {
        let name = name.trim();
        changed(self.conn.execute(
            "UPDATE chains SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?)?;
        Ok(Chain {
            id,
            name: name.to_owned(),
        })
    }

    /// Apaga a rede com lojas, equipamentos, perfis e regras. As senhas no cofre ficam com quem chama.
    pub fn delete_chain(&self, id: i64) -> Result<()> {
        changed(
            self.conn
                .execute("DELETE FROM chains WHERE id = ?1", [id])?,
        )
    }

    pub fn add_profile(&self, profile: &Profile) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO profiles (chain_id, name, engine, port, database, username, tls)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                profile.chain_id,
                profile.name.trim(),
                profile.engine.as_str(),
                profile.port,
                profile.database.trim(),
                profile.username.trim(),
                profile.tls
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// A rede do perfil não muda; `chain_id` é ignorado.
    pub fn update_profile(&self, profile: &Profile) -> Result<()> {
        changed(self.conn.execute(
            "UPDATE profiles SET name = ?2, engine = ?3, port = ?4, database = ?5, username = ?6, tls = ?7
             WHERE id = ?1",
            params![
                profile.id,
                profile.name.trim(),
                profile.engine.as_str(),
                profile.port,
                profile.database.trim(),
                profile.username.trim(),
                profile.tls
            ],
        )?)
    }

    pub fn delete_profile(&self, id: i64) -> Result<()> {
        changed(
            self.conn
                .execute("DELETE FROM profiles WHERE id = ?1", [id])?,
        )
    }

    pub fn profile(&self, id: i64) -> Result<Profile> {
        self.conn.query_row(
            "SELECT id, chain_id, name, engine, port, database, username, tls FROM profiles WHERE id = ?1",
            [id],
            profile_row,
        )
    }

    pub fn profiles(&self, chain_id: i64) -> Result<Vec<Profile>> {
        let mut statement = self.conn.prepare(
            "SELECT id, chain_id, name, engine, port, database, username, tls
             FROM profiles WHERE chain_id = ?1 ORDER BY name COLLATE NOCASE",
        )?;
        statement.query_map([chain_id], profile_row)?.collect()
    }

    pub fn add_store(&self, chain_id: i64, code: &str, name: &str, subnet: &str) -> Result<Store> {
        let (code, name, subnet) = (code.trim(), name.trim(), subnet.trim());
        self.conn.execute(
            "INSERT INTO stores (chain_id, code, name, subnet) VALUES (?1, ?2, ?3, ?4)",
            params![chain_id, code, name, subnet],
        )?;
        Ok(Store {
            id: self.conn.last_insert_rowid(),
            chain_id,
            code: code.to_owned(),
            name: name.to_owned(),
            subnet: subnet.to_owned(),
        })
    }

    /// A rede da loja não muda; `chain_id` é ignorado.
    pub fn update_store(&self, store: &Store) -> Result<()> {
        changed(self.conn.execute(
            "UPDATE stores SET code = ?2, name = ?3, subnet = ?4 WHERE id = ?1",
            params![
                store.id,
                store.code.trim(),
                store.name.trim(),
                store.subnet.trim()
            ],
        )?)
    }

    pub fn delete_store(&self, id: i64) -> Result<()> {
        changed(
            self.conn
                .execute("DELETE FROM stores WHERE id = ?1", [id])?,
        )
    }

    pub fn stores(&self, chain_id: i64) -> Result<Vec<Store>> {
        let mut statement = self.conn.prepare(
            "SELECT id, chain_id, code, name, subnet FROM stores WHERE chain_id = ?1
             ORDER BY length(code), code",
        )?;
        statement
            .query_map([chain_id], |row| {
                Ok(Store {
                    id: row.get(0)?,
                    chain_id: row.get(1)?,
                    code: row.get(2)?,
                    name: row.get(3)?,
                    subnet: row.get(4)?,
                })
            })?
            .collect()
    }

    /// Grava o equipamento e a ordem dos perfis numa única transação.
    pub fn add_device(&mut self, device: &Device) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let id = insert_device(&tx, device)?;
        tx.commit()?;
        Ok(id)
    }

    /// Troca nome, tipo, endereço e perfis; a loja do equipamento não muda.
    pub fn update_device(&mut self, device: &Device) -> Result<()> {
        let tx = self.conn.transaction()?;
        changed(tx.execute(
            "UPDATE devices SET name = ?2, kind = ?3, host = ?4 WHERE id = ?1",
            params![
                device.id,
                device.name.trim(),
                device.kind.trim(),
                device.host.trim()
            ],
        )?)?;
        tx.execute(
            "DELETE FROM device_profiles WHERE device_id = ?1",
            [device.id],
        )?;
        insert_device_profiles(&tx, device.id, &device.profile_ids)?;
        tx.commit()
    }

    pub fn delete_device(&self, id: i64) -> Result<()> {
        changed(
            self.conn
                .execute("DELETE FROM devices WHERE id = ?1", [id])?,
        )
    }

    pub fn devices(&self, chain_id: i64) -> Result<Vec<Device>> {
        let mut statement = self.conn.prepare(
            "SELECT d.id, d.store_id, d.name, d.kind, d.host,
                    (SELECT group_concat(profile_id) FROM
                        (SELECT profile_id FROM device_profiles WHERE device_id = d.id ORDER BY position))
             FROM devices d JOIN stores s ON s.id = d.store_id
             WHERE s.chain_id = ?1 ORDER BY length(s.code), s.code, d.id",
        )?;
        statement
            .query_map([chain_id], |row| {
                let profiles: Option<String> = row.get(5)?;
                Ok(Device {
                    id: row.get(0)?,
                    store_id: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    host: row.get(4)?,
                    profile_ids: profiles
                        .unwrap_or_default()
                        .split(',')
                        .filter_map(|id| id.parse().ok())
                        .collect(),
                })
            })?
            .collect()
    }

    pub fn rules(&self, chain_id: i64) -> Result<Vec<AddressRule>> {
        let mut statement = self.conn.prepare(
            "SELECT kind, template, profile_id FROM address_rules WHERE chain_id = ?1 ORDER BY position",
        )?;
        statement
            .query_map([chain_id], |row| {
                Ok(AddressRule {
                    kind: row.get(0)?,
                    template: row.get(1)?,
                    profile_id: row.get(2)?,
                })
            })?
            .collect()
    }

    /// Substitui a regra inteira da rede, na ordem dada.
    pub fn set_rules(&mut self, chain_id: i64, rules: &[AddressRule]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM address_rules WHERE chain_id = ?1", [chain_id])?;
        for (position, rule) in rules.iter().enumerate() {
            tx.execute(
                "INSERT INTO address_rules (chain_id, position, kind, template, profile_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    chain_id,
                    i64::try_from(position).unwrap_or(i64::MAX),
                    rule.kind.trim(),
                    rule.template.trim(),
                    rule.profile_id
                ],
            )?;
        }
        tx.commit()
    }

    pub fn saved_queries(&self, chain_id: i64) -> Result<Vec<SavedQuery>> {
        let mut statement = self.conn.prepare(
            "SELECT id, chain_id, name, description, profile_id, sql FROM saved_queries
             WHERE chain_id = ?1 ORDER BY name COLLATE NOCASE",
        )?;
        statement
            .query_map([chain_id], |row| {
                Ok(SavedQuery {
                    id: row.get(0)?,
                    chain_id: row.get(1)?,
                    name: row.get(2)?,
                    description: row.get(3)?,
                    profile_id: row.get(4)?,
                    sql: row.get(5)?,
                })
            })?
            .collect()
    }

    /// Grava uma consulta nova (`id` 0) ou altera a existente; devolve o `id`.
    pub fn save_query(&self, query: &SavedQuery) -> Result<i64> {
        if query.id == 0 {
            self.conn.execute(
                "INSERT INTO saved_queries (chain_id, name, description, profile_id, sql)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    query.chain_id,
                    query.name.trim(),
                    query.description.trim(),
                    query.profile_id,
                    query.sql
                ],
            )?;
            return Ok(self.conn.last_insert_rowid());
        }
        let changed = self.conn.execute(
            "UPDATE saved_queries SET name = ?2, description = ?3, profile_id = ?4, sql = ?5
             WHERE id = ?1",
            params![
                query.id,
                query.name.trim(),
                query.description.trim(),
                query.profile_id,
                query.sql
            ],
        )?;
        if changed == 0 {
            return Err(Error::QueryReturnedNoRows);
        }
        Ok(query.id)
    }

    pub fn delete_query(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM saved_queries WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Importa tudo ou nada; consulta com o mesmo nome na rede é substituída.
    /// Devolve quantas entraram novas e quantas substituíram.
    pub fn import_queries(&mut self, queries: &[SavedQuery]) -> Result<(usize, usize)> {
        let tx = self.conn.transaction()?;
        let (mut added, mut replaced) = (0, 0);
        for query in queries {
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM saved_queries WHERE chain_id = ?1 AND name = ?2",
                    params![query.chain_id, query.name.trim()],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                tx.execute(
                    "UPDATE saved_queries SET description = ?2, profile_id = ?3, sql = ?4
                     WHERE id = ?1",
                    params![id, query.description.trim(), query.profile_id, query.sql],
                )?;
                replaced += 1;
            } else {
                tx.execute(
                    "INSERT INTO saved_queries (chain_id, name, description, profile_id, sql)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        query.chain_id,
                        query.name.trim(),
                        query.description.trim(),
                        query.profile_id,
                        query.sql
                    ],
                )?;
                added += 1;
            }
        }
        tx.commit()?;
        Ok((added, replaced))
    }

    pub fn history_limits(&self) -> Result<Limits> {
        let read = |key: &str, default: u32| -> Result<u32> {
            Ok(self
                .setting(key)?
                .and_then(|value| value.parse().ok())
                .filter(|value| *value > 0)
                .unwrap_or(default))
        };
        Ok(Limits {
            days: read("history_days", history::DEFAULT_DAYS)?,
            megabytes: read("history_megabytes", history::DEFAULT_MEGABYTES)?,
        })
    }

    pub fn set_history_limits(&self, limits: Limits) -> Result<()> {
        self.set_setting("history_days", &limits.days.max(1).to_string())?;
        self.set_setting("history_megabytes", &limits.megabytes.max(1).to_string())?;
        let latest: Option<i64> =
            self.conn
                .query_row("SELECT MAX(id) FROM history", [], |row| row.get(0))?;
        self.prune_history(limits.days.max(1), limits.bytes().max(1), latest)
    }

    /// Equipamentos da mesma loja executando ao mesmo tempo (`Options::per_store`).
    pub fn per_store_limit(&self) -> Result<usize> {
        Ok(self
            .setting("per_store")?
            .and_then(|value| value.parse().ok())
            .filter(|value| (1..=crate::exec::MAX_CONCURRENCY).contains(value))
            .unwrap_or(crate::exec::DEFAULT_PER_STORE))
    }

    pub fn set_per_store_limit(&self, limit: usize) -> Result<()> {
        let limit = limit.clamp(1, crate::exec::MAX_CONCURRENCY);
        self.set_setting("per_store", &limit.to_string())
    }

    pub fn history_usage(&self) -> Result<Usage> {
        let (bytes, runs) = self.conn.query_row(
            "SELECT COALESCE(SUM(bytes), 0), COUNT(*) FROM history",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(Usage {
            limits: self.history_limits()?,
            bytes,
            runs,
        })
    }

    /// Grava a leitura e apaga o mais antigo que passou do limite, menos ela mesma.
    /// `replaces` é a execução anterior que esta substitui (a repetição das falhas).
    pub fn add_history(&mut self, run: &NewRun, replaces: Option<i64>) -> Result<i64> {
        let limits = self.history_limits()?;
        let script = history::redact(run.script.as_ref());
        let encoded: Vec<String> = run
            .devices
            .iter()
            .map(|(_, outcome)| serde_json::to_string(outcome).unwrap_or_default())
            .collect();
        let sql = script.map(|s| s.sql.as_str());
        let values = script.map_or_else(
            || "{}".to_owned(),
            |s| serde_json::to_string(&s.values).unwrap_or_else(|_| "{}".into()),
        );
        let bytes =
            encoded.iter().map(String::len).sum::<usize>() + sql.map_or(0, str::len) + values.len();
        let (responded, offline, failed) = history::counts(&run.devices);
        let rows: usize = run
            .devices
            .iter()
            .flat_map(|(_, o)| o.sets.iter().map(|set| set.rows.len()))
            .sum();
        let size = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
        let tx = self.conn.transaction()?;
        if let Some(previous) = replaces {
            tx.execute("DELETE FROM history WHERE id = ?1", [previous])?;
        }
        tx.execute(
            "INSERT INTO history (chain_id, label, sql, params, targets, responded, offline,
                                  failed, rows, elapsed_ms, bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                run.chain_id,
                history::label(run),
                sql,
                values,
                size(run.targets),
                responded,
                offline,
                failed,
                size(rows),
                i64::try_from(run.elapsed_ms).unwrap_or(i64::MAX),
                size(bytes)
            ],
        )?;
        let id = tx.last_insert_rowid();
        {
            let mut insert = tx.prepare(
                "INSERT INTO history_devices (history_id, position, store, device, host, outcome)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for ((origin, _), outcome) in run.devices.iter().zip(&encoded) {
                insert.execute(params![
                    id,
                    size(origin.order),
                    origin.store,
                    origin.device,
                    origin.host,
                    outcome
                ])?;
            }
        }
        tx.commit()?;
        self.prune_history(limits.days, limits.bytes(), Some(id))?;
        Ok(id)
    }

    /// Apaga o que passou de `days` dias e, do mais antigo para o mais novo, o que
    /// passa de `bytes`. `keep` nunca sai, nem quando sozinho passa do limite.
    fn prune_history(&self, days: u32, bytes: i64, keep: Option<i64>) -> Result<()> {
        let keep = keep.unwrap_or(0);
        self.conn.execute(
            "DELETE FROM history
             WHERE at < strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?1) AND id <> ?2",
            params![format!("-{days} days"), keep],
        )?;
        self.conn.execute(
            "DELETE FROM history WHERE id <> ?2 AND id IN (
                 SELECT id FROM (
                     SELECT id, SUM(bytes) OVER (ORDER BY (id = ?2) DESC, id DESC) AS total
                     FROM history
                 ) WHERE total > ?1
             )",
            params![bytes, keep],
        )?;
        Ok(())
    }

    fn history_entry(row: &rusqlite::Row<'_>) -> Result<HistoryEntry> {
        let values: String = row.get(4)?;
        Ok(HistoryEntry {
            id: row.get(0)?,
            at: row.get(1)?,
            label: row.get(2)?,
            sql: row.get(3)?,
            values: serde_json::from_str::<Values>(&values).unwrap_or_default(),
            targets: row.get(5)?,
            responded: row.get(6)?,
            offline: row.get(7)?,
            failed: row.get(8)?,
            rows: row.get(9)?,
            elapsed_ms: row.get(10)?,
            bytes: row.get(11)?,
            stores: Vec::new(),
        })
    }

    const HISTORY_COLUMNS: &str = "id, at, label, sql, params, targets, responded, offline,
                                   failed, rows, elapsed_ms, bytes";

    /// Execuções da rede, mais recentes primeiro.
    pub fn history(&self, chain_id: i64) -> Result<Vec<HistoryEntry>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {} FROM history WHERE chain_id = ?1 ORDER BY id DESC",
            Self::HISTORY_COLUMNS
        ))?;
        let mut entries = statement
            .query_map([chain_id], Self::history_entry)?
            .collect::<Result<Vec<_>>>()?;
        let mut stores = self.conn.prepare(
            "SELECT DISTINCT store FROM history_devices WHERE history_id = ?1 ORDER BY store",
        )?;
        for entry in &mut entries {
            entry.stores = stores
                .query_map([entry.id], |row| row.get(0))?
                .collect::<Result<_>>()?;
        }
        Ok(entries)
    }

    /// Uma execução com a origem e o resultado de cada equipamento, na ordem da grade.
    pub fn history_run(&self, id: i64) -> Result<(HistoryEntry, Vec<(Origin, Outcome)>)> {
        let entry = self.conn.query_row(
            &format!(
                "SELECT {} FROM history WHERE id = ?1",
                Self::HISTORY_COLUMNS
            ),
            [id],
            Self::history_entry,
        )?;
        let mut statement = self.conn.prepare(
            "SELECT position, store, device, host, outcome FROM history_devices
             WHERE history_id = ?1 ORDER BY position",
        )?;
        let devices = statement
            .query_map([id], |row| {
                let position: i64 = row.get(0)?;
                let outcome: String = row.get(4)?;
                let outcome: Outcome = serde_json::from_str(&outcome).map_err(|error| {
                    Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(error))
                })?;
                Ok((
                    Origin {
                        order: usize::try_from(position).unwrap_or(0),
                        store: row.get(1)?,
                        device: row.get(2)?,
                        host: row.get(3)?,
                    },
                    outcome,
                ))
            })?
            .collect::<Result<_>>()?;
        Ok((entry, devices))
    }

    pub fn history_chain(&self, id: i64) -> Result<i64> {
        self.conn
            .query_row("SELECT chain_id FROM history WHERE id = ?1", [id], |row| {
                row.get(0)
            })
    }

    pub fn delete_history(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM history WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn begin_audit(&self, start: &AuditStart) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO audit (user, chain, step, sql, planned, rehearsal_skipped, chain_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                start.user,
                start.chain,
                start.step,
                start.sql,
                i64::try_from(start.planned).unwrap_or(i64::MAX),
                start.rehearsal_skipped,
                start.chain_id
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn append_audit(&self, audit_id: i64, device: &AuditDevice) -> Result<()> {
        self.conn.execute(
            "INSERT INTO audit_devices
             (audit_id, store, device, host, profile, status, affected, message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                audit_id,
                device.store,
                device.device,
                device.host,
                device.profile,
                device.status,
                device.affected,
                device.message
            ],
        )?;
        Ok(())
    }

    /// Etapas mais recentes primeiro; com `chain_id`, só as daquela rede.
    pub fn audit_log(&self, chain_id: Option<i64>, limit: usize) -> Result<Vec<AuditEntry>> {
        let mut entries: Vec<AuditEntry> = {
            let mut statement = self.conn.prepare(
                "SELECT a.id, a.at, a.user, a.chain, a.step, a.sql, a.planned,
                        COUNT(d.id), COALESCE(SUM(d.affected), 0), a.rehearsal_skipped
                 FROM audit a LEFT JOIN audit_devices d ON d.audit_id = a.id
                 WHERE ?2 IS NULL OR a.chain_id = ?2
                    OR (a.chain_id IS NULL AND a.chain = (SELECT name FROM chains WHERE id = ?2))
                 GROUP BY a.id ORDER BY a.id DESC LIMIT ?1",
            )?;
            statement
                .query_map(
                    params![i64::try_from(limit).unwrap_or(i64::MAX), chain_id],
                    |row| {
                        Ok(AuditEntry {
                            id: row.get(0)?,
                            at: row.get(1)?,
                            user: row.get(2)?,
                            chain: row.get(3)?,
                            step: row.get(4)?,
                            sql: row.get(5)?,
                            planned: row.get(6)?,
                            finished: row.get(7)?,
                            affected: row.get(8)?,
                            rehearsal_skipped: row.get(9)?,
                            statuses: Vec::new(),
                            stores: Vec::new(),
                        })
                    },
                )?
                .collect::<Result<_>>()?
        };
        let mut statement = self.conn.prepare(
            "SELECT status, COUNT(*) FROM audit_devices WHERE audit_id = ?1
             GROUP BY status ORDER BY COUNT(*) DESC, status",
        )?;
        let mut stores = self.conn.prepare(
            "SELECT DISTINCT store FROM audit_devices WHERE audit_id = ?1 ORDER BY store",
        )?;
        for entry in &mut entries {
            entry.statuses = statement
                .query_map([entry.id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_>>()?;
            entry.stores = stores
                .query_map([entry.id], |row| row.get(0))?
                .collect::<Result<_>>()?;
        }
        Ok(entries)
    }

    pub fn audit_devices(&self, audit_id: i64) -> Result<Vec<AuditDevice>> {
        let mut statement = self.conn.prepare(
            "SELECT store, device, host, profile, status, affected, message
             FROM audit_devices WHERE audit_id = ?1 ORDER BY id",
        )?;
        statement
            .query_map([audit_id], |row| {
                Ok(AuditDevice {
                    store: row.get(0)?,
                    device: row.get(1)?,
                    host: row.get(2)?,
                    profile: row.get(3)?,
                    status: row.get(4)?,
                    affected: row.get(5)?,
                    message: row.get(6)?,
                })
            })?
            .collect()
    }

    /// Auditoria inteira para exportar: uma linha por equipamento de cada etapa, a mais
    /// antiga primeiro; etapa sem equipamento sai numa linha só. Data na hora local.
    pub fn audit_grid(&self, chain_id: Option<i64>) -> Result<crate::grid::Grid> {
        const COLUMNS: [(&str, bool); 14] = [
            ("Registro", true),
            ("Quando", false),
            ("Usuário", false),
            ("Rede", false),
            ("Etapa", false),
            ("Sem ensaio", false),
            ("SQL", false),
            ("Equipamentos previstos", true),
            ("Loja", false),
            ("Equipamento", false),
            ("IP", false),
            ("Perfil", false),
            ("Situação", false),
            ("Linhas", true),
        ];
        let mut statement = self.conn.prepare(
            "SELECT a.id, strftime('%d/%m/%Y %H:%M:%S', a.at, 'localtime'), a.user, a.chain,
                    a.step, a.rehearsal_skipped, a.sql, a.planned,
                    d.store, d.device, d.host, d.profile, d.status, d.affected, d.message
             FROM audit a LEFT JOIN audit_devices d ON d.audit_id = a.id
             WHERE ?1 IS NULL OR a.chain_id = ?1
                OR (a.chain_id IS NULL AND a.chain = (SELECT name FROM chains WHERE id = ?1))
             ORDER BY a.id, d.id",
        )?;
        let rows = statement
            .query_map([chain_id], |row| {
                let step: String = row.get(4)?;
                let status: Option<String> = row.get(12)?;
                Ok(vec![
                    Some(row.get::<_, i64>(0)?.to_string()),
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    Some(audit_step_label(&step).to_owned()),
                    Some(if row.get(5)? { "sim" } else { "não" }.to_owned()),
                    row.get(6)?,
                    Some(row.get::<_, i64>(7)?.to_string()),
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    status.map(|status| audit_status_label(&status).to_owned()),
                    row.get::<_, Option<i64>>(13)?.map(|n| n.to_string()),
                    row.get(14)?,
                ])
            })?
            .collect::<Result<Vec<_>>>()?;
        let mut columns: Vec<crate::grid::GridColumn> = COLUMNS
            .iter()
            .map(|(name, numeric)| crate::grid::GridColumn {
                name: (*name).to_owned(),
                numeric: *numeric,
                fixed: false,
            })
            .collect();
        columns.push(crate::grid::GridColumn {
            name: "Mensagem".into(),
            numeric: false,
            fixed: false,
        });
        Ok(crate::grid::Grid {
            columns,
            rows,
            sets: 1,
        })
    }

    /// Grava as lojas novas e os equipamentos com status `New` do plano, tudo ou nada.
    pub fn apply_plan(&mut self, chain_id: i64, plan: &Plan) -> Result<Applied> {
        let tx = self.conn.transaction()?;
        let mut applied = Applied::default();
        for spec in &plan.stores {
            tx.execute(
                "INSERT INTO stores (chain_id, code, name, subnet) VALUES (?1, ?2, ?3, ?4)",
                params![
                    chain_id,
                    spec.code.trim(),
                    spec.name.trim(),
                    spec.subnet.trim()
                ],
            )?;
            applied.stores += 1;
        }
        let ids: HashMap<String, i64> = {
            let mut statement =
                tx.prepare("SELECT lower(code), id FROM stores WHERE chain_id = ?1")?;
            statement
                .query_map([chain_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_>>()?
        };
        for planned in plan
            .devices
            .iter()
            .filter(|device| device.status == DeviceStatus::New)
        {
            let store_id = *ids
                .get(&planned.store_code.trim().to_lowercase())
                .ok_or(Error::QueryReturnedNoRows)?;
            insert_device(
                &tx,
                &Device {
                    id: 0,
                    store_id,
                    name: planned.name.clone(),
                    kind: planned.kind.clone(),
                    host: planned.host.clone(),
                    profile_ids: planned.profile_ids.clone(),
                },
            )?;
            applied.devices += 1;
        }
        tx.commit()?;
        Ok(applied)
    }
}

/// Atualização ou exclusão que não achou a linha vira erro, para a interface avisar.
fn changed(rows: usize) -> Result<()> {
    if rows == 0 {
        Err(Error::QueryReturnedNoRows)
    } else {
        Ok(())
    }
}

fn insert_device(tx: &rusqlite::Transaction, device: &Device) -> Result<i64> {
    tx.execute(
        "INSERT INTO devices (store_id, name, kind, host) VALUES (?1, ?2, ?3, ?4)",
        params![
            device.store_id,
            device.name.trim(),
            device.kind.trim(),
            device.host.trim()
        ],
    )?;
    let id = tx.last_insert_rowid();
    insert_device_profiles(tx, id, &device.profile_ids)?;
    Ok(id)
}

fn insert_device_profiles(
    tx: &rusqlite::Transaction,
    device_id: i64,
    profile_ids: &[i64],
) -> Result<()> {
    for (position, profile_id) in profile_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO device_profiles (device_id, profile_id, position) VALUES (?1, ?2, ?3)",
            params![
                device_id,
                profile_id,
                i64::try_from(position).unwrap_or(i64::MAX)
            ],
        )?;
    }
    Ok(())
}

fn profile_row(row: &rusqlite::Row) -> Result<Profile> {
    let engine: String = row.get(3)?;
    Ok(Profile {
        id: row.get(0)?,
        chain_id: row.get(1)?,
        name: row.get(2)?,
        engine: Engine::parse(&engine).ok_or_else(|| {
            Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                format!("motor desconhecido: {engine}").into(),
            )
        })?,
        port: row.get(4)?,
        database: row.get(5)?,
        username: row.get(6)?,
        tls: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fleet() -> (Db, Chain, Vec<i64>, Store) {
        let db = Db::open_in_memory().unwrap();
        let chain = db.add_chain(" Supermercados Aurora ").unwrap();
        let profiles = ["Econect", "Consinco"]
            .iter()
            .map(|name| {
                db.add_profile(&Profile {
                    id: 0,
                    chain_id: chain.id,
                    name: (*name).into(),
                    engine: Engine::Mysql,
                    port: 3306,
                    database: "pdv".into(),
                    username: "leitura".into(),
                    tls: false,
                })
                .unwrap()
            })
            .collect();
        let store = db.add_store(chain.id, "101", "Centro", "101").unwrap();
        (db, chain, profiles, store)
    }

    #[test]
    fn migrates_to_latest_version_once() {
        let path = std::env::temp_dir().join(format!("selectall-store-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Db::open(&path).unwrap().set_setting("k", "v").unwrap();
        let reopened = Db::open(&path).unwrap();
        assert_eq!(reopened.schema_version().unwrap(), MIGRATIONS.len());
        assert_eq!(reopened.setting("k").unwrap().as_deref(), Some("v"));
        drop(reopened);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn device_keeps_profile_order() {
        let (mut db, chain, profiles, store) = fleet();
        assert_eq!(chain.name, "Supermercados Aurora");
        let order = vec![profiles[1], profiles[0]];
        db.add_device(&Device {
            id: 0,
            store_id: store.id,
            name: "PDV 01".into(),
            kind: "PDV".into(),
            host: "10.101.3.1".into(),
            profile_ids: order.clone(),
        })
        .unwrap();
        let devices = db.devices(chain.id).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].profile_ids, order);
    }

    #[test]
    fn rejects_duplicates_and_blank_names() {
        let (mut db, chain, profiles, store) = fleet();
        assert!(db.add_chain("supermercados aurora").is_err());
        assert!(db.add_chain("   ").is_err());
        assert!(db.add_store(chain.id, "101", "Outra", "").is_err());
        let device = Device {
            id: 0,
            store_id: store.id,
            name: "PDV 01".into(),
            kind: "PDV".into(),
            host: "10.101.3.1".into(),
            profile_ids: vec![profiles[0]],
        };
        db.add_device(&device).unwrap();
        assert!(db.add_device(&device).is_err());
    }

    #[test]
    fn deleting_a_chain_removes_its_fleet() {
        let (db, chain, _, _) = fleet();
        db.conn
            .execute("DELETE FROM chains WHERE id = ?1", [chain.id])
            .unwrap();
        assert_eq!(db.stores(chain.id).unwrap(), []);
        assert_eq!(db.profiles(chain.id).unwrap(), []);
    }

    #[test]
    fn upgrades_version_one_keeping_stores() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.execute_batch(
            "INSERT INTO chains (name) VALUES ('Aurora');
             INSERT INTO stores (chain_id, code, name) VALUES (1, '101', 'Centro');
             PRAGMA user_version = 1;",
        )
        .unwrap();
        let db = Db::init(conn).unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len());
        let stores = db.stores(1).unwrap();
        assert_eq!(
            (stores[0].code.as_str(), stores[0].subnet.as_str()),
            ("101", "")
        );
    }

    #[test]
    fn edits_and_deletes_every_entity() {
        let (mut db, chain, profiles, mut store) = fleet();
        assert_eq!(
            db.rename_chain(chain.id, " Aurora ").unwrap().name,
            "Aurora"
        );
        assert!(db.rename_chain(999, "x").is_err());

        let mut profile = db.profile(profiles[0]).unwrap();
        profile.name = "Econect 8".into();
        profile.port = 3307;
        profile.chain_id = 999;
        db.update_profile(&profile).unwrap();
        let reread = db.profile(profiles[0]).unwrap();
        assert_eq!(
            (reread.name.as_str(), reread.port, reread.chain_id),
            ("Econect 8", 3307, chain.id)
        );

        store.name = "Centro Novo".into();
        store.subnet = "25".into();
        db.update_store(&store).unwrap();
        assert_eq!(db.stores(chain.id).unwrap(), [store.clone()]);

        let mut device = Device {
            id: 0,
            store_id: store.id,
            name: "PDV 01".into(),
            kind: "PDV".into(),
            host: "10.25.3.1".into(),
            profile_ids: vec![profiles[0]],
        };
        device.id = db.add_device(&device).unwrap();
        device.host = "10.25.3.2".into();
        device.profile_ids = vec![profiles[1], profiles[0]];
        db.update_device(&device).unwrap();
        assert_eq!(db.devices(chain.id).unwrap(), [device.clone()]);

        db.delete_profile(profiles[1]).unwrap();
        assert_eq!(db.devices(chain.id).unwrap()[0].profile_ids, [profiles[0]]);
        db.delete_device(device.id).unwrap();
        assert!(db.delete_device(device.id).is_err());
        db.delete_store(store.id).unwrap();
        assert_eq!(db.stores(chain.id).unwrap(), []);
        db.delete_chain(chain.id).unwrap();
        assert_eq!(db.chains().unwrap(), []);
    }

    #[test]
    fn rules_keep_order_and_lose_deleted_profiles() {
        let (mut db, chain, profiles, _) = fleet();
        let rules = vec![
            AddressRule {
                kind: "PDV".into(),
                template: "10.{sub}.3.1-50".into(),
                profile_id: Some(profiles[0]),
            },
            AddressRule {
                kind: "Concentrador".into(),
                template: "10.{sub}.3.200".into(),
                profile_id: Some(profiles[1]),
            },
        ];
        db.set_rules(chain.id, &rules).unwrap();
        assert_eq!(db.rules(chain.id).unwrap(), rules);
        db.delete_profile(profiles[1]).unwrap();
        assert_eq!(db.rules(chain.id).unwrap()[1].profile_id, None);
        db.set_rules(chain.id, &rules[..1]).unwrap();
        assert_eq!(db.rules(chain.id).unwrap().len(), 1);
    }

    #[test]
    fn applies_a_plan_atomically() {
        use crate::fleet::{Fleet, generate, plan};
        let (mut db, chain, profiles, _) = fleet();
        let rules = vec![AddressRule {
            kind: "PDV".into(),
            template: "10.{sub}.3.1-3".into(),
            profile_id: Some(profiles[0]),
        }];
        db.set_rules(chain.id, &rules).unwrap();
        let build = |db: &Db| {
            let (stores, devices, profiles) = (
                db.stores(chain.id).unwrap(),
                db.devices(chain.id).unwrap(),
                db.profiles(chain.id).unwrap(),
            );
            plan(
                generate(&rules, "101;Centro;101\n102;Jardim;102"),
                &Fleet {
                    stores: &stores,
                    devices: &devices,
                    rules: &rules,
                    profiles: &profiles,
                },
            )
        };
        let first = build(&db);
        assert_eq!(first.stores.len(), 1, "a loja 101 já existe");
        assert_eq!(
            db.apply_plan(chain.id, &first).unwrap(),
            Applied {
                stores: 1,
                devices: 6
            }
        );
        let again = build(&db);
        assert_eq!(db.apply_plan(chain.id, &again).unwrap(), Applied::default());

        let mut broken = first.clone();
        broken.stores.clear();
        broken.devices.truncate(1);
        broken.devices[0].store_code = "999".into();
        broken.devices[0].host = "10.9.9.9".into();
        broken.devices[0].status = DeviceStatus::New;
        assert!(db.apply_plan(chain.id, &broken).is_err());
        assert_eq!(db.devices(chain.id).unwrap().len(), 6);
    }

    #[test]
    fn audit_only_accepts_appends() {
        let db = Db::open_in_memory().unwrap();
        let start = AuditStart {
            user: "suporte".into(),
            chain_id: 1,
            chain: "Aurora".into(),
            step: "pilot".into(),
            sql: "UPDATE t SET a = 1 WHERE b = 2".into(),
            planned: 2,
            rehearsal_skipped: true,
        };
        let id = db.begin_audit(&start).unwrap();
        let device = |name: &str, status: &str, affected| AuditDevice {
            store: "101".into(),
            device: name.into(),
            host: "10.0.0.1".into(),
            profile: Some("Econect".into()),
            status: status.into(),
            affected,
            message: None,
        };
        db.append_audit(id, &device("PDV 01", "applied", 6))
            .unwrap();
        db.append_audit(id, &device("PDV 02", "unknown", 0))
            .unwrap();
        let log = db.audit_log(None, 10).unwrap();
        assert_eq!(log.len(), 1);
        assert!(log[0].rehearsal_skipped);
        assert_eq!(
            (log[0].planned, log[0].finished, log[0].affected),
            (2, 2, 6)
        );
        assert_eq!(
            log[0].statuses,
            [("applied".to_owned(), 1), ("unknown".to_owned(), 1)]
        );
        assert_eq!(db.audit_devices(id).unwrap()[1].device, "PDV 02");
        let empty = db
            .begin_audit(&AuditStart {
                step: "rehearsal".into(),
                rehearsal_skipped: false,
                ..start.clone()
            })
            .unwrap();
        let grid = db.audit_grid(None).unwrap();
        assert_eq!(grid.columns.len(), grid.rows[0].len());
        assert_eq!(
            grid.rows.len(),
            3,
            "dois equipamentos e uma etapa sem nenhum"
        );
        let cell = |row: usize, name: &str| {
            let column = grid.columns.iter().position(|c| c.name == name).unwrap();
            grid.rows[row][column].clone()
        };
        assert_eq!(cell(0, "Etapa").as_deref(), Some("Piloto"));
        assert_eq!(cell(0, "Sem ensaio").as_deref(), Some("sim"));
        assert_eq!(
            cell(1, "Situação").as_deref(),
            Some("Resultado desconhecido")
        );
        assert_eq!(cell(2, "Registro"), Some(empty.to_string()));
        assert_eq!(cell(2, "Equipamento"), None);
        assert_eq!(log[0].stores, ["101"]);
        let other = db
            .begin_audit(&AuditStart {
                chain_id: 2,
                chain: "Outra".into(),
                ..start.clone()
            })
            .unwrap();
        let only_first: Vec<i64> = db
            .audit_log(Some(1), 10)
            .unwrap()
            .iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(only_first, [empty, id]);
        assert_eq!(db.audit_log(Some(2), 10).unwrap()[0].id, other);
        assert_eq!(db.audit_grid(Some(2)).unwrap().rows.len(), 1);
        for sql in [
            "UPDATE audit SET sql = 'x'",
            "DELETE FROM audit",
            "UPDATE audit_devices SET affected = 0",
            "DELETE FROM audit_devices",
        ] {
            let error = db.conn.execute(sql, []).unwrap_err();
            assert!(
                error.to_string().contains("só aceita acréscimo"),
                "{sql}: {error}"
            );
        }
        assert!(
            db.begin_audit(&AuditStart {
                step: "outra".into(),
                ..start.clone()
            })
            .is_err()
        );
    }

    fn saved(chain_id: i64, name: &str, sql: &str) -> SavedQuery {
        SavedQuery {
            id: 0,
            chain_id,
            name: name.into(),
            description: String::new(),
            profile_id: None,
            sql: sql.into(),
        }
    }

    #[test]
    fn saved_queries_import_replaces_by_name() {
        let (mut db, chain, profiles, _) = fleet();
        let mut first = saved(chain.id, "Versão", "SELECT 1");
        first.profile_id = Some(profiles[0]);
        first.id = db.save_query(&first).unwrap();
        assert!(
            db.save_query(&saved(chain.id, "Versão", "SELECT 2"))
                .is_err()
        );
        let (added, replaced) = db
            .import_queries(&[
                saved(chain.id, "Versão", "SELECT 3"),
                saved(chain.id, "Carga", "SELECT 4"),
            ])
            .unwrap();
        assert_eq!((added, replaced), (1, 1));
        let all = db.saved_queries(chain.id).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].sql, "SELECT 3");
        assert_eq!(all[1].id, first.id);
        db.delete_profile(profiles[0]).unwrap();
        assert_eq!(db.saved_queries(chain.id).unwrap()[1].profile_id, None);
        db.delete_query(first.id).unwrap();
        assert_eq!(db.saved_queries(chain.id).unwrap().len(), 1);
    }

    fn outcome(device_id: i64, status: crate::exec::Status, rows: usize) -> Outcome {
        Outcome {
            device_id,
            profile: Some("Econect".into()),
            sets: vec![crate::exec::ResultSet {
                columns: vec![crate::exec::Column {
                    name: "v".into(),
                    numeric: false,
                }],
                rows: vec![vec![Some("x".repeat(100))]; rows],
                affected_rows: 0,
                warnings: 0,
                truncated: false,
            }],
            status,
            failure: None,
            elapsed_ms: 5,
            affected_rows: 0,
        }
    }

    fn reading(chain_id: i64, sql: &str, rows: usize) -> NewRun {
        let origin = |order: usize| Origin {
            order,
            store: "101".into(),
            device: format!("PDV {order}"),
            host: format!("10.0.0.{order}"),
        };
        NewRun {
            chain_id,
            label: None,
            script: Some(crate::params::Script::from(sql)),
            targets: 3,
            elapsed_ms: 40,
            devices: vec![
                (origin(0), outcome(1, crate::exec::Status::Success, rows)),
                (origin(1), outcome(2, crate::exec::Status::Offline, 0)),
            ],
        }
    }

    #[test]
    fn history_keeps_results_and_hides_credentials() {
        let (mut db, chain, _, _) = fleet();
        let id = db
            .add_history(&reading(chain.id, "SELECT v FROM t", 2), None)
            .unwrap();
        let secret = db
            .add_history(&reading(chain.id, "SELECT senha FROM t", 1), None)
            .unwrap();
        let list = db.history(chain.id).unwrap();
        assert_eq!(list.iter().map(|e| e.id).collect::<Vec<_>>(), [secret, id]);
        assert_eq!(list[1].stores, ["101"]);
        assert_eq!(list[0].sql, None);
        assert_eq!(
            (
                list[1].targets,
                list[1].responded,
                list[1].offline,
                list[1].rows
            ),
            (3, 1, 1, 2)
        );
        let (entry, devices) = db.history_run(id).unwrap();
        assert_eq!(entry.sql.as_deref(), Some("SELECT v FROM t"));
        assert_eq!(devices[1].0.device, "PDV 1");
        assert_eq!(devices[0].1.sets[0].rows.len(), 2);
        // A repetição substitui a execução anterior.
        let again = db
            .add_history(&reading(chain.id, "SELECT v FROM t", 3), Some(id))
            .unwrap();
        assert!(db.history_run(id).is_err());
        assert_eq!(db.history_run(again).unwrap().1[0].1.sets[0].rows.len(), 3);
        db.delete_chain(chain.id).unwrap();
        assert_eq!(db.history_usage().unwrap().runs, 0);
    }

    #[test]
    fn lowering_the_limits_keeps_the_latest_run() {
        let (mut db, chain, _, _) = fleet();
        // Cerca de 1,2 MB, acima do novo limite inteiro.
        let latest = db
            .add_history(&reading(chain.id, "SELECT 1", 12_000), None)
            .unwrap();
        db.set_history_limits(Limits {
            days: 30,
            megabytes: 1,
        })
        .unwrap();
        let kept: Vec<i64> = db.history(chain.id).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(kept, [latest]);
    }

    #[test]
    fn history_drops_the_oldest_past_the_limits() {
        let (mut db, chain, _, _) = fleet();
        db.set_history_limits(Limits {
            days: 30,
            megabytes: 1,
        })
        .unwrap();
        // Cada execução tem cerca de 400 KB; cabem duas em 1 MB.
        let ids: Vec<i64> = (0..4)
            .map(|_| {
                db.add_history(&reading(chain.id, "SELECT 1", 3500), None)
                    .unwrap()
            })
            .collect();
        let kept: Vec<i64> = db.history(chain.id).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(kept, [ids[3], ids[2]]);
        assert!(db.history_usage().unwrap().bytes <= 1024 * 1024);

        db.conn
            .execute(
                "UPDATE history SET at = '2000-01-01T00:00:00Z' WHERE id = ?1",
                [ids[2]],
            )
            .unwrap();
        let newest = db
            .add_history(&reading(chain.id, "SELECT 1", 1), None)
            .unwrap();
        let kept: Vec<i64> = db.history(chain.id).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(kept, [newest, ids[3]]);

        // A execução recém-gravada fica mesmo maior que o limite inteiro.
        let huge = db
            .add_history(&reading(chain.id, "SELECT 1", 12_000), None)
            .unwrap();
        let kept: Vec<i64> = db.history(chain.id).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(kept, [huge]);
        assert_eq!(
            db.history_limits().unwrap(),
            Limits {
                days: 30,
                megabytes: 1
            }
        );
    }

    #[test]
    fn per_store_limit_defaults_and_clamps() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(
            db.per_store_limit().unwrap(),
            crate::exec::DEFAULT_PER_STORE
        );
        db.set_per_store_limit(0).unwrap();
        assert_eq!(db.per_store_limit().unwrap(), 1);
        db.set_per_store_limit(500).unwrap();
        assert_eq!(db.per_store_limit().unwrap(), crate::exec::MAX_CONCURRENCY);
    }

    #[test]
    fn audit_without_chain_id_matches_by_chain_name() {
        let (db, chain, _, _) = fleet();
        db.conn
            .execute(
                "INSERT INTO audit (user, chain, step, sql, planned) VALUES ('u', ?1, 'pilot', 'x', 1)",
                [&chain.name],
            )
            .unwrap();
        assert_eq!(db.audit_log(Some(chain.id), 10).unwrap().len(), 1);
        assert!(db.audit_log(Some(chain.id + 1), 10).unwrap().is_empty());
    }
}
