//! Parâmetros `{{nome}}` de consultas salvas. O valor nunca entra cru no SQL: fora
//! de aspas vira um texto citado no dialeto do motor; dentro de um texto, entra com
//! as aspas e barras escapadas. Em comentário ou identificador, é recusado.

use crate::model::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Values = BTreeMap<String, String>;

/// SQL com os valores dos parâmetros. O texto final é montado por motor na análise,
/// porque cada dialeto escapa de um jeito.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    pub sql: String,
    pub values: Values,
}

impl Script {
    pub fn new(sql: impl Into<String>, values: Values) -> Self {
        Self {
            sql: sql.into(),
            values,
        }
    }
}

impl From<&str> for Script {
    fn from(sql: &str) -> Self {
        Self::new(sql, Values::new())
    }
}

impl From<&String> for Script {
    fn from(sql: &String) -> Self {
        Self::new(sql.as_str(), Values::new())
    }
}

impl From<String> for Script {
    fn from(sql: String) -> Self {
        Self::new(sql, Values::new())
    }
}

/// Fim do nome em `{{nome}}` que começa em `start`, ou `None` se não for parâmetro.
fn placeholder(chars: &[char], start: usize) -> Option<(String, usize)> {
    if chars.get(start) != Some(&'{') || chars.get(start + 1) != Some(&'{') {
        return None;
    }
    let first = *chars.get(start + 2)?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let mut end = start + 2;
    while chars
        .get(end)
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
    {
        end += 1;
    }
    if chars.get(end) != Some(&'}') || chars.get(end + 1) != Some(&'}') {
        return None;
    }
    Some((chars[start + 2..end].iter().collect(), end + 2))
}

/// Nomes dos parâmetros na ordem em que aparecem, sem repetição.
pub fn names(sql: &str) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut found: Vec<String> = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if let Some((name, end)) = placeholder(&chars, index) {
            if !found.contains(&name) {
                found.push(name);
            }
            index = end;
        } else {
            index += 1;
        }
    }
    found
}

/// Onde o parâmetro caiu: o que muda o jeito de escrever o valor.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    Code,
    /// Dentro de um texto delimitado por `quote`; `backslash` quando a barra escapa.
    Text {
        quote: char,
        backslash: bool,
    },
    Identifier {
        close: char,
    },
    Comment {
        depth: usize,
    },
    LineComment,
}

fn escape(value: &str, quote: char, backslash: bool) -> String {
    let mut text = String::with_capacity(value.len() + 2);
    for ch in value.chars() {
        if ch == quote {
            text.push(quote);
        } else if ch == '\\' && backslash {
            text.push('\\');
        }
        text.push(ch);
    }
    text
}

/// Texto citado para o valor, no dialeto do motor. No MySQL a barra escapa por
/// padrão; o PostgreSQL roda com `standard_conforming_strings = on`; o SQL Server
/// leva `N` para não perder acentos.
fn literal(value: &str, engine: Engine) -> String {
    match engine {
        Engine::Mysql => format!("'{}'", escape(value, '\'', true)),
        Engine::Postgres => format!("'{}'", escape(value, '\'', false)),
        Engine::Sqlserver => format!("N'{}'", escape(value, '\'', false)),
    }
}

/// Troca cada `{{nome}}` pelo valor, escrito como texto SQL do motor.
// Um único laço de varredura, como em `sql::analyze`.
#[allow(clippy::too_many_lines)]
pub fn fill(sql: &str, values: &Values, engine: Engine) -> Result<String, String> {
    let mysql = engine == Engine::Mysql;
    let chars: Vec<char> = sql.chars().collect();
    let mut output = String::with_capacity(sql.len());
    let mut context = Context::Code;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let next = chars.get(index + 1).copied();
        if let Some((name, end)) = placeholder(&chars, index) {
            let value = values
                .get(&name)
                .ok_or_else(|| format!("Preencha o parâmetro {name}."))?;
            if value.contains('\0') {
                return Err(format!("O valor de {name} tem um caractere nulo."));
            }
            match context {
                Context::Code => output.push_str(&literal(value, engine)),
                Context::Text { quote, backslash } => {
                    output.push_str(&escape(value, quote, backslash));
                }
                Context::Identifier { .. } => {
                    return Err(format!(
                        "O parâmetro {name} está num nome de tabela ou coluna; parâmetros valem só para valores."
                    ));
                }
                Context::Comment { .. } | Context::LineComment => {
                    return Err(format!(
                        "O parâmetro {name} está num comentário. Tire-o do comentário ou apague-o."
                    ));
                }
            }
            index = end;
            continue;
        }
        output.push(ch);
        match context {
            Context::LineComment => {
                if ch == '\n' {
                    context = Context::Code;
                }
            }
            Context::Comment { depth } => {
                if ch == '*' && next == Some('/') {
                    output.push('/');
                    index += 1;
                    context = if depth == 1 {
                        Context::Code
                    } else {
                        Context::Comment { depth: depth - 1 }
                    };
                } else if !mysql && ch == '/' && next == Some('*') {
                    output.push('*');
                    index += 1;
                    context = Context::Comment { depth: depth + 1 };
                }
            }
            Context::Text { quote, backslash } => {
                if ch == '\\' && backslash {
                    if let Some(escaped) = next {
                        output.push(escaped);
                        index += 1;
                    }
                } else if ch == quote {
                    if next == Some(quote) {
                        output.push(quote);
                        index += 1;
                    } else {
                        context = Context::Code;
                    }
                }
            }
            Context::Identifier { close } => {
                if ch == close {
                    if next == Some(close) {
                        output.push(close);
                        index += 1;
                    } else {
                        context = Context::Code;
                    }
                }
            }
            Context::Code => {
                if (mysql && ch == '#')
                    || (ch == '-'
                        && next == Some('-')
                        && (!mysql || chars.get(index + 2).is_none_or(|c| c.is_whitespace())))
                {
                    context = Context::LineComment;
                } else if ch == '/' && next == Some('*') {
                    output.push('*');
                    index += 1;
                    context = Context::Comment { depth: 1 };
                } else if ch == '\'' || (mysql && ch == '"') {
                    let prefix_e = engine == Engine::Postgres
                        && index > 0
                        && chars[index - 1].eq_ignore_ascii_case(&'e')
                        && (index < 2
                            || !(chars[index - 2].is_ascii_alphanumeric()
                                || chars[index - 2] == '_'));
                    context = Context::Text {
                        quote: ch,
                        backslash: mysql || prefix_e,
                    };
                } else if ch == '"'
                    || (mysql && ch == '`')
                    || (engine == Engine::Sqlserver && ch == '[')
                {
                    context = Context::Identifier {
                        close: if ch == '[' { ']' } else { ch },
                    };
                } else if engine == Engine::Postgres
                    && ch == '$'
                    && (index == 0
                        || !(chars[index - 1].is_ascii_alphanumeric() || chars[index - 1] == '_'))
                {
                    let indexed: Vec<(usize, char)> = chars.iter().copied().enumerate().collect();
                    if let Some(end) = crate::sql::dollar_quote(&indexed, index) {
                        let body: String =
                            chars[index + 1..chars.len().min(end + 1)].iter().collect();
                        if !names(&body).is_empty() {
                            return Err(
                                "Parâmetro dentro de texto entre cifrões ($$) não é aceito; use aspas simples.".into(),
                            );
                        }
                        output.push_str(&body);
                        index = end;
                    }
                }
            }
        }
        index += 1;
    }
    Ok(output)
}

/// SQL com cada parâmetro trocado por um texto vazio, para decidir se é leitura ou escrita.
pub fn sample(sql: &str, engine: Engine) -> Result<String, String> {
    let values = names(sql)
        .into_iter()
        .map(|name| (name, String::new()))
        .collect();
    fill(sql, &values, engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> Values {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn names_keep_order_without_repeats() {
        assert_eq!(
            names("SELECT {{b}}, {{a}}, {{b}}, {{ x }}, {{1a}}, {a}"),
            ["b", "a"]
        );
    }

    #[test]
    fn values_become_literals_per_engine() {
        let sql = "SELECT * FROM p WHERE codigo = {{codigo}}";
        let v = values(&[("codigo", r"7' OR '1'='1 \")]);
        assert_eq!(
            fill(sql, &v, Engine::Mysql).unwrap(),
            r"SELECT * FROM p WHERE codigo = '7'' OR ''1''=''1 \\'"
        );
        assert_eq!(
            fill(sql, &v, Engine::Postgres).unwrap(),
            r"SELECT * FROM p WHERE codigo = '7'' OR ''1''=''1 \'"
        );
        assert_eq!(
            fill(sql, &v, Engine::Sqlserver).unwrap(),
            r"SELECT * FROM p WHERE codigo = N'7'' OR ''1''=''1 \'"
        );
    }

    #[test]
    fn values_inside_text_are_escaped_without_new_quotes() {
        let v = values(&[("nome", "d'a\\")]);
        assert_eq!(
            fill("SELECT 1 WHERE n LIKE '%{{nome}}%'", &v, Engine::Mysql).unwrap(),
            r"SELECT 1 WHERE n LIKE '%d''a\\%'"
        );
        assert_eq!(
            fill(r#"SELECT "x{{nome}}""#, &v, Engine::Mysql).unwrap(),
            r#"SELECT "xd'a\\""#
        );
        assert_eq!(
            fill("SELECT E'{{nome}}', '{{nome}}'", &v, Engine::Postgres).unwrap(),
            r"SELECT E'd''a\\', 'd''a\'"
        );
    }

    #[test]
    fn placeholders_in_comments_and_identifiers_are_refused() {
        let v = values(&[("t", "x")]);
        for (sql, engine) in [
            ("SELECT 1 -- {{t}}", Engine::Mysql),
            ("SELECT 1 # {{t}}", Engine::Mysql),
            ("SELECT 1 /* {{t}} */", Engine::Postgres),
            ("SELECT * FROM `{{t}}`", Engine::Mysql),
            ("SELECT \"{{t}}\"", Engine::Postgres),
            ("SELECT [{{t}}]", Engine::Sqlserver),
            ("SELECT $$ {{t}} $$", Engine::Postgres),
        ] {
            assert!(fill(sql, &v, engine).is_err(), "{sql}");
        }
        // Fora do comentário, depois dele, vale normalmente.
        assert_eq!(
            fill("SELECT /* a */ {{t}} -- b\n, {{t}}", &v, Engine::Postgres).unwrap(),
            "SELECT /* a */ 'x' -- b\n, 'x'"
        );
    }

    #[test]
    fn missing_values_and_nul_are_refused() {
        assert_eq!(
            fill("SELECT {{a}}", &Values::new(), Engine::Mysql).unwrap_err(),
            "Preencha o parâmetro a."
        );
        assert!(fill("SELECT {{a}}", &values(&[("a", "\0")]), Engine::Mysql).is_err());
        assert_eq!(
            fill("SELECT '{{' , $$x$$", &Values::new(), Engine::Postgres).unwrap(),
            "SELECT '{{' , $$x$$"
        );
    }
}
