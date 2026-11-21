//! Armazenamento local em SQLite: frota, perfis e configuração. Sem senhas.

use crate::model::{Chain, Device, Engine, Profile, Store};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub use rusqlite::{Error, ErrorCode};
pub type Result<T> = rusqlite::Result<T>;

/// Cada posição é uma migração; o índice + 1 vira o `user_version` do banco.
const MIGRATIONS: &[&str] = &[r"
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
"];

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

    pub fn add_profile(&self, profile: &Profile) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO profiles (chain_id, name, engine, port, database, username, tls)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                profile.chain_id,
                profile.name.trim(),
                profile.engine.as_str(),
                profile.port,
                profile.database,
                profile.username,
                profile.tls
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn profiles(&self, chain_id: i64) -> Result<Vec<Profile>> {
        let mut statement = self.conn.prepare(
            "SELECT id, chain_id, name, engine, port, database, username, tls
             FROM profiles WHERE chain_id = ?1 ORDER BY name COLLATE NOCASE",
        )?;
        statement
            .query_map([chain_id], |row| {
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
            })?
            .collect()
    }

    pub fn add_store(&self, chain_id: i64, code: &str, name: &str) -> Result<Store> {
        let (code, name) = (code.trim(), name.trim());
        self.conn.execute(
            "INSERT INTO stores (chain_id, code, name) VALUES (?1, ?2, ?3)",
            params![chain_id, code, name],
        )?;
        Ok(Store {
            id: self.conn.last_insert_rowid(),
            chain_id,
            code: code.to_owned(),
            name: name.to_owned(),
        })
    }

    pub fn stores(&self, chain_id: i64) -> Result<Vec<Store>> {
        let mut statement = self.conn.prepare(
            "SELECT id, chain_id, code, name FROM stores WHERE chain_id = ?1
             ORDER BY length(code), code",
        )?;
        statement
            .query_map([chain_id], |row| {
                Ok(Store {
                    id: row.get(0)?,
                    chain_id: row.get(1)?,
                    code: row.get(2)?,
                    name: row.get(3)?,
                })
            })?
            .collect()
    }

    /// Grava o equipamento e a ordem dos perfis numa única transação.
    pub fn add_device(&mut self, device: &Device) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO devices (store_id, name, kind, host) VALUES (?1, ?2, ?3, ?4)",
            params![
                device.store_id,
                device.name,
                device.kind,
                device.host.trim()
            ],
        )?;
        let id = tx.last_insert_rowid();
        for (position, profile_id) in device.profile_ids.iter().enumerate() {
            tx.execute(
                "INSERT INTO device_profiles (device_id, profile_id, position) VALUES (?1, ?2, ?3)",
                params![id, profile_id, i64::try_from(position).unwrap_or(i64::MAX)],
            )?;
        }
        tx.commit()?;
        Ok(id)
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
        let store = db.add_store(chain.id, "101", "Centro").unwrap();
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
        assert!(db.add_store(chain.id, "101", "Outra").is_err());
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
}
