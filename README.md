# SelectAll

Aplicativo desktop gratuito e de código aberto para equipes de TI de varejo consultarem, com um único SQL, os bancos de todos os PDVs, concentradores e servidores das suas lojas.

Em desenvolvimento. A definição do produto está em [`docs/produto.md`](docs/produto.md) e a identidade visual em [`design/`](design/README.md).

## Estado

| Parte | Situação |
| --- | --- |
| Núcleo: leitura em transação somente leitura (MySQL/MariaDB, PostgreSQL) ou sempre desfeita (SQL Server), alternativa entre perfis, prazo, cancelamento, offline | Pronto e testado contra MySQL 8.4, PostgreSQL 17 e SQL Server 2022 |
| Armazenamento local (SQLite) de redes, perfis, lojas e equipamentos | Pronto |
| Cofre do sistema para senhas | Pronto |
| Janela: redes, tema claro e escuro, navegação | Pronto |
| Frota, consulta, correção com ensaio, piloto e lote, auditoria | Pronto |
| Consultas salvas, histórico | Próximas fases |

## Requisitos

- Rust estável e Node.js 20 ou mais novo.
- Linux: `libwebkit2gtk-4.1-dev`, `librsvg2-dev`, `libxdo-dev`, `libssl-dev`, `pkg-config`.
- Windows: ferramentas de compilação do Visual Studio (MSVC) e NASM.
- Docker, só para os testes contra MySQL, PostgreSQL e SQL Server.

## Desenvolvimento

```sh
cd app
npm ci
npx tauri dev
```

Verificações exigidas pela CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
(cd app && npm run check)
sh scripts/test-mysql.sh
sh scripts/test-postgres.sh
sh scripts/test-sqlserver.sh
```

Cada `scripts/test-<motor>.sh` sobe um servidor descartável (`mysql:8.4`, `postgres:17`, `mcr.microsoft.com/mssql/server:2022-latest`), cria um usuário só de leitura e outro de escrita e roda os testes de integração. As senhas são aleatórias a cada execução.

Instaladores: `cd app && npx tauri build` gera `.deb` e AppImage no Linux e `.exe` (NSIS) no Windows em `target/release/bundle/`.

## Estrutura

| Caminho | Conteúdo |
| --- | --- |
| `crates/core` | `selectall-core`: frota, SQLite, cofre e execução de SQL, sem interface |
| `app/src-tauri` | Comandos Tauri sobre o núcleo |
| `app/src` | Interface em Svelte 5 e TypeScript |
| `design` | Tokens de cor e tipografia, marca e protótipo |
| `docs` | Definição do produto |

No código, a rede de varejo é `Chain`, a loja é `Store`, o equipamento é `Device` e o perfil de sistema é `Profile`. Textos para a pessoa ficam em português.

## Dados e credenciais

- Frota e configurações ficam em `selectall.db` na pasta de dados do aplicativo (`~/.local/share/io.github.lucaswotta.selectall` no Linux, `%APPDATA%\io.github.lucaswotta.selectall` no Windows).
- Senhas ficam só no cofre do sistema: Secret Service no Linux e Gerenciador de Credenciais no Windows.

## Licença

MIT ou Apache-2.0, à sua escolha. Veja `LICENSE-MIT` e `LICENSE-APACHE`.
