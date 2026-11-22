# Changelog

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/); versões seguem [SemVer](https://semver.org/lang/pt-BR/).

## [0.1.0] - 2026-11-21

Primeira versão pública, para Windows e Linux.

### Frota

- Redes, lojas, equipamentos e perfis de sistema (MySQL/MariaDB, PostgreSQL e SQL Server), com senhas no cofre do sistema operacional.
- Cadastro por regra de endereçamento, lista colada, planilha CSV/XLSX ou varredura de faixa, sempre com prévia; escolha do perfil para as linhas que não dizem qual.
- Mais de um perfil por equipamento, tentados em ordem quando a autenticação é recusada.

### Consulta

- Um `SELECT` em todos os equipamentos escolhidos, em transação somente leitura, com progresso por loja e grade consolidada (filtro, ordenação, agrupamento por loja, cópia, CSV e XLSX).
- Editor SQL com destaque no dialeto dos alvos, números de linha, busca, `Ctrl+Enter` e execução só do trecho selecionado.
- Comparação de resultados entre equipamentos, com a maioria e os divergentes por loja.
- Painel de offline e falhas, com mensagem em português e "Repetir só estes".
- Limite de equipamentos da mesma loja executando ao mesmo tempo (padrão 4).

### Correção

- Ensaio em transação desfeita, piloto em um equipamento e lote no restante, com confirmação digitada.
- Bloqueio de `UPDATE`/`DELETE` sem `WHERE` que cite coluna e de comandos que mudam estrutura.
- Auditoria local só de acréscimo, por rede, com lojas, valores dos parâmetros e resultado de cada equipamento; exportação em CSV e XLSX.

### Biblioteca e histórico

- Consultas salvas por rede, com parâmetros `{{nome}}` citados no dialeto de cada motor; exportação e importação em JSON.
- Histórico das leituras com os dados de cada equipamento, reabertura, comparação e repetição das falhas; limites de 30 dias ou 500 MB.

### Interface

- Tema claro, escuro e automático; IBM Plex Sans e Mono embutidas.
- Navegação por teclado, foco visível, diálogos com foco preso e contraste conferido nos dois temas.
- Guia "Como usar" dentro do aplicativo e janela de Ajustes.

[0.1.0]: https://github.com/lucaswotta/selectall/releases/tag/v0.1.0
