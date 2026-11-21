//! Armazenamento local em SQLite: frota, perfis e configuração. Sem senhas.

use crate::fleet::{DeviceStatus, Plan};
use crate::model::{AddressRule, Chain, Device, Engine, Profile, Store};
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
];

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
}
