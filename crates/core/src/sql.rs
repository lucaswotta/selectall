#[derive(Clone, Debug)]
pub struct SqlPlan {
    pub statements: Vec<String>,
    pub read_only_candidate: bool,
    /// Palavras de cada instrução em maiúsculas, sem textos nem comentários.
    /// Identificadores entre crases entram como palavra; `(` e `)` marcam a profundidade.
    pub tokens: Vec<Vec<String>>,
    pub executable_comment: bool,
}

// Um único laço de varredura: dividir em funções exigiria passar todo o estado do lexer.
#[allow(clippy::too_many_lines)]
pub fn analyze(sql: &str) -> Result<SqlPlan, String> {
    let mut statements = Vec::new();
    let mut statement_tokens = Vec::new();
    let mut any_executable = false;
    let mut quote_start = 0;
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut start = 0;
    let mut quote = None;
    let mut line_comment = false;
    let mut block_comment = false;
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
        } else if block_comment {
            if ch == '*' && next == Some('/') {
                block_comment = false;
                index += 1;
            }
        } else if let Some(delimiter) = quote {
            if ch == '\\' {
                index += 1;
            } else if ch == delimiter {
                if next == Some(delimiter) {
                    index += 1;
                } else {
                    if delimiter == '`' {
                        tokens.push(sql[quote_start + 1..offset].to_uppercase());
                    }
                    quote = None;
                }
            }
        } else if ch == '#'
            || (ch == '-'
                && next == Some('-')
                && chars
                    .get(index + 2)
                    .is_none_or(|(_, ch)| ch.is_whitespace()))
        {
            flush(&mut token, &mut tokens);
            line_comment = true;
        } else if ch == '/' && next == Some('*') {
            flush(&mut token, &mut tokens);
            executable_comment |= chars.get(index + 2).is_some_and(|(_, ch)| *ch == '!');
            block_comment = true;
            index += 1;
        } else if matches!(ch, '\'' | '"' | '`') {
            flush(&mut token, &mut tokens);
            quote = Some(ch);
            quote_start = offset;
            has_code = true;
        } else if ch == ';' {
            flush(&mut token, &mut tokens);
            if has_code {
                safe &= read_only_tokens(&tokens) && !executable_comment;
                statements.push(sql[start..offset].trim().to_owned());
                statement_tokens.push(std::mem::take(&mut tokens));
            }
            any_executable |= executable_comment;
            start = offset + 1;
            has_code = false;
            executable_comment = false;
            tokens.clear();
        } else if ch.is_ascii_alphanumeric() || ch == '_' {
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
    if quote.is_some() || block_comment {
        return Err("SQL incompleto: feche as aspas ou o comentário.".into());
    }
    flush(&mut token, &mut tokens);
    any_executable |= executable_comment;
    if has_code || executable_comment {
        safe &= read_only_tokens(&tokens) && !executable_comment;
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
    if statements.len() > 100 {
        return Err(
            "Um lote pode conter no máximo 100 instruções. Divida o script em lotes menores."
                .into(),
        );
    }
    Ok(SqlPlan {
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

fn read_only_tokens(tokens: &[String]) -> bool {
    matches!(
        first_word(tokens),
        Some("SELECT" | "SHOW" | "DESC" | "DESCRIBE" | "EXPLAIN")
    ) && !tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "OUTFILE" | "DUMPFILE" | "GET_LOCK" | "RELEASE_LOCK"
        )
    })
}

/// Recusa o que não pode passar por ensaio, piloto e lote: só `INSERT`, `UPDATE`,
/// `DELETE` e `REPLACE`, e `UPDATE`/`DELETE` com `WHERE` que cite alguma coluna.
pub fn check_write(plan: &SqlPlan) -> Result<(), String> {
    if plan.executable_comment {
        return Err("Comentários executáveis (/*! ... */) não são aceitos em correções.".into());
    }
    for tokens in &plan.tokens {
        check_write_statement(tokens)?;
    }
    Ok(())
}

fn check_write_statement(tokens: &[String]) -> Result<(), String> {
    let first = first_word(tokens).unwrap_or("");
    match first {
        "ALTER" | "DROP" | "CREATE" | "TRUNCATE" | "RENAME" => {
            return Err(format!(
                "{first} não passa por ensaio: no MySQL ela grava na hora, com COMMIT implícito, e não pode ser desfeita."
            ));
        }
        "GRANT" | "REVOKE" => {
            return Err(format!(
                "{first} altera permissões e não é aceito em correções."
            ));
        }
        "START" | "BEGIN" | "COMMIT" | "ROLLBACK" | "SAVEPOINT" | "RELEASE" | "SET" | "LOCK"
        | "UNLOCK" | "XA" => {
            return Err(format!(
                "O SelectAll abre e fecha a transação de cada equipamento. Tire o {first} do SQL."
            ));
        }
        "SELECT" | "SHOW" | "DESC" | "DESCRIBE" | "EXPLAIN" | "WITH" => {
            return Err(
                "Corrigir aceita só INSERT, UPDATE, DELETE e REPLACE. Para ler, use Consultar."
                    .into(),
            );
        }
        "INSERT" | "REPLACE" | "UPDATE" | "DELETE" => {}
        other => {
            return Err(format!(
                "Corrigir aceita só INSERT, UPDATE, DELETE e REPLACE; {other} não é aceito."
            ));
        }
    }
    if let Some(word) = tokens.iter().find(|token| {
        matches!(
            token.as_str(),
            "OUTFILE" | "DUMPFILE" | "GET_LOCK" | "RELEASE_LOCK"
        )
    }) {
        return Err(format!("{word} não é aceito em correções."));
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
            "ORDER" | "LIMIT" if depth == 0 && condition.is_some() => break,
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
    let constant = |word: &&str| {
        word.chars().all(|ch| ch.is_ascii_digit())
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
}
