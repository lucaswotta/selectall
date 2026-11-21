//! Frota: redes, perfis de sistema, lojas e equipamentos.
//!
//! Glossário: `Chain` é a rede de varejo, `Store` a loja, `Device` o equipamento
//! (PDV, self, concentrador) e `Profile` o perfil de sistema de PDV.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Mysql,
    Postgres,
    Sqlserver,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mysql => "mysql",
            Self::Postgres => "postgres",
            Self::Sqlserver => "sqlserver",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "mysql" => Some(Self::Mysql),
            "postgres" => Some(Self::Postgres),
            "sqlserver" => Some(Self::Sqlserver),
            _ => None,
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Self::Mysql => 3306,
            Self::Postgres => 5432,
            Self::Sqlserver => 1433,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    pub id: i64,
    pub name: String,
}

/// Como falar com um sistema de PDV. A senha fica no cofre, nunca aqui.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: i64,
    pub chain_id: i64,
    pub name: String,
    pub engine: Engine,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub tls: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    pub id: i64,
    pub chain_id: i64,
    pub code: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: i64,
    pub store_id: i64,
    pub name: String,
    /// Livre para cada rede definir: "PDV", "Self", "Concentrador"...
    pub kind: String,
    pub host: String,
    /// Perfis tentados em ordem até um aceitar a autenticação.
    pub profile_ids: Vec<i64>,
}
