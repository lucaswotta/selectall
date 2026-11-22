//! Análise local do SQL antes de qualquer conexão: divide as instruções, decide se
//! é candidato a leitura e recusa correções inseguras. A divisão segue o dialeto
//! do motor, porque comentários e aspas mudam de sentido entre eles.

use crate::model::Engine;

#[derive(Clone, Debug)]
pub struct SqlPlan {
    pub engine: Engine,
    pub statements: Vec<String>,
    pub read_only_candidate: bool,
    /// Palavras de cada instrução em maiúsculas, sem textos nem comentários.
    /// Identificadores entre crases entram como palavra; os entre aspas duplas
    /// (PostgreSQL, SQL Server) ou colchetes (SQL Server) entram com `"` na frente,
    /// para nunca se confundirem com palavra reservada. `(` e `)` marcam a profundidade.
    pub tokens: Vec<Vec<String>>,
    pub executable_comment: bool,
}

/// Prefixos que só marcam o tipo do texto seguinte (`N'..'`, `E'..'`, `X'..'`,
/// `_utf8mb4'..'`): não são colunas e não podem contar como uma no `WHERE`.
fn literal_prefix(token: &str) -> bool {
    matches!(token.to_ascii_uppercase().as_str(), "N" | "E" | "X" | "B") || token.starts_with('_')
}

/// Último caractere de um texto entre cifrões do PostgreSQL (`$$..$$`, `$tag$..$tag$`)
/// que começa em `start`. `None`: não é texto entre cifrões; `chars.len()`: não foi fechado.
pub(crate) fn dollar_quote(chars: &[(usize, char)], start: usize) -> Option<usize> {
    let mut end = start + 1;
    while chars
        .get(end)
        .is_some_and(|(_, ch)| ch.is_ascii_alphanumeric() || *ch == '_')
    {
        end += 1;
    }
    if chars.get(end).map(|(_, ch)| *ch) != Some('$')
        || chars
            .get(start + 1)
            .is_some_and(|(_, ch)| ch.is_ascii_digit())
    {
        return None;
    }
    let tag: Vec<char> = chars[start..=end].iter().map(|(_, ch)| *ch).collect();
    let body = end + 1;
    Some(
        (body..chars.len())
            .find_map(|index| {
                let matches = tag
                    .iter()
                    .enumerate()
                    .all(|(offset, ch)| chars.get(index + offset).is_some_and(|(_, c)| c == ch));
                matches.then_some(index + tag.len() - 1)
            })
            .unwrap_or(chars.len()),
    )
}

// Um único laço de varredura: dividir em funções exigiria passar todo o estado do lexer.
#[allow(clippy::too_many_lines)]
pub fn analyze(sql: &str, engine: Engine) -> Result<SqlPlan, String> {
    let mysql = engine == Engine::Mysql;
    let mut statements = Vec::new();
    let mut statement_tokens = Vec::new();
    let mut any_executable = false;
    let mut quote_start = 0;
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut start = 0;
    let mut quote = None;
    let mut escapes = false;
    let mut line_comment = false;
    // PostgreSQL e SQL Server aninham comentários de bloco; o MySQL fecha no primeiro `*/`.
    let mut comment_depth = 0usize;
    let mut executable_comment = false;
    let mut has_code = false;
    let mut safe = true;
    let chars: Vec<(usize, char)> = sql.char_indices().collect();
    let mut index = 0;
    let flush = |token: &mut String, tokens: &mut Vec<String>| {
        if !token.is_empty() {
            tokens.push(std::mem::take(token).to_ascii_uppercase());
        }
    };
    while index < chars.len() {
        let (offset, ch) = chars[index];
        let next = chars.get(index + 1).map(|(_, ch)| *ch);
        if line_comment {
            if ch == '\n' {
                line_comment = false;
            }
        } else if comment_depth > 0 {
            if ch == '*' && next == Some('/') {
                comment_depth -= 1;
                index += 1;
            } else if !mysql && ch == '/' && next == Some('*') {
                comment_depth += 1;
                index += 1;
            }
        } else if let Some(delimiter) = quote {
            if ch == '\\' && escapes {
                index += 1;
            } else if ch == delimiter {
                if next == Some(delimiter) {
                    index += 1;
                } else {
                    let name = sql[quote_start + 1..offset].to_uppercase();
                    match delimiter {
                        '`' => tokens.push(name),
                        '"' if !mysql => tokens.push(format!("\"{name}")),
                        ']' => tokens.push(format!("\"{name}")),
                        _ => {}
                    }
                    quote = None;
                }
            }
        } else if (mysql && ch == '#')
            || (ch == '-'
                && next == Some('-')
                && (!mysql
                    || chars
                        .get(index + 2)
                        .is_none_or(|(_, ch)| ch.is_whitespace())))
        {
            flush(&mut token, &mut tokens);
            line_comment = true;
        } else if ch == '/' && next == Some('*') {
            flush(&mut token, &mut tokens);
            // `/*! ... */` roda no MySQL e no MariaDB; `/*M! ... */`, só no MariaDB.
            let bang = |at: usize| chars.get(at).is_some_and(|(_, ch)| *ch == '!');
            executable_comment |= mysql
                && (bang(index + 2)
                    || (chars.get(index + 2).is_some_and(|(_, ch)| *ch == 'M') && bang(index + 3)));
            comment_depth = 1;
            index += 1;
        } else if let Some(end) = (engine == Engine::Postgres && ch == '$' && token.is_empty())
            .then(|| dollar_quote(&chars, index))
            .flatten()
        {
            if end >= chars.len() {
                return Err("SQL incompleto: feche o texto entre cifrões ($$).".into());
            }
            has_code = true;
            index = end;
        } else if ch == '\''
            || ch == '"'
            || (mysql && ch == '`')
            || (engine == Engine::Sqlserver && ch == '[')
        {
            // No MySQL a barra escapa só dentro de texto; num nome entre crases ela é
            // um caractere comum, e tratá-la como escape esconderia o fim do nome.
            escapes = (mysql && ch != '`')
                || (engine == Engine::Postgres && ch == '\'' && token.eq_ignore_ascii_case("E"));
            if ch == '\'' && literal_prefix(&token) {
                token.clear();
            }
            flush(&mut token, &mut tokens);
            quote = Some(if ch == '[' { ']' } else { ch });
            quote_start = offset;
            has_code = true;
        } else if ch == ';' {
            flush(&mut token, &mut tokens);
            if has_code {
                safe &= read_only_tokens(&tokens, engine) && !executable_comment;
                statements.push(sql[start..offset].trim().to_owned());
                statement_tokens.push(std::mem::take(&mut tokens));
            }
            any_executable |= executable_comment;
            start = offset + 1;
            has_code = false;
            executable_comment = false;
            tokens.clear();
        } else if ch.is_ascii_alphanumeric()
            || ch == '_'
            || (!mysql && ch == '$' && !token.is_empty())
        {
            token.push(ch);
            has_code = true;
        } else {
            flush(&mut token, &mut tokens);
            if matches!(ch, '(' | ')') {
                tokens.push(ch.to_string());
            }
            has_code |= !ch.is_whitespace();
        }
        index += 1;
    }
    if quote.is_some() || comment_depth > 0 {
        return Err("SQL incompleto: feche as aspas ou o comentário.".into());
    }
    flush(&mut token, &mut tokens);
    any_executable |= executable_comment;
    if has_code || executable_comment {
        safe &= read_only_tokens(&tokens, engine) && !executable_comment;
        statements.push(sql[start..].trim().to_owned());
        statement_tokens.push(tokens);
    }
    if statements.is_empty() {
        return Err("Escreva uma consulta SQL antes de executar.".into());
    }
    if statements.iter().any(|statement| {
        statement
            .trim_start()
            .to_ascii_uppercase()
            .starts_with("DELIMITER")
    }) {
        return Err("DELIMITER é um comando de cliente. Envie a definição da rotina como uma instrução separada.".into());
    }
    if engine == Engine::Sqlserver
        && statements
            .iter()
            .flat_map(|statement| statement.lines())
            .any(|line| line.trim().eq_ignore_ascii_case("GO"))
    {
        return Err(
            "GO é um comando do SSMS, não do SQL Server. Separe as instruções com ;.".into(),
        );
    }
    if statements.len() > 100 {
        return Err(
            "Um lote pode conter no máximo 100 instruções. Divida o script em lotes menores."
                .into(),
        );
    }
    Ok(SqlPlan {
        engine,
        statements,
        read_only_candidate: safe,
        tokens: statement_tokens,
        executable_comment: any_executable,
    })
}

fn first_word(tokens: &[String]) -> Option<&str> {
    tokens
        .iter()
        .map(String::as_str)
        .find(|token| *token != "(")
}

/// Primeira palavra da instrução, sem comentários nem parênteses.
pub(crate) fn leading_word(statement: &str, engine: Engine) -> Option<String> {
    let plan = analyze(statement, engine).ok()?;
    plan.tokens
        .first()
        .and_then(|tokens| first_word(tokens).map(str::to_owned))
}

/// Funções com efeito fora da transação: a transação somente leitura e o `ROLLBACK`
/// não as desfazem.
fn forbidden_word(tokens: &[String], engine: Engine) -> Option<&str> {
    let list: &[&str] = match engine {
        Engine::Mysql => &["OUTFILE", "DUMPFILE", "GET_LOCK", "RELEASE_LOCK"],
        Engine::Postgres => &[
            "PG_TERMINATE_BACKEND",
            "PG_CANCEL_BACKEND",
            "PG_RELOAD_CONF",
            "PG_ROTATE_LOGFILE",
            "PG_PROMOTE",
            "PG_SWITCH_WAL",
            "PG_ADVISORY_LOCK",
            "PG_TRY_ADVISORY_LOCK",
            "PG_FILE_WRITE",
            "LO_IMPORT",
            "LO_EXPORT",
            "DBLINK",
            "DBLINK_EXEC",
            "DBLINK_CONNECT",
            "DBLINK_SEND_QUERY",
        ],
        Engine::Sqlserver => &[],
    };
    tokens
        .iter()
        .map(String::as_str)
        .find(|token| list.contains(token))
}

/// Palavras reservadas do T-SQL que começam uma instrução com efeito. O SQL Server
/// não exige `;` entre instruções: `SELECT 1 DELETE FROM t` são duas.
const SQLSERVER_STATEMENTS: &[&str] = &[
    "ALTER",
    "BACKUP",
    "BEGIN",
    "BULK",
    "COMMIT",
    "CREATE",
    "DBCC",
    "DELETE",
    "DENY",
    "DISABLE",
    "DROP",
    "DUMP",
    "ENABLE",
    "EXEC",
    "EXECUTE",
    "GRANT",
    "INSERT",
    "INTO",
    "KILL",
    "LOAD",
    "MERGE",
    "OPENDATASOURCE",
    "OPENQUERY",
    "OPENROWSET",
    "RECEIVE",
    "RECONFIGURE",
    "RESTORE",
    "REVOKE",
    "ROLLBACK",
    "SAVE",
    "SEND",
    "SET",
    "SETUSER",
    "SHUTDOWN",
    "TRUNCATE",
    "UPDATE",
    "UPDATETEXT",
    "USE",
    "WRITETEXT",
];

/// Primeira palavra de instrução escondida depois do início, fora o `INTO` logo
/// após `INSERT` e o primeiro `SET` de um `UPDATE`.
fn hidden_statement<'a>(first: &str, tokens: &'a [String]) -> Option<&'a str> {
    let start = tokens.iter().position(|token| token != "(")?;
    let mut set_seen = false;
    tokens
        .iter()
        .enumerate()
        .skip(start + 1)
        .map(|(index, token)| (index, token.as_str()))
        .find(|(index, word)| {
            SQLSERVER_STATEMENTS.contains(word)
                && !match *word {
                    "INTO" => first == "INSERT" && *index == start + 1,
                    "SET" => first == "UPDATE" && !std::mem::replace(&mut set_seen, true),
                    _ => false,
                }
        })
        .map(|(_, word)| word)
}

fn read_only_tokens(tokens: &[String], engine: Engine) -> bool {
    let first = first_word(tokens);
    matches!(
        first,
        Some("SELECT" | "SHOW" | "DESC" | "DESCRIBE" | "EXPLAIN")
    ) && forbidden_word(tokens, engine).is_none()
        && (engine != Engine::Sqlserver || hidden_statement(first.unwrap_or(""), tokens).is_none())
}

fn accepted(engine: Engine) -> &'static str {
    if engine == Engine::Mysql {
        "INSERT, UPDATE, DELETE e REPLACE"
    } else {
        "INSERT, UPDATE e DELETE"
    }
}

/// Recusa o que não pode passar por ensaio, piloto e lote: só `INSERT`, `UPDATE`,
/// `DELETE` (e `REPLACE` no MySQL), e `UPDATE`/`DELETE` com `WHERE` que cite alguma coluna.
pub fn check_write(plan: &SqlPlan) -> Result<(), String> {
    if plan.executable_comment {
        return Err("Comentários executáveis (/*! ... */) não são aceitos em correções.".into());
    }
    for tokens in &plan.tokens {
        check_write_statement(tokens, plan.engine)?;
    }
    Ok(())
}

fn check_write_statement(tokens: &[String], engine: Engine) -> Result<(), String> {
    let first = first_word(tokens).unwrap_or("");
    let accepted = accepted(engine);
    match first {
        "ALTER" | "DROP" | "CREATE" | "TRUNCATE" | "RENAME" => {
            return Err(if engine == Engine::Mysql {
                format!(
                    "{first} não passa por ensaio: no MySQL ela grava na hora, com COMMIT implícito, e não pode ser desfeita."
                )
            } else {
                format!(
                    "{first} não é aceito em correções: ensaio, piloto e lote valem só para {accepted}."
                )
            });
        }
        "GRANT" | "REVOKE" | "DENY" => {
            return Err(format!(
                "{first} altera permissões e não é aceito em correções."
            ));
        }
        "START" | "BEGIN" | "COMMIT" | "ROLLBACK" | "SAVEPOINT" | "RELEASE" | "SET" | "LOCK"
        | "UNLOCK" | "XA" | "SAVE" => {
            return Err(format!(
                "O SelectAll abre e fecha a transação de cada equipamento. Tire o {first} do SQL."
            ));
        }
        "SELECT" | "SHOW" | "DESC" | "DESCRIBE" | "EXPLAIN" | "WITH" => {
            return Err(format!(
                "Corrigir aceita só {accepted}. Para ler, use Consultar."
            ));
        }
        "INSERT" | "UPDATE" | "DELETE" => {}
        "REPLACE" if engine == Engine::Mysql => {}
        other => {
            return Err(format!(
                "Corrigir aceita só {accepted}; {other} não é aceito."
            ));
        }
    }
    if let Some(word) = forbidden_word(tokens, engine) {
        return Err(format!("{word} não é aceito em correções."));
    }
    if engine == Engine::Sqlserver
        && let Some(word) = hidden_statement(first, tokens)
    {
        return Err(format!(
            "{word} no meio de um {first}: o SQL Server aceita instruções sem ; entre elas, então o SelectAll recusa. Separe cada instrução com ;."
        ));
    }
    if matches!(first, "UPDATE" | "DELETE") {
        check_where(first, tokens)?;
    }
    Ok(())
}

/// O `WHERE` precisa estar no nível da instrução (não só numa subconsulta) e citar
/// alguma coluna: `WHERE 1 = 1` vale para a tabela inteira.
fn check_where(first: &str, tokens: &[String]) -> Result<(), String> {
    let mut depth = 0usize;
    let mut condition: Option<Vec<&str>> = None;
    for token in tokens {
        match token.as_str() {
            "(" => depth += 1,
            ")" => depth = depth.saturating_sub(1),
            "WHERE" if depth == 0 && condition.is_none() => condition = Some(Vec::new()),
            "ORDER" | "LIMIT" | "OPTION" if depth == 0 && condition.is_some() => break,
            word => {
                if let Some(words) = condition.as_mut() {
                    words.push(word);
                }
            }
        }
    }
    let Some(words) = condition else {
        return Err(format!(
            "{first} sem WHERE alteraria a tabela inteira em cada equipamento. Inclua um WHERE."
        ));
    };
    // Números, inclusive `0x01` e `1e3`: identificador não começa com dígito.
    let constant = |word: &&str| {
        word.starts_with(|ch: char| ch.is_ascii_digit())
            || matches!(
                *word,
                "TRUE" | "FALSE" | "NULL" | "NOT" | "AND" | "OR" | "XOR" | "IS"
            )
    };
    if words.iter().all(constant) {
        return Err(format!(
            "O WHERE deste {first} não cita nenhuma coluna e vale para todas as linhas."
        ));
    }
    Ok(())
}

/// Palavras e identificadores citados no SQL, para cruzar com nomes de tabela.
pub fn words(plan: &SqlPlan) -> std::collections::HashSet<String> {
    plan.tokens
        .iter()
        .flatten()
        .filter(|token| !matches!(token.as_str(), "(" | ")"))
        .cloned()
        .collect()
}

pub fn may_contain_credentials(sql: &str) -> bool {
    let upper = sql.to_ascii_uppercase();
    ["PASSWORD", "IDENTIFIED", "SECRET", "TOKEN", "SENHA"]
        .iter()
        .any(|word| upper.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze(sql: &str) -> Result<SqlPlan, String> {
        super::analyze(sql, Engine::Mysql)
    }

    #[test]
    fn strings_and_comments_do_not_split_statements() {
        let plan =
            analyze("-- update ignored\nSELECT 'a;b', 'it\\\'s'; /* ; */ SELECT `a;b` FROM t;")
                .unwrap();
        assert_eq!(plan.statements.len(), 2);
        assert!(plan.read_only_candidate);
    }
    #[test]
    fn writes_and_executable_comments_are_conservative() {
        for sql in [
            "SELECT 1; DELETE FROM t",
            "/*!50000 DELETE FROM t */",
            "SELECT 1 INTO OUTFILE 'x'",
            "WITH x AS (SELECT 1) SELECT * FROM x",
        ] {
            assert!(!analyze(sql).unwrap().read_only_candidate, "{sql}");
        }
        assert!(analyze("SELECT 'OUTFILE'").unwrap().read_only_candidate);
    }
    #[test]
    fn backslash_does_not_escape_a_backtick_name() {
        // No MySQL `a\` é um nome completo; a barra não pode esconder o `;` seguinte.
        let plan =
            analyze("UPDATE t SET a = 1 WHERE id IN (SELECT 1 AS `a\\`); DROP TABLE t; -- `)")
                .unwrap();
        assert_eq!(plan.statements.len(), 2);
        assert!(check_write(&plan).is_err());
        let hidden = analyze("SELECT 1 AS `a\\` INTO OUTFILE '/x' -- `").unwrap();
        assert!(!hidden.read_only_candidate);
    }

    #[test]
    fn mariadb_executable_comments_are_recognized() {
        assert!(
            !analyze("SELECT 1 /*M! INTO OUTFILE '/x' */")
                .unwrap()
                .read_only_candidate
        );
        let error = check_write(&analyze("UPDATE t SET a = 1 WHERE id = 1 /*M!100000 */").unwrap())
            .unwrap_err();
        assert!(error.contains("executáveis"), "{error}");
        assert!(
            analyze("SELECT 1 /* M! comum */")
                .unwrap()
                .read_only_candidate
        );
    }

    #[test]
    fn rejects_incomplete_and_empty_sql() {
        for sql in ["", "-- hi", "SELECT 'a", "/* no end"] {
            assert!(analyze(sql).is_err());
        }
    }

    #[test]
    fn writes_need_a_where_that_names_a_column() {
        let check = |sql: &str| check_write(&analyze(sql).unwrap());
        for sql in [
            "UPDATE produto SET bloqueado = 1 WHERE cod = 7",
            "DELETE FROM fila WHERE tipo = 'X' AND criado < '2026-11-01'",
            "UPDATE `produto` SET a = 1 WHERE `cod` = 7; INSERT INTO log VALUES (1)",
            "REPLACE INTO parametros (cod, valor) VALUES (1, 'a')",
            "UPDATE t SET a = 1 WHERE (cod IN (1, 2))",
        ] {
            assert_eq!(check(sql), Ok(()), "{sql}");
        }
        for (sql, part) in [
            ("UPDATE produto SET bloqueado = 1", "sem WHERE"),
            ("DELETE FROM fila", "sem WHERE"),
            (
                "UPDATE t SET a = (SELECT b FROM u WHERE u.id = 1)",
                "sem WHERE",
            ),
            ("DELETE FROM fila WHERE 1 = 1", "não cita nenhuma coluna"),
            (
                "UPDATE t SET a = 1 WHERE 'x' = 'x'",
                "não cita nenhuma coluna",
            ),
            (
                "UPDATE t SET a = 1 WHERE TRUE LIMIT 5",
                "não cita nenhuma coluna",
            ),
            ("ALTER TABLE t ADD c INT", "COMMIT implícito"),
            ("DROP TABLE t", "COMMIT implícito"),
            ("TRUNCATE t", "COMMIT implícito"),
            ("UPDATE t SET a = 1 WHERE id = 1; COMMIT", "Tire o COMMIT"),
            ("SELECT 1", "use Consultar"),
            ("CALL ajusta()", "CALL não é aceito"),
            ("/*!50000 UPDATE t SET a = 1 WHERE id = 1 */", "executáveis"),
            ("GRANT ALL ON *.* TO x", "permissões"),
        ] {
            let error = check(sql).unwrap_err();
            assert!(error.contains(part), "{sql}: {error}");
        }
    }

    #[test]
    fn backtick_identifiers_and_parentheses_become_tokens() {
        let plan = analyze("(SELECT `Cod Loja` FROM t)").unwrap();
        assert!(plan.read_only_candidate);
        assert!(words(&plan).contains("COD LOJA"));
    }

    #[test]
    fn literal_prefixes_and_hex_do_not_count_as_columns() {
        for (engine, sql) in [
            (Engine::Mysql, "DELETE FROM t WHERE N'a' = N'a'"),
            (Engine::Mysql, "DELETE FROM t WHERE X'01' = _utf8mb4'x'"),
            (Engine::Mysql, "DELETE FROM t WHERE 0x01 = 0x01"),
            (Engine::Postgres, "DELETE FROM t WHERE E'a' = E'a'"),
            (Engine::Sqlserver, "DELETE FROM t WHERE N'a' = N'a'"),
        ] {
            let error = check_write(&super::analyze(sql, engine).unwrap()).unwrap_err();
            assert!(error.contains("não cita nenhuma coluna"), "{sql}: {error}");
        }
    }

    #[test]
    fn postgres_comments_and_quotes_follow_the_server() {
        let pg = |sql: &str| super::analyze(sql, Engine::Postgres).unwrap();
        // `#` é operador no PostgreSQL, não comentário.
        assert_eq!(pg("SELECT 1 # 2; DELETE FROM t").statements.len(), 2);
        // Sem espaço depois de `--` ainda é comentário.
        let plan = pg("SELECT 1 --'\n; DELETE FROM t; --'");
        assert_eq!(plan.statements.len(), 2);
        assert!(!plan.read_only_candidate);
        // Barra invertida é literal fora de E'...'.
        let plan = pg(r"SELECT 'a\'; DELETE FROM t; --'");
        assert!(!plan.read_only_candidate);
        assert!(pg(r"SELECT E'a\'; b'").read_only_candidate);
        // Comentários aninhados.
        let plan = pg("/* /* */ ' */ DELETE FROM t; --'");
        assert!(!plan.read_only_candidate);
        // Texto entre cifrões.
        let plan = pg("SELECT $$'$$; DELETE FROM t; --'");
        assert!(!plan.read_only_candidate);
        assert!(pg("SELECT $f$ ; DELETE $f$, a$b FROM t").read_only_candidate);
        assert!(super::analyze("SELECT $x$ aberto", Engine::Postgres).is_err());
        assert!(!pg("SELECT pg_terminate_backend(1)").read_only_candidate);
        // Identificador entre aspas cita coluna e não é palavra reservada.
        let plan = pg(r#"UPDATE "produto" SET "update" = 1 WHERE "cod" = 1"#);
        assert_eq!(check_write(&plan), Ok(()));
        assert!(
            check_write(&pg("CREATE TABLE t (a int)"))
                .unwrap_err()
                .contains("não é aceito em correções")
        );
        assert!(
            check_write(&pg("REPLACE INTO t VALUES (1)"))
                .unwrap_err()
                .contains("REPLACE não é aceito")
        );
    }

    #[test]
    fn sqlserver_statements_without_semicolon_are_caught() {
        let ms = |sql: &str| super::analyze(sql, Engine::Sqlserver).unwrap();
        for sql in [
            "SELECT 1 DELETE FROM t",
            "SELECT * INTO copia FROM t",
            "SELECT * FROM OPENQUERY(srv, 'x')",
            "SELECT 1 EXEC xp_cmdshell 'dir'",
            "SELECT * FROM #t DELETE FROM x",
        ] {
            assert!(!ms(sql).read_only_candidate, "{sql}");
        }
        for sql in [
            "SELECT TOP 5 [cod], [update] FROM dbo.produto WITH (NOLOCK)",
            "SELECT a FROM t ORDER BY a OFFSET 0 ROWS FETCH NEXT 5 ROWS ONLY",
            "SELECT 'DELETE FROM t' AS texto",
        ] {
            assert!(ms(sql).read_only_candidate, "{sql}");
        }
        for sql in [
            "UPDATE produto SET bloqueado = 1 WHERE cod = 1",
            "UPDATE p SET a = 1 FROM produto p JOIN x ON x.id = p.id WHERE p.cod = 1",
            "INSERT INTO log (a) VALUES (N'x')",
            "DELETE TOP (5) FROM fila WHERE tipo = 1",
        ] {
            assert_eq!(check_write(&ms(sql)), Ok(()), "{sql}");
        }
        for (sql, part) in [
            (
                "UPDATE t SET a = 1 WHERE id = 1 DELETE FROM t",
                "DELETE no meio",
            ),
            (
                "UPDATE t SET a = 1 WHERE id = 1 SET ROWCOUNT 0",
                "SET no meio",
            ),
            ("INSERT INTO t EXEC proc", "EXEC no meio"),
            (
                "UPDATE t SET a = 1 WHERE id = 1 DISABLE TRIGGER tr ON t",
                "DISABLE no meio",
            ),
            ("MERGE t USING u ON t.a = u.a", "MERGE não é aceito"),
        ] {
            let error = check_write(&ms(sql)).unwrap_err();
            assert!(error.contains(part), "{sql}: {error}");
        }
        assert!(super::analyze("SELECT 1\nGO\nSELECT 2", Engine::Sqlserver).is_err());
        // Colchete fecha em `]]` escapado; o `;` dentro dele não divide.
        assert_eq!(ms("SELECT [a;]]b] FROM t; SELECT 2").statements.len(), 2);
    }
}
