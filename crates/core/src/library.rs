//! Biblioteca de consultas salvas de uma rede e o arquivo JSON que a equipe troca.

use crate::model::{Engine, Profile};
use crate::{params, sql};
use serde::{Deserialize, Serialize};

pub const FILE_FORMAT: &str = "selectall-consultas";
pub const FILE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedQuery {
    pub id: i64,
    pub chain_id: i64,
    pub name: String,
    pub description: String,
    /// Perfil de sistema a que a consulta se destina; decide o dialeto da análise.
    pub profile_id: Option<i64>,
    pub sql: String,
}

/// Consulta salva com o que a tela mostra: se escreve e quais parâmetros pede.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SavedView {
    #[serde(flatten)]
    pub query: SavedQuery,
    pub write: bool,
    pub params: Vec<String>,
}

/// `true` quando a consulta escreve: decide se abre em Consultar ou em Corrigir.
/// Os parâmetros entram como texto vazio só para a análise.
pub fn is_write(sql_text: &str, engine: Engine) -> Result<bool, String> {
    let plan = sql::analyze(&params::sample(sql_text, engine)?, engine)?;
    Ok(!plan.read_only_candidate)
}

fn engine_of(query: &SavedQuery, profiles: &[Profile]) -> Engine {
    query
        .profile_id
        .and_then(|id| profiles.iter().find(|p| p.id == id))
        .map_or(Engine::Mysql, |p| p.engine)
}

/// Recusa nome vazio e SQL que não passa na análise do dialeto do perfil.
pub fn validate(query: &SavedQuery, profiles: &[Profile]) -> Result<(), String> {
    if query.name.trim().is_empty() {
        return Err("Dê um nome à consulta.".into());
    }
    is_write(&query.sql, engine_of(query, profiles)).map(|_| ())
}

pub fn describe(query: SavedQuery, profiles: &[Profile]) -> SavedView {
    let write = is_write(&query.sql, engine_of(&query, profiles)).unwrap_or(true);
    SavedView {
        params: params::names(&query.sql),
        write,
        query,
    }
}

/// Consulta no arquivo: o perfil vai pelo nome, porque o número é desta instalação.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedQuery {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub profile: Option<String>,
    pub sql: String,
}

#[derive(Serialize, Deserialize)]
struct LibraryFile {
    format: String,
    version: u32,
    queries: Vec<SharedQuery>,
}

pub fn to_json(queries: &[SavedQuery], profiles: &[Profile]) -> Result<String, String> {
    let file = LibraryFile {
        format: FILE_FORMAT.into(),
        version: FILE_VERSION,
        queries: queries
            .iter()
            .map(|q| SharedQuery {
                name: q.name.clone(),
                description: q.description.clone(),
                profile: q
                    .profile_id
                    .and_then(|id| profiles.iter().find(|p| p.id == id))
                    .map(|p| p.name.clone()),
                sql: q.sql.clone(),
            })
            .collect(),
    };
    serde_json::to_string_pretty(&file)
        .map_err(|error| format!("Não foi possível montar o arquivo: {error}"))
}

/// Lê o arquivo e liga cada perfil pelo nome aos perfis desta rede (sem diferenciar
/// maiúsculas). Perfil sem par fica vazio. Nada é gravado aqui.
pub fn from_json(
    text: &str,
    chain_id: i64,
    profiles: &[Profile],
) -> Result<Vec<SavedQuery>, String> {
    let file: LibraryFile = serde_json::from_str(text)
        .map_err(|_| "O arquivo não é uma biblioteca de consultas do SelectAll.".to_owned())?;
    if file.format != FILE_FORMAT {
        return Err("O arquivo não é uma biblioteca de consultas do SelectAll.".into());
    }
    if file.version > FILE_VERSION {
        return Err(
            "A biblioteca foi exportada por uma versão mais nova do SelectAll. Atualize o aplicativo."
                .into(),
        );
    }
    file.queries
        .into_iter()
        .map(|shared| {
            let query = SavedQuery {
                id: 0,
                chain_id,
                name: shared.name.trim().to_owned(),
                description: shared.description,
                profile_id: shared.profile.and_then(|name| {
                    profiles
                        .iter()
                        .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
                        .map(|p| p.id)
                }),
                sql: shared.sql,
            };
            validate(&query, profiles)
                .map_err(|error| format!("Consulta \"{}\": {error}", query.name))?;
            Ok(query)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: i64, name: &str, engine: Engine) -> Profile {
        Profile {
            id,
            chain_id: 1,
            name: name.into(),
            engine,
            port: engine.default_port(),
            database: String::new(),
            username: String::new(),
            tls: false,
        }
    }

    fn query(name: &str, profile_id: Option<i64>, sql: &str) -> SavedQuery {
        SavedQuery {
            id: 1,
            chain_id: 1,
            name: name.into(),
            description: "d".into(),
            profile_id,
            sql: sql.into(),
        }
    }

    #[test]
    fn kind_and_params_come_from_the_sql() {
        let profiles = [profile(1, "Socin", Engine::Mysql)];
        let read = describe(
            query(
                "a",
                Some(1),
                "SELECT * FROM p WHERE c = {{codigo}} AND l = {{loja}}",
            ),
            &profiles,
        );
        assert!(!read.write);
        assert_eq!(read.params, ["codigo", "loja"]);
        let write = describe(
            query("b", None, "UPDATE p SET a = 1 WHERE c = {{codigo}}"),
            &profiles,
        );
        assert!(write.write);
    }

    #[test]
    fn validation_follows_the_profile_dialect() {
        let profiles = [profile(1, "PG", Engine::Postgres)];
        assert!(validate(&query("a", Some(1), "SELECT $$ {{x}} $$"), &profiles).is_err());
        assert!(validate(&query(" ", None, "SELECT 1"), &profiles).is_err());
        assert!(validate(&query("a", None, "SELECT '"), &profiles).is_err());
    }

    #[test]
    fn library_round_trips_with_profiles_by_name() {
        let here = [profile(1, "Socin", Engine::Mysql)];
        let there = [profile(7, "socin", Engine::Mysql)];
        let queries = [
            query("Versão", Some(1), "SELECT {{a}}"),
            query("Sem perfil", None, "SELECT 2"),
        ];
        let text = to_json(&queries, &here).unwrap();
        let imported = from_json(&text, 3, &there).unwrap();
        assert_eq!(imported[0].profile_id, Some(7));
        assert_eq!(imported[0].chain_id, 3);
        assert_eq!(imported[1].profile_id, None);
        assert_eq!(imported[0].sql, "SELECT {{a}}");
        assert!(from_json("{}", 3, &there).is_err());
        assert!(from_json(r#"{"format":"x","version":1,"queries":[]}"#, 3, &there).is_err());
        assert!(
            from_json(
                r#"{"format":"selectall-consultas","version":9,"queries":[]}"#,
                3,
                &there
            )
            .is_err()
        );
    }
}
