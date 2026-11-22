# SelectAll: definição de produto

SelectAll é um aplicativo desktop gratuito e de código aberto para equipes de TI de varejo consultarem e corrigirem, com um único SQL, os bancos de todos os PDVs, concentradores e servidores das suas lojas.

Documento de decisões de 21/11/2026. Substitui o QueryMass (`lucaswotta/querymass`) e o script `querymyall`.

## Problema

Uma rede de varejo tem dezenas de lojas e centenas de PDVs, cada um com banco próprio. Para saber "quais caixas estão com o parâmetro X errado" ou corrigir um cadastro em todos eles, o analista hoje tem três caminhos: acessar máquina por máquina, pedir ao fornecedor do PDV ou manter scripts próprios como o `querymyall`. Clientes SQL comuns (DBeaver, HeidiSQL) trabalham com uma conexão por vez e não conhecem loja, PDV nem concentrador.

## Para quem

- Analista de TI ou suporte de rede varejista que sabe SQL.
- Consultorias e fornecedores de PDV que atendem várias redes.
- Brasil primeiro: interface, documentação e nomes em português. Inglês depois.

## Princípios

1. A frota é o objeto central, não a conexão. O usuário pensa em "Loja 202, todos os caixas", não em `10.25.3.14:3306`.
2. Ler é rápido; escrever é deliberado. Toda alteração passa por ensaio, piloto e lote.
3. Um resultado, todas as máquinas. A saída é uma grade única com loja, equipamento e status em cada linha.
4. Offline é normal. PDV desligado é estado esperado, mostrado com calma, sem parecer erro grave.
5. Nada de senha em arquivo. Credenciais só no cofre do sistema operacional.

## Modelo de domínio

| Conceito | O que é |
| --- | --- |
| Rede | Cliente ou empresa. Um usuário pode ter várias. |
| Loja | Pertence a uma rede. Tem código, nome e regra de endereçamento. |
| Equipamento | Máquina com banco: PDV, self-checkout, concentrador, servidor de loja ou tipo definido pelo usuário. Concentrador é opcional. |
| Perfil de sistema | Como falar com um sistema de PDV: motor (MySQL/MariaDB, PostgreSQL, SQL Server), porta, banco, usuário e senha no cofre. Ex.: "Socin", "Consinco", "Linx". |
| Regra de endereçamento | Modelo que gera equipamentos de uma loja. Ex.: `10.{sub-rede}.3.{1-50}` como PDV, `.{100-130}` como self, `.200` como concentrador. Exceções por loja. |

Um equipamento aponta para um ou mais perfis em ordem; o app tenta o próximo se a autenticação falhar e registra qual perfil respondeu.

## Cadastro da frota

Meta: uma rede com 30 lojas cadastrada em menos de 10 minutos.

- Regra de rede: definir uma vez o modelo de endereçamento e a lista loja → sub-rede; o app gera os equipamentos.
- Colar lista: área de texto que aceita IPs, faixas (`10.25.3.1-50`) e linhas `loja;ip;tipo`.
- Importar CSV/XLSX com prévia, duplicados e erros por linha.
- Varredura: testa a porta do banco na faixa e mostra quem responde, com a versão do servidor; o usuário confirma o que entra.
- Exportar e importar a frota de uma rede em arquivo (sem senhas) para compartilhar com a equipe.

## Fluxos da v1

### Consultar

1. Escolher alvos: rede, lojas, tipos de equipamento ou seleção manual. Atalhos: "Todas as lojas", "Só concentradores". Cada loja abre a lista dos seus equipamentos, todos marcados por padrão; desmarcar um tira só ele, e marcar um equipamento de loja desmarcada escolhe só ele.
2. Escrever SQL ou abrir consulta salva.
3. Executar. Progresso por loja (online, offline, erro, concluído) com contadores.
4. Grade consolidada: colunas fixas `Loja`, `Equipamento`, `IP`, `Status`, `Perfil` + colunas do resultado. Filtro, ordenação, agrupamento por loja, cópia e exportação CSV/XLSX.
5. Máquinas offline ou com erro aparecem num painel à parte, com mensagem em português e ação "Repetir só estas".

### Corrigir (ensaio, piloto, lote)

1. Ensaio: executa em transação em todos os alvos e desfaz. Mostra linhas afetadas por máquina e total. Máquinas com contagem fora do padrão ficam destacadas.
2. Piloto: aplica de verdade em uma máquina escolhida e mostra o resultado.
3. Lote: aplica no restante, uma transação por máquina, com rollback em erro e repetição só das falhas.
4. Bloqueios: `UPDATE`/`DELETE` sem `WHERE`; DDL (`ALTER`, `DROP`, `CREATE`) fora do fluxo de ensaio, porque no MySQL faz commit implícito e não pode ser desfeito.
5. Pular ensaio: opção desligada por padrão. Ao marcar, uma janela explica o risco e só liga com "Estou ciente". Sem ensaio não há prévia de linhas nem destaque de fora do padrão; piloto e lote valem para os equipamentos escolhidos, os bloqueios continuam e a auditoria marca a etapa "sem ensaio".
6. Auditoria local, só acrescentada: quem (usuário do sistema operacional), quando, rede, SQL, lojas, perfil, linhas afetadas e status de cada máquina. A tela mostra a rede aberta, com opção de ver todas; usuário e rede só viram coluna quando variam, e a exportação em CSV ou XLSX leva tudo.

### Consultas salvas

Biblioteca por rede, com nome, descrição, perfil de sistema opcional, SQL e parâmetros (`{{codigo_produto}}`). A indicação de leitura ou escrita sai da análise do SQL no dialeto do perfil (MySQL quando não há perfil) e decide se a consulta abre em Consultar ou em Corrigir.

Os parâmetros são pedidos antes de executar. O valor nunca entra cru: fora de aspas vira texto citado no dialeto de cada motor da execução (`'...'` com a barra escapada no MySQL, `'...'` no PostgreSQL, `N'...'` no SQL Server); dentro de um texto (`'%{{nome}}%'`), entra com aspas e barras escapadas. Parâmetro em comentário, em nome de tabela ou coluna, ou em texto entre cifrões é recusado. Toda etapa de uma correção usa os mesmos valores, e a auditoria os lista abaixo do SQL.

A biblioteca é exportada para a pasta Downloads como `selectall-consultas-<data>.json`, com o perfil pelo nome. Ao importar, o perfil é ligado pelo nome aos perfis da rede, sem diferenciar maiúsculas, e consulta com o mesmo nome é substituída.

### Histórico com resultados

Cada leitura fica guardada com SQL, valores dos parâmetros, alvos, status e dados retornados de cada equipamento. Repetir só as falhas substitui a execução no histórico. Os limites valem para todas as redes (padrão: 30 dias ou 500 MB); ao passar de qualquer um, sai a execução mais antiga, e a mais recente sempre fica. SQL ou valor com `PASSWORD`, `IDENTIFIED`, `TOKEN`, `SECRET` ou `SENHA` não é guardado: a execução fica com os dados e sem o SQL, e não pode ser repetida. Reabrir uma execução mostra a mesma grade, com comparação, exportação e repetição das falhas. Correções vão para a auditoria.

### Comparar resultados

Agrupa os equipamentos que responderam por resultado idêntico nas colunas escolhidas (padrão: todas menos as de data e hora, reconhecidas pelos valores). A ordem das linhas não conta. O maior grupo é a maioria quando é maior que todos os outros; os demais aparecem como divergentes, com quantos equipamentos de cada loja caíram neles. Uso típico: "quais PDVs têm versão ou parâmetro diferente".

## Segurança por motor

| Motor | Modo leitura | Ensaio |
| --- | --- | --- |
| MySQL/MariaDB | `START TRANSACTION READ ONLY` + análise do SQL | Transação + `ROLLBACK`; DDL bloqueada |
| PostgreSQL | `START TRANSACTION READ ONLY` + análise do SQL | Transação + `ROLLBACK`; DDL bloqueada como no MySQL, embora o PostgreSQL a desfaça |
| SQL Server | Sem transação somente leitura no servidor: análise do SQL + transação sempre desfeita | `BEGIN TRAN` + `ROLLBACK`; DDL bloqueada |

A análise segue o dialeto de cada motor presente na execução: `#` só é comentário no MySQL, `--` dispensa espaço no PostgreSQL e no SQL Server, comentários de bloco aninham nesses dois, o PostgreSQL tem texto entre cifrões e o SQL Server aceita instruções sem `;` entre elas. No SQL Server, palavra de instrução com efeito (`DELETE`, `EXEC`, `INTO`, `SET` além do primeiro do `UPDATE` e outras) no meio de uma instrução é recusada. No PostgreSQL, cada instrução é preparada antes de rodar, e o protocolo recusa mais de uma por vez.

Cancelamento: `KILL QUERY` no MySQL, pedido de cancelamento do protocolo no PostgreSQL, `KILL` da sessão no SQL Server. Sem a permissão `ALTER ANY CONNECTION`, o SQL Server recusa o `KILL`; o SelectAll encerra a conexão e o servidor desfaz a transação ao perceber a queda.

A recomendação na documentação é usar no perfil de leitura um usuário de banco só com `SELECT`.

## Fora da v1

Servidor web e uso multiusuário, agendamento, Oracle, SQLite, autocompletar de SQL, outro idioma além do português, assinatura de instalador para macOS.

## Identidade

- Nome: SelectAll. Remete a `SELECT *` e a "todas as máquinas".
- Tom: operacional e calmo. Legível sob pressão, cor só onde informa.
- Símbolo: asterisco do `SELECT *`, geométrico, legível a 16 px.
- Cor da marca: azul-petróleo. Verde, âmbar e vermelho ficam reservados para status.
- Tema claro e escuro, seguindo o sistema, com troca manual.
- Tipografia: IBM Plex Sans para interface e IBM Plex Mono para SQL e dados.

## Arquitetura

| Parte | Escolha | Motivo |
| --- | --- | --- |
| Casca | Tauri 2 | Motor em Rust, interface web, instalador pequeno no Windows e Linux |
| Motor | crate Rust `selectall-core`, sem dependência de interface | Testável sem janela; permite CLI depois |
| Drivers | `mysql`, `postgres` e `tiberius` (SQL Server, num runtime `tokio` por equipamento) | SQL arbitrário precisa de valores dinâmicos, que o `sqlx` não expõe bem; todos em Rust, sem biblioteca nativa |
| Dados locais | SQLite | Frota, consultas, histórico e auditoria com consulta e migração |
| Credenciais | `keyring` | Cofre nativo do Windows e Secret Service no Linux |
| Interface | TypeScript, editor CodeMirror 6, grade virtualizada | Edição SQL e tabelas com centenas de milhares de linhas |

Do QueryMass são portados: análise de instruções do SQL, cancelamento por `KILL QUERY`, prazo por consulta, status "resultado desconhecido", limites de memória, cofre e trava de instância.

Licença: MIT OR Apache-2.0, padrão do ecossistema Rust.

## Fases

1. Identidade e protótipo: símbolo, cores, tipografia, temas e protótipo navegável das telas de frota, consulta e correção, validado antes de qualquer código de interface.
2. Fundação: repositório, Tauri, `selectall-core` com MySQL, SQLite, CI Windows/Linux.
3. Frota: redes, lojas, perfis, regras de endereçamento, colar lista, importação, varredura.
4. Consultar: execução em massa, grade consolidada, offline, exportação.
5. Corrigir: ensaio, piloto, lote, bloqueios e auditoria.
6. PostgreSQL e SQL Server no motor e nos testes.
7. Consultas salvas, histórico com resultados e comparação.
8. Acabamento: acessibilidade, instaladores, documentação, primeira versão pública.

## Em aberto

- Validação do símbolo e do protótipo pelo Lucas (fase 1 em `design/`).
- Domínio e redes sociais do nome.
- Formato do arquivo de frota compartilhável.
- Limite padrão de concorrência por loja para não saturar links de loja.
